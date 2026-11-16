use serde::{Deserialize, Serialize};

use crate::paths;

/// A user-defined size shown in the right-click menu as "name (10 MB)".
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub mb: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Size used by the "default" right-click entry, in MB (1 MB = 1,000,000 bytes).
    pub default_mb: f64,
    #[serde(alias = "slots")]
    pub presets: Vec<Preset>,
    /// Audio cues while compressing.
    pub sounds: bool,
}

/// Explorer gets unreliable with very long cascading menus.
pub const MAX_PRESETS: usize = 14;

impl Default for Config {
    fn default() -> Self {
        Self {
            default_mb: 20.0,
            presets: vec![
                Preset { name: "small".into(), mb: 10.0 },
                Preset { name: "medium".into(), mb: 25.0 },
                Preset { name: "large".into(), mb: 50.0 },
            ],
            sounds: true,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        std::fs::read_to_string(paths::config_file())
            .ok()
            .and_then(|s| serde_json::from_str::<Config>(&s).ok())
            .map(Config::normalized)
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(paths::install_dir())?;
        let json = serde_json::to_string_pretty(self).expect("config serializes");
        std::fs::write(paths::config_file(), json)
    }

    pub fn normalized(mut self) -> Self {
        // Earlier versions shipped "custom1/2/3"; upgrade them if the user never changed them.
        let old_defaults = [("custom1", 10.0), ("custom2", 25.0), ("custom3", 50.0)];
        if self.presets.len() == 3
            && self.presets.iter().zip(old_defaults).all(|(p, (n, mb))| p.name == n && p.mb == mb)
        {
            self.presets = Config::default().presets;
        }
        self.default_mb = clean_mb(self.default_mb);
        self.presets.truncate(MAX_PRESETS);
        for (i, preset) in self.presets.iter_mut().enumerate() {
            preset.mb = clean_mb(preset.mb);
            preset.name = preset.name.trim().to_string();
            if preset.name.is_empty() {
                preset.name = format!("preset {}", i + 1);
            }
        }
        self
    }

    /// First "preset N" name that isn't taken yet.
    pub fn next_preset_name(&self) -> String {
        (self.presets.len() + 1..)
            .map(|n| format!("preset {n}"))
            .find(|name| !self.presets.iter().any(|p| p.name.eq_ignore_ascii_case(name)))
            .unwrap()
    }

    /// How "big" each menu entry is on a 1–4 scale (default first, then the presets), for the
    /// block meter shown next to it. Relative to the other entries, on a log scale.
    pub fn size_levels(&self) -> Vec<u8> {
        let sizes: Vec<f64> = std::iter::once(self.default_mb).chain(self.presets.iter().map(|p| p.mb)).collect();
        let (lo, hi) = sizes.iter().fold((f64::MAX, f64::MIN), |(lo, hi), &s| (lo.min(s), hi.max(s)));
        sizes
            .iter()
            .map(|&s| {
                if hi / lo < 1.2 {
                    return 2;
                }
                1 + (3.0 * (s / lo).ln() / (hi / lo).ln()).round() as u8
            })
            .collect()
    }
}

fn clean_mb(mb: f64) -> f64 {
    (mb.clamp(1.0, 100_000.0) * 10.0).round() / 10.0
}

/// "10", "9.5"
pub fn fmt_mb(mb: f64) -> String {
    format!("{mb}")
}

pub fn human_size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 {
        format!("{:.2} GB", mb / 1000.0)
    } else if mb >= 100.0 {
        format!("{mb:.0} MB")
    } else if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.0} KB", bytes as f64 / 1000.0)
    }
}

/// "0:45", "12:03", "1:02:03"
pub fn fmt_duration(secs: f64) -> String {
    let s = secs.round() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_config_is_upgraded() {
        let old = r#"{"default_mb":20.0,"slots":[{"name":"custom1","mb":10.0},{"name":"custom2","mb":25.0},{"name":"custom3","mb":50.0}]}"#;
        let cfg = serde_json::from_str::<Config>(old).unwrap().normalized();
        assert_eq!(cfg, Config::default());

        let edited = r#"{"default_mb":15.0,"slots":[{"name":"custom1","mb":8.0}]}"#;
        let cfg = serde_json::from_str::<Config>(edited).unwrap().normalized();
        assert_eq!((cfg.default_mb, cfg.presets[0].name.as_str(), cfg.sounds), (15.0, "custom1", true));
    }

    #[test]
    fn size_levels_are_relative() {
        assert_eq!(Config::default().size_levels(), [2, 1, 3, 4]);
        let single = Config { presets: vec![], ..Config::default() };
        assert_eq!(single.size_levels(), [2]);
    }
}
