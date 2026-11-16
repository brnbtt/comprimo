//! Hardware (GPU) H.264 encoders: detection and settings.
//!
//! GPU encoders are several times faster than x264 and leave the CPU free, at roughly the quality
//! of x264's "fast" preset. comprimo always uses one when the PC has one that works (see `encode`).

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

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
