#!/bin/bash
# Build cache-benchmark inside WSL (Linux binary), independent of the Windows target dir.
set -e
export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/d/Code/touch/cache-benchmark
CARGO_TARGET_DIR="$HOME/cb-target" cargo build --release -p cache-benchmark -p cache-bench-ctl
