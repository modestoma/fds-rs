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

2026-11-08，分支 `feat/sdk-overhaul-idempotent-retry`（worktree `.worktrees/idempotent-retry`），基于 main `96be5e2`。任务文档写于迁移之前，按当前代码核对：`ReplaySafety`（Never/ReadOnly）在 `retry.rs`，重试循环在 `client.rs` 的 `do_request_redacted`，每次尝试前 `prepare_attempt` 取凭证并重新签名（本任务保留）；`Error::is_retryable()` 在 `error.rs`。

**1. 公开 API。**

- `RetryPolicy::retry_idempotent_writes(self) -> Self`（无参开关，默认关）和访问器 `retries_idempotent_writes() -> bool`。`max_retries == 0`（含 `RetryPolicy::default()`）时开关不起作用。
- `fds::RetryMode`（根路径，与 `RetryPolicy` 并列；`#[non_exhaustive]`）：`Policy`（默认，跟随 policy 开关）、`Enabled`（本请求按 policy 的次数和等待重试，即使 policy 没开幂等写）、`Disabled`（本请求只发一次）。
- 幂等写的 builder 增加 `.retry(RetryMode)`（`macros.rs` 的 `retry_option!`）：`PutObject`、`UploadPart`、`DeleteObject`、`SetObjectMetadata`、`SetLifecycleConfig`、`SetAccessLog`。永不重试的操作没有这个方法（类型层面保证，而不是运行时忽略）；读取也没有（读取的行为与改前相同，本任务不扩展读取的单请求开关）。
- lib.rs 的"Module layout"根导出清单加入 `RetryMode`，"Operation builders"约定加入 `retry(RetryMode)` 一条。

**2. 内部实现。** `ReplaySafety` 扩展为 `Never / ReadOnly / Idempotent`，`ReplaySafety::permits(mode, policy)` 决定是否允许重复：Never 永远 false；ReadOnly 永远 true（仍需 `max_retries > 0`）；Idempotent 按 `RetryMode`，`Policy` 时取 policy 开关。`ClientRequest` 增加 `retry_mode` 字段和 `.idempotent(mode)`。body 可重放（无 body 或 Bytes）是独立的第二个条件，沿用原逻辑：流和文件 body 的请求永远不重试，即使 `RetryMode::Enabled`。

**3. 共用判定函数。** 新增 `retry::is_transient(kind, status, dispatch)`，是唯一的规则：RateLimited、Timeout → true；ServerError 仅 502/503/504；Transport 在 `dispatch != Sent` 时 true；其余 false。`Error::is_retryable()` 直接调用它；重试循环对被拒绝的响应调用 `is_transient(ErrorKind::from_status(s), Some(s), Dispatch::Sent)`，对发送失败先 `SendFailure::into_error` 再调用 `error.is_retryable()`，因此两边不可能分叉。

为了统一，`is_retryable()` 有一处变化：**Transport 且 `Dispatch::Unknown`**（已开始发送、收到响应前连接中断）由 false 改为 true。原因：改前重试循环对读取会重试这类失败（`tests/retries.rs` 的断连用例依赖它），而 `is_retryable()` 返回 false，两套规则不一致；若改为收窄循环，会改变读取的默认重试行为（任务明确禁止）。改后与 `Timeout + Unknown` 一直返回 true 的处理一致，rustdoc 写明 true 不代表非幂等写在非 NotSent 时可以安全重复。读响应 body 时的 Transport 失败（`Dispatch::Sent`）仍为 false。`error.rs` 的单元测试相应更新。行为对比：reqwest 的发送错误只有 Request/Builder 两类（发送路径不会产生 Kind::Body），Builder 映射为 InvalidInput（不重试），其余与改前的循环条件逐一对应，读取的重试行为不变。

**4. 幂等性标注表。** 不确定的一律 Never。

