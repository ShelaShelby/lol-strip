//! `--probe`: write probe.txt next to the exe with what each source returns.

use super::http::WinHttp;
use super::{local_secs, now_secs, wide};
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};

pub fn run(dialog: bool) -> i32 {
    let now = now_secs();
    let mut out: Vec<String> = vec![
        format!("LoL Strip {} probe", env!("CARGO_PKG_VERSION")),
        format!("Time: {}", crate::logfile::format_utc(now)),
        format!(
            "Local time: {} (UTC offset {} min)",
            crate::timeutil::full(local_secs(now)),
            (local_secs(now) - now) / 60
        ),
        "Read-only HTTPS GET requests, every source in the order the app tries them.".into(),
        String::new(),
    ];
    out.extend(crate::probe::report(&crate::source::default_sources(), &WinHttp, now, &local_secs));
    let text = out.join("\r\n") + "\r\n";
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let mut written = None;
    if let Some(d) = exe_dir {
        let p = d.join("probe.txt");
        if std::fs::write(&p, &text).is_ok() {
            written = Some(p);
        }
    }
    if written.is_none() {
        let p = super::data_dir().join("probe.txt");
        if std::fs::write(&p, &text).is_ok() {
            written = Some(p);
        }
    }
    if dialog {
        let msg = match &written {
            Some(p) => format!("Probe finished.\n\nResults written to:\n{}\n\nPlease send this file.", p.display()),
            None => "Probe finished but probe.txt could not be written.".into(),
        };
        let m = wide(&msg);
        let t = wide("LoL Strip probe");
        unsafe { MessageBoxW(std::ptr::null_mut(), m.as_ptr(), t.as_ptr(), MB_OK | MB_ICONINFORMATION) };
    }
    if written.is_some() {
        0
    } else {
        1
    }
}
