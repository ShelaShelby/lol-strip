//! The taskbar strip application: one layered click-through window per
//! monitor, tray icon + menu, background match fetcher.

use super::http::WinHttp;
use super::{data_dir, local_secs, now_secs, wide};
use crate::display;
use crate::engine::{self, Notifier, Snapshot};
use crate::layout::{self, Rect};
use crate::logfile;
use crate::model::ALL_KEYS;
use crate::render::{self, Canvas};
use crate::settings::Settings;
use crate::timeutil;
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_APP_TRAY: u32 = WM_APP + 1;
const WM_APP_DATA: u32 = WM_APP + 2;
const WM_APP_RELAYOUT: u32 = WM_APP + 3;
const TIMER_TICK: usize = 1;
const TICK_MS: u32 = 2000;

const CMD_NOTIFY: u32 = 10;
const CMD_AUTOSTART: u32 = 12;
const CMD_REFRESH: u32 = 13;
const CMD_OPEN: u32 = 14;
const CMD_PROBE: u32 = 15;
const CMD_EXIT: u32 = 99;
const CMD_LEAGUE0: u32 = 30;
const CMD_MONITOR0: u32 = 100;

const SCHEDULE_URL: &str = "https://lolesports.com/en-US/schedule";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "LoLStrip";

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn main_hwnd() -> HWND {
    MAIN_HWND.load(Ordering::SeqCst) as HWND
}

struct Monitor {
    handle: HMONITOR,
    rect: Rect,
    name: String,
    dpi: u32,
}

struct Taskbar {
    rect: Rect,
    tray: Option<Rect>,
    monitor: HMONITOR,
}

struct Strip {
    name: String,
    hwnd: HWND,
    shown: bool,
    key: String,
}

struct App {
    hwnd: HWND,
    hinst: HINSTANCE,
    font: Option<fontdue::Font>,
    settings: Settings,
    settings_path: PathBuf,
    log_path: PathBuf,
    shared: Arc<Mutex<Snapshot>>,
    wake: mpsc::Sender<()>,
    snap: Snapshot,
    notifier: Notifier,
    strips: Vec<Strip>,
    monitors: Vec<(String, bool)>,
    icon: HICON,
    tip: String,
}

fn rect_of(r: &RECT) -> Rect {
    Rect::new(r.left, r.top, r.right, r.bottom)
}

fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn window_rect(h: HWND) -> Option<Rect> {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetWindowRect(h, &mut r) } == 0 {
        None
    } else {
        Some(rect_of(&r))
    }
}

unsafe extern "system" fn enum_monitor(h: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> BOOL {
    let v = &mut *(data as *mut Vec<Monitor>);
    let mut mi: MONITORINFOEXW = std::mem::zeroed();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(h, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) != 0 {
        let n = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
        let (mut dx, mut dy) = (96u32, 96u32);
        if GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) != 0 {
            dx = 96;
        }
        v.push(Monitor {
            handle: h,
            rect: rect_of(&mi.monitorInfo.rcMonitor),
            name: String::from_utf16_lossy(&mi.szDevice[..n]),
            dpi: dx,
        });
    }
    1
}

fn monitors() -> Vec<Monitor> {
    let mut v: Vec<Monitor> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(enum_monitor),
            &mut v as *mut Vec<Monitor> as LPARAM,
        )
    };
    v
}

unsafe extern "system" fn enum_secondary(h: HWND, data: LPARAM) -> BOOL {
    if class_name(h) == "Shell_SecondaryTrayWnd" {
        let v = &mut *(data as *mut Vec<HWND>);
        v.push(h);
    }
    1
}

fn taskbars() -> Vec<Taskbar> {
    let mut hwnds = Vec::new();
    let primary = unsafe { FindWindowW(wide("Shell_TrayWnd").as_ptr(), std::ptr::null()) };
    if !primary.is_null() {
        hwnds.push(primary);
    }
    let mut sec: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(enum_secondary), &mut sec as *mut Vec<HWND> as LPARAM) };
    hwnds.extend(sec);
    let mut out = Vec::new();
    for h in hwnds {
        let Some(rect) = window_rect(h) else { continue };
        let notify = unsafe { FindWindowExW(h, std::ptr::null_mut(), wide("TrayNotifyWnd").as_ptr(), std::ptr::null()) };
        let tray = if notify.is_null() { None } else { window_rect(notify) };
        let monitor = unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) };
        out.push(Taskbar { rect, tray, monitor });
    }
    out
}

