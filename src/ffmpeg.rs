use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::human_size;
use crate::encode::{CREATE_NO_WINDOW, Event, JobError, Stage};
use crate::paths;

enum Archive {
    Zip,
    SevenZip,
}

/// Where ffmpeg comes from, tried in order. Only `ffmpeg.exe` is extracted.
const SOURCES: [(&str, Archive); 3] = [
    // Our own minimal build (a few MB), see ffmpeg-slim/build.sh.
    ("https://github.com/brnbtt/comprimo/releases/download/ffmpeg/comprimo-ffmpeg-win64.zip", Archive::Zip),
    // Full builds as fallbacks: ~34 MB and ~200 MB downloads.
    ("https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.7z", Archive::SevenZip),
    ("https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip", Archive::Zip),
];

#[derive(Clone, Debug)]
pub struct Tools {
    pub ffmpeg: PathBuf,
}

pub fn find_local() -> Option<Tools> {
    let exe = paths::ffmpeg_dir().join("ffmpeg.exe");
    exe.is_file().then_some(Tools { ffmpeg: exe })
}

/// Our own copy first, then a suitable ffmpeg already on PATH.
pub fn find() -> Option<Tools> {
    find_local().or_else(|| {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join("ffmpeg.exe"))
            .find(|exe| exe.is_file() && has_x264(exe))
            .map(|ffmpeg| Tools { ffmpeg })
    })
}

fn has_x264(exe: &Path) -> bool {
    Command::new(exe)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("libx264"))
        .unwrap_or(false)
}

pub fn ensure(report: &mut dyn FnMut(Event), cancel: &AtomicBool) -> Result<Tools, JobError> {
    match find() {
        Some(t) => Ok(t),
        None => download(report, cancel),
    }
}

pub fn download(report: &mut dyn FnMut(Event), cancel: &AtomicBool) -> Result<Tools, JobError> {
    let dir = paths::ffmpeg_dir();
    fs::create_dir_all(&dir)?;
    let archive_path = dir.join("download.part");
    let target = dir.join("ffmpeg.exe");

    let mut errors = Vec::new();
    for (url, kind) in &SOURCES {
        report(Event::Status("downloading ffmpeg · one-time setup".into()));
        report(Event::Stage { stage: Stage::Preparing, attempt: 0 });
        report(Event::Progress(0.0));
        report(Event::Detail(String::new()));
        let result = download_file(url, &archive_path, report, cancel).and_then(|_| {
            report(Event::Status("unpacking ffmpeg".into()));
            report(Event::Detail(String::new()));
            let tmp = dir.join("ffmpeg.exe.tmp");
            match kind {
                Archive::Zip => extract_zip(&archive_path, &tmp),
                Archive::SevenZip => extract_7z(&archive_path, &tmp, cancel),
            }?;
            if !has_x264(&tmp) {
                let _ = fs::remove_file(&tmp);
                return Err("downloaded ffmpeg doesn't work on this PC".into());
            }
            fs::rename(&tmp, &target)?;
            Ok(())
        });
        let _ = fs::remove_file(&archive_path);
        match result {
            Ok(()) => return Ok(Tools { ffmpeg: target }),
            Err(JobError::Cancelled) => return Err(JobError::Cancelled),
            Err(JobError::Failed(e)) => errors.push(format!("{url}\n  {e}")),
        }
    }
    Err(format!("Couldn't download ffmpeg. Check your internet connection and try again.\n\n{}", errors.join("\n"))
        .into())
}

fn download_file(url: &str, dest: &Path, report: &mut dyn FnMut(Event), cancel: &AtomicBool) -> Result<(), JobError> {
    let response = ureq::get(url).call().map_err(|e| e.to_string())?;
    let total = response.body().content_length();
    let mut reader = response.into_body().into_with_config().limit(u64::MAX).reader();
    let mut file = File::create(dest)?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done: u64 = 0;
    let mut last_report: u64 = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(JobError::Cancelled);
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        if done - last_report >= 500_000 {
            last_report = done;
            match total {
                Some(t) if t > 0 => {
                    report(Event::Progress(done as f32 / t as f32));
                    report(Event::Detail(format!("{} / {}", human_size(done), human_size(t))));
                }
                _ => report(Event::Detail(human_size(done))),
            }
        }
    }
    file.flush()?;
    Ok(())
}

fn is_ffmpeg_exe(name: &str) -> bool {
    let name = name.replace('\\', "/").to_ascii_lowercase();
    name == "ffmpeg.exe" || name.ends_with("/ffmpeg.exe")
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<(), JobError> {
    let mut zip = zip::ZipArchive::new(File::open(archive)?).map_err(|e| e.to_string())?;
    let index = (0..zip.len())
        .find(|&i| zip.by_index_raw(i).is_ok_and(|e| e.name().is_ok_and(|n| is_ffmpeg_exe(&n))))
        .ok_or("ffmpeg.exe not found in the download")?;
    let mut entry = zip.by_index(index).map_err(|e| e.to_string())?;
    std::io::copy(&mut entry, &mut File::create(dest)?)?;
    Ok(())
}

fn extract_7z(archive: &Path, dest: &Path, cancel: &AtomicBool) -> Result<(), JobError> {
    use sevenz_rust2::{ArchiveReader, EntryDecision, Password};
    let mut reader = ArchiveReader::open(archive, Password::empty()).map_err(|e| e.to_string())?;
    let mut found = false;
    let mut io_error = None;
    reader
        .for_each_entries_with_decision(|entry, data| {
            if cancel.load(Ordering::Relaxed) {
                return Ok(EntryDecision::Stop);
            }
            if entry.is_directory() || !is_ffmpeg_exe(entry.name()) {
                return Ok(EntryDecision::Continue);
            }
            match File::create(dest).and_then(|mut f| std::io::copy(data, &mut f)) {
                Ok(_) => found = true,
                Err(e) => io_error = Some(e),
            }
            Ok(EntryDecision::Stop)
        })
        .map_err(|e| e.to_string())?;
    if cancel.load(Ordering::Relaxed) {
        return Err(JobError::Cancelled);
    }
    if let Some(e) = io_error {
        return Err(e.into());
    }
    if !found {
        return Err("ffmpeg.exe not found in the download".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ffmpeg_exe_only() {
        assert!(is_ffmpeg_exe("ffmpeg.exe"));
        assert!(is_ffmpeg_exe("ffmpeg-8.0-essentials_build/bin/ffmpeg.exe"));
        assert!(!is_ffmpeg_exe("ffmpeg-8.0-essentials_build/bin/ffprobe.exe"));
        assert!(!is_ffmpeg_exe("bin/notffmpeg.exe"));
    }

    /// Downloads and unpacks the gyan.dev 7z: `cargo test --release seven_zip -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn seven_zip_fallback() {
        let dir = std::env::temp_dir().join("comprimo-7z-test");
        fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("ffmpeg.7z");
        let cancel = AtomicBool::new(false);
        download_file(SOURCES[1].0, &archive, &mut |_| {}, &cancel).unwrap();
        let exe = dir.join("ffmpeg.exe");
        let start = std::time::Instant::now();
        extract_7z(&archive, &exe, &cancel).unwrap();
        println!("extracted {} in {:.1?}", human_size(fs::metadata(&exe).unwrap().len()), start.elapsed());
        assert!(has_x264(&exe));
        fs::remove_dir_all(&dir).unwrap();
    }
}
