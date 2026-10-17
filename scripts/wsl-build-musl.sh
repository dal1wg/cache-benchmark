#!/bin/bash
# 在 WSL 中执行 scripts/build.sh（musl 静态构建 + 打包）
set -e
export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/d/Code/touch/cache-benchmark
# 清理旧命名产物
rm -f dist/cache-benchmark-x86_64-unknown-linux-musl dist/cache-bench-ctl-x86_64-unknown-linux-musl
./scripts/build.sh x86_64