/// Monitors on which a fullscreen app is in the foreground; `all` for
/// exclusive D3D fullscreen / presentation mode.
fn fullscreen(own: &[HWND]) -> (Option<HMONITOR>, bool) {
    let mut st: QUERY_USER_NOTIFICATION_STATE = 0;
    let all = unsafe { SHQueryUserNotificationState(&mut st) } == 0 && (st == QUNS_RUNNING_D3D_FULL_SCREEN || st == QUNS_PRESENTATION_MODE);
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_null() || own.contains(&fg) || unsafe { IsWindowVisible(fg) } == 0 || unsafe { IsIconic(fg) } != 0 {
        return (None, all);
    }
    let cls = class_name(fg);
    if matches!(cls.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
        return (None, all);
    }
    let Some(r) = window_rect(fg) else {
        return (None, all);
    };
    let mon = unsafe { MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) };
    let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(mon, &mut mi) } == 0 {
        return (None, all);
    }
    if r.contains(&rect_of(&mi.rcMonitor)) {
        (Some(mon), all)
    } else {
        (None, all)
    }
}

fn present(hwnd: HWND, c: &Canvas, x: i32, y: i32) -> bool {
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = c.w as i32;
        bmi.bmiHeader.biHeight = -(c.h as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let mut ok = false;
        if !bmp.is_null() && !bits.is_null() {
            std::ptr::copy_nonoverlapping(c.px.as_ptr(), bits as *mut u8, c.px.len());
            let old = SelectObject(mem, bmp);
            let pt = POINT { x, y };
            let size = SIZE {
                cx: c.w as i32,
                cy: c.h as i32,
            };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            ok = UpdateLayeredWindow(hwnd, screen, &pt, &size, mem, &src, 0, &blend, ULW_ALPHA) != 0;
            SelectObject(mem, old);
            DeleteObject(bmp);
        }
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        ok
    }
}

fn make_icon() -> HICON {
    let s = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64);
    let mut px = vec![0u8; (s * s * 4) as usize];
    let dark = [0x10, 0x18, 0x30, 255];
    let blue = [0x2D, 0x6C, 0xDF, 255];
    let white = [0xF5, 0xF5, 0xF5, 255];
    let sf = s as f32;
    for y in 0..s {
        for x in 0..s {
            let edge = x == 0 || y == 0 || x == s - 1 || y == s - 1;
            let (fx, fy) = (x as f32 / sf, y as f32 / sf);
            // right-pointing triangle ("next / play")
            let tri = fx >= 0.36 && (fy - 0.5).abs() <= (0.76 - fx) * 0.7;
            let c = if edge {
                dark
            } else if tri {
                white
            } else {
                blue
            };
            let i = ((y * s + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[c[2], c[1], c[0], c[3]]);
        }
    }
    unsafe {
        let dc = GetDC(std::ptr::null_mut());
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = s;
        bmi.bmiHeader.biHeight = -s;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        ReleaseDC(std::ptr::null_mut(), dc);
        if color.is_null() || bits.is_null() {
            return LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
        }
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
        let mask_bits = vec![0u8; (((s + 15) / 16) * 2 * s) as usize];
        let mask = CreateBitmap(s, s, 1, 1, mask_bits.as_ptr() as *const _);
        let ii = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&ii);
        DeleteObject(color);
        DeleteObject(mask);
        if icon.is_null() {
            LoadIconW(std::ptr::null_mut(), IDI_APPLICATION)
        } else {
            icon
        }
    }
}

fn set_wstr(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

fn autostart_enabled() -> bool {
    let k = wide(RUN_KEY);
    let n = wide(RUN_NAME);
    let mut size = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            n.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        ) == ERROR_SUCCESS
    }
}

