#!/usr/bin/env bash
set -e
cd "$(dirname "$0")/.."
cargo build --release --bin floppy_spin
cargo run --release --bin gate
