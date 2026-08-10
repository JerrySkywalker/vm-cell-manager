#!/usr/bin/env sh
set -eu

: "${CARGO_TARGET_DIR:?CARGO_TARGET_DIR must be bound outside the checkout}"

cargo build --locked --release --bin vmcell
python3 tools/test-linux-package.py --binary "$CARGO_TARGET_DIR/release/vmcell"
