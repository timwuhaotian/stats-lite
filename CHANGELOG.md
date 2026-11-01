# Changelog

Notable changes to stats-lite. Versions match `Cargo.toml` and the app's Info.plist.

## Unreleased

## 0.2.4 — 2026-11-01

- **Disk % no longer looks stuck.** It now shows one decimal (e.g. `48.7%`):
  at whole-percent precision 1% of a ~500 GB disk is ~5 GB, so the number
  could sit unchanged for days while the disk filled underneath it. The
  reading itself was live — one `statfs` call, now every 30 s, of the
  whole APFS container; that is also why it differs from `df`'s Capacity
  column, which counts only the Data volume's own files.

## 0.2.3 — 2026-11-01

- **Disk % now agrees with `df` and Storage settings.** It reads the Data
  volume (`/System/Volumes/Data`, falling back to `/` on layouts without
  one) with `statfs`, so purgeable space no longer counts as free — the
  number that showed ~40% now shows the ~50% the system reports.
- **Update checks can't hang forever.** A finished check's result can no longer
  be swallowed before the update window shows it, and every external tool a
  check runs (curl, codesign, spctl, hdiutil, …) now times out and fails the
  check instead of leaving it on “Checking for updates…”.

## 0.2.2 — 2026-10-29

- **Update feedback.** **Check for Updates…** opens a small window that follows
  checking, downloading and verification, then reports the result or offers
  **Restart Now**. Automatic checks stay silent.

## 0.2.1 — 2026-10-29

- **An app icon.** A chip with its two numbers (teal CPU, amber disk) for Finder,
  Spotlight and the disk image.

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
