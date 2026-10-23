//! Windows-only parts: HTTP, probe mode, taskbar strip app.

mod app;
mod http;
mod probe;

use crate::logfile::civil;
use crate::timeutil::days_from_civil;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::Time::SystemTimeToTzSpecificLocalTime;
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// %APPDATA%\lol-strip (created on demand).
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let d = base.join("lol-strip");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// UTC seconds -> local wall clock seconds in the Windows time zone
/// (daylight saving of that instant included).
pub fn local_secs(utc: i64) -> i64 {
    let days = utc.div_euclid(86_400);
    let (y, m, d) = civil(days);
    let s = utc.rem_euclid(86_400);
    let st = SYSTEMTIME {
        wYear: y as u16,
        wMonth: m as u16,
        wDayOfWeek: 0,
        wDay: d as u16,
        wHour: (s / 3600) as u16,
        wMinute: ((s / 60) % 60) as u16,
        wSecond: (s % 60) as u16,
        wMilliseconds: 0,
    };
    let mut out: SYSTEMTIME = unsafe { std::mem::zeroed() };
    if unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &st, &mut out) } == 0 {
        return utc;
    }
    days_from_civil(out.wYear as i64, out.wMonth as u32, out.wDay as u32) * 86_400
        + out.wHour as i64 * 3600
        + out.wMinute as i64 * 60
        + out.wSecond as i64
}

pub fn main(args: &[String]) -> i32 {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if args.iter().any(|a| a == "--probe") {
        return probe::run(!args.iter().any(|a| a == "--no-dialog"));
    }
    app::run()
}