| 操作 | 标注 | 理由 |
| --- | --- | --- |
| get_object、head_object、object_exists、get_object_metadata、get_object_basic_info、get_object_acl、list_objects、bucket_exists、get_bucket_info、list_buckets、list_authorized_buckets、get_bucket_acl、get_lifecycle_config、get_access_log、get_endpoint、list_app_buckets、download_file 的 HEAD、delete_prefix 的列举 | ReadOnly | 只读，未改动 |
| download_file 的分片 Range GET | Never | 未改动：托管下载自行断点续传，不属于本任务范围 |
| put_object（Bytes body） | Idempotent | 同样内容覆盖写入，最终状态相同。可观察差异：版本化 bucket 可能多一个版本；两次尝试之间他人的写入会被覆盖 |
| put_object（文件/流 body） | 不重试 | body 不可重放 |
| upload_part（Bytes body） | Idempotent | 同一 part 号用同样的 bytes 重传，替换该 part，回执相同；托管上传 `upload_file` 的分片读入内存，因此开关打开时同样重试 |
| delete_object | Idempotent | 重复删除不改变最终状态；前一次已删除时重试可能返回 NotFound（rustdoc 写明） |
| set_object_metadata | Idempotent | 整体替换（不是 patch），JSON body |
| set_lifecycle_config | Idempotent | 整体替换整个配置 |
| set_access_log | Idempotent | 整体替换访问日志配置 |
| set_lifecycle_rule | Never | 服务端对同 ID 规则是替换还是追加、无 ID 时是否新增规则，文档没有写明 |
| set_object_acl（Add 与 Delete）、set_object_public、set_bucket_acl | Never | Add 是"添加授权"语义：`docs/open_api/put-object-acl.yaml` 只说缺省为添加，没有说明重复添加同一 grant 是合并还是追加重复项；Delete 删除不存在的 grant 是否报错也未说明；并发下 Add→他人 Delete→重试 Add 会恢复已撤销的授权。依据不足，按规则标 Never |
| copy_object | Never | 任务规定；发送后的失败本就是 OutcomeUnknown |
| rename_object | Never | 任务规定；首次成功后源对象已不存在，重试结果不同 |
| restore_object | Never | 任务规定 |
| create_multipart_upload | Never | 每次创建新的 upload ID，重复会遗留未完成的上传 |
| complete_multipart_upload | Never | 任务规定；发送后失败为 OutcomeUnknown |
| abort_multipart_upload | Never | 状态上幂等，但首次成功后重试很可能返回 NoSuchUpload，托管上传会把它误报为清理失败；托管上传文档承诺"abort 只尝试一次"；收益小 |
| delete_objects、delete_prefix（删除步骤） | Never | 任务规定；发送后失败为 OutcomeUnknown 并带 unconfirmed 列表 |
| create_bucket | Never | 任务规定；首次成功后重试返回冲突 |
| delete_bucket | Never | 低频且破坏性大，重试收益小；首次成功后重试返回 NotFound |
| migrate_bucket | Never | 首次成功后归属已变，重试可能因权限变化失败，语义不明 |
| prefetch_object、refresh_object | Never | CDN 异步任务，可能计入配额；原文档承诺不自动重试 |
| exchange_storage_token（OAuth 交换） | Never | 任务规定 |

**5. HTTP 500 的结论（默认假设，用户可推翻）。** 不加入可重试集合，也不做成配置项。理由：500 是服务端的通用失败，常常是该请求本身触发的确定性错误，重复同一请求多半仍失败，只会耗尽重试预算、增加延迟，并在服务故障期间放大负载；502/503/504 则明确表示网关或可用性问题。aws-sdk 的标准策略会重试 500，这是与之不同的有意选择。需要时调用者可按 `Error::status()` 自行重试。结论写在 `RetryPolicy` 的 rustdoc、`is_retryable()` 的 rustdoc、README 和 `docs/runtime.md`。如果用户希望与 aws-sdk 对齐，可以后续加一个默认关闭的 `RetryPolicy::retry_internal_errors()`，并让 `is_transient` 接收该参数（`is_retryable()` 固定传 false）。

**6. fds_cli（AC-7）。** CLI 只有 `--retries`、`--retry-initial-ms`、`--retry-max-ms`，构造的 `RetryPolicy` 不开启幂等写，帮助文本"Read retries only ... Mutations are never retried"依然成立。未改动 CLI 代码，行为不变。

**7. 文档。** rustdoc：`retry.rs` 模块与 `RetryPolicy`（三类操作清单、500 结论、示例）、`RetryMode`（示例）、`Error::is_retryable`、`ClientBuilder::retry_policy`、`UploadBody`、`MultipartUpload::upload`，以及六个幂等写 builder 的 `send`；`set_lifecycle_rule` 的 `send` 写明不重试的原因。README（配置表、上传、错误分类、"重试和整体期限"）、`docs/runtime.md`（重试与超时）、`docs/migration.md`（重试行）。

