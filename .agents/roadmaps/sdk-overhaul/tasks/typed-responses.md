# 返回值类型化与精简

- Task key：typed-responses（Roadmap sdk-overhaul）
- 类型：feat
- 状态、依赖：以 plan.json 为准；分支 `feat/sdk-overhaul-typed-responses` 存在即表示进行中

## 目标与范围

**问题（来自审视的"返回值"部分）：**

1. `PutObjectResponse`（`object/put.rs:28`）照搬 Go 结构体：`access_key_id`、`signature`、`expires: i64`、`outside_access` 对用户几乎没用；所有字段都用 `null_default`，缺失和空字符串无法区分；`previous_version_id` 应该是 `Option<String>`。complete multipart 也返回这个类型。
2. 时间和大小字段类型不统一：
   - `ObjectSummary.last_modified: Option<DateTime<Utc>>`，而 `ObjectSummary.upload_time: i64`；
   - `ObjectSummary.size: i64`、`ObjectBasicInfo.size: i64`、`ObjectBasicInfo.upload_time: i64`；
   - `GetBucketInfoResponse.creation_time / object_num / used_space: i64`；
   - `AppBucket.creation_time / used_space / num_objects: i64`；
   - `UploadPartResponse.part_size: i64`、`part_number: i32`（和 fluent-multipart-manager 的强类型保持一致）；
   - `ObjectListing.max_keys: i32`。
3. `GetObjectOutput`（`download.rs:83`）只有 `status() -> u16`、`headers()`、`metadata()`、`chunk()`、`bytes()`、`copy_to()`、`into_raw() -> reqwest::Response`：
   - 缺少类型化的访问方法：`content_length()`、`content_type()`、`etag()`、`last_modified()`、`content_range()`、`is_partial()`；
   - 缺少 `into_stream()`（`impl Stream<Item = Result<Bytes>>`）和 `into_async_read()`（`impl AsyncRead`），参考 aws 的 `ByteStream`；
   - `into_raw()` 暴露 reqwest，而且迫使 send() 在 OAuth 模式下做 URL 改写（见 internal-cleanup 第 3 点）。
4. presign 和 absolute URL 返回 `String`，应该返回 `url::Url`，这样类型就说明了"这是一个合法的 URL"。
5. `head_object` 返回 `Option<ObjectMetadata>` 的设计是好的，保留；但 ObjectMetadata 本身的问题由 object-metadata 处理。
6. 命名：`GetBucketInfoResponse` 改为 `BucketInfo`，`ListBucketsResponse` 改为 `BucketList`，`object_num` 改为 `object_count`，`storage_class_type` 改为 `storage_class`，`object_summaries` 改为 `objects`；同类问题一并清理（遵循 Rust API Guidelines C-CASE / C-WORD-ORDER）。
7. `StorageClass(pub String)` 带关联常量：服务端返回的大小写不稳定（`STANDARD` 和 `Standard` 都有），保留透传字符串的设计是对的，但公开字段 `.0` 应该改为私有，并提供 `is_standard()` 一类不区分大小写的判断方法。

**要做：**

- 对上面每个模型逐一调整：时间改为 `DateTime<Utc>`（毫秒时间戳在反序列化时转换，非法值报 ProtocolViolation），大小和计数改为 `u64`（负数报 ProtocolViolation），可能缺失的字段改为 `Option`，不对用户有用的字段删掉（如果担心丢信息，可以用 `#[serde(flatten)] extra` 或在文档中说明；建议直接删掉）。
- `PutObjectResponse` 精简为对用户有意义的字段（bucket、key、version / previous_version 等，以服务端实际返回为准；字段取舍要参考 `docs/open_api` 和 live 测试记录，并在任务里写明依据）。
- `GetObjectOutput` 增加类型化访问方法、`into_stream()`、`into_async_read()`，删除 `into_raw()`；`status()` 返回 `http::StatusCode`。
- presign / absolute URL 返回 `Url`。
- 模型类型名和字段名按第 6 点统一。
- `fds_cli` 的输出格式保持不变（CLI 内部做必要的格式转换，比如时间仍然以原来的格式输出）。

**不做：** ObjectMetadata 的结构调整（object-metadata 负责）。

- revision 6 调整（用户决定）：`PutObjectOutput` 的 `access_key_id`、`signature`、`expires`、`outside_access` 保留（CLI 输出依赖）；类型收紧导致的 CLI 非法值报错行为作为 AC-7 例外接受。

## 对应验收标准

AC-6（全部），AC-7、AC-8（本任务范围内）。

## 验收条件与验证方法

- [ ] 模型中不再有表示时间的 `i64` 毫秒字段，也不再有表示大小或计数的 `i64` / `i32` 字段（逐个模型核对，在执行记录中列出清单）。
- [ ] 反序列化测试覆盖：毫秒时间戳转换、null 或缺失变成 None、负数和非法时间戳报 ProtocolViolation。
- [ ] `GetObjectOutput` 的类型化访问方法、`into_stream()`、`into_async_read()` 都有测试；公开 API 中没有 reqwest 类型。
- [ ] presign 返回 `Url`，并且和改动前生成的字符串完全一致（golden 测试）。
- [ ] fds_cli 输出格式不变（`crates/fds_cli/tests/commands.rs` 通过）。

## 执行记录

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/typed-responses`，用 `git log --grep` 查。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul typed-responses

推进 Roadmap sdk-overhaul 的任务 typed-responses：返回值类型化与精简。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/typed-responses.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：逐个调整 crates/fds/src 下的响应模型（object/put.rs、listing.rs、metadata.rs、multipart.rs、bucket.rs、oauth.rs）：时间改 DateTime<Utc>，大小和计数改 u64，可缺失字段改 Option，删掉无用字段；GetObjectOutput 加类型化访问方法、into_stream、into_async_read，删除 into_raw；presign 返回 Url；按 Rust 命名规范改类型名和字段名；fds_cli 保持输出格式
验证：模型反序列化表驱动测试（转换、缺失、非法值）；presign golden 测试；fds_cli 输出对比；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