fn set_autostart(on: bool) -> Result<(), String> {
    let k = wide(RUN_KEY);
    let n = wide(RUN_NAME);
    let r = if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let v = wide(&format!("\"{}\"", exe.display()));
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                k.as_ptr(),
                n.as_ptr(),
                REG_SZ,
                v.as_ptr() as *const _,
                (v.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr()) }
    };
    if r == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("registry error {}", r))
    }
}
impl App {
    fn log(&self, s: String) {
        logfile::append(&self.log_path, &[s]);
    }

    fn save_settings(&self) {
        if let Err(e) = std::fs::write(&self.settings_path, self.settings.serialize()) {
            self.log(format!("saving settings failed: {}", e));
        }
    }

    /// Matches of the enabled leagues, sorted.
    fn upcoming(&self) -> Vec<crate::model::Match> {
        engine::enabled(&self.snap.matches, &self.settings)
    }

    fn lines(&self) -> Vec<display::Line> {
        display::lines(&self.upcoming(), now_secs(), &local_secs, self.snap.stale)
    }

    fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = self.icon;
        set_wstr(&mut nid.szTip, &self.tip);
        nid
    }

    fn make_tip(&self) -> String {
        let l: Vec<String> = self.lines().into_iter().map(|l| l.text).collect();
        format!("LoL Strip\n{}", l.join("\n"))
    }

    fn add_tray(&self) {
        let nid = self.tray_data();
        unsafe {
            if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
                Shell_NotifyIconW(NIM_MODIFY, &nid);
            }
        }
    }

    fn update_tray(&mut self) {
        let tip = self.make_tip();
        if tip == self.tip {
            return;
        }
        self.tip = tip;
        let nid = self.tray_data();
        unsafe {
            if Shell_NotifyIconW(NIM_MODIFY, &nid) == 0 {
                Shell_NotifyIconW(NIM_ADD, &nid);
            }
        }
    }

    fn balloon(&self, title: &str, text: &str) {
        let mut nid = self.tray_data();
        nid.uFlags |= NIF_INFO;
        nid.dwInfoFlags = NIIF_INFO;
        set_wstr(&mut nid.szInfoTitle, title);
        set_wstr(&mut nid.szInfo, text);
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
    }

    fn check_notify(&mut self) {
        let now = now_secs();
        let list = self.upcoming();
        for m in self.notifier.due(&list, now, self.settings.notify) {
            let mins = ((m.start - now) + 59) / 60;
            let bo = if m.best_of > 0 {
                format!(", Bo{}", m.best_of)
            } else {
                String::new()
            };
            self.balloon(
                &format!("{}: {} v {}", m.league.code(), m.t1, m.t2),
                &format!("Starts in {} min ({}{})", mins, timeutil::hhmm(local_secs(m.start)), bo),
            );
            self.log(format!("notify: {} {} v {} in {} min", m.league.code(), m.t1, m.t2, mins));
        }
    }

    fn tick(&mut self) {
        self.check_notify();
        self.refresh();
    }

    fn strip_for(&mut self, name: &str) -> Option<usize> {
        if let Some(i) = self.strips.iter().position(|s| s.name == name) {
            return Some(i);
        }
        let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let cls = wide("LolStripBar");
        let title = wide("LoL Strip");
        let h = unsafe {
            CreateWindowExW(
                ex,
                cls.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                self.hinst,
                std::ptr::null(),
            )
        };
        if h.is_null() {
            self.log(format!("CreateWindowEx failed: {}", unsafe { GetLastError() }));
            return None;
        }
        self.strips.push(Strip {
            name: name.to_string(),
            hwnd: h,
            shown: false,
            key: String::new(),
        });
        Some(self.strips.len() - 1)
    }

    fn hide(&mut self, i: usize) {
        let s = &mut self.strips[i];
        if s.shown {
            unsafe { ShowWindow(s.hwnd, SW_HIDE) };
            s.shown = false;
        }
    }

    /// Re-place / re-render every strip. Cheap enough to run every tick.
    fn refresh(&mut self) {
        let mons = monitors();
        let tbs = taskbars();
        let own: Vec<HWND> = self.strips.iter().map(|s| s.hwnd).collect();
        let (fs_mon, fs_all) = fullscreen(&own);
        self.monitors = mons
            .iter()
            .map(|m| (m.name.clone(), self.settings.monitor_enabled(&m.name)))
            .collect();
        // Drop strips of monitors that disappeared.
        let mut k = 0;
        while k < self.strips.len() {
            if mons.iter().any(|m| m.name == self.strips[k].name) {
                k += 1;
            } else {
                let s = self.strips.remove(k);
                unsafe { DestroyWindow(s.hwnd) };
            }
        }
        let lines = self.lines();
        for m in &mons {
            let tb = tbs.iter().find(|t| t.monitor == m.handle);
            let want = self.settings.monitor_enabled(&m.name) && tb.is_some() && !fs_all && fs_mon != Some(m.handle) && self.font.is_some();
            let Some(i) = self.strip_for(&m.name) else {
                continue;
            };
            if !want {
                self.hide(i);
                continue;
            }
            let tb = tb.unwrap();
            let Some(vis) = layout::visible_taskbar(&tb.rect, &m.rect) else {
                self.hide(i);
                continue;
            };
            let horizontal = layout::edge(&vis, &m.rect).horizontal();
            let thickness = if horizontal { vis.h() } else { vis.w() };
            let px = layout::font_px(thickness, lines.len());
            let box_h = if horizontal {
                thickness as u32
            } else {
                (px * 1.4 * lines.len() as f32).ceil() as u32 + 4
            };
            let font = self.font.as_ref().unwrap();
            let canvas = render::render_strip(font, &lines, px, box_h, layout::outline_px(m.dpi));
            let off = (
                layout::scale(self.settings.fallback_offset_x, m.dpi),
                layout::scale(self.settings.fallback_offset_y, m.dpi),
            );
            let Some(p) = layout::place(
                &tb.rect,
                &m.rect,
                tb.tray.as_ref(),
                canvas.w as i32,
                canvas.h as i32,
                layout::scale(6, m.dpi),
                off,
            ) else {
                self.hide(i);
                continue;
            };
            let key = format!("{:?}|{:?}|{}", lines, p.rect, m.dpi);
            let place_key = format!("{:?}|{}", p.rect, p.used_tray);
            if !self.strips[i].key.contains(&place_key) {
                let how = if p.used_tray {
                    "left of tray area"
                } else {
                    "fallback offset (tray area not found)"
                };
                self.log(format!(
                    "{}: strip at {:?}, {}, taskbar {:?}, dpi {}",
                    m.name, p.rect, how, tb.rect, m.dpi
                ));
            }
            let key = format!("{}|{}", key, place_key);
            let s = &mut self.strips[i];
            if s.key != key || !s.shown {
                // Placement may clip the canvas; keep the canvas size.
                if present(s.hwnd, &canvas, p.rect.l, p.rect.t) {
                    s.key = key;
                } else {
                    s.key.clear();
                }
                if !s.shown {
                    unsafe { ShowWindow(s.hwnd, SW_SHOWNOACTIVATE) };
                    s.shown = true;
                }
            }
            // Stay above the taskbar in the topmost band.
            unsafe {
                SetWindowPos(
                    s.hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
                )
            };
        }
    }

    fn take_snapshot(&mut self) {
        let s = match self.shared.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        };
        self.snap = s;
        self.update_tray();
        self.tick();
    }

    fn menu_items(&self) -> Vec<Item> {
        let now = now_secs();
        let list = self.upcoming();
        let mut next: Vec<Item> = list
            .iter()
            .filter(|m| m.is_open(now))
            .take(12)
            .map(|m| Item::Info(display::menu_text(m, now, &local_secs)))
            .collect();
        if next.is_empty() {
            next.push(Item::Info("No matches known".into()));
        }
        let mut v = vec![Item::Sub("Next matches".into(), next)];
        let leagues: Vec<Item> = ALL_KEYS
            .iter()
            .enumerate()
            .map(|(i, k)| Item::Check(CMD_LEAGUE0 + i as u32, k.code().to_string(), self.settings.league_on(*k)))
            .collect();
        v.push(Item::Sub("Leagues".into(), leagues));
        for (i, (name, on)) in self.monitors.iter().enumerate() {
            v.push(Item::Check(
                CMD_MONITOR0 + i as u32,
                format!("Strip on monitor {} ({})", i + 1, name.trim_start_matches(r"\\.\")),
                *on,
            ));
        }
        v.push(Item::Sep);
        v.push(Item::Check(CMD_NOTIFY, "Notify 10 min before".into(), self.settings.notify));
        v.push(Item::Check(CMD_AUTOSTART, "Start with Windows".into(), autostart_enabled()));
        v.push(Item::Sep);
        v.push(Item::Check(CMD_OPEN, "Open schedule".into(), false));
        v.push(Item::Check(CMD_REFRESH, "Refresh now".into(), false));
        v.push(Item::Check(CMD_PROBE, "Run probe (writes probe.txt)".into(), false));
        let status = if self.snap.fetched_at == 0 {
            "No data yet".to_string()
        } else {
            format!(
                "Data: {}, {}{}",
                self.snap.source,
                timeutil::hhmm(local_secs(self.snap.fetched_at)),
                if self.snap.stale { " (last refresh failed)" } else { "" }
            )
        };
        v.push(Item::Sep);
        v.push(Item::Info(status));
        v.push(Item::Sep);
        v.push(Item::Check(CMD_EXIT, "Exit".into(), false));
        v
    }

    /// Returns true when the app should exit.
    fn command(&mut self, cmd: u32) -> bool {
        match cmd {
            CMD_NOTIFY => {
                self.settings.notify = !self.settings.notify;
                self.save_settings();
            }
            CMD_AUTOSTART => {
                if let Err(e) = set_autostart(!autostart_enabled()) {
                    self.log(format!("autostart change failed: {}", e));
                }
            }
            CMD_REFRESH => {
                let _ = self.wake.send(());
            }
            CMD_OPEN => unsafe {
                ShellExecuteW(
                    std::ptr::null_mut(),
                    wide("open").as_ptr(),
                    wide(SCHEDULE_URL).as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    SW_SHOWNORMAL,
                );
            },
            CMD_PROBE => {
                if let Ok(exe) = std::env::current_exe() {
                    if let Err(e) = std::process::Command::new(exe).arg("--probe").spawn() {
                        self.log(format!("starting probe failed: {}", e));
                    }
                }
            }
            CMD_EXIT => return true,
            c if (CMD_LEAGUE0..CMD_LEAGUE0 + ALL_KEYS.len() as u32).contains(&c) => {
                let k = ALL_KEYS[(c - CMD_LEAGUE0) as usize];
                let on = !self.settings.league_on(k);
                self.settings.set_league(k, on);
                self.save_settings();
                self.update_tray();
            }
            c if c >= CMD_MONITOR0 => {
                if let Some((name, _)) = self.monitors.get((c - CMD_MONITOR0) as usize).cloned() {
                    self.settings.toggle_monitor(&name);
                    self.save_settings();
                }
            }
            _ => {}
        }
        self.refresh();
        false
    }
}

enum Item {
    Check(u32, String, bool),
    Info(String),
    Sep,
    Sub(String, Vec<Item>),
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|c| match c.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        Err(_) => None,
    })
}

