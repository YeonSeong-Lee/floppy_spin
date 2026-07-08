# Lesson: the ship gate runs on a fresh clone, and it runs LAST

A green working tree is not a green build. The working tree can pass every
gate while depending on an untracked file, an uncommitted edit, or a
generated artifact that a fresh checkout won't have. The only honest final
gate is: `git clone` the repo to a scratch directory and run the whole
battery there (`cargo build --release`, `cargo test --workspace --release`,
the `gate` bin, `--golden check`, the WAV golden, clippy, fmt, a determinism
double-run). FLOPPY SPIN's clone came up with zero untracked files and every
gate green, which is what "self-contained" actually means.

Corollary: re-run the clean checkout AFTER the last code change, not after
the last milestone. The window-scale fix landed as a post-verifier commit;
the ship gate that counts is the one on that commit (122e4bb), not the one
on the commit the final verifier happened to inspect (be29dbc). A fix, however
small and "presentation-only," resets the clock on the clean-checkout gate.
