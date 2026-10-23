//! What is shown: strip lines and menu texts.

use crate::model::Match;
use crate::timeutil::{self, LocalFn};

pub type Rgb = [u8; 3];

pub const WHITE: Rgb = [0xF2, 0xF2, 0xF2];
pub const YELLOW: Rgb = [0xFF, 0xC8, 0x00];
pub const RED: Rgb = [0xFF, 0x45, 0x3A];
pub const GREY: Rgb = [0xA0, 0xA0, 0xA0];

/// A match this close to its start is highlighted (and refreshed every minute).
pub const SOON: i64 = 15 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub colour: Rgb,
}

/// "T1 v GEN 18:00", "T1 v GEN Sun 18:00" (not today), "LIVE T1 v GEN".
pub fn match_text(m: &Match, now: i64, local: LocalFn) -> String {
    if m.is_live(now) {
        return format!("LIVE {} v {}", m.t1, m.t2);
    }
    let ls = local(m.start);
    if timeutil::day_of(ls) == timeutil::day_of(local(now)) {
        format!("{} v {} {}", m.t1, m.t2, timeutil::hhmm(ls))
    } else {
        format!("{} v {} {} {}", m.t1, m.t2, timeutil::weekday(ls), timeutil::hhmm(ls))
    }
}

fn colour(m: &Match, now: i64) -> Rgb {
    if m.is_live(now) {
        RED
    } else if m.start - now <= SOON {
        YELLOW
    } else {
        WHITE
    }
}

/// Up to two lines: next match, the one after. `matches` must be sorted and
/// already filtered by the enabled leagues. `stale` appends a "?".
pub fn lines(matches: &[Match], now: i64, local: LocalFn, stale: bool) -> Vec<Line> {
    let q = if stale { "?" } else { "" };
    let mut v: Vec<Line> = matches
        .iter()
        .filter(|m| m.is_open(now))
        .take(2)
        .map(|m| Line {
            text: format!("{}{}", match_text(m, now, local), q),
            colour: colour(m, now),
        })
        .collect();
    if v.is_empty() {
        v.push(Line {
            text: format!("LoL --{}", q),
            colour: GREY,
        });
    }
    v
}

/// Tray menu entry: "LCK  T1 v GEN  Sun 25 Oct 18:00  Bo5" / "LIVE".
pub fn menu_text(m: &Match, now: i64, local: LocalFn) -> String {
    let when = if m.is_live(now) {
        "LIVE".to_string()
    } else {
        timeutil::full(local(m.start))
    };
    let bo = if m.best_of > 0 {
        format!("  Bo{}", m.best_of)
    } else {
        String::new()
    };
    format!("{}  {} v {}  {}{}", m.league.code(), m.t1, m.t2, when, bo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{mk, LeagueKey};
    use crate::timeutil::parse_utc;

    fn utc(s: &str) -> i64 {
        parse_utc(s).unwrap()
    }
    // Fixed UTC+2 "time zone".
    fn plus2(t: i64) -> i64 {
        t + 7200
    }

    #[test]
    fn today_other_day_and_live() {
        let now = utc("2026-10-25T10:00:00Z"); // Sun 12:00 local
        let today = mk("1", LeagueKey::Lck, "T1", "GEN", utc("2026-10-25T16:00:00Z"), false);
        assert_eq!(match_text(&today, now, &plus2), "T1 v GEN 18:00");
        let later = mk("2", LeagueKey::Lck, "HLE", "DK", utc("2026-10-26T15:00:00Z"), false);
        assert_eq!(match_text(&later, now, &plus2), "HLE v DK Mon 17:00");
        // local midnight crossing: 23:30Z is already Monday 01:30 local
        let late = mk("3", LeagueKey::Lec, "G2", "FNC", utc("2026-10-25T23:30:00Z"), false);
        assert_eq!(match_text(&late, now, &plus2), "G2 v FNC Mon 01:30");
        let live = mk("4", LeagueKey::Lpl, "BLG", "JDG", utc("2026-10-25T09:00:00Z"), true);
        assert_eq!(match_text(&live, now, &plus2), "LIVE BLG v JDG");
    }

    #[test]
    fn strip_lines_empty_stale_and_colours() {
        let now = utc("2026-10-25T10:00:00Z");
        let l = lines(&[], now, &plus2, false);
        assert_eq!(
            l,
            vec![Line {
                text: "LoL --".into(),
                colour: GREY
            }]
        );
        assert_eq!(lines(&[], now, &plus2, true)[0].text, "LoL --?");
        let ms = vec![
            mk("1", LeagueKey::Lck, "T1", "GEN", now + 600, false),
            mk("2", LeagueKey::Lck, "HLE", "DK", now + 86_400, false),
            mk("3", LeagueKey::Lck, "KT", "NS", now + 90_000, false),
        ];
        let l = lines(&ms, now, &plus2, true);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].text, "T1 v GEN 12:10?");
        assert_eq!(l[0].colour, YELLOW);
        assert_eq!(l[1].text, "HLE v DK Mon 12:00?");
        assert_eq!(l[1].colour, WHITE);
        let live = vec![mk("9", LeagueKey::Lec, "G2", "FNC", now - 60, true)];
        let l = lines(&live, now, &plus2, false);
        assert_eq!(l[0].text, "LIVE G2 v FNC");
        assert_eq!(l[0].colour, RED);
    }

    #[test]
    fn menu_line() {
        let now = utc("2026-10-25T10:00:00Z");
        let mut m = mk("1", LeagueKey::Lck, "T1", "GEN", utc("2026-10-25T16:00:00Z"), false);
        m.best_of = 5;
        assert_eq!(menu_text(&m, now, &plus2), "LCK  T1 v GEN  Sun 25 Oct 18:00  Bo5");
        m.live = true;
        assert_eq!(menu_text(&m, now, &plus2), "LCK  T1 v GEN  LIVE  Bo5");
    }
}
