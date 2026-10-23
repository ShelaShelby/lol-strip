//! `--preview <dir>`: software-rendered PNGs of the strip on light and dark
//! taskbars, using the same renderer and placement maths as the app.

use crate::display::{self, Line, Rgb};
use crate::layout::{self, Rect};
use crate::model::{mk, LeagueKey, Match};
use crate::render::{self, Image};
use crate::timeutil::parse_utc;

fn cases(now: i64) -> Vec<(String, Vec<Line>)> {
    let local = |t: i64| t + 7200;
    let l = |ms: &[Match], stale: bool| display::lines(ms, now, &local, stale);
    let m = |id: &str, k: LeagueKey, a: &str, b: &str, dt: i64, live: bool| mk(id, k, a, b, now + dt, live);
    vec![
        (
            "next matches".into(),
            l(
                &[
                    m("1", LeagueKey::Lck, "T1", "GEN", 3 * 3600, false),
                    m("2", LeagueKey::Lck, "HLE", "DK", 30 * 3600, false),
                ],
                false,
            ),
        ),
        (
            "live + soon".into(),
            l(
                &[
                    m("1", LeagueKey::Lec, "G2", "FNC", -600, true),
                    m("2", LeagueKey::Lec, "KC", "VIT", 600, false),
                ],
                false,
            ),
        ),
        ("stale".into(), l(&[m("1", LeagueKey::Intl, "BLG", "JDG", 7200, false)], true)),
        ("empty".into(), l(&[], false)),
    ]
}

pub fn run(dir: &str) -> Result<Vec<String>, String> {
    let font = render::load_font()?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let now = parse_utc("2026-10-25T10:00:00Z").ok_or("time")?;
    let cases = cases(now);
    let themes: [(&str, Rgb, Rgb); 2] = [
        ("light", [0xEE, 0xEE, 0xF2], [0xD6, 0xD6, 0xDC]),
        ("dark", [0x1F, 0x1F, 0x24], [0x33, 0x33, 0x3A]),
    ];
    let mut written = Vec::new();
    for (name, bg, tray_c) in themes {
        let dpis = [96u32, 144];
        let width = 560;
        let gap = 10;
        let rows: u32 = dpis
            .iter()
            .map(|d| layout::scale(48, *d) as u32 * cases.len() as u32 + gap * cases.len() as u32)
            .sum();
        let mut img = Image::new(width, rows + gap, [0x80, 0x80, 0x80]);
        let mut y = gap as i32;
        for dpi in dpis {
            let th = layout::scale(48, dpi);
            for (_, lines) in cases.iter() {
                let tb = Rect::new(0, y, width as i32, y + th);
                img.fill(tb.l, tb.t, tb.w(), tb.h(), bg);
                let tray_w = layout::scale(180, dpi);
                let tray = Rect::new(tb.r - tray_w, tb.t, tb.r, tb.b);
                img.fill(tray.l + 4, tray.t + th / 5, tray.w() - 8, th - 2 * (th / 5), tray_c);
                let px = layout::font_px(th, lines.len());
                let outline = layout::outline_px(dpi);
                let c = render::render_strip(&font, lines, px, th as u32, outline);
                let mon = Rect::new(0, 0, width as i32, 100_000);
                let p = layout::place(&tb, &mon, Some(&tray), c.w as i32, c.h as i32, layout::scale(6, dpi), (140, 0))
                    .ok_or("placement failed")?;
                img.blit(&c, p.rect.l, p.rect.t);
                y += th + gap as i32;
            }
        }
        let path = std::path::Path::new(dir).join(format!("preview-{}.png", name));
        std::fs::write(&path, img.png()).map_err(|e| e.to_string())?;
        written.push(path.display().to_string());
    }
    Ok(written)
}
