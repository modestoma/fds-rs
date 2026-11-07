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

2026-11-07，分支 `refactor/sdk-overhaul-error-model`（基于 main `413341b`）。

**类型名。** 采用 `fds::Error`（参照 `std::io::Error` / opendal），不保留 `FdsError` 别名（决定记录：不考虑兼容）；`fds::Result<T>` 保留。理由写在 `crates/fds/src/error.rs` 模块文档里。

**公开 API。**
- `Error`：不透明结构体，内部是 `Box<Inner>`，`size_of::<Error>() == size_of::<usize>()`（有单元测试和集成测试断言）。`Inner` 包含 kind、`Cow<'static, str>` message、`Dispatch`、`Option<ResponseDetails>`、`Vec<(&'static str, String)>` 上下文、`Option<Box<dyn Error + Send + Sync>>` source，以及批量删除专用的 `Option<DeleteObjectsOutput>`。
- 公开方法：`new(kind, message)`（dispatch 默认 NotSent）、`with_context`、`with_source`、`kind`、`message`、`dispatch`、`response`、`status() -> Option<http::StatusCode>`、`service_code`、`request_id`、`context`、`is_not_found`、`is_retryable`、`is_outcome_unknown`、`partial_delete_output`。`impl From<std::io::Error>`（kind Io）。不再有 `From<reqwest::Error>` / `From<serde_json::Error>`，改为 crate 内部的 `send_failure` / `body_failure` / `json_encode` / `json_input`。
- `ErrorKind`（`#[non_exhaustive]`，带 `as_str()` 和 Display）：InvalidInput、ConfigInvalid、NotFound、PermissionDenied、Unauthenticated、CredentialsExpired、Conflict、PreconditionFailed、RateLimited、ServerError、ProtocolViolation、Timeout、Transport、Io、Cancelled、OutcomeUnknown、**PartialFailure**、Unexpected。PartialFailure 是任务文档列表之外新增的：批量删除"响应合法但部分对象失败"既不是结果未知也不是协议错误，在 fluent-listing-delete 把部分失败改成 `Ok(output)` 之前需要一个 kind；那个任务可以删掉它。
- `Dispatch`：NotSent / Sent / Unknown。
- `ResponseDetails`（替代 `ResponseError`）：`status() -> StatusCode`、`service_code`、`request_id`、`diagnostic_body`、`body_truncated`。`ResponseErrorKind` 删除，其语义并入 ErrorKind（EmptyJson / InvalidJson / BodyTooLarge → ProtocolViolation，BodyRead → Timeout/Transport + Sent，UnexpectedStatus → 按状态码映射）。
- `is_retryable()`：RateLimited、HTTP 502/503/504（与内部读重试使用的状态一致，500 不算）、Timeout、dispatch 为 NotSent 的 Transport。

**dispatch 与状态码映射（client.rs）。** `send()` 改为返回内部的 `SendFailure { Local(Error), Transport(reqwest::Error) }`，重试判断仍基于原始 reqwest 错误（`!is_body && (connect|timeout|request)`），行为不变。传输错误分类：builder → InvalidInput/NotSent；connect → Transport 或 Timeout（connect timeout）/NotSent；其他 timeout → Timeout/Unknown；其他 → Transport/Unknown。签名、构造请求、token 校验阶段的错误都是 NotSent。`check_response_status` 按 404/401/403/409/412/429/5xx/其余（含非允许的 2xx、3xx）映射 kind，dispatch Sent；读取错误体失败时原 reqwest 错误作为 source。`do_json_limited` 和 `GetObjectOutput::chunk/bytes`、Downloader 读 body 的失败都是 Sent。非 AccessKey 模式下 body、service code、request id 照旧清空（`redact_error` 现在覆盖 do_json_limited 的所有错误路径）。

**结果未知。** 统一用 `Error::into_outcome_unknown`：外层 kind OutcomeUnknown，保留原 dispatch 和 ResponseDetails（所以 `status()` 仍可读），原错误作为 source。
- Copy：`dispatch == NotSent` 的错误原样返回（例如连接被拒，原来会被包成 CopyOutcomeUnknown）；其余全部 OutcomeUnknown（保留原有的保守语义，包括 4xx/5xx 错误状态）；回执不符是 OutcomeUnknown，source 为 ProtocolViolation。
- 托管分片完成：同样规则；完成请求进行中被取消 → OutcomeUnknown，dispatch Unknown，source 为 Cancelled；对象身份不符 → OutcomeUnknown(source ProtocolViolation)；初始化没有返回 upload ID → OutcomeUnknown。
- 批量删除见下。delete.rs 不再匹配错误变体，改为读 `dispatch()`。

