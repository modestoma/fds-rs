# cslb 独立工具改为内部并删除死代码

- Task key：cslb-internal（Roadmap sdk-overhaul）
- 类型：refactor
- 状态、依赖：以 plan.json 为准；分支 `refactor/sdk-overhaul-cslb-internal` 存在即表示进行中

## 目标与范围

**背景（public-surface 执行中发现，2026-11-07 用户决定"改为内部并删除"）：** `cslb` feature 下，除 DNS resolver（public-surface 已改为 `pub(crate)`）外，`fds::cslb` 仍公开一批从 Go 移植来的独立负载均衡工具：`LoadBalancer`、`Service`（及 `StaticService`、`RrDnsService`）、`Strategy`、`HashedStrategy`、`Group`、`Metrics`、`CslbError` 等。`Client` 只通过内部 resolver 使用其中一小部分，其余对用户没有入口、也没有被 SDK 自身使用。

**要做：**

1. `pub mod cslb` 改为私有模块（`cslb` feature 仍然保留，作用只是给 Client 启用 CSLB DNS 解析）；模块内各项降为 `pub(crate)` 或私有。
2. 以 resolver 的实际调用链为准，删除不再可达的代码（编译器 dead_code 告警即依据）；只保留 resolver 真正用到的部分。逐项在执行记录中列出删除了什么、保留了什么及原因。
3. 清理随之不再需要的依赖（例如 cslb 专属的 `dashmap`、`arc-swap` 若不再使用；`thiserror` 若只剩 `CslbError` 使用也一并评估），同步 Cargo.toml 的 `cslb` feature 与 Cargo.lock。
4. 删除或改写 tests、examples 中直接使用这些公开工具的部分；保留并通过 resolver / Client 启用 CSLB 的行为测试。
5. README 与 docs 中"独立工具位于 `fds::cslb`"等描述删除，改为说明 `cslb` feature 只影响 Client 的 DNS 解析。
6. `ClientBuilder::cslb_max_nodes` 等与 Client 相关的配置保持可用。

**不做：** 不改变启用 `cslb` feature 时 Client 的解析行为。

## 对应验收标准

AC-5（公开面收缩：`fds::cslb` 不再公开，没有未使用的类型）、AC-8（本任务范围）。

## 验收条件与验证方法

- [ ] `cargo doc --all-features` 的公开项中不再出现 `cslb` 模块及其类型。
- [ ] `--no-default-features --features cslb` 与 `--all-features` 下编译无 dead_code 告警、测试通过；启用 cslb 时 Client 的解析行为测试保持通过。
- [ ] 不再使用的依赖已从 Cargo.toml / Cargo.lock 移除（列出清单）。
- [ ] README / docs 不再提及公开的 cslb 工具。

## 执行记录

2026-11-07，分支 `refactor/sdk-overhaul-cslb-internal`（worktree `.worktrees/cslb-internal`），基于 main `8e4e6fc`。

**1. 模块改为私有。** `lib.rs` 中 `#[cfg(feature = "cslb")] pub mod cslb;` 改为私有 `mod cslb;`（移到私有实现模块列表，仍受 `cslb` feature 控制）。模块内只有 `CslbResolver` 是 `pub(crate)`（供 `client.rs` 安装）；`LoadBalancer` 及常量是 `cslb` 模块私有，`Group`、`RrDnsService`、`RoundRobinStrategy` 为 `pub(super)`。"Module layout" 文档删除 `cslb` 子项，改为一句：`cslb` feature 不增加模块，只让 `Client` 通过内部 CSLB DNS resolver 解析主机，配置入口是 `ClientBuilder::cslb_max_nodes`。

**2. 删除依据。** 先只把 `pub mod` 改为 `mod`，`cargo check -p fds --no-default-features --features cslb` 报 19 条 dead_code / unused 告警：`NodeFailedCallback`、`StaticService`(+`new`)、`HashFunc`、`HashedStrategy`(+`new`)、`Strategy::next_for`、`Service::node_failed_callback`、`HEALTHY_NODE_MUST_ALL`、`CslbError` 的 `HashedNeedsInput`/`SampleNotEnough`/`InvalidRatio`/`Other`、`LoadBalancerOption::min_healthy_node_ratio`、`LbInner::option`、`LbInner::node_failed`、`LoadBalancer::{next_for, node_failed, refresh}`、`Group::{exile, original_count, current_count}`、`Metrics::min_sample_size`、`Metrics::{node_failed_inc, reset_node, get_node_failed_ratio}`。实际调用链：`Client::new` → `CslbResolver::new(conf.cslb_max_nodes, true, true)` → `Resolve::resolve` → 每主机一个 `LoadBalancer::new`（RrDnsService + RoundRobinStrategy + TTL 600 秒）→ `next()` 与 `current_nodes()`。没有任何调用者上报节点失败，所以 exile / 失败率整条链都不可达。

