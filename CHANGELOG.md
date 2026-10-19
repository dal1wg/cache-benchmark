# Changelog

本项目的所有显著变更都记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本管理遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

## [0.1.4] - 2026-10-19

### 新增

- **集群模式按 master 分组的真 pipeline**（合并 PR #1，`cluster_pipeline.rs`）：
  GET/SET/INCR 等常见单 key 命令按哈希槽映射到 master 后批量发送；MOVED/ASK
  等重定向仅重试被服务端拒绝的命令，不重放已成功的写入。本地 3 主节点实测
  QPS 提升约 2 倍，客户端 CPU 与内存大幅下降。
- **`--max-connections`**：`-c 0` 自动扩连的逻辑客户端上限（默认 1024）；
  自动扩连停止阈值从 30% 收紧到 5%。
- **`--key-prefix` 自定义 key 前缀（memtier 风格）**：`{key ...}` 占位符生成的
  key 前缀可配置（默认仍为 `key_`，向后兼容），支持 uniform/zipfian/sequence
  全部分布；数字保留 10 位零填充；前缀含空白直接报错拒绝（避免把 key 撕成
  多个命令参数）；前缀带 `{tag}` 时按 cluster hashtag 正确聚合 slot。
  写压测/灌数据/读压测用同一前缀即可闭环验证（见 README 示例）。

### 修复

- `-t` 目标 QPS 按请求数扣减配额（原先按 pipeline 批次扣减，`-P > 1` 时实际
  QPS 超标约 P 倍）；拒绝 `-P 0` 与超出限流器整数范围的 `-t`。
- 统计口径统一：自动扩连预热阶段不计入 QPS/count/延迟直方图；`-n` 在预热
  结束前耗尽时统计整段执行，不再误报 QPS 为 0。
- 直方图按工作线程分片 + 每批一次记录，消除多核原子争用；计时检查改
  `OnceLock`，去除热路径读锁。
- 默认核心范围与 `core_affinity` 使用同一可用核心列表，避免两套探测方式
  不一致导致越界。

### 变更

- **cache-bench-ctl 默认内置 russh 传输**：`russh` feature 进入 default features，
  `cargo build --release` 与官方分发的二进制同时支持 `--transport ssh` 和
  `--transport russh`，无需再加 `--features russh` 编译；需要最小构建时用
  `--no-default-features`。**升级注意**：此前用 `--no-default-features` 意图
  关闭其他 feature 的构建不受影响；但若依赖"默认无 russh"需显式关闭。

## [0.1.3] - 2026-10-18

### 新增

- **ctl 时钟偏移自动补偿**：`run` 前通过 SSH 逐台探测远端时钟（`date +%s.%N`，
  NTP 中点法估算偏移），按偏移为每台机器计算独立的 `--start-at`，压测机时钟
  不一致（实测 ±20s）也能真实同步启动；探测失败自动退化为 offset=0
- worker `--start-at` 支持小数时间戳（毫秒级精度：粗睡 + 最后 2ms 自旋）

### 修复

- **russh 传输无法分发 worker**：`upload` 未先向服务器请求 `sftp` 子系统，SFTP
  初始化包无人应答，经过 10 秒请求超时后 `distribute` 失败（`Timeout`）
- **russh 传输行缓冲**：stdout 的一行若跨多个 SSH 数据包到达会被拆成多段返回，
  导致 `run` 收集 `--json` 结果时解析失败；现在按换行缓存完整行
- **russh 密码认证失败不再中止**：服务端拒绝 password 方法时继续尝试公钥认证
- `run` 失败时的错误信息现在包含远端命令与 worker 实际输出，便于诊断

## [0.1.2] - 2026-10-18

### 新增

