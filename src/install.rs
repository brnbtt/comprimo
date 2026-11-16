use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};

use crate::config::{Config, fmt_mb};
use crate::encode::CREATE_NO_WINDOW;
use crate::icon::Glyph;
use crate::paths::{self, APP};
use crate::{icon, win};

/// File types that get the right-click entry.
const EXTENSIONS: &[&str] =
    &["mp4", "mkv", "mov", "avi", "webm", "wmv", "flv", "m4v", "mpg", "mpeg", "ts", "mts", "m2ts", "3gp", "ogv"];

/// Explorer's "show a separator above this item" flag.
const ECF_SEPARATORBEFORE: u32 = 0x20;

fn uninstall_key() -> String {
    format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{APP}")
}

fn menu_key(ext: &str) -> String {
    format!(r"Software\Classes\SystemFileAssociations\.{ext}\shell\{APP}")
}

pub fn is_installed() -> bool {
    RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(uninstall_key(), KEY_READ).is_ok()
        && paths::installed_exe().is_file()
}

/// Copies the running exe into the install folder and registers everything.
pub fn install(cfg: &Config) -> Result<PathBuf, String> {
    let dir = paths::install_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let target = paths::installed_exe();
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    if !paths::same_path(&current, &target) {
        std::fs::copy(&current, &target).map_err(|e| {
            format!(
                "Couldn't copy {APP} to {}: {e}\n\nIf a video is being compressed right now, wait for it to finish and try again.",
                target.display()
            )
        })?;
    }
    write_icons();
    cfg.save().map_err(|e| format!("Couldn't save settings: {e}"))?;
    register_menu(&target, cfg).map_err(|e| format!("Couldn't add the right-click menu: {e}"))?;
    register_uninstaller(&target).map_err(|e| format!("Couldn't register the uninstaller: {e}"))?;
    Ok(target)
}

/// The app icon and the menu glyphs (both menu themes).
fn write_icons() {
    let _ = std::fs::create_dir_all(paths::icons_dir());
    let _ = std::fs::remove_file(paths::legacy_icon_file());
    let _ = std::fs::write(paths::icon_file(), icon::ico_bytes());
    for light in [false, true] {
        for glyph in (1..=4).map(Glyph::Size).chain([Glyph::Settings]) {
            let _ = std::fs::write(paths::menu_icon_file(glyph, light), icon::glyph_ico(glyph, light));
        }
    }
}

/// Context menus follow the Windows app theme; the menu glyphs come in a light and a dark ink.
fn light_menus() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|k| k.get_value::<u32, _>("AppsUseLightTheme"))
        .is_ok_and(|v| v != 0)
}

/// Re-registers the menu if its glyphs don't match the current theme (or are missing, e.g. after
/// updating from a version without them). Explorer can't swap them by itself, so this runs
/// whenever comprimo starts.
pub fn refresh_menu_icons() {
    if !is_installed() {
        return;
    }
    let expected = paths::menu_icon_file(Glyph::Settings, light_menus());
    let current = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(format!(r"{}\shell", menu_key("mp4")))
        .ok()
        .and_then(|shell| shell.enum_keys().flatten().last().and_then(|k| shell.open_subkey(k).ok()))
        .and_then(|item| item.get_value::<String, _>("Icon").ok());
    if current.as_deref() != Some(&*expected.display().to_string()) || !expected.is_file() {
        write_icons();
        let _ = register_menu(&paths::installed_exe(), &Config::load());
    }
}

/// Where to find the entry: Windows 11's compact menu tucks classic entries under "show more options".
pub fn where_to_find() -> String {
    let build = RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .and_then(|k| k.get_value::<String, _>("CurrentBuildNumber"))
        .ok()
        .and_then(|b| b.parse::<u32>().ok())
        .unwrap_or(0);
    let classic_forced = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Classes\CLSID\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}\InprocServer32")
        .is_ok();
    if build >= 22000 && !classic_forced {
        format!("right-click a video → show more options → {APP}")
    } else {
        format!("right-click any video → {APP}")
    }
}

/// The right-click submenu: "default (20 MB)", one entry per preset, then "settings…".
pub fn menu_entries(cfg: &Config) -> Vec<(String, String, Glyph)> {
    let levels = cfg.size_levels();
    let mut entries = vec![(
        format!("default ({} MB)", fmt_mb(cfg.default_mb)),
        "compress \"%1\"".to_string(),
        Glyph::Size(levels[0]),
    )];
    for (preset, &level) in cfg.presets.iter().zip(&levels[1..]) {
        entries.push((
            format!("{} ({} MB)", preset.name, fmt_mb(preset.mb)),
            format!("compress --size {} \"%1\"", preset.mb),
            Glyph::Size(level),
        ));
    }
    entries.push(("settings…".into(), "settings".into(), Glyph::Settings));
    entries
}

