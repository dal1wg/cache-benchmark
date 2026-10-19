# cache-benchmark

<p align="center">
  <strong>高性能分布式压测工具，支持 Redis、Valkey、Tair 及任何 RESP 兼容数据库</strong>
</p>

<p align="center">
  纯 Rust（无 Python 依赖）&bull; 单一静态二进制 &bull; 多服务器编排
</p>

<p align="center">
  <a href="#安装">安装</a> &bull; <a href="#快速开始">快速开始</a> &bull; <a href="#命令行选项">命令行选项</a> &bull; <a href="#分布式压测">分布式压测</a>
</p>

---

`cache-benchmark` 是 [resp-benchmark](https://github.com/tair-opensource/resp-benchmark) 的纯 Rust 移植版（移除 Python 依赖），并新增了 `cache-bench-ctl` 管理工具——可在一台管理机上把压测程序通过 SSH 分发到多台服务器、同步启动所有实例，并将结果收敛为一份聚合报告。

## 组件

| 二进制 | 角色 |
|---|---|
| `cache-benchmark` | 压测程序（worker）。一个进程压测一个目标：standalone 或 Redis Cluster（通过 `CLUSTER SLOTS` 自动发现拓扑，请求按 key 哈希槽路由，压力均匀分布到所有主节点）。 |
| `cache-bench-ctl` | 管理工具（controller）。仅在管理机上运行：通过 SSH 分发 worker、同步启动、按桶合并延迟直方图生成聚合报告。 |

## 架构

```
管理机                                压测机                      被压目标
┌─────────────────────┐   分发/启动/收敛   ┌──────────────────┐
│ cache-bench-ctl     │ ────────────────▶ │ cache-benchmark  │ ──▶ Redis standalone
│ (仅一台)            │                   │ (worker, 多台)    │ ──▶ Redis Cluster (多 master)
└─────────────────────┘                   └──────────────────┘
```

## 安装

从 [Releases](../../releases) 下载预编译包（Linux x86_64/aarch64 提供 musl 静态链接版本，任意 glibc 版本的 Linux 均可直接运行；另提供 macOS 和 Windows 版本）。压缩包内包含 `cache-benchmark`、`cache-bench-ctl` 两个二进制和一个 `hosts.toml` 服务器清单模板。

或从源码构建：

```bash
cargo build --release
# 如需纯 Rust SSH 通道（不依赖系统 ssh 命令）：
cargo build --release --features russh
```

**构建静态链接的 Linux 二进制**（推荐用于分发，无 glibc 版本要求）：

```bash
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools   # Debian/Ubuntu
cargo build --release --target x86_64-unknown-linux-musl -p cache-benchmark -p cache-bench-ctl
```

## 快速开始

```bash
# 基础压测：随机 key + 64 字节 value 的 SET，持续 10 秒
cache-benchmark -s 10 "SET {key uniform 100000} {value 64}"

# 先灌入 100 万数据，再压测读取
cache-benchmark --load -n 1000000 "SET {key sequence 1000000} {value 64}"
cache-benchmark -s 10 "GET {key uniform 1000000}"

# 128 连接、pipeline 深度 10，持续 30 秒
cache-benchmark -c 128 -P 10 -s 30 "SET {key uniform 1000000} {value 128}"

# 集群模式：自动发现拓扑，压力均匀打到所有主节点
cache-benchmark --cluster -h cluster-endpoint -p 7000 -s 30 "GET {key uniform 1000000}"

# Lua 脚本
cache-benchmark --lua -s 10 "local key = bench.key(10000, 'uniform', 'test'); function generate() return {'SET', key(), bench.value(64)()} end"
cache-benchmark --lua-file -s 10 workloads/hello.lua
```

> **注意**：高 RTT 环境下（如跨网段压测）请使用 `-P`（pipeline），否则每请求一次网络往返会严重限制 QPS。压测程序应尽量与被测服务器同机或同网段部署。

### 占位符语法

与 resp-benchmark 完全兼容：

| 占位符 | 说明 | 示例 |
|---|---|---|
| `{key uniform N}` | 从 `key_0000000000` 到 `key_{N-1}` 均匀随机 | `{key uniform 100000}` → `key_0000042371` |
| `{key sequence N}` | 顺序生成 0 到 N-1，循环；适合灌数据（每个 key 恰好写一次） | `{key sequence 100000}` |
| `{key zipfian N}` | Zipfian 分布（指数 1.03），少数 key 承接大部分请求，模拟真实缓存热点 | `{key zipfian 100000}` |
| `{value N}` | 每次请求生成 N 字节随机字母数字串 | `{value 64}` |
| `{rand N}` | 0 到 N-1 的随机整数 | `{rand 1000}` → `742` |
| `{range N W}` | 两个整数：[0, N-1] 内随机起点和 start+W（截断到 N-1），适合范围查询 | `{range 1000 10}` → `45 55` |

### 集群模式说明

- 连接种子节点（集群任意节点均可）后自动执行 `CLUSTER SLOTS` 发现全部主从节点
- 启动时打印拓扑：`cluster topology: 3 masters, 3 replicas, slots 16384/16384 covered` + 主节点列表
- 每条命令按 key 的 CRC16 哈希槽路由到对应 master，key 均匀 → 压力均匀分布到所有主节点
- MOVED/ASK 自动跟随，拓扑变更自动刷新；只压主节点
- 集群模式下 GET、SET、INCR 等常见单 key 命令按 master 分组，向各 master 并发发送真正的 pipeline；其他命令保留通用集群路由。
- MOVED/ASK 等重定向发生时，仅重试被拒绝的命令，并切换回通用集群客户端，不重放已成功的写入。
- `--json` 输出包含 `cluster_nodes`（主节点地址列表）

## 命令行选项

| 选项 | 说明 | 默认值 |
|---|---|---|
| `-h` `-p` `-u` `-a` | 服务器地址 / 端口 / ACL 用户名 / 密码 | 127.0.0.1 / 6379 / "" / "" |
| `-c` | 连接数（0 = 自动扩连：QPS 增长不足 5% 时锁定） | 0 |
| `--max-connections` | `-c 0` 自动扩连上限（每个 worker 的逻辑客户端数；集群模式每个客户端可连接多个 master） | 1024 |
| `-n` / `-s` | 总请求数 / 持续秒数（0 = 不限） | 0 |
| `-t` | 目标 QPS 限流（0 = 不限） | 0 |
| `-P` | pipeline 深度 | 1 |
| `--cores` | 绑定 CPU 核心，如 `0,1,2` 或 `0-3` | 全部 |
| `--cluster` | 集群模式 | false |
| `--load` | 仅灌数据，不压测 | false |
| `--short-connection` | 每条命令新建连接（不支持 load，pipeline 必须为 1） | false |
| `--lua` / `--lua-file` | 命令是 Lua 脚本 / Lua 脚本文件路径 | false |
| `-q` | 静默模式 | false |
| `--tls` | TLS 连接（`rediss://`） | false |
| `--json` | 结束时输出单行 JSON 结果（供 ctl 消费） | false |
| `--start-at <unix_ts>` | 等待到指定 unix 时间戳再开始（分布式同步启动，支持小数秒；ctl 自动按各机时钟偏移修正） | — |

## 分布式压测

### 1. 编写服务器清单 `hosts.toml`

压缩包内自带模板；也可以直接用项目根目录的 [hosts.toml](hosts.toml) 复制修改：

```toml
# 每条 [[hosts]] 定义一台压测机，可重复多条
[[hosts]]
address = "10.0.0.1"       # 必填，压测机 IP / 主机名
port = 22                  # 可选，ssh 端口，默认 22
user = "root"              # 可选，ssh 用户，默认 root
remote_dir = "/root/cache-benchmark/tmp"   # 可选，worker 落盘目录
# arch = "x86_64"          # 可选，覆盖架构自动探测
# password = "xxx"         # 可选，仅 --transport russh 生效

[[hosts]]
address = "10.0.0.2"
```

> 使用前确保管理机到各压测机已配置 SSH 免密：`ssh-copy-id root@<address>`

### 2. 分发

把编译好的 worker 放入本地产物目录（命名 `cache-benchmark-<架构>`，如 `cache-benchmark-x86_64`；也兼容旧的 `cache-benchmark-<平台三元组>` 命名）：

```bash
./scripts/build.sh                  # 编译并输出 dist/cache-benchmark-x86_64、dist/cache-bench-ctl-x86_64
cache-bench-ctl distribute          # 自动探测每台机器架构（uname -m）并选择匹配的二进制
```

### 3. 同时运行并收敛结果

```bash
# 所有机器同时压测一个集群，结果收敛为一份聚合报告
cache-bench-ctl run --lead 10 --output result.json -- \
    -h 10.0.1.10 -p 7000 --cluster -c 128 -s 30 "GET {key uniform 1000000}"
```

`run` 的工作流程：

1. 计算统一启动时刻（当前时间 + `--lead` 秒），加 `--skip-distribute` 可跳过分发
2. 逐台探测远端时钟偏移（NTP 中点法，`date +%s.%N`），为每台机器生成按其本地钟修正的 `--start-at <ts>`——压测机时钟不准（偏差可达几十秒）也能真实同步；worker 支持小数时间戳（毫秒精度）
3. 在每台机器上启动 `cache-benchmark <参数> --start-at <修正后的 ts> -q --json`（对时补偿后启动误差 <10ms）
3. 实时转发各机器输出，带 `[user@host]` 前缀
4. 收敛聚合：
   - 总 QPS = 各机器求和
   - 延迟分位数（p50/p95/p99/avg）由**按桶合并的直方图**重算——数学上正确，不是对各机器均值再平均
   - 单台失败不影响其余机器，报告中标注失败节点

### SSH 通道

```bash
cache-bench-ctl --transport ssh run ...    # 默认：系统 ssh/scp（复用 ~/.ssh/config、密钥、跳板机）
cache-bench-ctl --transport russh run ...  # 纯 Rust 实现（需 --features russh 编译；支持密钥文件 + 密码认证）
```

**密码认证**：在 `hosts.toml` 中为某台机器配置 `password` 后：

- **系统 ssh 通道**：自动通过 `sshpass` 使用密码登录（需在管理机安装：`apt install sshpass` / `yum install sshpass`）；未安装时会给出明确提示
- **russh 通道**：原生支持，无需额外工具

未配置 `password` 时，系统 ssh 通道使用密钥 / ssh-agent / `~/.ssh/config`，无免密配置时会快速失败（`BatchMode=yes`）。

## 开发

```bash
cargo test --workspace          # 单元测试 + 功能测试（无需外部环境）

# 集成测试（默认跳过，通过环境变量指定目标后手动运行）
REDIS_HOST=192.168.102.34:6379 cargo test -p cache-benchmark-tests --test integration_standalone -- --ignored
CLUSTER_SEED=192.168.102.34:16385 cargo test -p cache-benchmark-tests --test integration_cluster -- --ignored
CTL_HOSTS_FILE=hosts.toml cargo test -p cache-benchmark-tests --test integration_distributed_run -- --ignored

./scripts/build.sh              # 构建 musl 静态二进制并打包（在 WSL/Linux 中）
./scripts/build.sh aarch64      # aarch64 musl 交叉编译
```

测试体系分三层（位于 `tests/`）：

| 层 | 目录 | 说明 |
|---|---|---|
| 单元测试 | `tests/benchmark/`、`tests/bench-ctl/` | 内部函数级：占位符生成、分布采样、并发限额、聚合数学等 |
| 功能测试 | `tests/functional/` | 黑盒运行二进制：CLI 行为、`--json` 契约、`--start-at`、真实压测流程（对内置 mock RESP 服务器） |
| 集成测试 | `tests/integration/` | 真实环境：standalone/集群拓扑与负载均匀性/Lua 数据验证/ctl 多机编排，未设置环境变量自动跳过 |

## Changelog

见 [CHANGELOG.md](CHANGELOG.md)。

## License

MIT

## 本地压力优化版 0.1.3-pressure.1

本版本优化集群按 master 批处理、命令生成和延迟统计；统计按工作线程分片，每批只更新一次计数，减少多核原子操作争用。默认核心范围使用与 CPU 绑定相同的可用核心列表长度，避免两套核心计数不一致。

自动扩连默认上限仍为每 worker 1024 个逻辑客户端，可使用 `--max-connections 4096` 调整；这不是物理 TCP 连接总上限。自动模式按 QPS 增长决定何时停止扩连，不以 Redis CPU 是否达到 100% 为停止条件。

```bash
cache-benchmark -h 10.6.205.47 -p 16380 --cluster \
  -a '实际Redis密码' -c 0 --max-connections 4096 -P 64 -s 120 \
  'SET {key uniform 1000000} {value 64}'
```

自动模式的 QPS、count 和延迟直方图统一排除已完成的扩连预热阶段。`-s` 从扩连结束后开始计时；`-n` 仍限制包含预热在内的总发出请求数。若在预热结束前已执行完 `-n`，结果会统计整段执行，不再错误报告 QPS 为 0。

`-t` 现按请求数扣减配额，而不是按 pipeline 批次扣减。延迟仍为整批完成时间，并非逐请求单独计时。

CPU 是否满载需要现场检查主从角色、压力端能力、网络及 Redis 配置；此修改不承诺每个 Redis 进程都达到 100%。同机压测还需注意压力程序和 Redis 的资源竞争。
