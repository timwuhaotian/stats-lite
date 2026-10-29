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
4. **README.** It must describe what this release does. Put no version number in it.
5. **Build and install it here**, as a user would get it:
   ```sh
   scripts/bundle.sh release
   ditto target/release/StatsLite.app /Applications/StatsLite.app
   defaults read /Applications/StatsLite.app/Contents/Info.plist CFBundleShortVersionString
   ```
   Launch it and check the menu-bar numbers appear and update.
6. **Commit** on `main`, everything at once, as `stats-lite x.y.z: <what changed,
   briefly>`, with a body listing the changes.
7. **Push** `main` and check CI passes:
   ```sh
   git push origin main
   ```