pub fn register_menu(exe: &Path, cfg: &Config) -> io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let exe = exe.display().to_string();
    let icon = paths::icon_file();
    let entries = menu_entries(cfg);
    let light = light_menus();

    for ext in EXTENSIONS {
        let path = menu_key(ext);
        let _ = hkcu.delete_subkey_all(&path);
        let (root, _) = hkcu.create_subkey(&path)?;
        root.set_value("MUIVerb", &APP)?;
        root.set_value("SubCommands", &"")?;
        root.set_value("MultiSelectModel", &"Player")?;
        if icon.is_file() {
            root.set_value("Icon", &icon.display().to_string())?;
        }
        let last = entries.len() - 1;
        for (i, (label, args, glyph)) in entries.iter().enumerate() {
            let (item, _) = root.create_subkey(format!(r"shell\{i:02}"))?;
            item.set_value("MUIVerb", label)?;
            item.set_value("MultiSelectModel", &"Player")?;
            let glyph_file = paths::menu_icon_file(*glyph, light);
            if glyph_file.is_file() {
                item.set_value("Icon", &glyph_file.display().to_string())?;
            }
            if i == last {
                item.set_value("CommandFlags", &ECF_SEPARATORBEFORE)?;
            }
            let (cmd, _) = item.create_subkey("command")?;
            cmd.set_value("", &format!("\"{exe}\" {args}"))?;
        }
    }
    win::notify_associations_changed();
    Ok(())
}

fn register_uninstaller(exe: &Path) -> io::Result<()> {
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(uninstall_key())?;
    let exe_s = exe.display().to_string();
    let icon = paths::icon_file();
    let dir_kb: u64 = std::fs::read_dir(paths::install_dir())
        .map(|d| d.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum())
        .unwrap_or(0)
        / 1024;
    // ffmpeg is downloaded right after this, so include its approximate size.
    let size_kb = (dir_kb + 18_000) as u32;
    key.set_value("DisplayName", &APP)?;
    key.set_value("DisplayVersion", &env!("CARGO_PKG_VERSION"))?;
    key.set_value("Publisher", &format!("{APP} (open source)"))?;
    key.set_value("DisplayIcon", &if icon.is_file() { icon.display().to_string() } else { exe_s.clone() })?;
    key.set_value("InstallLocation", &paths::install_dir().display().to_string())?;
    key.set_value("URLInfoAbout", &env!("CARGO_PKG_REPOSITORY"))?;
    key.set_value("UninstallString", &format!("\"{exe_s}\" uninstall"))?;
    key.set_value("QuietUninstallString", &format!("\"{exe_s}\" uninstall --quiet"))?;
    key.set_value("NoModify", &1u32)?;
    key.set_value("NoRepair", &1u32)?;
    key.set_value("EstimatedSize", &size_kb)?;
    Ok(())
}

/// Removes registry entries and deletes the install folder (after we exit, if we're running from it).
pub fn uninstall() {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for ext in EXTENSIONS {
        let _ = hkcu.delete_subkey_all(menu_key(ext));
    }
    let _ = hkcu.delete_subkey_all(uninstall_key());
    win::notify_associations_changed();

    let dir = paths::install_dir();
    if paths::running_from_install_dir() {
        // Our exe is locked while we run, so retry the delete for a while after we exit.
        let d = dir.display();
        let _ = Command::new("cmd.exe")
            .raw_arg(format!(
                "/C for /L %i in (1,1,60) do (ping -n 2 127.0.0.1 >NUL & rmdir /S /Q \"{d}\" 2>NUL & if not exist \"{d}\" exit)"
            ))
            .current_dir(std::env::temp_dir())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    } else {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Preset;

    #[test]
    fn menu_labels_show_sizes() {
        let cfg = Config {
            default_mb: 20.0,
            presets: vec![Preset { name: "small".into(), mb: 10.0 }, Preset { name: "email".into(), mb: 24.5 }],
            sounds: true,
        };
        let labels: Vec<String> = menu_entries(&cfg).into_iter().map(|e| e.0).collect();
        assert_eq!(labels, ["default (20 MB)", "small (10 MB)", "email (24.5 MB)", "settings…"]);
        let glyphs: Vec<Glyph> = menu_entries(&cfg).into_iter().map(|e| e.2).collect();
        assert_eq!(glyphs, [Glyph::Size(3), Glyph::Size(1), Glyph::Size(4), Glyph::Settings]);
    }
}