fn menu_text(s: &str) -> Vec<u16> {
    wide(&s.replace('&', "&&"))
}

unsafe fn build_menu(items: &[Item]) -> HMENU {
    let menu = CreatePopupMenu();
    for it in items {
        match it {
            Item::Sep => {
                AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            }
            Item::Info(t) => {
                AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, menu_text(t).as_ptr());
            }
            Item::Check(id, t, checked) => {
                AppendMenuW(
                    menu,
                    MF_STRING | if *checked { MF_CHECKED } else { MF_UNCHECKED },
                    *id as usize,
                    menu_text(t).as_ptr(),
                );
            }
            Item::Sub(t, v) => {
                let sub = build_menu(v);
                AppendMenuW(menu, MF_STRING | MF_POPUP, sub as usize, menu_text(t).as_ptr());
            }
        }
    }
    menu
}

fn show_menu(h: HWND) {
    let Some(items) = with_app(|a| a.menu_items()) else {
        return;
    };
    unsafe {
        let menu = build_menu(&items);
        if menu.is_null() {
            return;
        }
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(h);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            pt.x,
            pt.y,
            0,
            h,
            std::ptr::null(),
        );
        PostMessageW(h, WM_NULL, 0, 0);
        DestroyMenu(menu);
        if cmd > 0 && with_app(|a| a.command(cmd as u32)) == Some(true) {
            DestroyWindow(h);
        }
    }
}

