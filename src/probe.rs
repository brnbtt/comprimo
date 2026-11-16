//! Reads what we need about a video from `ffmpeg -i <file>` output, so ffprobe isn't required.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::encode::{CREATE_NO_WINDOW, JobError};
use crate::ffmpeg::Tools;

#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub duration: f64,
    /// Display width/height (after applying rotation metadata).
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub has_audio: bool,
    pub audio_bps: Option<f64>,
    pub audio_channels: u32,
    pub size: u64,
}

pub fn probe(tools: &Tools, input: &Path) -> Result<MediaInfo, JobError> {
    let out = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(input)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Couldn't run ffmpeg: {e}"))?;
    // ffmpeg exits with an error here ("no output file") — that's expected.
    let text = String::from_utf8_lossy(&out.stderr);
    let size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let mut info = parse(&text, size).map_err(|e| {
        let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).rev().take(3).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        JobError::Failed(format!("{e}\n\n{}", tail.join("\n")))
    })?;
    if info.duration <= 0.0 {
        info.duration = measure_duration(tools, input).ok_or("Couldn't determine the video's length.")?;
    }
    Ok(info)
}

/// Some files (e.g. live-streamed MKV/WebM) don't store a duration. Remuxing the video
/// stream to nowhere is fast and tells us the real length.
fn measure_duration(tools: &Tools, input: &Path) -> Option<f64> {
    let out = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(input)
        .args(["-map", "0:v:0", "-c", "copy", "-f", "null", "-"])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    let pos = text.rfind("time=")?;
    let value = text[pos + 5..].split_whitespace().next()?;
    parse_hms(value).filter(|d| *d > 0.0)
}

fn parse_hms(s: &str) -> Option<f64> {
    let mut parts = s.trim().split(':');
    let (h, m, sec) = (parts.next()?, parts.next()?, parts.next()?);
    Some(h.parse::<f64>().ok()? * 3600.0 + m.parse::<f64>().ok()? * 60.0 + sec.parse::<f64>().ok()?)
}

/// Words of a stream line, with the punctuation ffmpeg uses as separators removed.
fn words(s: &str) -> Vec<&str> {
    s.split([',', ' ', '[', ']', '(', ')']).filter(|w| !w.is_empty()).collect()
}

fn number_before(words: &[&str], unit: &str) -> Option<f64> {
    words.windows(2).find(|w| w[1] == unit).and_then(|w| w[0].parse::<f64>().ok())
}

fn parse_resolution(word: &str) -> Option<(u32, u32)> {
    let (w, h) = word.split_once('x')?;
    let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
    (w >= 2 && h >= 2 && w <= 16384 && h <= 16384).then_some((w, h))
}

fn parse_channels(words: &[&str]) -> u32 {
    for (i, w) in words.iter().enumerate() {
        let n = match *w {
            "mono" => 1,
            "stereo" => 2,
            "quad" => 4,
            "channels" => match i.checked_sub(1).and_then(|j| words[j].parse().ok()) {
                Some(n) => n,
                None => continue,
            },
            w if w.starts_with("5.1") => 6,
            w if w.starts_with("7.1") => 8,
            _ => continue,
        };
        return n;
    }
    2
}