- **三层测试体系**（`tests/` 目录，workspace 成员 `cache-benchmark-tests`，约 100 个用例）
  - 单元测试（`benchmark/`、`bench-ctl/`）：cores 解析、占位符生成与非法输入、
    分布采样值域与偏斜性、SharedContext 并发发放精确性、ConnLimiter 阻塞/增长语义、
    AutoConnection 锁定状态机、连接串与拓扑解析、hosts 解析、二进制架构匹配、
    shell 转义、聚合数学（QPS 求和、直方图按桶合并重算分位数）
  - 功能测试（`functional/`）：黑盒运行二进制——CLI 帮助/版本/参数校验、
    `--json` 单行契约与字段齐全、`--start-at` 等待/立即启动、对内置 mock RESP
    服务器完成计数/pipeline/load/Lua 全流程、连接失败的错误信息
  - 集成测试（`integration/`，默认 `#[ignore]`，环境变量触发）：standalone
    数据写入验证与 sequence 不重不漏、集群拓扑一致性 + pipeline 无 CrossSlot 回归 +
    负载均匀性（各 master ops 偏差 <15%）、Lua 数据可验证、ctl 多机 distribute/run
    与单机失败不阻塞
  - 内置 mock RESP 服务器（`tests/src/mock_resp.rs`）：支持 +OK/PING/CLUSTER SLOTS
    可配置拓扑，测试无需外部 Redis
- **CI 增强**：push/PR 触发测试；test job 提供 redis service 容器并自动运行
  standalone/Lua 集成测试
- `cache-bench-ctl` 拆分 lib + bin，`report::aggregate` 提取为纯函数（可测试）

### 修复（相对 resp-benchmark 的行为，均由新增测试发现）

- **`{range N W}` 多值占位符参数粘连**：生成器把多 token 输出直接拼接到前一个词后
  （如 `LRANGE k {range 100 10}` 产生 `k9699` 而非 `k 96 99`），现在以空格分隔
- **SharedContext::fetch 并发超发**：原实现 `fetch_add` 后检查，多线程同时领取最后一个
  名额会超发；改为 CAS 循环，并发下发放总量精确等于 max_count
- **超短压测 QPS 显示为 0**：运行在一个统计周期（233ms）内结束时，最终 QPS 未计算即
  退出；现在退出前补算最终值
- **ConnLimiter::add_conn 无等待者时挂起**：连接目标提升后若无并发等待者注册连接，
  函数无限等待；现在 100ms 超时返回（等待者稍后自行检查目标值）
- **find_worker_binary 误匹配同名目录**：`dist/cache-benchmark-x86_64` 若是目录会被
  当作二进制选中；现在只匹配普通文件

### 变更

- crates 目录重命名：`crates/cache-benchmark` → `crates/benchmark`、
  `crates/cache-bench-ctl` → `crates/bench-ctl`（仅目录名，包名/二进制名/仓库名不变）
- 测试脚本并入测试体系：移除 `scripts/smoke-test.sh` 与
  `scripts/check-distribution.sh`（能力由 `tests/integration/` 覆盖），
  `scripts/` 只保留构建脚本
- zipfian 键域语义文档化：`rand_distr::Zipf` 采样区间为 [1, N] 含端点
  （继承自 resp-benchmark，与 uniform 的 [0, N) 不同）

## [0.1.1] - 2026-10-17

### 修复

- **CI aarch64 musl 交叉编译失败**（`mlua-sys` 报 `failed to find tool "aarch64-linux-musl-gcc"`）：
  用 `taiki-e/setup-cross-toolchain-action` 替代 `musl-tools`，自动安装各 target 的交叉
  C 工具链（aarch64-linux-musl-gcc 等，mlua vendored Lua 构建必需）并设置
  `CC_*` / `CARGO_TARGET_*_LINKER` 环境变量
- **CI macOS x86_64 交叉编译**：同一 action 覆盖 ARM runner 上构建 `x86_64-apple-darwin`
  的场景（clang `-arch x86_64`）
- **CI Windows 打包失败**：Package 步骤为 Windows 目标追加 `.exe` 后缀

## [0.1.0] - 2026-10-17

### 新增

