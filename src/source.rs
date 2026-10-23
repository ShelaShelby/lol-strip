//! Data sources: lolesports.com gateway (primary candidate) and the
//! Leaguepedia Cargo API (fallback). Parsing is pure and tested on saved
//! fixtures; the network sits behind the `Http` trait.

use crate::logfile::format_utc;
use crate::model::{classify, tidy, LeagueKey, Match};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub trait Http {
    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, String>;
}

/// Upcoming / running matches of the wanted leagues, plus list statistics.
#[derive(Debug, Default)]
pub struct Fetched {
    pub leagues_total: usize,
    pub leagues_wanted: usize,
    pub matches: Vec<Match>,
}

pub trait Source {
    fn name(&self) -> &'static str;
    fn fetch(&self, http: &dyn Http, now: i64) -> Result<Fetched, String>;
}

fn get_ok(http: &dyn Http, url: &str, headers: &[(&str, &str)]) -> Result<String, String> {
    let r = http.get(url, headers)?;
    if !(200..300).contains(&r.status) {
        return Err(format!("HTTP {} for {}", r.status, url));
    }
    Ok(r.body)
}

pub fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{:02X}", b));
        }
    }
    o
}

fn s<'a>(v: &'a Value, ptr: &str) -> &'a str {
    v.pointer(ptr).and_then(Value::as_str).unwrap_or("")
}

// ------------------------------------------------------------ lolesports

pub const LOL_BASE: &str = "https://esports-api.lolesports.com/persisted/gw";
/// Public key shipped in the lolesports.com web client; also used by many
/// open-source clients. Verified live by the CI probe, not assumed.
pub const LOL_KEY: &str = "0TvQnueqKa5mxJntVWt0w4LpLfEkrV1Ta8rQBb9Z";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct League {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub region: String,
}

pub fn parse_leagues(body: &str) -> Result<Vec<League>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("leagues: bad JSON: {}", e))?;
    let arr = v
        .pointer("/data/leagues")
        .and_then(Value::as_array)
        .ok_or("leagues: no data.leagues array")?;
    Ok(arr
        .iter()
        .map(|l| League {
            id: s(l, "/id").to_string(),
            slug: s(l, "/slug").to_string(),
            name: s(l, "/name").to_string(),
            region: s(l, "/region").to_string(),
        })
        .filter(|l| !l.slug.is_empty())
        .collect())
}

pub fn wanted_slugs(leagues: &[League]) -> HashMap<String, LeagueKey> {
    leagues
        .iter()
        .filter_map(|l| classify(&l.slug, &l.name, &l.region).map(|k| (l.slug.clone(), k)))
        .collect()
}

pub struct Page {
    pub matches: Vec<Match>,
    pub newer: Option<String>,
}

/// getSchedule / getLive body -> open matches of wanted leagues.
pub fn parse_schedule(body: &str, slugs: &HashMap<String, LeagueKey>, force_live: bool, now: i64) -> Result<Page, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("schedule: bad JSON: {}", e))?;
    let events = v
        .pointer("/data/schedule/events")
        .and_then(Value::as_array)
        .ok_or("schedule: no data.schedule.events array")?;
    let newer = v
        .pointer("/data/schedule/pages/newer")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    let mut out = Vec::new();
    for e in events {
        let state = s(e, "/state");
        if state == "completed" || e.get("match").is_none() {
            continue;
        }
        let slug = s(e, "/league/slug");
        let Some(&key) = slugs.get(slug) else { continue };
        let start = match crate::timeutil::parse_utc(s(e, "/startTime")) {
            Some(t) => t,
            None if force_live => now,
            None => continue,
        };
        let teams = e.pointer("/match/teams").and_then(Value::as_array);
        let team = |i: usize| -> String {
            teams
                .and_then(|t| t.get(i))
                .map(|t| {
                    let c = s(t, "/code");
                    if c.is_empty() { s(t, "/name") } else { c }.to_string()
                })
                .unwrap_or_default()
        };
        let (t1, t2) = (team(0), team(1));
        if t1.is_empty() || t2.is_empty() {
            continue;
        }
        let id = match s(e, "/match/id") {
            "" => s(e, "/id").to_string(),
            i => i.to_string(),
        };
        out.push(Match {
            id,
            league: key,
            league_name: s(e, "/league/name").to_string(),
            t1,
            t2,
            start,
            best_of: e.pointer("/match/strategy/count").and_then(Value::as_u64).unwrap_or(0).min(9) as u8,
            live: force_live || state == "inProgress",
        });
    }
    Ok(Page { matches: out, newer })
}

