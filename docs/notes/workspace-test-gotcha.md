# `cargo test` from the repo root does NOT test workspace member crates

**Lesson: always run `cargo test --workspace` — bare `cargo test` in this repo tests
only the root `floppy_spin` package (bins), silently skipping every `crates/*` lib.**

Observed at M0: bare `cargo test` reported "0 tests" across the board while
`floppy_io` had 13 passing tests. The root Cargo.toml is both a package and the
workspace root, so cargo scopes to the current package by default.

All gates/CI invocations use `--workspace` (`test`, `clippy`). Same for one-package
runs during development: prefer `-p <crate>` explicitly.
