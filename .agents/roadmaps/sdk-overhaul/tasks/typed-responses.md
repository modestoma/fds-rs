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

2026-11-07，分支 `feat/sdk-overhaul-typed-responses`（worktree `.worktrees/typed-responses`），基于 main `c50dc14`。任务文档写于迁移之前，按当前代码逐项核对：模型在 `fds::object`、`fds::bucket`、`fds::multipart`、`fds::oauth`；`InitMultipartUploadResponse` 已是内部类型，不在本任务范围。

**1. 反序列化机制。** `de.rs` 新增 `opt_millis`（`Option<DateTime<Utc>>`，null 为 None）和 `millis`（必填）：只接受非负、`DateTime<Utc>` 能表示的整数毫秒，负数、超范围（如 9e18）、浮点、字符串都报 serde 错误。`lenient!` 宏改为逐字段处理（tt muncher），字段可写 `pub t: Option<DateTime<Utc>> => "crate::de::opt_millis"` 指定自己的 null 容忍反序列化器，其余字段照旧用 `null_default`。大小和计数直接用 `u64`/`u32`，负数由 serde 拒绝。`Client::do_json` 已把任何 JSON 解码失败映射为 `ErrorKind::ProtocolViolation`（`Dispatch::Sent`），所以非法值无需额外处理；`response_models.rs` 用 mockito 对 list/basicInfo/bucket info/put/upload part 逐一断言了这一点。

**2. 命名方案。** 资源类模型用名词，操作结果没有资源名词时用 `<Operation>Output`（与已有的 `GetObjectOutput`、`DeleteObjectsOutput` 一致，取代混用的 `Response` 后缀）：

| 改前 | 改后 |
| --- | --- |
| `bucket::GetBucketInfoResponse` | `bucket::BucketInfo` |
| `bucket::ListBucketsResponse` | `bucket::BucketList` |
| `oauth::AppBuckets` | `oauth::AppBucketList` |
| `object::PutObjectResponse` | `object::PutObjectOutput` |
| `object::CopyObjectResponse` | `object::CopyObjectOutput`（字段不变） |
| `multipart::UploadPartResponse` | `multipart::UploadPartOutput` |

字段名：`object_num`/`num_objects` → `object_count`，`storage_class_type` → `storage_class`，`object_summaries` → `objects`，`AppBucket.name` → `bucket_name`（与 `BucketInfo` 一致）。`bucket_name`/`object_name` 在各模型中保持一致，未改。线上 key 全部不变（`rename` 指向原 key，`lenient` 表驱动测试逐 key 验证）。

**3. 字段对照（改前 → 改后）。** 时间字段由毫秒转换；缺失或 null 的时间为 `None`；lenient 模型中缺失或 null 的大小/计数仍为 0，字符串为空串，在 rustdoc 写明。

| 模型 | 字段 | 改前 | 改后 |
| --- | --- | --- | --- |
| `PutObjectOutput` | `bucket_name` / `object_name` | `String` | 不变 |
| | `previous_version_id` | `String`（缺失与空串不可分） | `Option<String>`（缺失/null 为 None，空串为 `Some("")`） |
| | `expires` | `i64` 毫秒 | `Option<DateTime<Utc>>` |
| | `access_key_id` / `signature` / `outside_access` | `String` / `String` / `bool` | 暂时保留不变，见第 7 条待决 |
| `ObjectSummary` | `size` | `i64` | `u64` |
| | `upload_time` | `i64` 毫秒 | `Option<DateTime<Utc>>` |
| | `last_modified` | `Option<DateTime<Utc>>`（RFC 3339 字符串） | 不变 |
| | `etag` / `object_name` / `owner` | — | 不变 |
| `ObjectListing` | `max_keys` | `i32` | `u32`（与 builder 的 `NonZeroU32` 一致） |
| | `object_summaries` | `Vec<ObjectSummary>` | `objects` |
| | 其余（`bucket_name`、`prefix`、`marker`、`truncated`、`next_marker`、`delimiter`、`common_prefixes`） | — | 不变 |
| `ObjectBasicInfo` | `size` | `i64` | `u64` |
| | `upload_time` | `i64` 毫秒 | `Option<DateTime<Utc>>` |
| | `storage_class_type` | `Option<StorageClass>` | `storage_class` |
| | `allow_outside_access` / `metadata` | — | 不变 |
| `BucketInfo` | `creation_time` | `i64` 毫秒 | `Option<DateTime<Utc>>` |
| | `object_num` | `i64` | `object_count: u64` |
| | `used_space` | `i64` | `u64` |
| | `storage_class_type` | `Option<StorageClass>` | `storage_class` |
| | `bucket_name` / `allow_outside_access` / `enable_sse` | — | 不变 |
| `BucketList` | `owner` / `buckets` | `Vec<GetBucketInfoResponse>` | `Vec<BucketInfo>` |
| `AppBucket`（全部必填） | `name` | `String` | `bucket_name` |
| | `creation_time` | `i64` 毫秒 | `DateTime<Utc>` |
| | `used_space` | `i64` | `u64` |
| | `num_objects` | `i64` | `object_count: u64` |
| | `org_id` | — | 不变 |
| `AppBucketList` | `owner` / `buckets` | — | 不变 |
| `UploadPartOutput`（全部必填） | `part_number` | `i32` | `NonZeroU16`（与 `upload_part` 入参同型；0、负数、>65535 拒绝） |
| | `part_size` | `i64` | `u64` |
| | `etag` | — | 不变；derive 去掉 `Default`（`NonZeroU16` 无默认值），加 `PartialEq, Eq` |

