# 错误模型重构为 FdsError + ErrorKind

- Task key：error-model（Roadmap sdk-overhaul）
- 类型：refactor
- 状态、依赖：以 plan.json 为准；分支 `refactor/sdk-overhaul-error-model` 存在即表示进行中

## 目标与范围

**问题（来自审视的"错误处理"部分）：**

1. `FdsError::Invalid(String)`（`crates/fds/src/error.rs:46`）同时承载三种语义，调用方只能靠匹配字符串区分：
   - 调用方参数错误（bucket 为空、range 反转、metadata key 不合法）；
   - 服务端违反协议（`"multipart part receipt does not match request"`、`"listing has missing or cyclic nextMarker"`、`"Copy receipt destination does not match request"`）；
   - 凭证状态（`"storage token expired; explicitly exchange a new token"`）。
2. HTTP 404 / 403 / 429 / 5xx 没有语义映射，调用方只能写 `err.status() == Some(404)`。
3. "结果未知"有三种表达方式：Copy 用 `FdsError::CopyOutcomeUnknown`，multipart 完成阶段用 `Invalid("...outcome unknown")`（`crates/fds/src/multipart.rs:250`），批量删除用 `DeleteObjectsError.unconfirmed`。`crates/fds/src/object/delete.rs:110` 还靠匹配 `Invalid | Endpoint | Json | Http(builder)` 推断"请求是否已经发出"，非常脆弱。
4. `Debug` 隐藏太多：`impl Debug for FdsError`（`error.rs:139`）对 `Http`、`Json`、`Io`、`Invalid` 只打印变体名，`unwrap()` 之后完全无法排查。其实 `reqwest::Error::without_url()` 已经去掉了敏感 URL，可以安全打印 source 链。`ResponseError` 的 Display 只有 `"FDS response 403: UnexpectedStatus"`，连 service code 都没有。
5. 错误类型碎片化：`FdsError`、`DeleteObjectsError`（`object/delete.rs:38`）、`MultipartUploadError`（`multipart.rs:106`）、`ManagerError`（`manager/error.rs`）四种互不转换，用户函数返回 `fds::Result` 时没法对后三种直接用 `?`。
6. `ManagerError` 有 5 个从 Go 移植来、从未构造过的变体：`BucketOrObjectNotMatching`、`Md5NotMatching`、`RangeNotMatching`、`FileNotFound`、`TooManyUploadParts`；它的 `Io` / `Json` 变体和 FdsError 重复。
7. `FdsError::Http(reqwest::Error)` 把 reqwest 带进了公开 API，以后升级 reqwest 0.13 就会破坏 API。
8. 状态码用 `u16`（`ResponseError::status`、`GetObjectOutput::status`），而 `http` 已经是依赖，应该用 `http::StatusCode`。
9. `thiserror` 还是 1.x；`ResponseError` 直接内联在枚举里导致 `FdsError` 偏大，multipart 已经为此加了 `#[allow(clippy::result_large_err)]`（`multipart.rs:196`）。
10. `FdsError::Cancelled` 的文档写着"在实践中通常表现为 Http"，语义含糊；`FdsError::Endpoint` 不带任何信息。

**要做（参照 opendal 的 Error 模型）：**

