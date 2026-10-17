# Changelog

本项目的所有显著变更都记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本管理遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

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

[Unreleased]: https://gitlab.gzky.com/dale.wang/cache-benchmark/compare/v0.1.1...HEAD
[0.1.1]: https://gitlab.gzky.com/dale.wang/cache-benchmark/compare/v0.1.0...v0.1.1
[0.1.0]: https://gitlab.gzky.com/dale.wang/cache-benchmark/releases/tag/v0.1.0
