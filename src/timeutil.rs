//! Calendar maths on UTC seconds, no time zone database. The local time
//! zone comes from the caller as a `LocalFn` (UTC seconds -> local seconds).

use crate::logfile::civil;

/// UTC seconds -> local seconds (wall clock expressed as if it were UTC).
pub type LocalFn<'a> = &'a dyn Fn(i64) -> i64;

pub const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
pub const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Days since 1970-01-01 for a civil date (inverse of `civil`).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn num(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// "2026-10-24T16:00:00Z", "2026-10-24T16:00:00.000+02:00" or
/// "2026-10-24 16:00:00" (Leaguepedia, UTC) -> UTC seconds.
pub fn parse_utc(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 || !s.is_char_boundary(19) {
        return None;
    }
    let (head, rest) = s.split_at(19);
    let b = head.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d) = (num(&head[0..4])?, num(&head[5..7])?, num(&head[8..10])?);
    let (h, mi, se) = (num(&head[11..13])?, num(&head[14..16])?, num(&head[17..19])?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut rest = rest;
    if let Some(r) = rest.strip_prefix('.') {
        let n = r.bytes().take_while(|c| c.is_ascii_digit()).count();
        rest = &r[n..];
    }
    let off = match rest {
        "" | "Z" | "z" => 0,
        r if (r.starts_with('+') || r.starts_with('-')) && r.len() >= 3 => {
            let sign = if r.starts_with('-') { -1 } else { 1 };
            let hh = num(r.get(1..3)?)?;
            let mm = match r.get(3..) {
                Some("") | None => 0,
                Some(t) => num(t.trim_start_matches(':'))?,
            };
            sign * (hh * 3600 + mm * 60)
        }
        _ => return None,
    };
    Some(days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3600 + mi * 60 + se - off)
}

pub fn weekday(local: i64) -> &'static str {
    WEEKDAYS[(local.div_euclid(86_400) + 4).rem_euclid(7) as usize]
}

pub fn day_of(local: i64) -> i64 {
    local.div_euclid(86_400)
}

pub fn hhmm(local: i64) -> String {
    let s = local.rem_euclid(86_400);
    format!("{:02}:{:02}", s / 3600, (s / 60) % 60)
}

/// "Sun 25 Oct 18:00"
pub fn full(local: i64) -> String {
    let (_, m, d) = civil(day_of(local));
    format!("{} {} {} {}", weekday(local), d, MONTHS[(m - 1) as usize], hhmm(local))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_roundtrips() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2000-02-29T00:00:00Z"), Some(951_782_400));
        assert_eq!(parse_utc("2026-10-22T10:34:56Z"), Some(1_792_665_296));
        assert_eq!(parse_utc("2026-10-22 10:34:56"), Some(1_792_665_296));
        assert_eq!(parse_utc("2026-10-22T10:34:56.123Z"), Some(1_792_665_296));
        assert_eq!(parse_utc("2026-10-22T12:34:56+02:00"), Some(1_792_665_296));
        assert_eq!(parse_utc("2026-10-22T05:34:56-0500"), Some(1_792_665_296));
        assert_eq!(parse_utc("nope"), None);
        assert_eq!(parse_utc("2026-13-22T10:34:56Z"), None);
        for days in [-1000i64, 0, 59, 60, 11_000, 20_000, 24_000] {
            let (y, m, d) = civil(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }

    #[test]
    fn weekday_and_format() {
        assert_eq!(weekday(0), "Thu");
        let t = parse_utc("2026-10-25T18:05:00Z").unwrap();
        assert_eq!(weekday(t), "Sun");
        assert_eq!(hhmm(t), "18:05");
        assert_eq!(full(t), "Sun 25 Oct 18:05");
    }
}
