//! Hardware (GPU) H.264 encoders: detection and settings.
//!
//! GPU encoders are several times faster than x264 and leave the CPU free, at roughly the quality
//! of x264's "fast" preset. comprimo uses them when x264 would make you wait (see `encode`).

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::encode::CREATE_NO_WINDOW;
use crate::ffmpeg::Tools;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gpu {
    Nvenc,
    Qsv,
    Amf,
}

impl Gpu {
    /// In order of preference.
    pub const ALL: [Gpu; 3] = [Gpu::Nvenc, Gpu::Qsv, Gpu::Amf];

    pub fn codec(self) -> &'static str {
        match self {
            Gpu::Nvenc => "h264_nvenc",
            Gpu::Qsv => "h264_qsv",
            Gpu::Amf => "h264_amf",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Gpu::Nvenc => "nvidia gpu",
            Gpu::Qsv => "intel gpu",
            Gpu::Amf => "amd gpu",
        }
    }

    pub fn from_name(name: &str) -> Option<Gpu> {
        match name {
            "nvenc" => Some(Gpu::Nvenc),
            "qsv" => Some(Gpu::Qsv),
            "amf" => Some(Gpu::Amf),
            _ => None,
        }
    }

    /// Highest-quality settings for each vendor, rate-controlled to an average bitrate.
    pub fn args(self, bps: u64) -> Vec<String> {
        let (max, buf) = ((bps * 3 / 2).to_string(), (bps * 2).to_string());
        let bps = bps.to_string();
        let a: Vec<&str> = match self {
            Gpu::Nvenc => vec![
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p7",
                "-tune",
                "hq",
                "-rc",
                "vbr",
                "-multipass",
                "fullres",
                "-b:v",
                &bps,
                "-maxrate",
                &max,
                "-bufsize",
                &buf,
                "-rc-lookahead",
                "32",
                "-spatial-aq",
                "1",
                "-temporal-aq",
                "1",
                "-bf",
                "3",
                "-b_ref_mode",
                "middle",
                "-profile:v",
                "high",
                "-pix_fmt",
                "yuv420p",
            ],
            Gpu::Qsv => vec![
                "-c:v",
                "h264_qsv",
                "-preset",
                "veryslow",
                "-b:v",
                &bps,
                "-maxrate",
                &max,
                "-bufsize",
                &buf,
                "-profile:v",
                "high",
                "-pix_fmt",
                "nv12",
            ],
            Gpu::Amf => vec![
                "-c:v",
                "h264_amf",
                "-usage",
                "transcoding",
                "-quality",
                "quality",
                "-rc",
                "vbr_peak",
                "-b:v",
                &bps,
                "-maxrate",
                &max,
                "-bufsize",
                &buf,
                "-profile:v",
                "high",
                "-pix_fmt",
                "nv12",
            ],
        };
        a.into_iter().map(String::from).collect()
    }
}

/// Finds the best GPU encoder that actually works here, by encoding a few frames of the real
/// input with the real settings (catches missing GPUs, old drivers and unsupported options).
pub fn detect(tools: &Tools, input: &Path, filters: &str, bps: u64) -> Option<Gpu> {
    let listed = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    Gpu::ALL.into_iter().filter(|g| listed.contains(g.codec())).find(|g| {
        Command::new(&tools.ffmpeg)
            .args(["-hide_banner", "-v", "error", "-nostdin", "-i"])
            .arg(input)
            .args(["-frames:v", "8", "-an", "-vf", filters])
            .args(g.args(bps))
            .args(["-f", "null", "NUL"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// GPU detection running in the background, so it overlaps with the analysis pass.
pub struct Probe(Arc<Mutex<Option<Option<Gpu>>>>);

impl Probe {
    pub fn start(tools: Tools, input: PathBuf, filters: String, bps: u64) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let out = slot.clone();
        std::thread::spawn(move || {
            let found = detect(&tools, &input, &filters, bps);
            *out.lock().unwrap() = Some(found);
        });
        Probe(slot)
    }

    pub fn ready(gpu: Option<Gpu>) -> Self {
        Probe(Arc::new(Mutex::new(Some(gpu))))
    }

    /// `None` while still testing.
    pub fn result(&self) -> Option<Option<Gpu>> {
        *self.0.lock().unwrap()
    }

    pub fn wait(&self, max: Duration) -> Option<Gpu> {
        let start = Instant::now();
        loop {
            if let Some(r) = self.result() {
                return r;
            }
            if start.elapsed() >= max {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
