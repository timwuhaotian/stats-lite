# Releasing stats-lite

Maintainers build locally; CI never builds releases. A release, in order:

1. **Start clean and green.** Everything to ship is committed on `main`, and these pass:
   ```sh
   cargo +nightly fmt --all --check
   cargo clippy --all-targets --locked -- -D warnings
   cargo test --locked
   ```
2. **Bump the version** in `Cargo.toml` (`[package] version`). `Cargo.lock` follows
   during the build.
3. **Changelog.** In `CHANGELOG.md`, put a `## x.y.z — YYYY-MM-DD` heading under
   `## Unreleased`, so the release takes its entries and `Unreleased` starts empty.
4. **README.** It must describe what this release does: keep its download link exactly
   `https://github.com/timwuhaotian/stats-lite/raw/main/releases/StatsLite-arm64.dmg`
   (step 5 refuses to start otherwise), and put no version number in it.
5. **Build:** `scripts/release.sh`. It signs with Developer ID and the hardened runtime,
   notarizes and staples the app, then the DMG, checks both with `spctl`, and overwrites
   `releases/StatsLite-arm64.dmg`, its `.sha256` and `releases/latest.toml` (the version
   and checksum every installed copy's updater reads). It fails unless Apple accepts
   both. Notarization takes a few minutes; credentials are described at the top of
   the script.
6. **Install it here**, from the DMG, as a user would get it:
   ```sh
   mnt=$(hdiutil attach -nobrowse -readonly releases/StatsLite-arm64.dmg | awk -F'\t' '/\/Volumes\//{print $NF}')
   rm -rf /Applications/StatsLite.app && ditto "$mnt/StatsLite.app" /Applications/StatsLite.app
   hdiutil detach -quiet "$mnt"
   defaults read /Applications/StatsLite.app/Contents/Info.plist CFBundleShortVersionString
   spctl -a -vv /Applications/StatsLite.app     # must say: Notarized Developer ID
   ```
   Launch it and check the menu-bar numbers appear and update.
7. **Commit** on `main`, everything at once (version, lock, changelog, README, DMG,
   sha256, `latest.toml`), as `stats-lite x.y.z: <what changed, briefly>`, with a body
   listing the changes and the notarization result.
8. **Push** `main`, and check that CI passes and the download link serves the new DMG:
   ```sh
   git push origin main
   curl -sIL https://github.com/timwuhaotian/stats-lite/raw/main/releases/StatsLite-arm64.dmg | grep -i content-length
   ```
   The length matches `stat -f %z releases/StatsLite-arm64.dmg`.
