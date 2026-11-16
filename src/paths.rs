use std::path::{Path, PathBuf};

/// The app's name, used for the install folder, registry keys and window titles.
pub const APP: &str = "comprimo";

fn local_appdata() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("AppData").join("Local")))
        .unwrap_or_else(std::env::temp_dir)
}

/// Everything the app owns lives here, so uninstalling is a single folder delete.
pub fn install_dir() -> PathBuf {
    local_appdata().join("Programs").join(APP)
}

pub fn installed_exe() -> PathBuf {
    install_dir().join(format!("{APP}.exe"))
}

pub fn icons_dir() -> PathBuf {
    install_dir().join("icons")
}

pub fn icon_file() -> PathBuf {
    icons_dir().join(format!("{APP}.ico"))
}

/// Where earlier versions kept the icon (Explorer caches icons by path, so the new one moved).
pub fn legacy_icon_file() -> PathBuf {
    install_dir().join(format!("{APP}.ico"))
}

pub fn menu_icon_file(glyph: crate::icon::Glyph, light_menu: bool) -> PathBuf {
    icons_dir().join(format!("{}-{}.ico", if light_menu { "light" } else { "dark" }, glyph.file_stem()))
}

pub fn ffmpeg_dir() -> PathBuf {
    install_dir().join("ffmpeg")
}

pub fn config_file() -> PathBuf {
    install_dir().join("config.json")
}

pub fn lock_file() -> PathBuf {
    install_dir().join("queue.lock")
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase();
    norm(a) == norm(b)
}

pub fn running_from_install_dir() -> bool {
    std::env::current_exe().map(|exe| same_path(&exe, &installed_exe())).unwrap_or(false)
}
