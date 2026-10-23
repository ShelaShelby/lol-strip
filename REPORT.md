# Report (0.1.0)

Built without Windows and without access to the LoL hosts; checked by unit
tests, cross type-check/clippy for x86_64-pc-windows-msvc and the Windows CI.

## Verified
- Unit tests (no network) on hand-written fixtures that follow the community
  docs for both sources: parsing, league filter incl. INTERNATIONAL, local
  time + weekday rule, strip text (today / other day / LIVE / empty / stale),
  sorting, fallback between sources, notify once per match, settings, cache.
- Windows code type-checked and clippy-clean for x86_64-pc-windows-msvc.

## Unverified (needs the live probe / Alex's PC)
- Leaguepedia fallback: live answer never seen (rate limited, see below).
- Why LEC/LPL/LCK/LCS/CBLOL show 0 matches in the probe (36 Intl): the API
  returned no upcoming domestic events on 2026-10-23, most likely off-season
  while Worlds runs; not confirmed independently.
- Taskbar placement, tray menu, balloon, Start with Windows on a real PC.
- The x-api-key came from open-source projects (GitHub code search), not from
  the site's own JavaScript; the CI probe confirmed it works today.

## Which source works
- lolesports gateway WORKS (CI probe, Windows runner, 2026-10-23): getLeagues,
  getSchedule, getLive all HTTP 200; 51 leagues, 14 wanted; 36 upcoming
  Worlds matches, first one LIVE. It ships as default.
- Leaguepedia Cargo: HTTP 200 but {"error":"ratelimited"} from the runner,
  so unverified; kept as automatic fallback.

## Behaviour notes
- Order: lolesports gateway, then Leaguepedia Cargo as automatic fallback.
- Refresh 10 min; 1 min while LIVE or a match starts within 15 min; a failed
  refresh retries after 2 min, keeps the last good data (cache.json) and
  shows "?".
- PandaScore needs a token: not used. Option if both sources fail.
