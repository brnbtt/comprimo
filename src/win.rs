//! Small Win32 helpers.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{GlobalFree, POINT};
use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::Win32::UI::Shell::{DROPFILES, SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_YESNO, MessageBoxW,
};

const CF_HDROP: u32 = 15;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn error_box(text: &str) {
    unsafe {
        MessageBoxW(null_mut(), wide(text).as_ptr(), wide(crate::paths::APP).as_ptr(), MB_ICONERROR);
    }
}

pub fn info_box(text: &str) {
    unsafe {
        MessageBoxW(null_mut(), wide(text).as_ptr(), wide(crate::paths::APP).as_ptr(), MB_ICONINFORMATION);
    }
}

pub fn confirm(text: &str) -> bool {
    unsafe {
        MessageBoxW(null_mut(), wide(text).as_ptr(), wide(crate::paths::APP).as_ptr(), MB_YESNO | MB_ICONQUESTION)
            == IDYES
    }
}

/// Opts out of legacy injection (global window hooks, AppInit DLLs). Overlays like RivaTuner /
/// MSI Afterburner use these to draw FPS counters into anything that renders with the GPU,
/// which is wrong for a small utility window. Must run before any window is created.
pub fn block_overlay_injection() {
    use windows_sys::Win32::System::Threading::{ProcessExtensionPointDisablePolicy, SetProcessMitigationPolicy};
    const DISABLE_EXTENSION_POINTS: u32 = 1;
    unsafe {
        SetProcessMitigationPolicy(
            ProcessExtensionPointDisablePolicy,
            &DISABLE_EXTENSION_POINTS as *const u32 as *const _,
            std::mem::size_of::<u32>(),
        );
    }
}

/// Puts the process (and every ffmpeg it spawns) in a job that dies with us,
/// so closing the window never leaves a stray encoder running.
pub fn kill_children_on_exit() {
    unsafe {
        let job = CreateJobObjectW(null(), null());
        if job.is_null() {
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        AssignProcessToJobObject(job, GetCurrentProcess());
        // The handle is intentionally leaked: the job lives exactly as long as this process.
    }
}

/// Blends the native title bar into the window: dark caption in our background color,
/// a hairline border and rounded corners (Windows 11; ignored on older versions).
pub fn style_title_bar(hwnd: *mut core::ffi::c_void, caption: u8, border: u8, text: u8) {
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    const DARK_MODE: u32 = 20;
    const CORNERS: u32 = 33;
    const BORDER_COLOR: u32 = 34;
    const CAPTION_COLOR: u32 = 35;
    const TEXT_COLOR: u32 = 36;
    let rgb = |g: u8| g as u32 | (g as u32) << 8 | (g as u32) << 16;
    let set = |attr: u32, value: u32| unsafe {
        DwmSetWindowAttribute(hwnd, attr, &value as *const u32 as *const _, 4);
    };
    set(DARK_MODE, 1);
    set(CORNERS, 2);
    set(BORDER_COLOR, rgb(border));
    set(CAPTION_COLOR, rgb(caption));
    set(TEXT_COLOR, rgb(text));
}

pub fn notify_associations_changed() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as _, SHCNF_IDLIST, null(), null()) };
}

pub fn show_in_folder(path: &Path) {
    let _ = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
}

/// Copies the file itself (like Ctrl+C in Explorer) so it can be pasted straight into Discord.
pub fn copy_file_to_clipboard(path: &Path) -> bool {
    let path_w = wide(&path.display().to_string());
    let header = std::mem::size_of::<DROPFILES>();
    // Path list is double-null terminated.
    let bytes = header + (path_w.len() + 1) * 2;
    unsafe {
        let hmem = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes);
        if hmem.is_null() {
            return false;
        }
        let ptr = GlobalLock(hmem) as *mut u8;
        if ptr.is_null() {
            GlobalFree(hmem);
            return false;
        }
        let df = DROPFILES { pFiles: header as u32, pt: POINT { x: 0, y: 0 }, fNC: 0, fWide: 1 };
        std::ptr::write_unaligned(ptr as *mut DROPFILES, df);
        std::ptr::copy_nonoverlapping(path_w.as_ptr(), ptr.add(header) as *mut u16, path_w.len());
        GlobalUnlock(hmem);

        if OpenClipboard(null_mut()) == 0 {
            GlobalFree(hmem);
            return false;
        }
        EmptyClipboard();
        let ok = !SetClipboardData(CF_HDROP, hmem).is_null();
        CloseClipboard();
        if !ok {
            GlobalFree(hmem);
        }
        ok
    }
}
