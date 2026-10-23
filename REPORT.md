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
- Live answers of both sources (see "Which source works").
- Taskbar placement, tray menu, balloon, Start with Windows on a real PC.
- The public x-api-key (0TvQ...) is the one used by the lolesports.com client
  and by several open-source projects; found via GitHub code search, not in
  the site's own JavaScript (host not reachable from the build sandbox).

## Which source works
- Pending: first CI run (probe step, artifact probe.txt).

## Behaviour notes
- Order: lolesports gateway, then Leaguepedia Cargo as automatic fallback.
- Refresh 10 min; 1 min while LIVE or a match starts within 15 min; a failed
  refresh retries after 2 min, keeps the last good data (cache.json) and
  shows "?".
- PandaScore needs a token: not used. Option if both sources fail.
