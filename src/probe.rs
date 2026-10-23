//! Text of probe.txt: one section per source with HTTP status, league and
//! match counts, the first matches and the raw start of every response.

use crate::display;
use crate::model::ALL_KEYS;
use crate::source::{Http, HttpResponse, Source};
use crate::timeutil::LocalFn;
use std::cell::RefCell;

pub const RAW_BYTES: usize = 2048;

struct Exchange {
    url: String,
    outcome: Result<(u16, usize, String), String>,
}

/// Records every request/response of a source for the report.
struct Recorder<'a> {
    inner: &'a dyn Http,
    log: RefCell<Vec<Exchange>>,
}

fn head(s: &str, max: usize) -> String {
    let mut n = max.min(s.len());
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    s[..n].to_string()
}

impl Http for Recorder<'_> {
    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, String> {
        let r = self.inner.get(url, headers);
        let outcome = match &r {
            Ok(x) => Ok((x.status, x.body.len(), head(&x.body, RAW_BYTES))),
            Err(e) => Err(e.clone()),
        };
        self.log.borrow_mut().push(Exchange {
            url: url.to_string(),
            outcome,
        });
        r
    }
}

pub fn report(sources: &[Box<dyn Source>], http: &dyn Http, now: i64, local: LocalFn) -> Vec<String> {
    let mut out = Vec::new();
    let mut working: Vec<&str> = Vec::new();
    for src in sources {
        let rec = Recorder {
            inner: http,
            log: RefCell::new(vec![]),
        };
        out.push(format!("== Source: {} ==", src.name()));
        let res = src.fetch(&rec, now);
        for x in rec.log.borrow().iter() {
            match &x.outcome {
                Ok((st, n, _)) => out.push(format!("GET {}\n    HTTP status {}, {} bytes", x.url, st, n)),
                Err(e) => out.push(format!("GET {}\n    request error: {}", x.url, e)),
            }
        }
        match &res {
            Ok(f) => {
                out.push(format!("leagues found: {} (wanted: {})", f.leagues_total, f.leagues_wanted));
                let counts: Vec<String> = ALL_KEYS
                    .iter()
                    .map(|k| format!("{} {}", k.code(), f.matches.iter().filter(|m| m.league == *k).count()))
                    .collect();
                out.push(format!("upcoming matches per wanted league: {}", counts.join(", ")));
                out.push(format!("matches total: {}", f.matches.len()));
                out.push("first 5 matches:".into());
                if f.matches.is_empty() {
                    out.push("    (none)".into());
                }
                for m in f.matches.iter().take(5) {
                    out.push(format!("    {}", display::menu_text(m, now, local)));
                }
                if f.leagues_total > 0 && !f.matches.is_empty() {
                    working.push(src.name());
                }
            }
            Err(e) => out.push(format!("ERROR: {}", e)),
        }
        out.push(format!("-- raw responses (first {} bytes each) --", RAW_BYTES));
        for x in rec.log.borrow().iter() {
            out.push(format!("[{}]", x.url));
            match &x.outcome {
                Ok((_, _, raw)) => out.push(raw.clone()),
                Err(e) => out.push(format!("(no response: {})", e)),
            }
        }
        out.push(String::new());
    }
    out.push("== Summary ==".into());
    out.push(if working.is_empty() {
        "No source returned matches. The strip will show \"LoL --\".".into()
    } else {
        format!("Sources that returned matches: {}", working.join(", "))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::tests::{lol_mock, now};
    use crate::source::{Leaguepedia, LolEsports};

    #[test]
    fn report_has_counts_matches_and_errors() {
        let src: Vec<Box<dyn Source>> = vec![Box::new(LolEsports), Box::new(Leaguepedia)];
        let t = report(&src, &lol_mock(), now(), &|u| u + 7200).join("\n");
        assert!(t.contains("== Source: lolesports =="));
        assert!(t.contains("HTTP status 200"));
        assert!(t.contains("leagues found: 12 (wanted: 10)"));
        assert!(t.contains("LEC 1, LPL 1, LCK 1, LCS 0, CBLOL 1, Intl 1"));
        assert!(t.contains("LPL  BLG v JDG  LIVE  Bo3"));
        assert!(t.contains("== Source: leaguepedia =="));
        assert!(t.contains("ERROR: connection refused"));
        assert!(t.contains("Sources that returned matches: lolesports"));
        assert!(t.contains("\"leagues\""));
    }

    #[test]
    fn raw_is_cut_on_char_boundary() {
        let s = "é".repeat(2000);
        assert_eq!(head(&s, 2048).len(), 2048);
        assert_eq!(head("héllo", 2), "h");
    }
}
