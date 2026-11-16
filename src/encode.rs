use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::{fmt_duration, fmt_mb, human_size};
use crate::ffmpeg::Tools;
use crate::gpu::{self, Gpu};
use crate::probe::{MediaInfo, probe};

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Lowest video bitrate we're willing to produce before telling the user it won't fit.
const MIN_VIDEO_BPS: f64 = 40_000.0;
/// Fraction of the size limit we aim for, leaving room for MP4 container overhead.
const HEADROOM: f64 = 0.96;
/// Resolution ladder (short side, in pixels) we step down through when bitrate is scarce.
const LADDER: [u32; 7] = [1440, 1080, 720, 540, 480, 360, 240];
const MAX_ATTEMPTS: usize = 5;

/// Where a job is, for the UI's labels and audio cues.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Preparing,
    Analyzing,
    /// Two-pass: first (analysis) pass.
    Pass1,
    /// Two-pass: second (encoding) pass.
    Pass2,
    /// Single-pass fallback.
    Single,
}

pub enum Event {
    Status(String),
    Progress(f32),
    Detail(String),
    /// `attempt` counts re-encodes after an overshoot (0 = first try).
    Stage {
        stage: Stage,
        attempt: usize,
    },
    /// One-line description of the input file.
    Source(String),
}

#[derive(Debug)]
pub enum JobError {
    Cancelled,
    Failed(String),
}

impl From<String> for JobError {
    fn from(s: String) -> Self {
        JobError::Failed(s)
    }
}

impl From<&str> for JobError {
    fn from(s: &str) -> Self {
        JobError::Failed(s.to_string())
    }
}