- **cache-benchmark**：resp-benchmark 的纯 Rust 移植，移除 Python/PyO3 依赖
  - 占位符命令生成：`{key uniform|sequence|zipfian N}`、`{value N}`、`{rand N}`、`{range N W}`
  - Lua 脚本生成器：`--lua` / `--lua-file`，提供 `bench.key/value/rand` 与 `json.encode` API，同名生成器跨线程共享状态
  - 连接自动扩缩（`-c 0`）：QPS 增长 >30% 时连接翻倍，增长不足锁定并开始正式计时
  - QPS 限流（governor 令牌桶）、pipeline、短连接模式、CPU 核心绑定
  - 直方图分桶延迟统计（501 桶，10us~10s 变精度），支持 avg/p50/p95/p99
  - `--json`：输出单行 JSON 结果，含全量直方图分桶（供 ctl 聚合）
  - `--start-at <unix_ts>`：定时启动，供分布式同步压测
  - `--tls`：放开 resp-benchmark 中被禁用的 TLS 支持
  - 集群拓扑打印与 `cluster_nodes` JSON 字段（resp-benchmark 没有的增强）
- **cache-bench-ctl**：分布式压测管理工具
  - `hosts.toml` 服务器清单（address/port/user/arch/remote_dir/password），项目根目录提供带完整注释的模板
  - `distribute`：并发分发 worker 二进制，按远端 `uname -m` 自动匹配 `cache-benchmark-<arch>`（兼容旧的完整三元组命名）
  - `run`：同步启动（`--start-at` 时间戳）、每主机启动/完成时间戳与耗时、实时日志转发（`[user@host]` 前缀）、结果收敛
  - 聚合报告：总 QPS 求和 + 直方图按桶合并重算 p50/p95/p99（非均值再平均），单节点失败不阻塞
  - 双 SSH 通道：系统 `ssh`/`scp`（默认，复用 `~/.ssh/config`、密钥、跳板机）与纯 Rust `russh`（`--features russh`）
  - 密码认证：`hosts.toml` 配置 `password` 后，系统 ssh 通道自动通过 `sshpass -e` 登录（密码经 `SSHPASS` 环境变量传递，不出现在命令行参数中；缺 sshpass 时给出明确安装提示），russh 通道原生支持
- **cb-histogram**：独立 crate，worker 序列化与 ctl 合并共用同一分桶语义
- **scripts/build.sh**：musl 静态链接构建 + 打包，产物命名 `cache-benchmark-<arch>` / `cache-bench-ctl-<arch>`，压缩包内含 `hosts.toml` 模板；支持 `aarch64` 交叉编译与 `SKIP_TARBALL`
- **scripts/smoke-test.sh / check-distribution.sh**：冒烟测试与集群负载均匀性检查脚本
- CI：GitHub Actions 多平台构建（musl 静态链接 / macOS / Windows）与测试

### 修复（相对 resp-benchmark 的行为）

- **集群模式 + pipeline 报 `CrossSlot` 错误**：redis-rs 集群连接不允许一个 pipeline 批次跨 slot。改为批次内命令通过 clone 的连接句柄并发发送（`futures::future::join_all`），每条命令仍按 key 的 slot 路由到对应 master。高 RTT 环境实测 QPS 提升 19 倍（25 → 480），且负载分布保持均匀（三主节点偏差 <5%）

### 验证（真实环境）

- 3 主 3 从集群 + 1 standalone 环境完成冒烟测试：拓扑发现、灌数据、读压测、Lua、JSON 输出全部通过
- 三机分布式压测：同步启动，聚合 QPS 236,959，p99 7.4ms；各 master `total_commands_processed` 增量偏差 <5%

[Unreleased]: https://gitlab.gzky.com/dale.wang/cache-benchmark/compare/v0.1.4...HEAD
[0.1.4]: https://gitlab.gzky.com/dale.wang/cache-benchmark/compare/v0.1.3...v0.1.4
[0.1.3]: https://gitlab.gzky.com/dale.wang/cache-benchmark/compare/v0.1.2...v0.1.3
[0.1.2]: https://gitlab.gzky.com/dale.wang/cache-benchmark/compare/v0.1.1...v0.1.2
[0.1.1]: https://gitlab.gzky.com/dale.wang/cache-benchmark/compare/v0.1.0...v0.1.1
[0.1.0]: https://gitlab.gzky.com/dale.wang/cache-benchmark/releases/tag/v0.1.0