**8. 测试。** `tests/retries.rs` 新增 5 个用例：
- `writes_are_single_attempt_unless_idempotent_writes_are_enabled`：只有读重试的 policy 下，6 个幂等写和 18 个非幂等/流式写遇到 503 都只请求 1 次；开启 `retry_idempotent_writes()` 后 6 个幂等写各请求 3 次（max_retries=2），流式 put、流式 upload_part（即使 `RetryMode::Enabled`）、copy、rename、restore、multipart init/complete/abort、delete_objects、两种 ACL、set_object_public、set_lifecycle_rule、create/delete/migrate bucket、prefetch、refresh 逐项各 1 次；
- `per_request_mode_overrides_the_policy`：未开开关 + `Enabled` → 3 次；开了开关 + `Disabled` → 1 次；Client 没有 RetryPolicy + `Enabled` → 1 次；
- `delete_prefix_retries_only_its_listing`：列举遇 503 重试，删除步骤遇 503 只 1 次；
- `idempotent_put_replays_the_same_bytes_and_uses_the_shared_rule`：两次 503 后成功，三次 body 相同；耗尽的 503 错误 `is_retryable()` 为 true，500 只请求 1 次且 `is_retryable()` 为 false；
- `idempotent_write_is_retried_after_a_dropped_connection`：delete_object 首次发送后断连（Transport + Unknown）被重试并成功。

`retry.rs` 单元测试新增分类×模式×开关的矩阵和 `is_transient` 规则表；`error.rs` 的 `retryability` 更新。原有 `writes_streams_and_success_decode_errors_are_never_replayed`、`client.rs` 的 `retry_safety_tests` 不变并通过。变异检查：把 Idempotent 的 `Policy` 分支改成 true、Never 改成 true 后，3 个用例失败；已恢复。wire 快照 fixture 与签名 golden 未改动。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/idempotent-retry`，用 `git log --grep` 查。

- 状态：实现与验证完成，分支上一笔带 Roadmap 末行的提交，未合并。
- `bash scripts/check.sh`（2026-11-08，本 worktree，最后一次代码改动之后运行，退出码 0）：fmt；clippy `-D warnings`；四种 feature 组合的 fds test/check；fds_cli test/check；`cargo doc -D warnings`；`fds_cli --help`；MSRV 1.86.0 三种组合的 test/check 及 fds_cli check；`check_coverage.py`（35/35 YAML、33/33 操作）；`check_docs.py`（117 个本地链接）；`git diff --check`。日志中 170 个 `test result: ok`，没有失败；`tests/retries.rs`（8 个用例）在 stable 4 种与 MSRV 3 种组合中都运行并通过。之后只改了本任务文档，另行运行 `git diff --check` 通过。
- wire 快照 fixture 与签名 golden：未改动，全部通过。
- 共享文件（合并时注意）：`crates/fds/src/client.rs`（重试循环、`ClientRequest.retry_mode`）、`retry.rs`、`error.rs`（`is_retryable`）、`lib.rs`、`macros.rs`、`config.rs`、`upload.rs`、`multipart.rs`、`object/{put,delete,metadata,multipart}.rs`、`bucket/config.rs`；`tests/retries.rs`；`README.md`、`docs/runtime.md`、`docs/migration.md`。fds_cli 未改动。
- 未验证：`tests/live_acceptance.rs`（需要真实凭证）；真实 FDS 上重复 put/delete/upload_part/set_metadata 的服务端行为（例如重复删除返回 404、版本化 bucket 的版本数）没有 live 证据，只有 mock 测试。
- 待用户决定：(1) `Error::is_retryable()` 对 Transport + `Dispatch::Unknown` 由 false 改为 true（统一判定函数的必然结果，否则要收窄读取的默认重试行为）；(2) HTTP 500 维持不重试、不可配置（默认假设）；(3) ACL 修改、`set_lifecycle_rule`、`abort_multipart_upload`、`delete_bucket` 按"拿不准标 Never"处理，若服务端语义确认幂等可再放开。
- 协调者复验：确认分支没有改动 plan.json 和 roadmap.md，wire fixture 全部不变。rebase 到最新 main 后完整运行 `bash scripts/check.sh`，退出码 0。回报中的三项待决事项，用户已确认维持当前实现：is_retryable 对已发出后断连返回 true；500 不重试；保守标注维持 Never。

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
