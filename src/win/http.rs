//! Minimal HTTPS GET on WinHTTP (system proxy settings, system TLS stack).

use super::wide;
use crate::source::{Http, HttpResponse};
use std::ffi::c_void;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::*;

const UA: &str = concat!(
    "lol-strip/",
    env!("CARGO_PKG_VERSION"),
    " (Windows; +https://github.com/ShelaShelby/lol-strip)"
);
const MAX_BODY: usize = 4 * 1024 * 1024;

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn fail(stage: &str) -> String {
    format!("WinHTTP {} failed (error {})", stage, unsafe { GetLastError() })
}

pub struct WinHttp;

impl Http for WinHttp {
    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, String> {
        let rest = url.strip_prefix("https://").ok_or("only https:// URLs are supported")?;
        let (host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let mut hdr = String::from("Accept: application/json\r\n");
        for (k, v) in headers {
            hdr.push_str(&format!("{}: {}\r\n", k, v));
        }
        unsafe {
            let session = Handle(WinHttpOpen(
                wide(UA).as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                std::ptr::null(),
                std::ptr::null(),
                0,
            ));
            if session.0.is_null() {
                return Err(fail("open"));
            }
            WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 20_000);
            let conn = Handle(WinHttpConnect(session.0, wide(host).as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0));
            if conn.0.is_null() {
                return Err(fail("connect"));
            }
            let req = Handle(WinHttpOpenRequest(
                conn.0,
                wide("GET").as_ptr(),
                wide(path).as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                WINHTTP_FLAG_SECURE,
            ));
            if req.0.is_null() {
                return Err(fail("open request"));
            }
            let hw = wide(&hdr);
            if WinHttpSendRequest(req.0, hw.as_ptr(), u32::MAX, std::ptr::null(), 0, 0, 0) == 0 {
                return Err(fail("send"));
            }
            if WinHttpReceiveResponse(req.0, std::ptr::null_mut()) == 0 {
                return Err(fail("receive"));
            }
            let mut status: u32 = 0;
            let mut len = std::mem::size_of::<u32>() as u32;
            if WinHttpQueryHeaders(
                req.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                std::ptr::null(),
                &mut status as *mut u32 as *mut c_void,
                &mut len,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err(fail("query status"));
            }
            let mut body: Vec<u8> = Vec::new();
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                let mut got: u32 = 0;
                if WinHttpReadData(req.0, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut got) == 0 {
                    return Err(fail("read"));
                }
                if got == 0 {
                    break;
                }
                body.extend_from_slice(&buf[..got as usize]);
                if body.len() > MAX_BODY {
                    return Err("response larger than 4 MB".into());
                }
            }
            Ok(HttpResponse {
                status: status as u16,
                body: String::from_utf8_lossy(&body).into_owned(),
            })
        }
    }
}
