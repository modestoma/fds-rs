# 恢复基线检查全绿（clippy 新 lint）

- Task key：baseline-green（Roadmap sdk-overhaul）
- 类型：chore
- 状态、依赖：以 plan.json 为准；分支 `chore/sdk-overhaul-baseline-green` 存在即表示进行中

## 目标与范围

**问题。** 2026-11-07 在初始导入的提交上实测（本机 stable 1.99.1）：

1. `cargo clippy --locked --workspace --all-features --all-targets -- -D warnings` 失败。原因是 `crates/fds/src/cslb/service.rs:17` 的 `#[async_trait] pub trait Service` 展开后，触发了新版 clippy 的 `double_must_use` lint（"this function has a `#[must_use]` attribute with no message, but returns a type already considered as `#[must_use]`"）。代码在 1.86 下没有问题，是工具链升级带来的新 lint。
2. 默认 feature 和全部 feature 下的 `cargo check --workspace --all-targets` 都能通过。
3. `cargo test -p fds --all-features` 中有 3 个用例失败：`client::response_security_tests::request_trace_omits_tokens_keys_and_signatures`、`client::retry_safety_tests::semantic_default_deny_and_body_replayability_are_independent`、`manager::download::tests::panic_does_not_publish`。失败原因都是 mockito 在沙箱中绑定本地端口被拒（`Operation not permitted`），属于 Agent 运行环境的限制，不是代码问题。tests/ 目录下的集成测试大多同样依赖 mockito。

如果不先修好基线，后续每个任务的 `-D warnings` 检查都会因为这个和本任务无关的 lint 而失败，没法判断改动本身是否干净。

**要做：**

- 消除 cslb `Service` trait 上的 `double_must_use`。优先考虑去掉 `async_trait`：MSRV 1.86 已经支持 trait 中的 `async fn`，但如果这个 trait 需要以 `dyn Service` 的形式使用，原生 async fn 不满足对象安全，就要评估改为返回 `Pin<Box<dyn Future>>`，或者保留 async_trait 并在最小范围内 `#[allow(clippy::double_must_use)]`，同时注明原因。方案由任务决定并记录。
- 确认 1.86 和 stable 下 clippy 都通过（1.86 工具链未安装时如实标为未验证）。
- 在沙箱放开本地端口绑定（`sandbox.network.allowLocalBinding`）之后，完整运行一次 `scripts/check.sh`，确认基线全绿；把结果写进执行记录，作为后续任务的对照。

**不做：** 不修改 cslb 的行为和公开 API 以外的部分。

## 对应验收标准

AC-8（基线部分）：为后续所有任务提供可信的「全绿」起点。

## 验收条件与验证方法

- [ ] stable 工具链下 `cargo clippy --locked --workspace --all-features --all-targets -- -D warnings` 通过。
- [ ] cslb feature 下的测试通过（`cargo test --locked -p fds --no-default-features --features cslb -- --test-threads=1`）。
- [ ] 放开本地端口绑定后完整运行 `scripts/check.sh`，结果记录在本任务的执行记录中；1.86 工具链未安装时，MSRV 部分标为未验证。

## 执行记录

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/baseline-green`，用 `git log --grep` 查。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul baseline-green

推进 Roadmap sdk-overhaul 的任务 baseline-green：恢复基线检查全绿（clippy 新 lint）。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/baseline-green.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：处理 crates/fds/src/cslb/service.rs 的 async_trait 在新版 clippy 下触发的 double_must_use：优先改为原生 async fn 或返回 BoxFuture；如果 dyn 兼容性不允许，就在最小范围内 allow 并注明原因；然后完整运行 check.sh 记录基线结果
验证：stable 下 clippy -D warnings 通过；cslb feature 测试通过；完整运行一次 check.sh 作为基线；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