**删除：**
- 告警列出的全部项（见上）。
- `metrics.rs` 整个文件与 `LbInner::metrics`：删掉 `node_failed` 后，resolver 路径上只剩 `node_inc`（选中时 +1）和 `reset_all_nodes`（刷新时清空）两个写操作，计数从来没有被读取，对选址没有可观察的影响。编译器看不出"只写不读"，这一项是按语义判断删除的。
- 随之不可达的：`LoadBalancerOption`（删掉失败率字段后只剩节点上限和 TTL，改为 `LoadBalancer::new` 的两个参数）及 `Default`、`TTL_UNLIMITED` 判断（resolver 恒传 600 秒，后台刷新任务总会启动，与原行为一致）、`DEFAULT_MIN_SAMPLE_SIZE`、`HEALTHY_NODE_ANY`、`NODE_FAILED_UNLIMITED`、`NODE_FAILED_ANY`、`HEALTHY_NODE_MUST_ALL`。
- 只有单一实现的抽象：`Service` trait（BoxFuture）、`Strategy` trait、`Node` trait（节点实际只有 `IpAddr`），改为具体类型 `RrDnsService`、`RoundRobinStrategy` 和 `IpAddr`。因此 resolver 里 `to_string().parse::<IpAddr>()` 往返转换也去掉（`IpAddr` 转字符串再解析是无损的，原来的 `if let Ok` 分支恒成立）。
- `CslbError`：只剩 `EmptyNodeList` 一个变体，且只用于"空列表时刷新后重试"的判断，改为 `Option<IpAddr>`。
- `RrDnsService` 的 `ArcSwap` 节点缓存与多主机列表：节点只在 `LbInner::refresh` 持锁期间"刷新后立即读取"，改为 `resolve()` 直接返回；每个 balancer 只有一个主机。
- 测试：`StaticService`、`HashedStrategy`、`Metrics` 的单元测试，以及基于 `StaticService` 的 LoadBalancer 测试。

**保留（及原因）：**
- `CslbResolver`（`pub(crate)`）：`Client` 安装的 reqwest resolver；每主机一个 balancer，首选为轮询结果，其余节点作为备选，无节点时回退系统 DNS——逻辑不变。
- `CslbResolver::new` 的 `ipv4`/`ipv6` 参数和"0 视为不限"分支：仍被调用；`cslb_max_nodes` 的校验保证不会传 0，但保留防御分支不影响行为。
- `LoadBalancer`（`new`/`next`/`current_nodes`）、`LbInner::refresh` 的 `tokio::sync::Mutex` 串行化、Weak 引用的 TTL 刷新任务（balancer 释放后停止）、"空列表刷新一次再重试"。
- `Group`：去重并截断到 `max_node_count`。存储由 `DashMap<String, Arc<dyn Node>>` 改为 `RwLock<HashSet<IpAddr>>`，同一个 balancer 内保留同一个 hasher，更新仍是"插入新节点再删去不在列表中的"。两者都是哈希序（未指定顺序），截断时取到的都是任意子集，语义一致；`set` 现在原子替换，并发读不会看到半更新的状态。
- `RoundRobinStrategy`：索引从 `u64::MAX` 起步、首次取下标 0、空列表不推进索引。节点列表由 `ArcSwap<Vec<_>>` 改为 `RwLock<Vec<IpAddr>>`，语义相同。resolver 每次建连才调用一次，锁竞争可以忽略。
- 常量 `NODE_COUNT_UNLIMITED`、`DNS_CACHE_TTL`。
- 锁中毒统一用 `unwrap_or_else(PoisonError::into_inner)`（仓库现有写法，见 `manager/download.rs`）。

**3. 依赖。** 从 `crates/fds/Cargo.toml` 移除：`arc-swap`（可选，仅 cslb 用）、`dashmap` 6（可选，仅 cslb 用）、`thiserror`（非可选，但全 crate 只有 `CslbError` 使用）。`cslb` feature 现为 `["tokio/sync", "tokio/net"]`（`tokio::sync::Mutex`、`lookup_host` 仍在用）。Cargo.lock 删除 `arc-swap 1.9.2`、`dashmap 6.2.1` 两个包；`dashmap 5.5.3` 仍由 `governor`（manager）传递引入，`thiserror 2.0.18` 仍由 `quinn`/`quinn-proto` 传递引入，留在 lock 中，但不再是 fds 的直接依赖。没有新增依赖。`futures-util` 在其他模块仍在使用。