清单核对：公开模型中已没有表示时间的 `i64` 字段，也没有表示大小或计数的 `i64`/`i32` 字段（`grep -rn "i64\|i32" crates/fds/src` 只剩 `de.rs` 的转换函数、内部 `WireToken.expire_time`（交换时立即转为 `DateTime`）、manager 的分片上限检查，以及 `ObjectMetadata::set/get_content_length` 的 `i64`，后者属 object-metadata）。未改动的模型：`DeleteObjectsOutput`、`DeleteObjectFailure`、`CopyObjectOutput`、ACL 模型、`LifecycleRule`/`LifecycleConfig`/`LifecycleBaseItem`（`days: f64` 是协议定义的浮点）、`AccessLog`、`MultipartProgress`（`total_parts: usize`、`file_size: u64`）、`StorageAccessToken`（已是 `DateTime`）、`ObjectMetadata`（object-metadata 负责）。

`PutObjectOutput` 字段取舍依据：`docs/open_api/put-object.yaml` 与 `complete-multipart-upload.yaml` 定义 `bucketName`、`objectName`、`accessKeyId`、`signature`、`expires`（"签名失效时间戳（UTC），单位毫秒，默认有效时间30天"，即服务端为新对象签发的下载签名）；`previousVersionId`、`outsideAccess` 不在 YAML 中，来自初始导入时照搬的 Go 结构体。仓库中没有 live 测试的响应记录（`live_acceptance.rs` 由用户本地运行，只断言 `object_name`），无法据实判断服务端是否返回后两者，故保留为宽松字段。

**4. `StorageClass`。** `.0` 改为私有；新增不区分 ASCII 大小写的 `is_standard()`、`is_standard_ia()`、`is_archive()` 和 `Display`；`==` 仍比较原始字符串（单元测试 `storage_class_predicates_ignore_ascii_case`，10 组取值）。

**5. `GetObjectOutput`。** `status() -> http::StatusCode`；新增 `is_partial()`（206）、`content_length() -> Option<u64>`、`content_type() -> Option<&str>`、`etag() -> Option<&str>`（原样含引号）、`last_modified() -> Option<DateTime<Utc>>`（IMF-fixdate）、`content_range() -> Option<ContentRange>`；头部缺失或无法解析一律 `None`，不让下载失败，原始值仍在 `headers()`（`http::HeaderMap`）。新类型 `object::ContentRange`：`start()`、`end()`（含）、`complete_length() -> Option<u64>`（`*` 为 None），解析拒绝反向区间、超出总长、`bytes */n`、非数字。新增 `into_stream() -> impl Stream<Item = Result<Bytes>> + Send + Unpin + 'static`（错误与 `chunk()` 相同，经 `Error::body_failure`）和 `into_async_read() -> impl AsyncRead + Send + Unpin + 'static`（`tokio_util::io::StreamReader`；错误为 `io::Error`，超时 `TimedOut`、其余 `Other`，内层可 downcast 回 `fds::Error`）。删除 `into_raw()`；公开 API 中不再有 reqwest 类型。随之删除 `client.rs` 中只为 `into_raw` 存在的 OAuth 响应 URL 改写（`Redaction::response_url`、`url_extension`），这是 internal-cleanup 第 3 条记录的后续：现在没有任何返回类型暴露响应 URL，body 读取错误本来就经 `without_url()` 去掉 URL。`oauth_discovery.rs` 中依赖 `into_raw` 检查 URL 的断言随之删除（Debug 不含 secret 的断言保留）。

**6. presign / absolute URL。** `PresignObject::build()` 和 `Client::absolute_object_url()` 返回 `url::Url`。签名逻辑不变，最后一步用 `parse_verbatim` 解析并要求 `as_str()` 与签名字符串逐字节相同（否则 `Unexpected`，实际不会发生：路径部分本来就来自已解析的 `Url`，query 已编码，签名是 base64）。`golden.rs` 的 presign/absolute 用例改为比较 `as_str()`（并断言 `to_string() == as_str()`），golden.json 与 `wire_object_ops.txt` 的 presign/absolute 段落一字节未改。

