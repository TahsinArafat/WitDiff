#!/usr/bin/env bash
set -euo pipefail
cargo install --path crates/witdiff-cli --locked 2>/dev/null || cargo install --path crates/witdiff-cli