/// Order matters: the first source that answers wins, the rest are fallbacks.
pub fn default_sources() -> Vec<Box<dyn Source>> {
    vec![Box::new(LolEsports), Box::new(Leaguepedia)]
}

pub struct LolEsports;

impl Source for LolEsports {
    fn name(&self) -> &'static str {
        "lolesports"
    }

    fn fetch(&self, http: &dyn Http, now: i64) -> Result<Fetched, String> {
        let h = [("x-api-key", LOL_KEY)];
        let leagues = parse_leagues(&get_ok(http, &format!("{}/getLeagues?hl=en-US", LOL_BASE), &h)?)?;
        let slugs = wanted_slugs(&leagues);
        let ids: Vec<&str> = leagues
            .iter()
            .filter(|l| slugs.contains_key(&l.slug))
            .map(|l| l.id.as_str())
            .collect();
        if leagues.is_empty() {
            return Err("getLeagues returned no leagues".into());
        }
        if ids.is_empty() {
            return Err("none of the wanted leagues found in getLeagues".into());
        }
        let ids = ids.join(",");
        let mut all: Vec<Match> = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..4 {
            let mut url = format!("{}/getSchedule?hl=en-US&leagueId={}", LOL_BASE, ids);
            if let Some(t) = &token {
                url.push_str(&format!("&pageToken={}", enc(t)));
            }
            let p = match get_ok(http, &url, &h).and_then(|b| parse_schedule(&b, &slugs, false, now)) {
                Ok(p) => p,
                Err(e) if page > 0 => {
                    let _ = e;
                    break;
                }
                Err(e) => return Err(e),
            };
            all.extend(p.matches);
            let upcoming = all.iter().filter(|m| m.is_open(now)).count();
            match p.newer {
                Some(t) if upcoming < 14 => token = Some(t),
                _ => break,
            }
        }
        // getLive is a bonus: it marks running matches precisely.
        if let Ok(b) = get_ok(http, &format!("{}/getLive?hl=en-US", LOL_BASE), &h) {
            if let Ok(p) = parse_schedule(&b, &slugs, true, now) {
                all.extend(p.matches);
            }
        }
        Ok(Fetched {
            leagues_total: leagues.len(),
            leagues_wanted: slugs.len(),
            matches: tidy(all, now),
        })
    }
}

// ----------------------------------------------------------- leaguepedia

pub const WIKI_API: &str = "https://lol.fandom.com/api.php";

fn cargo_rows(body: &str) -> Result<Vec<Value>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("cargo: bad JSON: {}", e))?;
    if let Some(err) = v.get("error") {
        return Err(format!("cargo error: {} {}", s(err, "/code"), s(err, "/info")));
    }
    let rows = v.get("cargoquery").and_then(Value::as_array).ok_or("cargo: no cargoquery array")?;
    Ok(rows.iter().filter_map(|r| r.get("title").cloned()).collect())
}

/// Team name -> short code from the Teams table.
pub fn parse_teams(body: &str) -> Result<HashMap<String, String>, String> {
    Ok(cargo_rows(body)?
        .iter()
        .filter(|r| !s(r, "/Name").is_empty() && !s(r, "/Short").is_empty())
        .map(|r| (s(r, "/Name").to_string(), s(r, "/Short").to_string()))
        .collect())
}

/// Fallback when the Teams table has no short code.
pub fn short_code(name: &str) -> String {
    let letters: String = name.chars().filter(|c| c.is_alphanumeric()).collect();
    if letters.chars().count() <= 4 {
        return letters.to_uppercase();
    }
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.len() >= 2 {
        words
            .iter()
            .filter_map(|w| w.chars().next())
            .take(4)
            .collect::<String>()
            .to_uppercase()
    } else {
        letters.chars().take(3).collect::<String>().to_uppercase()
    }
}