**7. 与 AC-7 冲突、需用户决定的两点（未自行决定）。**
- `PutObjectOutput` 的 `access_key_id`、`signature`、`expires`、`outside_access`：任务要求删除，但 `fds_cli object put`、`multipart complete`、`multipart upload` 的 stdout 正是这几个 key（`output.rs::put`，注释写明有意保留签名），CLI 只能从 SDK 拿到这些值，删除就必然改变 CLI 输出。按"超出范围的决定先报回"处理：暂时保留这四个字段（`expires` 已类型化），CLI 输出不变；删除只需去掉字段并调整 `output.rs::put`。
- 类型收紧带来的 CLI 行为差异（对非法输入/非法服务端数据）：服务端返回负数大小、负数或超范围时间戳时，原来照常打印负数并以 0 退出，现在报 `ProtocolViolation` 并以 1 退出（`multipart upload` 为 completion 已发出后的 `OutcomeUnknown`，状态 `CompletionUnknown`）；`multipart complete --parts` 文件里 `partNumber` 为 0、负数或大于 65535、`partSize` 为负时，原来照发请求，现在读取文件时报错并以 1 退出。这是 AC-6 要求的行为，但严格说属于 AC-7 的例外，需用户确认。

**8. fds_cli、examples、文档。** `output.rs` 在 CLI 内部转换回原格式：时间 `Option<DateTime>` → 毫秒，None → 0；`AppBucket.creation_time` → 毫秒；`previous_version_id` None → `""`；`StorageClass` 序列化不变；`object get --show-headers` 的 `status` 用 `as_u16()`。`smoke.rs`、`transfer.rs` 只改类型名和 `Url` 的字符串判断。examples：`quickstart.rs`（`objects`）、`object_requests.rs`（展示类型化访问方法）、`copy_prefix.rs` 和 `oauth_inventory.rs`（`size` 已是 `u64`，去掉 `try_from`）。README 的 Bucket 查询、StorageClass、PUT 结果、流式读取（新增 `into_async_read` 示例和访问方法说明）、Range 示例（改用 `is_partial`/`content_range`）、basicInfo、列举、Copy、手动/托管分片、预签名、OAuth 各段；`docs/runtime.md` 流式表格加一行；`lib.rs` 模块布局列出 `ContentRange`。

**9. 测试。**
- `response_models.rs` 新增 `typed_fields_convert`（0、1、典型值、9999 年的毫秒值；`u64::MAX` 与超过 `i64::MAX` 的大小；`maxKeys = u32::MAX`；片号 1/10000/65535；改名字段的线上 key）、`optional_fields_distinguish_missing_from_empty`、`invalid_times_sizes_and_part_numbers_are_rejected`（14 组"模型 × 字段"，每个时间字段 8 种非法值、每个大小字段 5 种以上，含嵌套页/列表）、`invalid_typed_values_are_protocol_violations`（mockito，6 个操作，断言 `ErrorKind::ProtocolViolation`）；原有 lenient/strict 表驱动测试改用新类型名后照常通过。
- 新文件 `tests/get_object_output.rs`（6 个测试）：206 响应的全部访问方法与 Debug；缺失与 11 种畸形头部返回 None；`into_stream` 接续 `chunk()` 后的剩余内容、EOF 稳定、截断 body 报 `Transport`/`Sent`；`into_async_read` 读全文、`tokio::io::copy` 空体、截断（`Other` → `Transport`）与 read timeout（`TimedOut` → `Timeout`）的 downcast；编译期断言 `Send + Unpin + 'static`。
- 单元测试：`basic_info_entry_list_metadata` 检查 `upload_time` 与 `is_standard()`；presign 单元测试改用 `Url`。
- 其余测试只改调用写法：`object_builders.rs`（`into_raw` 测试改为 `content_length`/`is_partial`）、`oauth_discovery.rs`、`golden.rs`、`wire_snapshot.rs`（`receipt` 辅助函数改为 `u16`/`u64`）、`client_builder.rs`、`upload_bodies.rs`、`multipart_upload.rs`、`wire.rs`、`pagination_options.rs`、`live_acceptance.rs`、fds_cli `commands.rs`。wire 快照 fixture 未改动（`git diff c50dc14 -- crates/fds/tests/fixtures` 为空）。

