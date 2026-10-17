#!/bin/bash
# 构建 musl 静态链接的二进制并打包。
#
# 产物：dist/cache-benchmark-<arch>       （如 dist/cache-benchmark-x86_64）
#       dist/cache-bench-ctl-<arch>       （如 dist/cache-bench-ctl-x86_64）
#       dist/hosts.toml                   （服务器清单模板，随包分发）
#       dist/cache-benchmark-<version>-<arch>-linux-musl.tar.gz  (打包，含上面三个文件)
#
# 用法：
#   ./scripts/build.sh              # x86_64 musl（默认）
#   ./scripts/build.sh aarch64      # aarch64 musl（交叉编译，需安装交叉工具链）
#   SKIP_TARBALL=1 ./scripts/build.sh   # 只编译不打包
set -e

ARCH="${1:-x86_64}"
TARGET="${ARCH}-unknown-linux-musl"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

export PATH="$HOME/.cargo/bin:$PATH"

echo "==> 检查 musl 工具链"
if ! command -v musl-gcc >/dev/null 2>&1; then
    echo "    未找到 musl-gcc，尝试安装 musl-tools（需要 sudo）..."
    sudo apt-get update -qq && sudo apt-get install -y -qq musl-tools
fi

if [ "$ARCH" = "aarch64" ]; then
    if ! command -v aarch64-linux-gnu-gcc >/dev/null 2>&1; then
        echo "    未找到 aarch64 交叉编译器，尝试安装 gcc-aarch64-linux-gnu（需要 sudo）..."
        sudo apt-get install -y -qq gcc-aarch64-linux-gnu
    fi
    export CC_aarch64_unknown_linux_musl=aarch64-linux-gnu-gcc
    export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc
fi

echo "==> 添加 Rust target: $TARGET"
rustup target add "$TARGET"

echo "==> 编译（release + musl 静态链接）"
cargo build --release --target "$TARGET" -p cache-benchmark -p cache-bench-ctl

mkdir -p dist
WORKER="cache-benchmark-$ARCH"
CTL="cache-bench-ctl-$ARCH"
cp "target/$TARGET/release/cache-benchmark" "dist/$WORKER"
cp "target/$TARGET/release/cache-bench-ctl" "dist/$CTL"

echo "==> 校验链接方式（应显示 statically linked）"
if command -v file >/dev/null 2>&1; then
    file "dist/$WORKER"
fi

if [ -z "$SKIP_TARBALL" ]; then
    VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
    TARBALL="dist/cache-benchmark-${VERSION}-${ARCH}-linux-musl.tar.gz"
    echo "==> 打包 $TARBALL（含 hosts.toml 模板）"
    if [ ! -f hosts.toml ]; then
        echo "    ERROR: 未找到项目根目录的 hosts.toml 模板" >&2
        exit 1
    fi
    cp hosts.toml dist/hosts.toml
    tar -czf "$TARBALL" -C dist "$WORKER" "$CTL" "hosts.toml"
fi

echo "==> 完成。产物目录："
ls -la dist/
