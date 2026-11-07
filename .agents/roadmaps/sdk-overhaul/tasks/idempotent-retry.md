# 幂等写操作的可选重试

- Task key：idempotent-retry（Roadmap sdk-overhaul）
- 类型：feat
- 状态、依赖：以 plan.json 为准；分支 `feat/sdk-overhaul-idempotent-retry` 存在即表示进行中

## 目标与范围

**问题：** 现在只有标成 `ReplaySafety::ReadOnly` 的读操作能重试（`crates/fds/src/client.rs:172`、`crates/fds/src/retry.rs:89`），写操作一律只尝试一次，包括语义上幂等的写：

- 用内存 Bytes 作 body 的 `put_object`：同样的内容覆盖写入，结果幂等；
- `delete_object`：重复删除不改变最终状态；
- `set_object_metadata`、`set_object_acl`（Add 语义需要单独评估）、`set_bucket_acl`、`set_lifecycle_config`、`set_access_log`、`upload_part`（同一 part 号用同样的 bytes 重传）等。

另外，可重试的状态码只有 429、502、503、504（`client.rs:189`），500 不重试。这个选择偏保守，需要明确记录理由，或者改为可配置。

aws-sdk 默认对所有操作重试可重试的错误，前提是请求可重放。本 SDK 的安全默认值（写操作不重试）是有意设计的，应该保留，只提供显式开启的选项。

**要做：**

- 把内部的 `ReplaySafety` 扩展为 `Never` / `ReadOnly` / `Idempotent` 三档。逐个操作标注幂等性，在任务文档中列出表格并写明理由；不确定的标为 Never。copy、rename、restore、multipart complete、bulk delete、create_bucket、OAuth 交换一律 Never。
- `RetryPolicy` 增加开关（例如 `.retry_idempotent_writes(true)`），默认关闭；单个 builder 也可以显式覆盖（`.retry(RetryMode::...)`，名字由任务定）。
- body 只有可重放（Bytes）时才重试；流式和文件 body 永远不重试（沿用现有逻辑）。
- 评估 500 是否加入可重试集合，或做成可配置项；结论和理由写进 rustdoc 和决定记录（这是一个默认假设，用户评审时可以推翻）。
- `is_retryable()`（error-model 引入的）和重试循环使用同一套判定逻辑，避免两套规则。
- fds_cli 如果有重试相关的配置项，同步调整，行为不变。

**不做：** 不改默认行为（默认仍然只有读操作重试，并且整个重试功能默认关闭）。

## 对应验收标准

AC-9（重试部分），AC-8（本任务范围内）。

## 验收条件与验证方法

- [ ] 默认配置下，任何写操作遇到 503 都只发出一次请求（现有 `tests/retries.rs` 继续通过）。
- [ ] 开启幂等写重试后，Bytes body 的 put 和 delete 遇到 503 会按策略重试；流式 body 的 put、copy、rename、complete 永远不重试（逐项测试）。
- [ ] 幂等性标注表写进任务执行记录和 rustdoc。
- [ ] `is_retryable()` 和重试循环共用同一个判定函数。

## 执行记录

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/idempotent-retry`，用 `git log --grep` 查。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul idempotent-retry

推进 Roadmap sdk-overhaul 的任务 idempotent-retry：幂等写操作的可选重试。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/idempotent-retry.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：client.rs 的 ReplaySafety 扩展为 Idempotent 一档，在各操作 builder 上标注；retry.rs 的 RetryPolicy 增加幂等写开关（默认关），builder 支持单请求覆盖；可重试判定与 Error::is_retryable 共用；评估 500 是否可重试并记录结论
验证：tests/retries.rs 扩展：默认不重试写、开启后幂等写重试、非幂等写和流式 body 永不重试；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