/// MatchSchedule rows (aliases T1,T2,DT,BO,W,MID,Lg,Rg) -> (distinct league names, matches).
pub fn parse_cargo_matches(body: &str, teams: &HashMap<String, String>, now: i64) -> Result<(BTreeSet<String>, Vec<Match>), String> {
    let mut leagues = BTreeSet::new();
    let mut out = Vec::new();
    let code = |n: &str| teams.get(n).cloned().unwrap_or_else(|| short_code(n));
    for r in cargo_rows(body)? {
        let lg = s(&r, "/Lg");
        leagues.insert(lg.to_string());
        let Some(key) = classify("", lg, s(&r, "/Rg")) else { continue };
        let Some(start) = crate::timeutil::parse_utc(s(&r, "/DT")) else {
            continue;
        };
        if !s(&r, "/W").is_empty() || s(&r, "/T1").is_empty() || s(&r, "/T2").is_empty() {
            continue;
        }
        let id = match s(&r, "/MID") {
            "" => format!("{}|{}|{}", s(&r, "/T1"), s(&r, "/T2"), s(&r, "/DT")),
            i => i.to_string(),
        };
        out.push(Match {
            id,
            league: key,
            league_name: lg.to_string(),
            t1: code(s(&r, "/T1")),
            t2: code(s(&r, "/T2")),
            start,
            best_of: s(&r, "/BO").parse::<u8>().unwrap_or(0).min(9),
            live: start <= now,
        });
    }
    Ok((leagues, out))
}

pub struct Leaguepedia;

fn utc_text(t: i64) -> String {
    format_utc(t).trim_end_matches('Z').to_string()
}

