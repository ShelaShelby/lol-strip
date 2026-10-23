//! Refresh logic that does not touch Windows: source fallback, stale
//! handling, refresh cadence, disk cache, notify-once bookkeeping.

use crate::display::SOON;
use crate::model::{sort_matches, LeagueKey, Match};
use crate::settings::Settings;
use crate::source::{Http, Source};
use serde_json::{json, Value};
use std::collections::HashSet;

pub const SLOW_SECS: u64 = 600;
pub const FAST_SECS: u64 = 60;
pub const RETRY_SECS: u64 = 120;
/// Notify this long before the start.
pub const NOTIFY_BEFORE: i64 = 10 * 60;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub matches: Vec<Match>,
    /// Last refresh failed (or nothing fetched yet): show "?".
    pub stale: bool,
    pub fetched_at: i64,
    pub source: String,
    pub error: String,
}

/// Try the sources in order; the first one that answers wins. When all fail
/// the previous data is kept and marked stale.
pub fn refresh(sources: &[Box<dyn Source>], http: &dyn Http, now: i64, prev: &Snapshot) -> Snapshot {
    let mut errors = Vec::new();
    for src in sources {
        match src.fetch(http, now) {
            Ok(f) if f.leagues_total > 0 => {
                return Snapshot {
                    matches: f.matches,
                    stale: false,
                    fetched_at: now,
                    source: src.name().to_string(),
                    error: String::new(),
                };
            }
            Ok(_) => errors.push(format!("{}: no leagues in answer", src.name())),
            Err(e) => errors.push(format!("{}: {}", src.name(), e)),
        }
    }
    Snapshot {
        stale: true,
        error: errors.join(" | "),
        ..prev.clone()
    }
}

/// Seconds until the next refresh: every minute while something is live or
/// about to start, otherwise every 10 minutes; a failed refresh retries
/// after 2 minutes at the latest.
pub fn next_interval(s: &Snapshot, now: i64) -> u64 {
    let busy = s.matches.iter().any(|m| m.is_live(now) || (m.start > now && m.start - now <= SOON));
    let mut wait = if busy { FAST_SECS } else { SLOW_SECS };
    // Wake up when the next match enters the 15 minute window.
    if let Some(until) = s
        .matches
        .iter()
        .filter(|m| m.start - SOON > now)
        .map(|m| (m.start - SOON - now) as u64)
        .min()
    {
        wait = wait.min(until.max(30));
    }
    if s.stale {
        wait = wait.min(RETRY_SECS);
    }
    wait
}

/// Matches of enabled leagues, sorted.
pub fn enabled(matches: &[Match], st: &Settings) -> Vec<Match> {
    let mut v: Vec<Match> = matches.iter().filter(|m| st.league_on(m.league)).cloned().collect();
    sort_matches(&mut v);
    v
}

// ------------------------------------------------------------------ cache

pub fn cache_to_string(s: &Snapshot) -> String {
    let ms: Vec<Value> = s
        .matches
        .iter()
        .map(|m| {
            json!({"id": m.id, "league": m.league.code(), "league_name": m.league_name, "t1": m.t1, "t2": m.t2,
                   "start": m.start, "best_of": m.best_of, "live": m.live})
        })
        .collect();
    json!({"fetched_at": s.fetched_at, "source": s.source, "matches": ms}).to_string()
}

/// Cached data always comes back stale until a refresh succeeds.
pub fn cache_from_str(text: &str) -> Option<Snapshot> {
    let v: Value = serde_json::from_str(text).ok()?;
    let mut matches = Vec::new();
    for m in v.get("matches")?.as_array()? {
        let st = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
        matches.push(Match {
            id: st("id")?,
            league: LeagueKey::from_code(&st("league")?)?,
            league_name: st("league_name").unwrap_or_default(),
            t1: st("t1")?,
            t2: st("t2")?,
            start: m.get("start")?.as_i64()?,
            best_of: m.get("best_of").and_then(Value::as_u64).unwrap_or(0) as u8,
            live: m.get("live").and_then(Value::as_bool).unwrap_or(false),
        });
    }
    Some(Snapshot {
        matches,
        stale: true,
        fetched_at: v.get("fetched_at").and_then(Value::as_i64).unwrap_or(0),
        source: v.get("source").and_then(Value::as_str).unwrap_or("").to_string(),
        error: "cached data".into(),
    })
}

// --------------------------------------------------------------- notify

#[derive(Default)]
pub struct Notifier {
    done: HashSet<String>,
}