**4. 测试与示例。** examples 与 fds_cli 没有使用 `fds::cslb` 的公开类型，无需改动（fds_cli 只用 `cslb_max_nodes`）。新增或改写的测试（都会实际解析 `localhost`，不使用 mock 的 DNS）：
- `cslb::resolver::tests::returns_distinct_nodes_with_rotating_primary`：返回的地址都是回环地址、端口为 0、互不重复；两次解析的节点集合相同；节点多于 1 个时首选地址会轮换。
- `cslb::resolver::tests::node_limit_and_family_filter_apply`：节点上限为 1 时只返回 1 个地址；只开 IPv4 时只返回 IPv4 地址。
- `cslb::tests::load_balancer_rotates_over_resolved_nodes`：轮询依次访问每个节点，然后回绕。
- `cslb::group::tests::set_deduplicates_replaces_and_caps`，以及保留并改写的 `cslb::strategy::tests::{round_robin_starts_at_index_zero, round_robin_empty_returns_none}`。
- `tests/client_builder.rs::cslb_client_resolves_hostname_endpoints`（`#[cfg(feature = "cslb")]`）：端到端，Client 以 `http://localhost:<port>` 连接只监听 127.0.0.1 的 mockito；IP 字面量不走 DNS，主机名才会经过 CSLB resolver。这条也覆盖了 `::1` 被拒后回退到下一个节点。连续运行 8 次都通过。原有的 `cslb_max_nodes` 校验与 getter 测试保留。

**5. 文档。** README 的 feature 表与"CSLB 负载均衡"一节删除"独立工具位于 `fds::cslb`"，改为说明 `cslb` feature 只影响 Client 的 DNS 解析，唯一配置入口是 `cslb_max_nodes`；`docs/runtime.md` 删除"独立 LoadBalancer 的失败统计"一句；`docs/open_api/README.md` 的 CSLB 行改写；`docs/migration.md` 说明原 `fds::cslb` 及其类型已删除、没有替代 API。`docs/cli.md` 的 `--cslb-max-nodes` 不变。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/cslb-internal`，用 `git log --grep` 查。

- 状态：实现与验证完成，分支上是 WIP 提交，等待协调者压成一笔后合并。
- `bash scripts/check.sh`（2026-11-07，本 worktree，退出码 0）：fmt；clippy `--workspace --all-features --all-targets -D warnings`；四种 feature 组合的 `cargo test -p fds -- --test-threads=1` 和 `cargo check --all-targets`；fds_cli test 和 check；`cargo doc -D warnings`；`fds_cli --help`；MSRV 1.86.0 下三种组合的 test/check 及 fds_cli check；`check_coverage.py`（35/35 YAML、33/33 操作）；`check_docs.py`（114 个本地链接，retired 扫描通过）；`git diff --check`。日志中有 142 个 `test result: ok`，没有失败，也没有 warning / error 行。7 个 cslb 测试在 stable `--all-features`、stable `--no-default-features --features cslb`、MSRV `--all-features` 下各跑一次，共 3 轮，全部通过。
- 其他实际运行：
  - `cargo check --locked -p fds --all-targets`，分别用 `--no-default-features --features cslb` 和 `--all-features`：无 warning。
  - `cargo clippy --locked -p fds --no-default-features --features cslb --all-targets -- -D warnings`：通过。
  - `RUSTDOCFLAGS='-D warnings' cargo doc --locked -p fds --all-features --no-deps`：`target/doc/fds/` 下没有 `cslb` 目录；`all.html`、`sidebar-items.js` 中没有 cslb 条目；`LoadBalancer`、`CslbResolver`、`HashedStrategy`、`StaticService`、`CslbError` 在文档中零出现。`cslb` 只出现在 crate 首页的 feature 说明和 `cslb_max_nodes` 中。
  - `cslb_client_resolves_hostname_endpoints` 连续运行 8 次，均通过。
  - 改动前的 dead_code 告警清单（19 条，见执行记录第 2 项）。
- 未验证：`tests/live_acceptance.rs` 的 2 个 live 测试（需要 `FDS_LIVE=1` 和真实凭证）；resolver 在真实 FDS 域名、多 A 记录（多于 `cslb_max_nodes`）下的表现，这类场景只能用单元测试和本机 `localhost` 覆盖。
- 依赖变化：移除 `arc-swap`、`dashmap`（6.x）、`thiserror`，Cargo.lock 删除 `arc-swap 1.9.2`、`dashmap 6.2.1`。
- 待用户决定：无。
- 协调者复验：审查 diff（cslb 改为私有模块，以 resolver 调用链为准删除不可达代码，移除 arc-swap / dashmap / thiserror 直接依赖），在任务 worktree 中完整运行 `bash scripts/check.sh`，退出码 0；之后 rebase 到只改了 AGENTS.md 的最新 main。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul cslb-internal

推进 Roadmap sdk-overhaul 的任务 cslb-internal：cslb 独立工具改为内部并删除死代码。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/cslb-internal.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：lib.rs 中 pub mod cslb 改为私有；以内部 CslbResolver 的调用链为准删除 LoadBalancer/Strategy/Group/Metrics 等不可达代码；清理随之无用的依赖（dashmap、arc-swap、thiserror 等，按实际评估）；改写使用这些工具的 tests/examples；README/docs 改为 cslb feature 只影响 Client 的 DNS 解析
验证：cargo doc 公开项检查；cslb feature 组合下无 dead_code 告警且测试通过；依赖清单对比；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