impl From<std::io::Error> for JobError {
    fn from(e: std::io::Error) -> Self {
        JobError::Failed(e.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub output: PathBuf,
    pub before: u64,
    pub after: u64,
    /// True when the input was already small enough and nothing was written.
    pub untouched: bool,
}

#[derive(Debug)]
pub struct Plan {
    pub out_w: u32,
    pub out_h: u32,
    pub fps: f64,
    pub filters: String,
    pub video_bps: f64,
    pub audio_bps: f64,
    pub audio_channels: u32,
}

impl Plan {
    /// Plain-words summary for the progress window, e.g. "720p · 30 fps · on your graphics card".
    pub fn describe(&self, encoder: Option<Encoder>) -> String {
        let mut s = format!("{} · {} fps", res_label(self.out_w, self.out_h), self.fps.round());
        match encoder {
            Some(Encoder::Gpu(_)) => s.push_str(" · on your graphics card"),
            Some(Encoder::X264) => s.push_str(" · on your processor"),
            None => {}
        }
        s
    }
}

/// "1080p", "720p": the short side, which is how people know video sizes (works for portrait too).
pub fn res_label(w: u32, h: u32) -> String {
    format!("{}p", w.min(h))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoder {
    /// Two-pass x264 at [`X264_PRESET`], for PCs without a usable hardware encoder.
    X264,
    Gpu(Gpu),
}

/// x264 effort when encoding on the CPU: comprimo always picks the fastest way to encode, and
/// "fast" is where x264 is quick without visibly losing quality.
const X264_PRESET: &str = "fast";

/// The fastest way to encode on this PC: a GPU encoder if one works here (several times faster
/// than the CPU, and leaves it free), otherwise x264. `COMPRIMO_ENCODER=x264|nvenc|qsv|amf`
/// forces one.
fn pick_encoder(tools: &Tools, input: &Path, plan: &Plan) -> Encoder {
    match std::env::var("COMPRIMO_ENCODER").ok().as_deref() {
        Some("x264") => Encoder::X264,
        Some(name) if Gpu::from_name(name).is_some() => Encoder::Gpu(Gpu::from_name(name).unwrap()),
        _ => gpu::detect(tools, input, &plan.filters, plan.video_bps as u64).map_or(Encoder::X264, Encoder::Gpu),
    }
}

/// Audio gets a share of the budget that scales with how much room there is overall.
fn pick_audio(total_bps: f64) -> (f64, u32) {
    match total_bps {
        t if t >= 3_000_000.0 => (160_000.0, 2),
        t if t >= 1_500_000.0 => (128_000.0, 2),
        t if t >= 800_000.0 => (96_000.0, 2),
        t if t >= 400_000.0 => (64_000.0, 2),
        t if t >= 200_000.0 => (48_000.0, 1),
        _ => (32_000.0, 1),
    }
}

fn even(x: f64) -> u32 {
    ((x / 2.0).round() as u32 * 2).max(2)
}

pub fn make_plan(info: &MediaInfo, target_bytes: u64, bitrate_scale: f64, max_short: u32) -> Result<Plan, JobError> {
    let total_bps = target_bytes as f64 * 8.0 * HEADROOM / info.duration;
    // Never spend more bits than the source had; it can't look better than the original.
    let source_bps = info.size as f64 * 8.0 / info.duration;
    let budget = total_bps.min(source_bps.max(MIN_VIDEO_BPS * 2.0));

    let (mut audio_bps, mut audio_channels) = if info.has_audio { pick_audio(total_bps) } else { (0.0, 0) };
    if info.has_audio {
        if let Some(src) = info.audio_bps {
            audio_bps = audio_bps.min(src.max(32_000.0));
        }
        audio_channels = audio_channels.min(info.audio_channels.max(1));
    }
    let mut video_bps = budget - audio_bps;
    if video_bps < MIN_VIDEO_BPS && info.has_audio && audio_bps > 24_000.0 {
        audio_bps = 24_000.0;
        audio_channels = 1;
        video_bps = budget - audio_bps;
    }
    video_bps *= bitrate_scale;
    if video_bps < MIN_VIDEO_BPS {
        return Err(format!(
            "This video is too long to fit in {} MB.\n\nThere wouldn't be enough room left to make it watchable. \
             Try a bigger size, or trim the video first.",
            fmt_mb(target_bytes as f64 / 1_000_000.0),
        )
        .into());
    }

    // Choose the highest resolution / frame rate that still gets enough bits per pixel.
    let (dw, dh) = (info.width as f64, info.height as f64);
    let landscape = dw >= dh;
    let short = dw.min(dh);
    let aspect = dw.max(dh) / short;
    let short_px = short as u32;
    let src_fps = info.fps.min(60.0);
    let high_fps = src_fps > 31.0;

    let top = short_px.min(max_short);
    let mut sizes = vec![top];
    sizes.extend(LADDER.iter().copied().filter(|&s| s < top));

    let bpp = |s: u32, fps: f64| video_bps / ((s as f64).powi(2) * aspect * fps);
    // Smaller frames need more bits per pixel to look decent.
    let threshold = |s: u32| 0.06 * (1080.0 / s as f64).max(1.0).powf(0.25);

    // Preference order: keep the frame rate down to 720p, then drop to 30 fps, then go below 720p.
    let mut candidates: Vec<(u32, f64)> =
        sizes.iter().filter(|&&s| s >= 720 || s == top).map(|&s| (s, src_fps)).collect();
    if high_fps {
        candidates.extend(sizes.iter().map(|&s| (s, 30.0)));
    } else {
        candidates.extend(sizes.iter().filter(|&&s| s < 720 && s != top).map(|&s| (s, src_fps)));
    }
    let (s, fps) = candidates
        .iter()
        .copied()
        .find(|&(s, f)| bpp(s, f) >= threshold(s))
        .unwrap_or((*sizes.last().unwrap(), src_fps.min(30.0)));

    let mut filters = Vec::new();
    if fps < info.fps - 0.5 {
        filters.push(format!("fps={fps}"));
    }
    let (out_w, out_h);
    if s < short_px {
        let long = even(s as f64 * aspect);
        if landscape {
            (out_w, out_h) = (long, s);
            filters.push(format!("scale=-2:{s}:flags=lanczos"));
        } else {
            (out_w, out_h) = (s, long);
            filters.push(format!("scale={s}:-2:flags=lanczos"));
        }
    } else {
        // yuv420p needs even dimensions.
        (out_w, out_h) = (info.width & !1, info.height & !1);
        filters.push("scale=trunc(iw/2)*2:trunc(ih/2)*2".to_string());
    }

    Ok(Plan { out_w, out_h, fps, filters: filters.join(","), video_bps, audio_bps, audio_channels })
}

/// `clip.mov` + 20 MB → `clip_20MB.mp4` (or `clip_20MB (2).mp4` if that exists).
fn output_path(input: &Path, target_mb: f64) -> PathBuf {
    let dir = input.parent().unwrap_or(Path::new("."));
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    let base = format!("{stem}_{}MB", fmt_mb(target_mb));
    let mut path = dir.join(format!("{base}.mp4"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base} ({n}).mp4"));
        n += 1;
    }
    path
}

/// How a single ffmpeg run encodes video.
#[derive(Clone, Copy)]
enum Run<'a> {
    /// x264 analysis pass.
    Analysis(&'a Path),
    /// x264 final pass using the analysis stats.
    Final(&'a Path),
    /// x264 single pass (fallback when two-pass rate control misses).
    X264Single,
    Gpu(Gpu),
}

fn video_args(input: &Path, plan: &Plan, run: Run) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::new();
    for s in ["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-progress", "pipe:1", "-nostats", "-i"] {
        a.push(s.into());
    }
    a.push(input.as_os_str().to_owned());
    for s in ["-map", "0:v:0", "-vf"] {
        a.push(s.into());
    }
    a.push(plan.filters.clone().into());
    let bps = plan.video_bps.round() as u64;
    if let Run::Gpu(g) = run {
        a.extend(g.args(bps).into_iter().map(OsString::from));
        return a;
    }
    for s in ["-c:v", "libx264", "-preset", X264_PRESET, "-profile:v", "high", "-pix_fmt", "yuv420p"] {
        a.push(s.into());
    }
    a.push("-b:v".into());
    a.push(bps.to_string().into());
    if let Run::Analysis(log) | Run::Final(log) = run {
        a.push("-pass".into());
        a.push(if matches!(run, Run::Analysis(_)) { "1" } else { "2" }.into());
        a.push("-passlogfile".into());
        a.push(log.as_os_str().to_owned());
    }
    a
}

/// Runs ffmpeg, reporting progress (0..1). `on_progress` returns false to stop early;
/// the result is `Ok(false)` in that case.
fn run_ffmpeg(
    tools: &Tools,
    args: &[OsString],
    duration: f64,
    on_progress: &mut dyn FnMut(f32) -> bool,
    cancel: &AtomicBool,
) -> Result<bool, JobError> {
    let mut child = Command::new(&tools.ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("Couldn't start ffmpeg: {e}"))?;

    let mut stderr = child.stderr.take().expect("stderr piped");
    let stderr_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });

    let stdout = child.stdout.take().expect("stdout piped");
    for line in BufReader::new(stdout).lines() {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(JobError::Cancelled);
        }
        let Ok(line) = line else { break };
        let fraction = if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
            (us / 1_000_000.0 / duration).clamp(0.0, 1.0) as f32
        } else if line == "progress=end" {
            1.0
        } else {
            continue;
        };
        if !on_progress(fraction) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(false);
        }
    }

    let status = child.wait()?;
    if cancel.load(Ordering::Relaxed) {
        return Err(JobError::Cancelled);
    }
    let errors = stderr_reader.join().unwrap_or_default();
    if !status.success() {
        let tail: Vec<&str> = errors.lines().rev().take(8).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        return Err(format!("The video engine (ffmpeg) stopped with an error:\n{}", tail.join("\n")).into());
    }
    Ok(true)
}

