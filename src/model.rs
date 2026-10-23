//! Leagues and matches shared by all data sources.

use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LeagueKey {
    Lec,
    Lpl,
    Lck,
    Lcs,
    Cblol,
    Intl,
}

pub const ALL_KEYS: [LeagueKey; 6] = [
    LeagueKey::Lec,
    LeagueKey::Lpl,
    LeagueKey::Lck,
    LeagueKey::Lcs,
    LeagueKey::Cblol,
    LeagueKey::Intl,
];

impl LeagueKey {
    pub fn code(self) -> &'static str {
        match self {
            LeagueKey::Lec => "LEC",
            LeagueKey::Lpl => "LPL",
            LeagueKey::Lck => "LCK",
            LeagueKey::Lcs => "LCS",
            LeagueKey::Cblol => "CBLOL",
            LeagueKey::Intl => "Intl",
        }
    }
    pub fn from_code(c: &str) -> Option<LeagueKey> {
        ALL_KEYS.iter().copied().find(|k| k.code().eq_ignore_ascii_case(c))
    }
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

/// Decide whether a league is wanted, from its slug / name / region as the
/// source reports them (never from numeric IDs). Any league whose region is
/// INTERNATIONAL counts as international (Worlds, MSI, First Stand, ...).
pub fn classify(slug: &str, name: &str, region: &str) -> Option<LeagueKey> {
    let slug = slug.trim().to_lowercase();
    match slug.as_str() {
        "lec" => return Some(LeagueKey::Lec),
        "lpl" => return Some(LeagueKey::Lpl),
        "lck" => return Some(LeagueKey::Lck),
        "lcs" => return Some(LeagueKey::Lcs),
        "cblol" | "cblol-brazil" | "cblol_brazil" => return Some(LeagueKey::Cblol),
        "worlds" | "msi" | "first_stand" | "first-stand" => return Some(LeagueKey::Intl),
        _ => {}
    }
    if region.trim().eq_ignore_ascii_case("international") {
        return Some(LeagueKey::Intl);
    }
    // Exact (normalised) names only, so academy / challenger leagues stay out.
    match norm(name).as_str() {
        "lec" | "lolemeachampionship" | "lolemea" | "lecemea" => Some(LeagueKey::Lec),
        "lpl" | "lolproleague" | "tencentlolprofessionalleague" => Some(LeagueKey::Lpl),
        "lck" | "lolchampionskorea" | "lckchampionskorea" => Some(LeagueKey::Lck),
        "lcs" | "leaguechampionshipseries" => Some(LeagueKey::Lcs),
        "cblol" | "campeonatobrasileirodeleagueoflegends" | "cblolbrazil" => Some(LeagueKey::Cblol),
        "worldchampionship" | "worlds" | "midseasoninvitational" | "msi" | "firststand" | "firststandtournament" => Some(LeagueKey::Intl),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub id: String,
    pub league: LeagueKey,
    pub league_name: String,
    pub t1: String,
    pub t2: String,
    /// UTC seconds.
    pub start: i64,
    pub best_of: u8,
    /// Source says the match is in progress.
    pub live: bool,
}

/// How long after its start a match may still count as running.
pub const MAX_RUN: i64 = 8 * 3600;

impl Match {
    pub fn is_live(&self, now: i64) -> bool {
        (self.live || self.start <= now) && now < self.start + MAX_RUN
    }
    /// Not finished as far as we can tell.
    pub fn is_open(&self, now: i64) -> bool {
        self.start + MAX_RUN > now
    }
}

pub fn sort_matches(v: &mut [Match]) {
    v.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| cmp_str(&a.id, &b.id)));
}

fn cmp_str(a: &str, b: &str) -> Ordering {
    a.cmp(b)
}

/// Drop duplicates (same id; the live flag wins), drop finished, sort.
pub fn tidy(mut v: Vec<Match>, now: i64) -> Vec<Match> {
    sort_matches(&mut v);
    let mut out: Vec<Match> = Vec::new();
    for m in v {
        if !m.is_open(now) {
            continue;
        }
        if let Some(e) = out.iter_mut().find(|e| e.id == m.id) {
            e.live |= m.live;
            continue;
        }
        out.push(m);
    }
    out
}

/// Compact constructor (tests and the preview).
pub fn mk(id: &str, key: LeagueKey, t1: &str, t2: &str, start: i64, live: bool) -> Match {
    Match {
        id: id.into(),
        league: key,
        league_name: key.code().into(),
        t1: t1.into(),
        t2: t2.into(),
        start,
        best_of: 3,
        live,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_slug_name_region() {
        assert_eq!(classify("lec", "LEC", "EUROPE"), Some(LeagueKey::Lec));
        assert_eq!(classify("lck", "LCK", "KOREA"), Some(LeagueKey::Lck));
        assert_eq!(classify("lpl", "LPL", "CHINA"), Some(LeagueKey::Lpl));
        assert_eq!(classify("lcs", "LCS", "NORTH AMERICA"), Some(LeagueKey::Lcs));
        assert_eq!(classify("cblol-brazil", "CBLOL", "BRAZIL"), Some(LeagueKey::Cblol));
        assert_eq!(classify("worlds", "Worlds", "INTERNATIONAL"), Some(LeagueKey::Intl));
        assert_eq!(classify("msi", "MSI", "INTERNATIONAL"), Some(LeagueKey::Intl));
        // unknown slug, international region
        assert_eq!(classify("first_stand", "First Stand", "INTERNATIONAL"), Some(LeagueKey::Intl));
        assert_eq!(classify("something_new", "Brand New Cup", "International"), Some(LeagueKey::Intl));
        // Leaguepedia style names
        assert_eq!(classify("", "LoL EMEA Championship", "Europe"), Some(LeagueKey::Lec));
        assert_eq!(classify("", "LCK", "Korea"), Some(LeagueKey::Lck));
        assert_eq!(classify("", "World Championship", "International"), Some(LeagueKey::Intl));
        // not wanted
        assert_eq!(classify("lck_challengers_league", "LCK Challengers", "KOREA"), None);
        assert_eq!(classify("lta_n", "LTA North", "NORTH AMERICA"), None);
        assert_eq!(classify("lec_academy", "LEC Academy", "EUROPE"), None);
        assert_eq!(classify("", "", ""), None);
    }

    #[test]
    fn tidy_sorts_dedups_and_drops_finished() {
        let now = 100_000;
        let v = vec![
            mk("b", LeagueKey::Lck, "A", "B", now + 500, false),
            mk("a", LeagueKey::Lck, "C", "D", now - 600, true),
            mk("b", LeagueKey::Lck, "A", "B", now + 500, true),
            mk("old", LeagueKey::Lec, "E", "F", now - MAX_RUN - 1, true),
            mk("c", LeagueKey::Lpl, "G", "H", now + 100, false),
        ];
        let t = tidy(v, now);
        let ids: Vec<&str> = t.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a", "c", "b"]);
        assert!(t[2].live);
        assert!(t[0].is_live(now));
        assert!(!t[1].is_live(now));
    }
}