impl Source for Leaguepedia {
    fn name(&self) -> &'static str {
        "leaguepedia"
    }

    fn fetch(&self, http: &dyn Http, now: i64) -> Result<Fetched, String> {
        let fields = "MatchSchedule.Team1=T1,MatchSchedule.Team2=T2,MatchSchedule.DateTime_UTC=DT,MatchSchedule.BestOf=BO,\
                      MatchSchedule.Winner=W,MatchSchedule.MatchId=MID,Tournaments.League=Lg,Tournaments.Region=Rg";
        let wh = format!(
            "MatchSchedule.DateTime_UTC >= '{}' AND MatchSchedule.DateTime_UTC <= '{}'",
            utc_text(now - crate::model::MAX_RUN),
            utc_text(now + 14 * 86_400)
        );
        let url = format!(
            "{}?action=cargoquery&format=json&limit=500&tables=MatchSchedule,Tournaments&join_on={}&fields={}&where={}&order_by={}",
            WIKI_API,
            enc("MatchSchedule.OverviewPage=Tournaments.OverviewPage"),
            enc(fields),
            enc(&wh),
            enc("MatchSchedule.DateTime_UTC")
        );
        let body = get_ok(http, &url, &[])?;
        // First pass with heuristic codes to learn which teams we need.
        let (leagues, first) = parse_cargo_matches(&body, &HashMap::new(), now)?;
        let names: BTreeSet<String> = cargo_rows(&body)?
            .iter()
            .flat_map(|r| [s(r, "/T1").to_string(), s(r, "/T2").to_string()])
            .filter(|n| !n.is_empty())
            .collect();
        let mut teams = HashMap::new();
        if !first.is_empty() && !names.is_empty() {
            let list: Vec<String> = names.iter().map(|n| format!("'{}'", n.replace('\'', "''"))).collect();
            let turl = format!(
                "{}?action=cargoquery&format=json&limit=500&tables=Teams&fields=Name,Short&where={}",
                WIKI_API,
                enc(&format!("Name IN ({})", list.join(",")))
            );
            if let Ok(tb) = get_ok(http, &turl, &[]) {
                teams = parse_teams(&tb).unwrap_or_default();
            }
        }
        let (_, matches) = parse_cargo_matches(&body, &teams, now)?;
        if leagues.is_empty() {
            return Err("MatchSchedule returned no rows".into());
        }
        let wanted = leagues.iter().filter(|l| classify("", l, "").is_some()).count();
        Ok(Fetched {
            leagues_total: leagues.len(),
            leagues_wanted: wanted,
            matches: tidy(matches, now),
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::timeutil::parse_utc;
    use std::cell::RefCell;

    const LEAGUES: &str = include_str!("../fixtures/lolesports_leagues.json");
    const SCHEDULE: &str = include_str!("../fixtures/lolesports_schedule.json");
    const LIVE: &str = include_str!("../fixtures/lolesports_live.json");
    const CARGO: &str = include_str!("../fixtures/leaguepedia_matches.json");
    const TEAMS: &str = include_str!("../fixtures/leaguepedia_teams.json");
    const EMPTY_PAGE: &str = r#"{"data":{"schedule":{"pages":{"older":null,"newer":null},"events":[]}}}"#;

    pub fn now() -> i64 {
        parse_utc("2026-10-25T09:00:00Z").unwrap()
    }

    /// Canned responses by URL substring (first match wins), records requests.
    pub struct Mock {
        pub routes: Vec<(&'static str, u16, String)>,
        pub seen: RefCell<Vec<String>>,
    }

    impl Http for Mock {
        fn get(&self, url: &str, _h: &[(&str, &str)]) -> Result<HttpResponse, String> {
            self.seen.borrow_mut().push(url.to_string());
            for (pat, status, body) in &self.routes {
                if url.contains(pat) {
                    return Ok(HttpResponse {
                        status: *status,
                        body: body.clone(),
                    });
                }
            }
            Err("connection refused".into())
        }
    }

    pub fn lol_mock() -> Mock {
        Mock {
            routes: vec![
                ("getLeagues", 200, LEAGUES.into()),
                ("pageToken=NEWER_TOKEN", 200, EMPTY_PAGE.into()),
                ("getSchedule", 200, SCHEDULE.into()),
                ("getLive", 200, LIVE.into()),
            ],
            seen: RefCell::new(vec![]),
        }
    }

    pub fn wiki_mock() -> Mock {
        Mock {
            routes: vec![("tables=Teams", 200, TEAMS.into()), ("tables=MatchSchedule", 200, CARGO.into())],
            seen: RefCell::new(vec![]),
        }
    }

    #[test]
    fn parses_leagues_and_filters_wanted() {
        let l = parse_leagues(LEAGUES).unwrap();
        assert_eq!(l.len(), 12);
        let w = wanted_slugs(&l);
        for slug in [
            "lec",
            "lck",
            "lpl",
            "lcs",
            "cblol-brazil",
            "worlds",
            "msi",
            "first_stand",
            "road_of_legends",
            "all-star",
        ] {
            assert!(w.contains_key(slug), "{}", slug);
        }
        assert_eq!(w.get("worlds"), Some(&LeagueKey::Intl));
        assert_eq!(w.get("road_of_legends"), Some(&LeagueKey::Intl));
        assert!(!w.contains_key("lck_challengers_league"));
        assert!(!w.contains_key("ljl-japan"));
        assert!(parse_leagues("{}").is_err());
        assert!(parse_leagues("<html>").is_err());
    }

    #[test]
    fn parses_schedule_page() {
        let w = wanted_slugs(&parse_leagues(LEAGUES).unwrap());
        let p = parse_schedule(SCHEDULE, &w, false, now()).unwrap();
        assert_eq!(p.newer.as_deref(), Some("NEWER_TOKEN"));
        let ids: Vec<&str> = p.matches.iter().map(|m| m.id.as_str()).collect();
        // completed, unwanted league and the "show" without match are gone
        assert_eq!(ids, ["1002", "1003", "1005", "1006"]);
        assert!(p.matches[0].live);
        assert_eq!(
            (p.matches[1].t1.as_str(), p.matches[1].t2.as_str(), p.matches[1].best_of),
            ("T1", "GEN", 5)
        );
        assert_eq!(p.matches[1].start, parse_utc("2026-10-25T16:00:00Z").unwrap());
        assert_eq!(p.matches[3].league, LeagueKey::Intl);
        assert_eq!(p.matches[3].t1, "TBD");
        assert!(parse_schedule("{\"data\":{}}", &w, false, 0).is_err());
    }

    #[test]
    fn parses_live_without_start_time() {
        let w = wanted_slugs(&parse_leagues(LEAGUES).unwrap());
        let p = parse_schedule(LIVE, &w, true, now()).unwrap();
        assert_eq!(p.matches.len(), 2);
        assert!(p.matches.iter().all(|m| m.live));
        assert_eq!(p.matches[1].start, now());
        assert_eq!(p.matches[1].league, LeagueKey::Cblol);
    }

    #[test]
    fn lolesports_full_flow() {
        let mock = lol_mock();
        let f = LolEsports.fetch(&mock, now()).unwrap();
        assert_eq!((f.leagues_total, f.leagues_wanted), (12, 10));
        let ids: Vec<&str> = f.matches.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["1002", "1007", "1003", "1005", "1006"]);
        assert!(f.matches[0].live && f.matches[1].live);
        // second page was requested with the token
        assert!(mock.seen.borrow().iter().any(|u| u.contains("pageToken=NEWER_TOKEN")));
        assert!(mock.seen.borrow()[1].contains("leagueId=98767991299243165,"));
    }

    #[test]
    fn lolesports_errors() {
        let bad = Mock {
            routes: vec![("getLeagues", 403, "forbidden".into())],
            seen: RefCell::new(vec![]),
        };
        assert_eq!(
            LolEsports.fetch(&bad, 0).unwrap_err(),
            format!("HTTP 403 for {}/getLeagues?hl=en-US", LOL_BASE)
        );
        let empty = Mock {
            routes: vec![("getLeagues", 200, r#"{"data":{"leagues":[]}}"#.into())],
            seen: RefCell::new(vec![]),
        };
        assert!(LolEsports.fetch(&empty, 0).is_err());
        let down = Mock {
            routes: vec![],
            seen: RefCell::new(vec![]),
        };
        assert_eq!(LolEsports.fetch(&down, 0).unwrap_err(), "connection refused");
    }

    #[test]
    fn leaguepedia_parse_and_short_codes() {
        let teams = parse_teams(TEAMS).unwrap();
        assert_eq!(teams.get("Gen.G").map(String::as_str), Some("GEN"));
        let (leagues, m) = parse_cargo_matches(CARGO, &teams, now()).unwrap();
        assert!(leagues.contains("LoL EMEA Championship") && leagues.contains("Some Academy Cup"));
        // finished (winner set), unwanted league and bad date are dropped
        let ids: Vec<&str> = m.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "LCK/2026 Season/Finals_Round 1_Match 1",
                "LPL/2026 Season/Split 3_Week 1_1",
                "Cloud9|100 Thieves|2026-10-26 20:00:00",
                "WCS/1"
            ]
        );
        assert_eq!((m[0].t1.as_str(), m[0].t2.as_str(), m[0].best_of), ("T1", "GEN", 5));
        assert!(m[1].live && !m[0].live);
        assert_eq!(m[3].league, LeagueKey::Intl);
        assert_eq!(m[2].t1, "CLO"); // no Teams row: heuristic code
    }

    #[test]
    fn short_code_heuristic() {
        assert_eq!(short_code("T1"), "T1");
        assert_eq!(short_code("Gen.G"), "GENG");
        assert_eq!(short_code("Hanwha Life Esports"), "HLE");
        assert_eq!(short_code("Fnatic"), "FNA");
        assert_eq!(short_code("Dplus KIA"), "DK");
    }

    #[test]
    fn leaguepedia_full_flow_and_cargo_error() {
        let mock = wiki_mock();
        let f = Leaguepedia.fetch(&mock, now()).unwrap();
        assert!(f.matches.len() == 4 && f.leagues_total >= 5 && f.leagues_wanted >= 4);
        assert_eq!(f.matches[0].t1, "BLG");
        let err = Mock {
            routes: vec![(
                "tables=MatchSchedule",
                200,
                r#"{"error":{"code":"internal_api_error","info":"bad"}}"#.into(),
            )],
            seen: RefCell::new(vec![]),
        };
        assert!(Leaguepedia
            .fetch(&err, 0)
            .unwrap_err()
            .starts_with("cargo error: internal_api_error"));
    }

    #[test]
    fn url_encoding() {
        assert_eq!(enc("a b,c'd"), "a%20b%2Cc%27d");
        assert_eq!(enc("Az09-_.~"), "Az09-_.~");
    }
}