pub fn parse(text: &str, size: u64) -> Result<MediaInfo, String> {
    let mut duration = 0.0;
    let mut video: Option<(u32, u32, f64)> = None;
    let mut rotation = 0.0;
    let mut in_video_stream = false;
    let mut audio: Option<(Option<f64>, u32)> = None;
    let mut seen_input = false;

    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("Input #") {
            if seen_input {
                break;
            }
            seen_input = true;
        } else if t.starts_with("Output #") {
            break;
        } else if let Some(rest) = t.strip_prefix("Duration:") {
            duration = parse_hms(rest.split(',').next().unwrap_or("")).unwrap_or(0.0);
        } else if t.starts_with("Stream #") {
            in_video_stream = false;
            if let Some(i) = t.find(": Video:") {
                if video.is_none() && !t.contains("(attached pic)") {
                    let w = words(&t[i + 8..]);
                    if let Some((width, height)) = w.iter().find_map(|w| parse_resolution(w)) {
                        let fps = number_before(&w, "fps")
                            .or_else(|| number_before(&w, "tbr"))
                            .filter(|f| *f > 0.5 && *f < 1000.0)
                            .unwrap_or(30.0);
                        video = Some((width, height, fps));
                        in_video_stream = true;
                    }
                }
            } else if let Some(i) = t.find(": Audio:")
                && audio.is_none()
            {
                let w = words(&t[i + 8..]);
                let bps = number_before(&w, "kb/s").map(|k| k * 1000.0).filter(|b| *b > 0.0);
                audio = Some((bps, parse_channels(&w)));
            }
        } else if in_video_stream && let Some(i) = t.to_ascii_lowercase().find("rotation of") {
            rotation = t[i + 11..].split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        }
    }

    if !seen_input {
        return Err("This file couldn't be read as a video.".into());
    }
    let (mut width, mut height, fps) = video.ok_or("This file doesn't contain a video stream.")?;
    if (f64::abs(rotation).round() as i64) % 180 == 90 {
        std::mem::swap(&mut width, &mut height);
    }
    Ok(MediaInfo {
        duration,
        width,
        height,
        fps,
        has_audio: audio.is_some(),
        audio_bps: audio.and_then(|a| a.0),
        audio_channels: audio.map(|a| a.1).unwrap_or(0),
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROTATED: &str = r"Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'C:\clips\rot.mp4':
  Metadata:
    major_brand     : isom
  Duration: 00:01:02.50, start: 0.000000, bitrate: 871 kb/s
  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(tv, bt709, progressive), 1920x1080 [SAR 1:1 DAR 16:9], 787 kb/s, 29.97 fps, 29.97 tbr, 15360 tbn (default)
    Metadata:
      handler_name    : VideoHandler
    Side data:
      Display Matrix: rotation of -90.00 degrees
  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, stereo, fltp, 192 kb/s (default)
At least one output file must be specified";

    const MKV_NO_DURATION: &str = r"Input #0, matroska,webm, from 'obs.mkv':
  Duration: N/A, start: 0.000000, bitrate: N/A
  Stream #0:0: Video: hevc (Main 10), yuv420p10le(tv, bt2020nc/bt2020/smpte2084), 3840x2160, SAR 1:1 DAR 16:9, 60 fps, 60 tbr, 1k tbn
  Stream #0:1: Audio: opus, 48000 Hz, 5.1(side), fltp
  Stream #0:2: Audio: aac (LC), 48000 Hz, mono, fltp";

    const MUSIC_WITH_COVER: &str = r"Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'song.m4a':
  Duration: 00:03:00.00, start: 0.000000, bitrate: 256 kb/s
  Stream #0:0[0x1](und): Audio: aac (LC) (mp4a / 0x6134706D), 44100 Hz, stereo, fltp, 256 kb/s (default)
  Stream #0:1[0x0]: Video: mjpeg (Baseline), yuvj420p(pc, bt470bg/unknown/unknown), 600x600 [SAR 1:1 DAR 1:1], 90k tbr, 90k tbn (attached pic)";

    #[test]
    fn rotated_phone_video() {
        let info = parse(ROTATED, 5_000_000).unwrap();
        assert_eq!((info.width, info.height), (1080, 1920));
        assert!((info.duration - 62.5).abs() < 1e-9);
        assert!((info.fps - 29.97).abs() < 1e-9);
        assert_eq!((info.audio_bps, info.audio_channels), (Some(192_000.0), 2));
    }

    #[test]
    fn mkv_without_duration_or_audio_bitrate() {
        let info = parse(MKV_NO_DURATION, 1).unwrap();
        assert_eq!((info.width, info.height, info.fps), (3840, 2160, 60.0));
        assert_eq!(info.duration, 0.0);
        assert_eq!((info.has_audio, info.audio_bps, info.audio_channels), (true, None, 6));
    }

    #[test]
    fn cover_art_is_not_video() {
        assert!(parse(MUSIC_WITH_COVER, 1).is_err());
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(parse("C:\\x.txt: Invalid data found when processing input", 1).is_err());
    }
}
