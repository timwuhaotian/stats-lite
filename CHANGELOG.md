# Changelog

Notable changes to stats-lite. Versions match `Cargo.toml` and the app's Info.plist.

## Unreleased

## 0.1.0 — 2026-10-29

- Menu-bar CPU and disk % as SF Symbol icons + monospaced numbers: CPU sampled
  every 2 s, `/` volume every 60 s, title only rebuilt when a number changes.
- Right-aligned numbers keep neighbouring icons still; an unreadable volume shows
  `—`, never `0%`.
- The only UI besides the numbers is Quit (`⌘Q`); the tooltip carries the words
  for hover and VoiceOver.
