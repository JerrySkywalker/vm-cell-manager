#!/usr/bin/env sh
set -eu

rustc --version
cargo --version
cargo metadata --locked --offline --all-features --format-version 1 >/dev/null
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets --all-features
cargo test --locked --workspace --all-features --doc
sh -n tools/linux-kvm-preflight.sh
sh -n tools/test-linux-kvm-preflight.sh
sh tools/test-linux-kvm-preflight.sh
