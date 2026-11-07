# 内部实现去冗余：serde 样板、header、脱敏、HTTP 配置

- Task key：internal-cleanup（Roadmap sdk-overhaul）
- 类型：refactor
- 状态、依赖：以 plan.json 为准；分支 `refactor/sdk-overhaul-internal-cleanup` 存在即表示进行中

## 目标与范围

**问题（来自审视的"内部实现冗余"部分）：**

1. **serde 样板代码。** 几乎每个响应字段都写成 `#[serde(rename = "xxx", default, deserialize_with = "crate::de::null_default")]`，每个字段占 5 到 6 行，而且不少 rename 前后同名（比如 `rename = "etag"` 用在 `etag` 字段上，`rename = "prefix"` 用在 `prefix` 字段上）。涉及 `object/put.rs`、`object/listing.rs`、`object/metadata.rs`、`object/multipart.rs`、`bucket.rs`、`acl.rs` 等。
2. **header 构造重复。**
   - 把 metadata 转成 HeaderMap 的循环在 `client.rs:162` 和 `object/presign.rs:116`（`metadata_headers`）各写了一遍；
   - `HeaderValue::from_str(..).map_err(|e| FdsError::invalid(e.to_string()))` 在 `client.rs`、`object/mod.rs`、`object/basic.rs`、`object/presign.rs`、`bucket.rs` 等处重复约 10 次；
   - `object/mod.rs` 的 `insert_header_opt` 只处理 `&Option<String>`。
3. **脱敏逻辑分散在三处**，并且 `matches!(self.inner.credentials, Credentials::AccessKey(..))` 重复了 4 次：
   - `send()` 在 OAuth 模式下拆开 `reqwest::Response` 再重新组装，只为改写里面的 URL（`client.rs:329-349`，包含 `ResponseBuilderExt` 和 `.unwrap()`）；
   - `do_json_limited` 在非 AccessKey 模式下清空 request_id（`client.rs:231-237`）；
   - `redact_error` 在非 AccessKey 模式下清空 body、service_code、request_id（`client.rs:355`）。

   request id 本身不是秘密。URL 改写只是为了 `GetObjectOutput::into_raw()` 暴露的原始 response，而 `into_raw` 会在 typed-responses 中删掉。
4. **同类逻辑两套实现。** `ListObjectsRequest::pages()` 用 Brent 算法检测分页循环（`object/listing.rs:153`），`delete_objects_with_prefix` 却用一个不断增长的 HashSet 自己重新分页（`object/delete.rs:160`）。本任务先把循环检测抽成一个内部 helper，让两处共用；删除流程改为复用 `pages()` 放在 fluent-listing-delete 中做。
5. **绕开了已有封装。** `MultipartUpload::start` 只检查 bucket 和 object 非空（`multipart.rs:149`），没有调用 `validate_object`，bucket 名规则校验不一致。
6. **HTTP 客户端配置。**
   - 没有设置 User-Agent（`constant.rs` 的注释明确写着"Rust does not set a User-Agent"），建议设成 `fds-rs/<CARGO_PKG_VERSION>`，并允许追加应用标识；
   - `pool_max_idle_per_host(100)` 写死在代码里，改为 builder 可配置（保留默认值 100）。
7. **tokio feature 过多。** 作为库，`crates/fds/Cargo.toml` 不应强制开启 `rt-multi-thread`，`net`、`macros` 也要按实际用到的功能评估（`tokio::select!` 需要 `macros`，`JoinSet` 和 `spawn_blocking` 需要 `rt`）。只保留真正需要的 feature。
8. **重试随机源。** `RetryPolicy::delay` 用 `RandomState::new().hash_one(SEQUENCE)` 生成抖动（`retry.rs:53-54`），能用但可读性差。可以加注释说明原理，或者换成 `fastrand`（新增依赖要说明理由）。
9. 函数前的 🔴 注释和 "Ports `fds/xxx.go`" 一类注释只对移植阶段有意义。逐一保留协议相关的说明，删掉纯粹的移植痕迹。

**要做：** 逐项处理以上 1 到 9。serde 部分建议在容器上加 `#[serde(rename_all = "camelCase", default)]`，配合 `serde_with::DefaultOnNull`（通过 `#[serde_as]`）或一个 crate 内部的小宏，表达"null 视为默认值"。字段名和 camelCase 不一致的（例如 `name` 对应 `bucket_name`、`objects` 对应 `object_summaries`）继续用显式 rename。**线上 JSON 的兼容性必须保持不变**：缺失键、null、未知字段的处理行为和改动前一致。

**不做：** 不改字段的类型（`i64` 改 `u64`、时间改 DateTime 由 typed-responses 负责）；不改公开 API 的形态。

## 对应验收标准

AC-8（本任务范围内）；为 AC-6 打基础。没有直接对用户可见的 AC，本任务对外行为不变。

## 验收条件与验证方法

- [ ] 所有响应模型的反序列化测试保持通过；新增覆盖"字段缺失 / 字段为 null / 未知字段"的表驱动测试，每个模型至少一条。
- [ ] metadata 转 header 只剩一个 helper；`HeaderValue::from_str(..).map_err(..)` 的重复调用被一个 helper 或 `From` 实现替代。
- [ ] 脱敏策略集中在一个地方（比如 `Client::redaction_policy()` 或 credentials 上的一个方法），`matches!(credentials, AccessKey)` 不再到处重复；现有脱敏测试全部通过。
- [ ] 分页循环检测只有一份实现。
- [ ] 请求带上 `User-Agent: fds-rs/<version>`，有 wire 测试断言。
- [ ] `cargo tree -e features -p fds` 确认 tokio 不再强制开启 `rt-multi-thread`；默认 feature 和全部 feature 下都能编译。
- [ ] 签名 golden 测试不受 User-Agent 影响（UA 不参与签名，需要确认）。

## 执行记录

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/internal-cleanup`，用 `git log --grep` 查。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul internal-cleanup

推进 Roadmap sdk-overhaul 的任务 internal-cleanup：内部实现去冗余：serde 样板、header、脱敏、HTTP 配置。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/internal-cleanup.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：响应模型改用容器级 rename_all + default + DefaultOnNull（或内部宏），线上兼容性不变；client.rs 中合并 metadata→header helper、HeaderValue 错误转换、脱敏策略（删掉 do_json_limited 中清空 request_id 的逻辑）；分页循环检测抽成内部 helper；MultipartUpload::start 改用 validate_object；设置 UA、pool_max_idle_per_host 可配置；精简 tokio features；清理移植注释
验证：新增缺失 / null / 未知字段的反序列化表驱动测试和 UA 的 wire 测试；cargo tree -e features -p fds 检查 tokio feature；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