impl Notifier {
    /// Matches starting within 10 minutes that were not announced yet
    /// (each match id is returned exactly once). `matches` must already be
    /// filtered to the enabled leagues.
    pub fn due(&mut self, matches: &[Match], now: i64, on: bool) -> Vec<Match> {
        if !on {
            return vec![];
        }
        let mut out = Vec::new();
        for m in matches {
            if m.live || m.start <= now || m.start - now > NOTIFY_BEFORE {
                continue;
            }
            if self.done.insert(m.id.clone()) {
                out.push(m.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::mk;
    use crate::source::tests::{lol_mock, now, wiki_mock, Mock};
    use crate::source::{Leaguepedia, LolEsports};
    use std::cell::RefCell;

    fn both() -> Vec<Box<dyn Source>> {
        vec![Box::new(LolEsports), Box::new(Leaguepedia)]
    }

    #[test]
    fn first_source_wins() {
        let s = refresh(&both(), &lol_mock(), now(), &Snapshot::default());
        assert_eq!(s.source, "lolesports");
        assert!(!s.stale && s.fetched_at == now() && s.matches.len() == 5);
    }

    #[test]
    fn falls_back_to_second_source() {
        let mut routes = wiki_mock().routes;
        routes.push(("esports-api", 403, "no".into()));
        let m = Mock {
            routes,
            seen: RefCell::new(vec![]),
        };
        let s = refresh(&both(), &m, now(), &Snapshot::default());
        assert_eq!(s.source, "leaguepedia");
        assert!(!s.stale && !s.matches.is_empty());
    }

    #[test]
    fn all_fail_keeps_last_good_data_stale() {
        let good = refresh(&both(), &lol_mock(), now(), &Snapshot::default());
        let down = Mock {
            routes: vec![],
            seen: RefCell::new(vec![]),
        };
        let s = refresh(&both(), &down, now() + 600, &good);
        assert!(s.stale);
        assert_eq!(s.matches, good.matches);
        assert_eq!(s.fetched_at, good.fetched_at);
        assert!(s.error.contains("lolesports: connection refused") && s.error.contains("leaguepedia:"));
    }

    #[test]
    fn cadence() {
        let t = now();
        let mut s = Snapshot {
            matches: vec![mk("a", LeagueKey::Lck, "A", "B", t + 7200, false)],
            ..Default::default()
        };
        assert_eq!(next_interval(&s, t), SLOW_SECS);
        s.matches.push(mk("b", LeagueKey::Lck, "A", "B", t + 14 * 60, false));
        assert_eq!(next_interval(&s, t), FAST_SECS);
        s.matches = vec![mk("c", LeagueKey::Lck, "A", "B", t - 60, true)];
        assert_eq!(next_interval(&s, t), FAST_SECS);
        s.matches = vec![mk("a", LeagueKey::Lck, "A", "B", t + 16 * 60, false)];
        assert_eq!(next_interval(&s, t), 60);
        s.matches = vec![mk("a", LeagueKey::Lck, "A", "B", t + 15 * 60 + 5, false)];
        assert_eq!(next_interval(&s, t), 30);
        s.matches = vec![mk("a", LeagueKey::Lck, "A", "B", t + 3 * 3600, false)];
        assert_eq!(next_interval(&s, t), SLOW_SECS);
        s.stale = true;
        assert_eq!(next_interval(&s, t), SLOW_SECS.min(RETRY_SECS));
    }

    #[test]
    fn cache_roundtrip() {
        let s = refresh(&both(), &lol_mock(), now(), &Snapshot::default());
        let back = cache_from_str(&cache_to_string(&s)).unwrap();
        assert!(back.stale);
        assert_eq!(back.matches, s.matches);
        assert_eq!(back.fetched_at, s.fetched_at);
        assert!(cache_from_str("garbage").is_none());
        assert!(cache_from_str("{\"matches\":[{\"id\":\"x\"}]}").is_none());
    }

    #[test]
    fn league_filter_applies() {
        let ms = vec![
            mk("1", LeagueKey::Lck, "A", "B", 20, false),
            mk("2", LeagueKey::Lec, "C", "D", 10, false),
            mk("3", LeagueKey::Intl, "E", "F", 15, false),
        ];
        let mut st = Settings::default();
        assert_eq!(enabled(&ms, &st).len(), 3);
        st.set_league(LeagueKey::Lec, false);
        let ids: Vec<String> = enabled(&ms, &st).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["3", "1"]);
    }

    #[test]
    fn notify_once_per_match() {
        let t = 1_000_000;
        let ms = vec![
            mk("soon", LeagueKey::Lck, "A", "B", t + 9 * 60, false),
            mk("far", LeagueKey::Lck, "C", "D", t + 11 * 60, false),
            mk("live", LeagueKey::Lck, "E", "F", t - 5, true),
        ];
        let mut n = Notifier::default();
        assert!(n.due(&ms, t, false).is_empty());
        let d = n.due(&ms, t, true);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].id, "soon");
        assert!(n.due(&ms, t + 30, true).is_empty(), "no repeat");
        let d = n.due(&ms, t + 90, true);
        assert_eq!(d.len(), 1, "'far' enters the 10 minute window");
        assert_eq!(d[0].id, "far");
        assert!(n.due(&ms, t + 120, true).is_empty());
    }
}