unsafe extern "system" fn main_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let tbc = TASKBAR_CREATED.load(Ordering::SeqCst);
    match msg {
        WM_APP_TRAY => {
            let ev = (l as u32) & 0xFFFF;
            if ev == WM_RBUTTONUP || ev == WM_LBUTTONUP || ev == WM_CONTEXTMENU {
                show_menu(h);
            }
            0
        }
        WM_APP_DATA => {
            if with_app(|a| a.take_snapshot()).is_none() {
                PostMessageW(h, WM_APP_DATA, 0, 0);
            }
            0
        }
        WM_TIMER => {
            with_app(|a| {
                a.update_tray();
                a.tick();
            });
            0
        }
        WM_APP_RELAYOUT | WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED => {
            with_app(|a| a.refresh());
            if msg == WM_APP_RELAYOUT {
                0
            } else {
                DefWindowProcW(h, msg, w, l)
            }
        }
        WM_DESTROY => {
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = h;
            nid.uID = 1;
            Shell_NotifyIconW(NIM_DELETE, &nid);
            PostQuitMessage(0);
            0
        }
        m if tbc != 0 && m == tbc => {
            // Explorer restarted: re-add tray icon, re-place strips.
            with_app(|a| {
                a.log("Explorer restarted (TaskbarCreated)".into());
                a.add_tray();
                for s in &mut a.strips {
                    s.key.clear();
                }
                a.refresh();
            });
            0
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

unsafe extern "system" fn strip_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            PostMessageW(main_hwnd(), WM_APP_RELAYOUT, 0, 0);
            0
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

unsafe extern "system" fn on_foreground(_h: HWINEVENTHOOK, _ev: u32, _w: HWND, _o: i32, _c: i32, _t: u32, _ms: u32) {
    PostMessageW(main_hwnd(), WM_APP_RELAYOUT, 0, 0);
}

fn spawn_fetcher(hwnd: HWND, shared: Arc<Mutex<Snapshot>>, log_path: PathBuf, cache_path: PathBuf, rx: mpsc::Receiver<()>) {
    let hwnd_val = hwnd as isize;
    std::thread::spawn(move || {
        let sources = crate::source::default_sources();
        let mut last_msg = String::new();
        loop {
            let prev = match shared.lock() {
                Ok(g) => g.clone(),
                Err(p) => p.into_inner().clone(),
            };
            let now = now_secs();
            let snap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| engine::refresh(&sources, &WinHttp, now, &prev)))
                .unwrap_or_else(|_| Snapshot {
                    stale: true,
                    error: "refresh failed unexpectedly (panic)".into(),
                    ..prev.clone()
                });
            let msg = if snap.stale {
                format!("refresh failed, keeping {} cached matches: {}", snap.matches.len(), snap.error)
            } else {
                format!("refresh ok via {}: {} matches", snap.source, snap.matches.len())
            };
            if msg != last_msg {
                logfile::append(&log_path, std::slice::from_ref(&msg));
                last_msg = msg;
            }
            if !snap.stale {
                let _ = std::fs::write(&cache_path, engine::cache_to_string(&snap));
            }
            let wait = engine::next_interval(&snap, now_secs());
            match shared.lock() {
                Ok(mut g) => *g = snap,
                Err(p) => *p.into_inner() = snap,
            }
            unsafe { PostMessageW(hwnd_val as HWND, WM_APP_DATA, 0, 0) };
            match rx.recv_timeout(Duration::from_secs(wait)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
}

pub fn run() -> i32 {
    unsafe {
        let mname = wide("Local\\LolStrip.SingleInstance");
        let mutex = CreateMutexW(std::ptr::null(), 0, mname.as_ptr());
        if !mutex.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
            return 0;
        }
        let dir = data_dir();
        let log_path = dir.join("log.txt");
        let settings_path = dir.join("settings.ini");
        let cache_path = dir.join("cache.json");
        let mut settings = std::fs::read_to_string(&settings_path)
            .map(|t| Settings::parse(&t))
            .unwrap_or_default();
        logfile::append(&log_path, &[format!("LoL Strip {} started", env!("CARGO_PKG_VERSION"))]);
        // "Start with Windows" is ON by default (first run only); afterwards
        // the menu decides. When it is on, re-write the Run entry so it
        // follows the exe if it was moved.
        if !settings.autostart_init {
            match set_autostart(true) {
                Ok(()) => {
                    settings.autostart_init = true;
                    let _ = std::fs::write(&settings_path, settings.serialize());
                    logfile::append(&log_path, &["autostart switched on (first run)".to_string()]);
                }
                Err(e) => logfile::append(&log_path, &[format!("autostart default failed: {}", e)]),
            }
        } else if autostart_enabled() {
            let _ = set_autostart(true);
        }
        let font = match render::load_font() {
            Ok(f) => Some(f),
            Err(e) => {
                logfile::append(&log_path, &[format!("font: {}", e)]);
                None
            }
        };
        let cached = std::fs::read_to_string(&cache_path).ok().and_then(|t| engine::cache_from_str(&t));
        let snap = cached.unwrap_or(Snapshot {
            stale: true,
            ..Snapshot::default()
        });

        let hinst = GetModuleHandleW(std::ptr::null());
        let main_cls = wide("LolStripMain");
        let strip_cls = wide("LolStripBar");
        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(main_proc);
        wc.hInstance = hinst;
        wc.lpszClassName = main_cls.as_ptr();
        RegisterClassExW(&wc);
        let mut wc2: WNDCLASSEXW = std::mem::zeroed();
        wc2.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc2.lpfnWndProc = Some(strip_proc);
        wc2.hInstance = hinst;
        wc2.lpszClassName = strip_cls.as_ptr();
        RegisterClassExW(&wc2);

        TASKBAR_CREATED.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), Ordering::SeqCst);
        let title = wide("LoL Strip");
        // Hidden top-level window (not message-only: it must receive the
        // TaskbarCreated / WM_DISPLAYCHANGE broadcasts).
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            main_cls.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            logfile::append(&log_path, &[format!("main window creation failed: {}", GetLastError())]);
            return 1;
        }
        MAIN_HWND.store(hwnd as isize, Ordering::SeqCst);

        let shared = Arc::new(Mutex::new(snap.clone()));
        let (tx, rx) = mpsc::channel();
        let mut app = App {
            hwnd,
            hinst,
            font,
            settings,
            settings_path,
            log_path: log_path.clone(),
            shared: shared.clone(),
            wake: tx,
            snap,
            notifier: Notifier::default(),
            strips: Vec::new(),
            monitors: Vec::new(),
            icon: make_icon(),
            tip: String::new(),
        };
        app.tip = app.make_tip();
        app.add_tray();
        APP.with(|c| *c.borrow_mut() = Some(app));
        with_app(|a| a.refresh());

        SetTimer(hwnd, TIMER_TICK, TICK_MS, None);
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            std::ptr::null_mut(),
            Some(on_foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        spawn_fetcher(hwnd, shared, log_path, cache_path, rx);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|c| {
            if let Ok(mut g) = c.try_borrow_mut() {
                if let Some(a) = g.take() {
                    a.log("exit".into());
                    for s in &a.strips {
                        DestroyWindow(s.hwnd);
                    }
                }
            }
        });
        0
    }
}