**合并的错误类型。**
- `ManagerError` 删除（`manager/error.rs` 删除），5 个从未构造的变体随之消失；`DownloadOptionsBuilder::build`、`Downloader::download`、`DownloadRequest::send` 返回 `fds::Result`。分类：分片大小/并发/分片过多/缺文件名/范围越界 → InvalidInput；ETag 中途变化 → PreconditionFailed(Sent)；Range/Content-Length/长度不符、HEAD 缺 Content-Length → ProtocolViolation；暂存路径不是普通文件或有硬链接 → Io；发布前被取消 → Cancelled；worker/blocking 任务 panic 等内部不变量 → Unexpected。
- `MultipartUploadError` 删除，`MultipartUpload::upload` 返回 `fds::Result<PutObjectResponse>`，`#[allow(clippy::result_large_err)]` 已删除（全仓库不再有这个 allow）。返回原始错误；AbortOnFailure 的 abort 也失败时，abort 错误的 Display 文本作为 `cleanup_error` 上下文附加（会出现在 Display/Debug 中），状态保持 Failed。
- `DeleteObjectsError` 删除。`DeleteObjectsOutput` 增加 `unconfirmed` 字段；`delete_objects` / `delete_objects_with_prefix` 返回 `fds::Result<DeleteObjectsOutput>`，没有改调用形态，部分失败仍然是 Err（交给 fluent-listing-delete）：本地校验或 NotSent → 原错误，不附带 partial output；请求可能已到达后的任何失败或出现未请求的名字 → OutcomeUnknown，`unconfirmed` 为整批；合法响应含逐项失败 → PartialFailure，带 deleted/failures。前缀删除的任何错误都通过 `partial_delete_output()` 携带此前所有页已确认的结果；无效页/marker → ProtocolViolation。原 `not_submitted` 字段去掉（由 `dispatch() == NotSent` 表达）。

**`FdsError::invalid` 调用点。** 全部改为具体 kind：调用方参数（bucket/object 为空、range、metadata key、max_keys、presign 期限、upload id、单请求 timeout、MultipartOptions、源文件变化等）→ InvalidInput；ClientBuilder（endpoint、凭证、超时、keepalive、cslb_max_nodes）、RetryPolicy、OAuth 要求 HTTPS、凭证类型不适用（presign 需要 AK、list_app_buckets 需要 token）、未配置 CDN → ConfigInvalid；token 过期 → CredentialsExpired；分页 marker 缺失/循环、发现返回非法主机名、交换返回的过期时间非法或 token 不可用、分片回执不符 → ProtocolViolation；本地 JSON 序列化失败 → Unexpected（或 Io），只保留 serde 的错误类别；`LifecycleConfig/LifecycleRule::from_json` 的输入错误 → InvalidInput，只保留类别和行列（serde 消息可能回显输入）。`FdsError::Endpoint` 改为带说明的 ConfigInvalid，`Cancelled` 只在真正的合作式取消时出现。

**Display / Debug 与脱敏。** Display：`{kind}: {message} (HTTP {status}, service code {code}) [k: v, ...]`。Debug：kind、message、dispatch、ResponseDetails（status、service code、body 长度、是否截断）、上下文、delete_output（只有计数）、source 链（每一层按 Display 输出，不调用外部类型的 Debug）。不输出 request id 和原始 body。service code 只有形如标识符（`[A-Za-z0-9_.-]{1,64}`）时才进入格式化输出，否则显示 `<withheld>`，防止服务端把签名等内容回显进 code；访问器仍返回原文。传输错误照旧用 `without_url()` 去掉 URL。`wire.rs::diagnostics_are_bounded_and_formatting_omits_untrusted_strings` 原来断言格式化输出不含 service code，按任务要求改为允许 `AccessDenied`，其他断言（凭证、token、签名、request id、body）保留。

**依赖。** `thiserror` 1 → 2（`crates/fds/Cargo.toml`）。`Error` 和 `JsonResponseSource` 改为手写 impl，thiserror 2 只剩 `cslb::CslbError` 在用。Cargo.lock 的变化只是去掉 thiserror 1.0.69（2.0.18 原本就由其他依赖引入），没有新下载。

**fds_cli。** `--diagnostics` 的 JSON 中 `kind` 现在是 ErrorKind 名（原来是 ResponseErrorKind 名），`status` 是数字；批量删除失败时仍输出 `output`、`unconfirmed`、`not_submitted`（delete-many 在 dispatch 为 NotSent 时把输入名列为 not_submitted，prefix 删除为空数组）；分片上传的 cleanup 错误从上下文读取。退出码不变（CLI 测试全部未改动并通过）。

**文档。** README 中与错误类型直接相关的代码片段和说明（Range、元数据、列举求和、Copy、批量删除、手动/托管分片、下载器、服务发现、"区分错误种类"一节）以及 `docs/runtime.md` 的响应与错误、批量删除、分片清理段落已同步。`docs/migration.md` 未改（按决定记录在 docs-release 删除）。`check_docs.py` 不检查 API 名称。

