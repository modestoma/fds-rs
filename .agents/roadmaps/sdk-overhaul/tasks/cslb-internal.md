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

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/cslb-internal`，用 `git log --grep` 查。

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
