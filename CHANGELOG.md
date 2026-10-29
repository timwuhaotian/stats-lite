# Changelog

Notable changes to stats-lite. Versions match `Cargo.toml` and the app's Info.plist.

## Unreleased

## 0.2.0 — 2026-10-29

- **Check for Updates….** stats-lite checks `releases/latest.toml` at launch and
  every six hours, and whenever you ask from the menu. A newer release downloads
  in the background (progress shows in the menu item), is checked against its
  checksum, version, Developer ID signature and Apple notarization, then waits:
  quitting installs it, and **Restart to Update to x.y.z** quits and reopens it.
- **Direct download.** The release is one file,
  [`StatsLite-arm64.dmg`](https://github.com/timwuhaotian/stats-lite/raw/main/releases/StatsLite-arm64.dmg),
  always the latest; drag **StatsLite** to **Applications**.

## 0.1.0 — 2026-10-29

- Menu-bar CPU and disk % as SF Symbol icons + monospaced numbers: CPU sampled
  every 2 s, `/` volume every 60 s, title only rebuilt when a number changes.
- Right-aligned numbers keep neighbouring icons still; an unreadable volume shows
  `—`, never `0%`.
- The only UI besides the numbers is Quit (`⌘Q`); the tooltip carries the words
  for hover and VoiceOver.