**10. CLI 新旧二进制对比。** 改动前在本 worktree 构建 `fds_cli`（main `c50dc14`）留存，改动后再构建新二进制；用 Python 写的固定应答服务（Date/Server 头固定）跑 5 套应答（full：全部字段非默认、超过 2^53 的大小、title-case 存储类别；sparse：大多数 key 缺失；nulls：全部 null；zeros：0、空串、小写 `archive`；invalid：负数大小和时间戳），每套 17 条命令：`bucket info`、`bucket list`、`bucket list-authorized`、`bucket list-app`（含 OAuth 交换）、`object list`（带 prefix/delimiter、`--all`）、`object basic-info`、`object put`、`object get --show-headers`、`object get --range`、`object url`（两种对象名）、`multipart init`、`multipart part`、`multipart complete`（含/不含 `--expires`，回执含超过 2^53 的 partSize）、`multipart upload`（3 片，同时比较 recovery 文件内容）。比较 stdout、stderr、退出码。先用旧二进制自比 85/85 一致，确认对比稳定；新旧对比：full/sparse/nulls/zeros 四套 68/68 完全一致，invalid 套 6 条一致、11 条按第 7 条所述改为 ProtocolViolation 退出。`object presign` 依赖当前时间，未纳入二进制对比，由 golden/wire 快照和 `commands.rs` 中"CLI 输出等于 SDK 输出"的测试覆盖。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/typed-responses`，用 `git log --grep` 查。

- 状态：实现与验证完成，分支上是 WIP 提交加一笔带 Roadmap 末行的收尾提交，未合并；等待用户对执行记录第 7 条的两点作出决定。
- `bash scripts/check.sh`（2026-11-07，本 worktree，最后一次代码改动之后运行，退出码 0）：fmt；clippy `-D warnings`；四种 feature 组合的 fds test/check；fds_cli test/check；`cargo doc -D warnings`；`fds_cli --help`；MSRV 1.86.0 三种组合的 test/check 及 fds_cli check；`check_coverage.py`（35/35 YAML、33/33 操作）；`check_docs.py`（114 个本地链接）；`git diff --check`。日志中 163 个 `test result: ok`，没有失败，也没有 warning / error 行；新增的 `get_object_output.rs` 和 `response_models.rs` 新用例在 stable 4 种与 MSRV 3 种组合中都运行并通过。
- wire 快照与 golden：fixture 未改动，presign/absolute 在返回 `Url` 后字符串逐字节一致。
- CLI 新旧二进制对比：见执行记录第 10 条（4 套合法应答 68/68 一致；非法应答按第 7 条改为报错退出）。
- 共享文件（合并时注意）：`crates/fds/src/de.rs`（`lenient!` 宏改写）、`client.rs`（删除响应 URL 改写）、`download.rs`、`object/{mod,put,listing,metadata,multipart,basic,presign,delete,tests}.rs`、`bucket/{mod,basic}.rs`、`oauth.rs`、`multipart.rs`、`lib.rs`；tests：`response_models.rs`、新文件 `get_object_output.rs`、`object_builders.rs`、`oauth_discovery.rs`、`golden.rs`、`wire_snapshot.rs`、`client_builder.rs`、`upload_bodies.rs`、`multipart_upload.rs`、`wire.rs`、`pagination_options.rs`、`live_acceptance.rs`；examples 5 个；fds_cli `output.rs`、`object.rs`、`smoke.rs`、`transfer.rs`、`tests/commands.rs`；`README.md`、`docs/runtime.md`。object-metadata 若并行修改 `object/metadata.rs`、`download.rs`、`ObjectMetadata` 相关测试，会与本任务相邻。
- 未验证：`tests/live_acceptance.rs`（需要真实凭证，由用户本地执行），因此 `PutObjectOutput` 等字段在真实服务上的出现情况、`maxKeys` 实际是数字还是字符串（YAML 写的是 string，现有实现和测试按数字处理）都没有 live 证据；fds_cli `smoke` 需要真实 FDS，未运行；`object presign` 未做新旧二进制对比（依赖当前时间）。
- 依赖变化：无（`url`、`tokio-util` 的 `io` 已是现有依赖）。
- 待用户决定：执行记录第 7 条：(1) 是否删除 `PutObjectOutput` 的 `access_key_id`、`signature`、`expires`、`outside_access`（删除会改变 `object put`/`multipart complete`/`multipart upload` 的 CLI 输出，需要 AC-7 例外）；(2) 是否接受类型收紧对非法服务端数据和非法 `--parts` 文件的 CLI 行为变化作为 AC-7 例外。
- 协调者复验：确认分支没有改动 plan.json 和 roadmap.md，wire fixture 全部不变；完整运行 `bash scripts/check.sh`，退出码 0。待决的两项已由用户在 revision 6 中决定：保留四个字段；类型收紧带来的变化接受为 AC-7 例外。

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
