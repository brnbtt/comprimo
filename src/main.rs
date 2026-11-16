#![windows_subsystem = "windows"]

mod blocks;
mod config;
mod encode;
mod ffmpeg;
mod gpu;
mod icon;
mod install;
mod paths;
mod probe;
mod sound;
mod theme;
mod ui;
mod win;

use std::path::PathBuf;

use config::Config;
use paths::APP;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let quiet = args.iter().any(|a| a == "--quiet");

    let result = match args.first().map(String::as_str) {
        None | Some("settings") => ui::run_setup(),
        Some("compress") => compress(&args[1..]),
        Some("sounds") => {
            sound::demo();
            Ok(())
        }
        Some("install") => install::install(&Config::load()).map(|_| {
            if !quiet {
                win::info_box(&format!("{APP} is installed. Right-click any video → {APP}."));
            }
        }),
        Some("uninstall") => {
            if quiet || win::confirm(&format!("Remove {APP} from this computer?")) {
                install::uninstall();
                if !quiet {
                    win::info_box(&format!("{APP} has been removed."));
                }
            }
            Ok(())
        }
        Some(other) => Err(format!(
            "Unknown command: {other}\n\nUsage:\n  {APP} [settings]\n  {APP} compress [--size MB] <video>\n  {APP} install [--quiet]\n  {APP} uninstall [--quiet]"
        )),
    };

    if let Err(e) = result {
        if !quiet {
            win::error_box(&e);
        }
        std::process::exit(1);
    }
}

fn compress(args: &[String]) -> Result<(), String> {
    let cfg = Config::load();
    let mut size = cfg.default_mb;
    let mut input = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--size" => {
                let v = it.next().ok_or("--size needs a value in MB")?;
                if v != "default" {
                    size = v.parse::<f64>().ok().filter(|s| *s > 0.0).ok_or(format!("Invalid size: {v}"))?;
                }
            }
            _ => input = Some(PathBuf::from(arg)),
        }
    }
    let input = input.ok_or("No video file given.")?;
    if !input.is_file() {
        return Err(format!("File not found:\n{}", input.display()));
    }
    win::kill_children_on_exit();
    ui::run_compress(input, size, cfg.sounds)
}