- 公开一个 `fds::Error`（是否保留 `FdsError` 这个名字由任务决定，`fds::Result<T>` 不变），内部是一个不透明的结构体：`kind: ErrorKind`、`message: Cow<'static, str>`、可选的 `ResponseDetails`（`StatusCode`、service code、request id、有上限的诊断 body）、`dispatch: Dispatch`（NotSent / Sent / Unknown）、`source: Option<Box<dyn Error + Send + Sync>>`。
- `#[non_exhaustive] pub enum ErrorKind`，至少包括：`InvalidInput`、`ConfigInvalid`、`NotFound`、`PermissionDenied`、`Unauthenticated`、`CredentialsExpired`、`Conflict`、`PreconditionFailed`、`RateLimited`、`ServerError`、`ProtocolViolation`（服务端响应不合约定，包括 EmptyJson / InvalidJson / 回执不匹配）、`Timeout`、`Transport`、`Io`、`Cancelled`、`OutcomeUnknown`、`Unexpected`。
- 便捷方法：`kind()`、`status() -> Option<StatusCode>`、`service_code()`、`request_id()`、`is_not_found()`、`is_retryable()`（RateLimited、5xx 中可重试的、Timeout、未发出的 Transport）、`is_outcome_unknown()`。
- 由 `do_request` 统一维护 `dispatch`：构造请求和签名阶段失败是 NotSent，连接失败是 NotSent，收到响应后是 Sent，body 发送中途失败或超时是 Unknown。delete.rs 和 copy 改为读取这个字段，不再匹配变体。
- 状态码映射：404 → NotFound，401 → Unauthenticated，403 → PermissionDenied，409 → Conflict，412 → PreconditionFailed，429 → RateLimited，5xx → ServerError，其他 4xx → Unexpected（保留 status）。
- 把 `ManagerError`、`MultipartUploadError`（cleanup 错误作为附加上下文或 source 保留）、`DeleteObjectsError` 合并进统一错误。批量删除的部分失败这一项**只调整错误类型**：返回类型改为 `fds::Result`，局部失败暂时用 `OutcomeUnknown` / 带结构化上下文的错误表示；"部分失败作为 `Ok(output)` 返回"属于 API 形态变化，交给 fluent-listing-delete。为了避免两次改动打架，本任务可以先把 `DeleteObjectsOutput` 扩展为同时承载 deleted、failures、unconfirmed，留给后续任务调整返回形态。
- 传输错误不透明化：不再公开 `reqwest::Error`，通过 kind（Timeout / Transport）加 source 链暴露信息。
- `Debug` 打印 kind、message、status、service code 和 source 链；脱敏要求不变：不输出凭证、签名、带 token 的 URL，也不输出原始响应 body。Display 包含 status 和 service code（AccessKey 模式下）。
- 升级 `thiserror` 到 2（或者手写 impl，不依赖 thiserror）；保证 `size_of::<fds::Error>()` 不大，并去掉 `result_large_err` 的 allow。
- 全 crate 所有 `FdsError::invalid(...)` 调用点按语义改为正确的 kind，`fds_cli` 同步适配（CLI 的错误输出文案可以变，退出码不变）。

**不做：** 不改操作的调用形态（fluent-* 负责）；不改返回值模型（typed-responses 负责）。

## 对应验收标准

AC-3（全部），AC-7、AC-8（本任务范围内），并为 AC-6 中 `xxx_with_response` 的去重扫清障碍。

## 验收条件与验证方法

- [ ] 公开 API 中只有一个错误类型，`ManagerError`、`DeleteObjectsError`、`MultipartUploadError` 都已删除，或改成统一错误的上下文。
- [ ] 测试覆盖：404、401、403、409、412、429、500、503 分别映射到正确的 kind；EmptyJson、InvalidJson、回执不匹配映射到 ProtocolViolation；token 过期映射到 CredentialsExpired；参数错误映射到 InvalidInput。
- [ ] `dispatch` 测试覆盖：本地校验失败是 NotSent，连接被拒是 NotSent，收到响应是 Sent；delete 的 not_submitted / unconfirmed 判定改为基于 dispatch。
- [ ] `format!("{:?}", err)` 包含 kind 和 source 链；现有脱敏测试继续通过（`client.rs` 中的 tracing 测试、`tests/oauth_discovery.rs` 中的诊断测试等），并为 Debug 输出新增"不含 AK/SK、token、签名"的断言。
- [ ] `reqwest` 类型不出现在任何公开签名中（可以通过 `cargo doc` 输出或 grep `pub` 签名确认）。
- [ ] clippy 不再需要 `result_large_err`。

## 执行记录

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/error-model`，用 `git log --grep` 查。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul error-model

推进 Roadmap sdk-overhaul 的任务 error-model：错误模型重构为 FdsError + ErrorKind。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/error-model.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：重写 crates/fds/src/error.rs（Error、ErrorKind、Dispatch、ResponseDetails）；client.rs 的 do_request/check_response_status/do_json_limited 负责状态码映射和 dispatch；全 crate 替换 FdsError::invalid 调用点；合并 manager/error.rs、multipart.rs 的 MultipartUploadError、object/delete.rs 的 DeleteObjectsError；delete.rs/basic.rs(copy)/multipart.rs 的结果未知改用 OutcomeUnknown + dispatch；fds_cli 适配
验证：新增状态码映射、dispatch、Debug 脱敏测试；确认公开签名中没有 reqwest 类型；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