fn remove_passlogs(prefix: &Path) {
    let (Some(dir), Some(name)) = (prefix.parent(), prefix.file_name()) else { return };
    let name = name.to_string_lossy().to_string();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with(&name) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

pub fn compress(
    tools: &Tools,
    input: &Path,
    target_mb: f64,
    report: &mut dyn FnMut(Event),
    cancel: &AtomicBool,
) -> Result<Outcome, JobError> {
    report(Event::Status("reading video".into()));
    report(Event::Stage { stage: Stage::Analyzing, attempt: 0 });
    let info = probe(tools, input)?;
    report(Event::Source(format!(
        "{} · {} · {} fps · {}",
        human_size(info.size),
        res_label(info.width, info.height),
        info.fps.round(),
        fmt_duration(info.duration)
    )));
    let target_bytes = (target_mb * 1_000_000.0) as u64;

    let ext = input.extension().unwrap_or_default().to_string_lossy().to_lowercase();
    if info.size <= target_bytes && matches!(ext.as_str(), "mp4" | "mov" | "m4v" | "webm") {
        return Ok(Outcome { output: input.to_path_buf(), before: info.size, after: info.size, untouched: true });
    }

    // Fail early with a clear message if it can't possibly fit.
    make_plan(&info, target_bytes, 1.0, u32::MAX)?;

    let output = output_path(input, target_mb);
    let passlog = std::env::temp_dir().join(format!("{}-{}", crate::paths::APP, std::process::id()));
    let result = encode(tools, input, &output, &info, target_bytes, &passlog, report, cancel);
    remove_passlogs(&passlog);
    if result.is_err() {
        let _ = std::fs::remove_file(&output);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn encode(
    tools: &Tools,
    input: &Path,
    output: &Path,
    info: &MediaInfo,
    target_bytes: u64,
    passlog: &Path,
    report: &mut dyn FnMut(Event),
    cancel: &AtomicBool,
) -> Result<Outcome, JobError> {
    let mut bitrate_scale = 1.0;
    let mut max_short = u32::MAX;
    let mut two_pass = true;
    let mut corrected = false;
    let mut analyzed: Option<(u32, u32, u64)> = None;
    let mut last_size = 0;

    report(Event::Status("getting ready".into()));
    let first = make_plan(info, target_bytes, 1.0, u32::MAX)?;
    let mut encoder = pick_encoder(tools, input, &first);
    if cancel.load(Ordering::Relaxed) {
        return Err(JobError::Cancelled);
    }

    let mut attempt = 0;
    while attempt < MAX_ATTEMPTS {
        let Ok(plan) = make_plan(info, target_bytes, bitrate_scale, max_short) else {
            break;
        };
        report(Event::Detail(plan.describe(Some(encoder))));
        // Plain words; retries say what's going on.
        let label = |what: &str| if attempt == 0 { what.to_string() } else { "making it a little smaller".to_string() };

        // Pass-1 stats can be reused for a new bitrate, but not for a new resolution / frame rate.
        let key = (plan.out_w, plan.out_h, plan.fps.to_bits());
        if encoder == Encoder::X264 && two_pass && analyzed != Some(key) {
            report(Event::Progress(0.0));
            report(Event::Status(label("analyzing")));
            report(Event::Stage { stage: Stage::Pass1, attempt });
            let mut pass1 = video_args(input, &plan, Run::Analysis(passlog));
            for s in ["-an", "-f", "null", "NUL"] {
                pass1.push(s.into());
            }
            run_ffmpeg(
                tools,
                &pass1,
                info.duration,
                &mut |f| {
                    report(Event::Progress(f * 0.5));
                    true
                },
                cancel,
            )?;
            analyzed = Some(key);
        }

        let run = match encoder {
            Encoder::Gpu(g) => Run::Gpu(g),
            Encoder::X264 if two_pass => Run::Final(passlog),
            Encoder::X264 => Run::X264Single,
        };
        let (stage, start) = match run {
            Run::Final(..) => (Stage::Pass2, 0.5),
            _ => (Stage::Single, 0.0),
        };
        let mut args = video_args(input, &plan, run);
        report(Event::Progress(start));
        report(Event::Status(label("compressing")));
        report(Event::Stage { stage, attempt });
        if plan.audio_bps > 0.0 {
            for s in ["-map", "0:a:0?", "-c:a", "aac", "-b:a"] {
                args.push(s.into());
            }
            args.push(format!("{}", plan.audio_bps.round() as u64).into());
            args.push("-ac".into());
            args.push(plan.audio_channels.to_string().into());
        } else {
            args.push("-an".into());
        }
        for s in ["-sn", "-dn", "-max_muxing_queue_size", "1024", "-movflags", "+faststart"] {
            args.push(s.into());
        }
        args.push(output.as_os_str().to_owned());
        let span = 1.0 - start;
        let result = run_ffmpeg(
            tools,
            &args,
            info.duration,
            &mut |f| {
                report(Event::Progress(start + f * span));
                true
            },
            cancel,
        );
        match result {
            // A GPU encoder that fails mid-way (driver hiccup, unusual input): fall back to the CPU.
            Err(JobError::Failed(_)) if matches!(run, Run::Gpu(_)) => {
                encoder = Encoder::X264;
                two_pass = true;
                continue;
            }
            r => r?,
        };

        last_size = std::fs::metadata(output)?.len();
        if last_size <= target_bytes {
            return Ok(Outcome { output: output.to_path_buf(), before: info.size, after: last_size, untouched: false });
        }
        attempt += 1;

        // Overshot. A big miss means 2-pass rate control mispredicted (typical for grainy or
        // noisy footage), so switch to single-pass ABR, which adapts as it goes. Otherwise aim
        // proportionally lower, and if that already failed once, step down a resolution.
        if matches!(run, Run::Final(..)) && last_size as f64 > target_bytes as f64 * 1.10 {
            two_pass = false;
            bitrate_scale *= 0.98;
        } else {
            bitrate_scale *= target_bytes as f64 * 0.97 / last_size as f64;
            if corrected {
                let short = plan.out_w.min(plan.out_h);
                max_short = LADDER.iter().copied().find(|&s| s < short).unwrap_or(short);
            }
            corrected = true;
        }
    }
    Err(format!(
        "Couldn't get this video under {} MB (best attempt: {}).\n\nThis video is very hard to compress. \
         Try a bigger size or trim it first.",
        fmt_mb(target_bytes as f64 / 1_000_000.0),
        human_size(last_size)
    )
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(w: u32, h: u32, fps: f64, duration: f64) -> MediaInfo {
        MediaInfo {
            duration,
            width: w,
            height: h,
            fps,
            has_audio: true,
            audio_bps: Some(192_000.0),
            audio_channels: 2,
            size: 500_000_000,
        }
    }

    #[test]
    fn short_clip_keeps_resolution() {
        let p = make_plan(&info(1920, 1080, 60.0, 10.0), 10_000_000, 1.0, u32::MAX).unwrap();
        assert_eq!((p.out_w, p.out_h, p.fps), (1920, 1080, 60.0));
    }

    #[test]
    fn longer_clip_steps_down() {
        let p = make_plan(&info(1920, 1080, 60.0, 60.0), 10_000_000, 1.0, u32::MAX).unwrap();
        assert!(p.out_h < 1080, "{p:?}");
        let total = (p.video_bps + p.audio_bps) * 60.0 / 8.0;
        assert!(total <= 10_000_000.0 * HEADROOM + 1.0);
    }

    #[test]
    fn portrait_scales_width() {
        let p = make_plan(&info(1080, 1920, 30.0, 120.0), 10_000_000, 1.0, u32::MAX).unwrap();
        assert!(p.out_w < p.out_h && p.filters.contains(":-2"), "{p:?}");
    }

    #[test]
    fn way_too_long_fails() {
        assert!(make_plan(&info(1920, 1080, 30.0, 3.0 * 3600.0), 10_000_000, 1.0, u32::MAX).is_err());
    }

    #[test]
    fn details_are_plain_words() {
        let p = make_plan(&info(1920, 1080, 29.97, 10.0), 10_000_000, 1.0, u32::MAX).unwrap();
        assert_eq!(p.describe(Some(Encoder::X264)), "1080p · 30 fps · on your processor");
        assert_eq!(p.describe(Some(Encoder::Gpu(Gpu::Nvenc))), "1080p · 30 fps · on your graphics card");
        assert_eq!(res_label(1080, 1920), "1080p");
    }

    #[test]
    fn output_name_contains_size() {
        let dir = std::env::temp_dir();
        assert_eq!(output_path(&dir.join("my clip.mov"), 20.0), dir.join("my clip_20MB.mp4"));
        assert_eq!(output_path(&dir.join("a.mkv"), 9.5), dir.join("a_9.5MB.mp4"));
    }

    /// Compress any file and print each step:
    /// `$env:COMPRIMO_FILE="C:\clip.mp4"; $env:COMPRIMO_MB="10"; cargo test --release one_file -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn one_file() {
        let tools = std::env::var_os("COMPRIMO_FFMPEG")
            .map(|p| Tools { ffmpeg: PathBuf::from(p) })
            .or_else(crate::ffmpeg::find)
            .expect("ffmpeg needed");
        let input = PathBuf::from(std::env::var("COMPRIMO_FILE").expect("set COMPRIMO_FILE"));
        let mb: f64 = std::env::var("COMPRIMO_MB").ok().and_then(|s| s.parse().ok()).unwrap_or(20.0);
        let mut report = |e: Event| {
            if let Event::Status(s) | Event::Detail(s) = e {
                println!("  {s}");
            }
        };
        let start = std::time::Instant::now();
        let out = compress(&tools, &input, mb, &mut report, &AtomicBool::new(false));
        println!("  {out:?} in {:.1?}", start.elapsed());
    }

    /// End-to-end test with real ffmpeg: `cargo test --release -- --ignored --nocapture`
    /// Uses `COMPRIMO_FFMPEG` if set, otherwise the installed/PATH ffmpeg.
    #[test]
    #[ignore]
    fn end_to_end() {
        let tools = std::env::var_os("COMPRIMO_FFMPEG")
            .map(|p| Tools { ffmpeg: PathBuf::from(p) })
            .or_else(crate::ffmpeg::find)
            .expect("ffmpeg needed");
        // Test sources are generated with a full ffmpeg on PATH (needs lavfi), which may differ from `tools`.
        let generator = Tools { ffmpeg: PathBuf::from("ffmpeg") };
        let dir = std::env::temp_dir().join("comprimo-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // (file, source filter, seconds, rotation metadata, target MB, must use most of the budget)
        let cases = [
            ("gameplay.mkv", "testsrc2=size=1920x1080:rate=60,noise=alls=12:allf=t", 45, "", 10.0, true),
            ("phone.mp4", "testsrc2=size=1920x1080:rate=30,noise=alls=8:allf=t", 25, "-90", 10.0, true),
            ("short_grainy.mov", "testsrc2=size=1280x720:rate=30,noise=alls=10:allf=t", 12, "", 5.0, true),
            ("pure_noise.mkv", "testsrc2=size=1920x1080:rate=60,noise=alls=40:allf=t", 30, "", 10.0, false),
        ];
        for (name, src, secs, rotate, mb, strict) in cases {
            let input = dir.join(name);
            let raw = dir.join(format!("raw_{name}"));
            let ok = Command::new(&generator.ffmpeg)
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", src, "-f", "lavfi", "-i", "sine=frequency=440"])
                .args(["-t", &secs.to_string(), "-c:v", "libx264", "-preset", "ultrafast", "-crf", "16", "-c:a", "aac"])
                .arg(&raw)
                .status()
                .unwrap()
                .success();
            assert!(ok);
            // Phones store rotation as metadata; remuxing adds it without touching the pixels.
            let mut remux = Command::new(&generator.ffmpeg);
            remux.args(["-v", "error", "-y"]);
            if !rotate.is_empty() {
                remux.args(["-display_rotation", rotate]);
            }
            assert!(remux.arg("-i").arg(&raw).args(["-c", "copy"]).arg(&input).status().unwrap().success());
            std::fs::remove_file(&raw).unwrap();

            let cancel = AtomicBool::new(false);
            let mut report = |e: Event| {
                if let Event::Status(s) | Event::Detail(s) = e {
                    println!("  [{name}] {s}");
                }
            };
            let out = match compress(&tools, &input, mb, &mut report, &cancel) {
                Ok(o) => o,
                Err(e) if !strict => {
                    println!("  [{name}] gave up cleanly: {e:?}");
                    continue;
                }
                Err(e) => panic!("{name}: {e:?}"),
            };
            let size = std::fs::metadata(&out.output).unwrap().len();
            println!("  [{name}] {} -> {}", std::fs::metadata(&input).unwrap().len(), size);
            let target = (mb * 1_000_000.0) as u64;
            assert!(size <= target, "{name} is {size} bytes");
            assert!(!strict || size >= target * 85 / 100, "{name} wasted budget: {size} bytes");
            if !rotate.is_empty() {
                let check = probe(&tools, &out.output).unwrap();
                assert!(check.height > check.width, "{name} should come out portrait: {check:?}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