**新增 / 修改的测试。**
- 新文件 `crates/fds/tests/error_model.rs`：`rejected_statuses_map_to_kinds`（404、401、403、409、412、429、500、503、400 经真实 HTTP 映射，并检查 dispatch、service code、is_not_found、is_retryable 和 Display）；`protocol_violations_are_classified`（空 JSON、非法 JSON、Copy 回执不符）；`caller_and_configuration_errors_are_distinct`（InvalidInput 与 ConfigInvalid）；`dispatch_tracks_whether_the_request_was_sent`（本地校验 NotSent、连接被拒 NotSent + Transport + 可重试、连接被拒的 Copy 不是 OutcomeUnknown、连接被拒的批删没有 partial output、收到响应 Sent、批删 500 → OutcomeUnknown + unconfirmed）；`debug_is_diagnostic_and_omits_credentials`（AK 模式 403 回显 AK/SK/签名、传输错误的 source 链、OAuth token 在 URL 中的传输错误，断言 Display/Debug/`{:#?}`/source 链都不含凭证、token、签名，且 Debug 含 kind 和 source 链）；`error_stays_small_without_result_large_err`。
- `error.rs` 单元测试：指针大小、状态码映射表、is_retryable、Display/Debug 格式与脱敏（含 withheld service code）、JSON 序列化错误不含 serde 消息、连接被拒的 `send_failure`（NotSent，URL 中的 token/签名不出现在任何层级，原 `wire.rs::transport_error_conversion_removes_url_from_source` 迁移到这里，因为 `From<reqwest::Error>` 已不再公开）。
- `oauth_discovery.rs`：token 过期断言 `CredentialsExpired` + NotSent；HTTP origin 断言 ConfigInvalid。`pagination_options.rs`：循环 marker 断言 ProtocolViolation。`head_delete.rs`、`copy_receipt.rs`、`multipart_upload.rs`、`wire.rs`、`client_builder.rs`、`object_builders.rs` 等按新 API 改写断言，覆盖面不变或更严（例如 Copy 失败断言 dispatch 和 status，分片完成失败断言 OutcomeUnknown，cleanup 错误断言出现在上下文和 Display 中）。

**公开签名中的 reqwest。** 用 `cargo doc` 输出检查：错误相关签名中已没有 reqwest 类型。剩下两处不属于错误模型，按其他任务文档处理：`GetObjectOutput::into_raw() -> reqwest::Response`（typed-responses 明确负责删除，并补 `into_stream` / `into_async_read`；本任务删掉它会丢掉 OAuth URL 脱敏测试所依赖的逃生口）；`cslb::CslbResolver` 实现了 `reqwest::dns::Resolve`（public-surface 收缩 cslb 公开面时处理）。`HeaderMap` 是 `http` 的重导出，允许出现。`GetObjectOutput::status()` 仍是 `u16`，typed-responses 负责改为 `StatusCode`。`cslb::CslbError` 仍是 cslb 模块独立的公开错误类型（不由任何 Client 操作返回），留给 public-surface。

**验证（都实际运行过）。**
- `cargo test --offline -p fds --all-features -- --test-threads=1`、`cargo test --offline -p fds_cli --all-features`：全部通过。
- `cargo clippy --offline --locked --workspace --all-features --all-targets -- -D warnings`：通过，没有 `result_large_err` allow。
- `bash scripts/check.sh`：**退出码 0**，覆盖 fmt、clippy、四种 feature 组合的 fds 测试与 check、fds_cli 测试与 check、`cargo doc -D warnings`、`fds_cli --help`、MSRV 1.86（三种 feature 组合的测试与 check、fds_cli check）、check_coverage、check_docs、`git diff --check`。没有拉取新依赖。

**未验证。**
- `tests/live_acceptance.rs`（需要真实 FDS 凭证，按决定记录由用户本地执行）：只确认能编译，没有运行。真实服务的错误状态/service code 形态没有用真实环境验证，映射测试使用本地 mock。
- windows 平台没有编译。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/error-model`，用 `git log --grep` 查。

交付内容：分支 `refactor/sdk-overhaul-error-model` 上的 WIP 提交（由协调者压成一笔并快进合并）。主要改动：重写 `crates/fds/src/error.rs`；`client.rs` 负责状态码映射和 dispatch；删除 `manager/error.rs`、`MultipartUploadError`、`DeleteObjectsError`；全 crate 错误调用点改为具体 kind；新增 `crates/fds/tests/error_model.rs`；同步 tests、examples、fds_cli、README、`docs/runtime.md`；thiserror 升级到 2。`bash scripts/check.sh` 退出码 0。
- 协调者复验：审查 diff（错误类型的公开 API、Debug/Display 脱敏、dispatch 维护）后，在任务 worktree 中完整运行 `bash scripts/check.sh`，退出码 0。

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
