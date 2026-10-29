# Contributing to stats-lite

Thanks for helping. Bug reports, fixes and features are all welcome. For anything larger than
a fix, open an issue first so we can agree on the shape before you write the code.

## Setup

You need a Mac with Apple silicon and Rust 1.94 or later. The formatter needs a nightly
toolchain for its import options:

```sh
rustup toolchain install nightly --component rustfmt
cargo run --release            # straight from the tree
scripts/bundle.sh debug        # or a StatsLite.app in target/debug
```

## Before you open a pull request

CI runs these on Apple silicon; run them first:

```sh
cargo +nightly fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

## How we work

- **Comments explain why**, not what. Match the density and style of the code around you.
- **Commit messages** start with a short summary in the imperative or as a statement of what
  now happens ("disk errors show an em dash"), then explain the problem and the fix.
- **Licensing:** stats-lite is MIT.

## Releases

Releases are built locally by the maintainer; [`RELEASING.md`](RELEASING.md) has the steps.
Each one bumps the version in `Cargo.toml` and dates the `Unreleased` entries in
[`CHANGELOG.md`](CHANGELOG.md).

By contributing, you agree that your contributions are licensed under the MIT license, and
to follow the [code of conduct](CODE_OF_CONDUCT.md).
