#!/usr/bin/env bash
set -euo pipefail
cargo build --release
echo "Binary at: target/release/sentra_fim"
