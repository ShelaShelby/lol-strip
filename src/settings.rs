//! Small key=value settings file in %APPDATA%\lol-strip.

use crate::model::{LeagueKey, ALL_KEYS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Per league on/off, indexed like `ALL_KEYS`.
    pub leagues: [bool; 6],
    /// Tray balloon 10 minutes before a match.
    pub notify: bool,
    /// "Start with Windows" was switched on by default once (first run).
    pub autostart_init: bool,
    /// Monitor device names (e.g. \\.\DISPLAY2) where the strip is hidden.
    pub hidden_monitors: Vec<String>,
    /// Fallback placement when the tray rect is unknown: logical px from the
    /// right (or bottom) end of the taskbar, and upward shift.
    pub fallback_offset_x: i32,
    pub fallback_offset_y: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            leagues: [true; 6],
            notify: true,
            autostart_init: false,
            hidden_monitors: vec![],
            fallback_offset_x: 140,
            fallback_offset_y: 0,
        }
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn idx(k: LeagueKey) -> usize {
    ALL_KEYS.iter().position(|x| *x == k).unwrap_or(0)
}

impl Settings {
    pub fn league_on(&self, k: LeagueKey) -> bool {
        self.leagues[idx(k)]
    }

    pub fn set_league(&mut self, k: LeagueKey, on: bool) {
        self.leagues[idx(k)] = on;
    }

    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            if let Some(code) = k.strip_prefix("league_") {
                if let (Some(key), Some(b)) = (LeagueKey::from_code(code), parse_bool(v)) {
                    s.set_league(key, b);
                }
                continue;
            }
            match k {
                "notify" => s.notify = parse_bool(v).unwrap_or(s.notify),
                "autostart_init" => s.autostart_init = parse_bool(v).unwrap_or(s.autostart_init),
                "hidden_monitors" => s.hidden_monitors = v.split('|').map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect(),
                "fallback_offset_x" => s.fallback_offset_x = v.parse::<i32>().map(|x| x.clamp(0, 4000)).unwrap_or(s.fallback_offset_x),
                "fallback_offset_y" => s.fallback_offset_y = v.parse::<i32>().map(|x| x.clamp(-500, 500)).unwrap_or(s.fallback_offset_y),
                _ => {}
            }
        }
        s
    }

    pub fn serialize(&self) -> String {
        let mut o = String::from("# LoL Strip settings\n");
        for k in ALL_KEYS {
            o.push_str(&format!("league_{}={}\n", k.code().to_ascii_lowercase(), self.league_on(k) as u8));
        }
        o.push_str(&format!(
            "notify={}\nautostart_init={}\nhidden_monitors={}\n# fallback placement if the tray area cannot be read (logical px)\nfallback_offset_x={}\nfallback_offset_y={}\n",
            self.notify as u8,
            self.autostart_init as u8,
            self.hidden_monitors.join("|"),
            self.fallback_offset_x,
            self.fallback_offset_y
        ));
        o
    }

    pub fn monitor_enabled(&self, name: &str) -> bool {
        !self.hidden_monitors.iter().any(|m| m.eq_ignore_ascii_case(name))
    }

    pub fn toggle_monitor(&mut self, name: &str) {
        if self.monitor_enabled(name) {
            self.hidden_monitors.push(name.to_string());
        } else {
            self.hidden_monitors.retain(|m| !m.eq_ignore_ascii_case(name));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_all_on() {
        let s = Settings::default();
        assert!(ALL_KEYS.iter().all(|k| s.league_on(*k)));
        assert!(s.notify && !s.autostart_init);
        assert_eq!(Settings::parse(""), s);
    }

    #[test]
    fn roundtrip() {
        let mut s = Settings {
            notify: false,
            autostart_init: true,
            fallback_offset_x: 200,
            ..Settings::default()
        };
        s.set_league(LeagueKey::Lcs, false);
        s.set_league(LeagueKey::Intl, false);
        s.toggle_monitor(r"\\.\DISPLAY2");
        let t = s.serialize();
        assert!(t.contains("league_lcs=0") && t.contains("league_cblol=1"));
        assert_eq!(Settings::parse(&t), s);
        assert_eq!(
            Settings::parse("garbage\nnotify=maybe\nleague_zzz=0\nleague_lck=off\nfallback_offset_x=-5"),
            {
                let mut d = Settings {
                    fallback_offset_x: 0,
                    ..Settings::default()
                };
                d.set_league(LeagueKey::Lck, false);
                d
            }
        );
    }

    #[test]
    fn monitor_toggle() {
        let mut s = Settings::default();
        assert!(s.monitor_enabled(r"\\.\DISPLAY1"));
        s.toggle_monitor(r"\\.\DISPLAY1");
        assert!(!s.monitor_enabled(r"\\.\display1"));
        s.toggle_monitor(r"\\.\DISPLAY1");
        assert!(s.monitor_enabled(r"\\.\DISPLAY1"));
    }
}
