# ObjectMetadata 拆分系统字段与用户元数据

- Task key：object-metadata（Roadmap sdk-overhaul）
- 类型：feat
- 状态、依赖：以 plan.json 为准；分支 `feat/sdk-overhaul-object-metadata` 存在即表示进行中

## 目标与范围

**问题（来自审视的"返回值 · ObjectMetadata"部分）：**

1. `ObjectMetadata`（`crates/fds/src/metadata.rs`）把系统 HTTP 头（content-type、content-length、cache-control、last-modified、content-md5……）和用户元数据（`x-xiaomi-meta-*`）混在同一个 `HashMap<String, String>` 里，用户要读 content type 只能 `get("content-type")` 拿到字符串。
2. API 不一致：
   - `set(&mut self, k, v) -> Result<()>` 在运行时校验 key 白名单；
   - `set_content_length` / `set_content_type` 静默吞掉错误（`let _ = self.set(..)`）；
   - `get_content_length() -> Result<i64>`，而 `get_content_type() -> Option<&str>`；
   - `get_` 前缀不符合 C-GETTER。
3. 公开方法叫 `serialize()`，返回 `Result<Vec<u8>>`，名字和 serde 的 `Serialize` 冲突，容易混淆；同时 `impl Deserialize` 解析的是另一种格式（entry-list），两种格式不对称。
4. `from_headers(&reqwest::header::HeaderMap)` 是公开的，类型来自 reqwest。
5. `PutObjectRequest` 已经单独提供了 `content_type` / `cache_control` 等 builder 方法，同时又接受 `metadata(ObjectMetadata)`，两条路径设置同一个头，冲突时谁生效没有定义。

**要做（参照 aws-sdk-s3：`content_type` 等是类型化字段，`metadata` 只放用户自定义元数据）：**

- 拆成两部分：
  - 系统属性（名字由任务定，例如 `ObjectAttributes` 或 `ObjectHead`）：`content_type: Option<String>`、`content_length: Option<u64>`、`content_encoding`、`cache_control`、`content_md5`、`last_modified: Option<DateTime<Utc>>`、`etag`、`storage_class`、`crc64ecma`、`server_side_encryption` 等，均为类型化只读字段，用于 head / get / basic_info 的结果；
  - 用户元数据 `UserMetadata`：key 不带 `x-xiaomi-meta-` 前缀，SDK 负责加前缀、校验和大小写归一化；`insert` 返回 Result，或者在 builder 的 `send()` 时统一校验。
- 写入路径：put / multipart / set_metadata 的 builder 用类型化方法设置系统头，用 `.user_metadata(..)` 设置用户元数据；不再出现两条路径设置同一个头的情况。
- 三种线上格式（发送用 map 形式、basic_info 用 entry-list 形式、从响应头解析）全部变成内部实现，不公开 `serialize()` / `from_headers()`。
- 涉及 `x-xiaomi-meta-content-length`、`storage-class` 等"带 meta 前缀但其实是系统字段"的 key（见 `constant.rs`）：按协议含义归入系统属性，并在任务中列出映射表。
- fds_cli（metadata 相关命令和输出）、tests（`sdk_operations.rs`、`head_delete.rs`、`wire.rs`、`object_builders.rs` 等）、examples 同步修改。

**不做：** 不改线上格式。

- revision 7 调整（用户决定）：旧白名单 meta key 拒绝（AC-7 例外）；系统头空值不发送；`UserMetadata` 在发送前拒绝空 key 和自带 `x-xiaomi-meta-` 前缀的 key（AC-7 例外）。

## 对应验收标准

AC-6（元数据部分），AC-7、AC-8（本任务范围内）。

## 验收条件与验证方法

- [ ] 系统属性都是类型化字段，用户元数据单独存放；公开 API 中没有 `get_content_length` 一类不一致的访问方法，也没有静默吞错误的 setter。
- [ ] 三种线上格式的 wire 测试和 golden 测试与改动前逐字节一致，包括 set_metadata 的请求体、basic_info 的解析、HEAD 响应头的解析、put 请求头。
- [ ] 系统头和用户元数据的冲突（比如用户元数据里出现 content-type）有明确行为，并有测试覆盖。
- [ ] 公开 API 中没有 reqwest 的 `HeaderMap`。
- [ ] fds_cli 的 metadata 相关命令输出不变。

## 执行记录

2026-11-07，分支 `feat/sdk-overhaul-object-metadata`（worktree `.worktrees/object-metadata`），基于 main `f33af23`。任务文档写于迁移之前，按当前代码核对：所有操作已是流式 builder，`ObjectMetadata` 在 `fds::object`，`GetObjectOutput::metadata()` 也返回它，metadata 转 header 已只有 `append_to_headers` 一处，`ObjectBasicInfo.metadata` 已存在。

**1. 公开类型（均在 `fds::object`）。**

- `ObjectMetadata`（只读结果，`head_object`、`get_object_metadata`、`GetObjectOutput::metadata()`、`ObjectBasicInfo.metadata` 返回）：`pub attributes: ObjectAttributes`、`pub user_metadata: UserMetadata`，以及 `entries()`（按 key 顺序返回收到的全部原始条目，供展示和诊断；fds_cli 的 head/get-metadata/basic-info 输出靠它保持不变）。没有 `new`/`set`/`serialize`/`from_headers`/`Deserialize`，`Default` 为空。
- `ObjectAttributes`（`#[non_exhaustive]`，全部 `pub` 只读字段）：`content_type`、`content_length: Option<u64>`、`content_encoding`、`cache_control`、`content_md5`、`last_modified: Option<DateTime<Utc>>`、`etag`、`storage_class: Option<StorageClass>`、`server_side_encryption`、`crc64ecma`、`meta_content_length: Option<u64>`、`ongoing_restore`、`restore_expiry`。字符串原样；缺失为 None；数值只接受十进制数字，日期只接受 IMF-fixdate（与 `GetObjectOutput` 的访问方法共用 `parse_decimal`/`parse_http_date`），非法值为 None 而不报错，原值在 `entries()` 里。响应头取同名头的最后一个可见 ASCII 值；`etag` 从响应头单独读取（不在 `entries()` 里，与旧白名单一致），basicInfo 从条目读取。
- `UserMetadata`：`new`、`insert(key, value) -> Option<String>`、`get`、`remove`、`iter`（key 顺序）、`len`、`is_empty`，实现 `FromIterator`、`Extend`、`Clone`、`PartialEq`。key 不带 `x-xiaomi-meta-` 前缀，存储时转小写，查找忽略大小写。校验选在 `send()`/`build()` 时统一做（任务文档允许的方案之一），失败为 `InvalidInput` + `NotSent`，请求不发出：所有用到用户元数据的操作（put、create/complete multipart、upload_file、set_object_metadata、presign）都拒绝空 key 和已带 `x-xiaomi-meta-` 前缀（不区分大小写）的 key（revision 7 决定）；走请求头的操作另要求 `x-xiaomi-meta-{key}` 是合法头名、value 是合法头值；`set_object_metadata` 走 JSON body，除 key 规则外不按头校验（与改前一致）。
- `ResponseHeaders<'a>`：`GetObjectOutput::headers()` 的新返回类型，见第 5 条。

**2. 写入 builder。** 系统头只有类型化方法，用户元数据只有 `.user_metadata(UserMetadata)`，删除了所有 `.metadata(ObjectMetadata)`：

| builder | 方法 | 空值 |
| --- | --- | --- |
| `PutObject` | 原有 `cache_control`、`content_disposition`、`content_encoding`、`content_type`、`content_md5`、`expect`，新增 `user_metadata` | 不发送（原文档行为） |
| `CreateMultipartUpload`、`CompleteMultipartUpload`、`UploadFile` | `cache_control`、`content_encoding`、`content_type`、`content_md5`、`user_metadata`（`write_metadata_methods!` 宏生成；`UploadFile` 在 create 与 complete 各发一次） | 不发送 |
| `SetObjectMetadata` | 同上五个；`Client::set_object_metadata(bucket, object)` 去掉第三个参数 | 原样放进 body（与改前 CLI 行为一致） |
| `PresignObject` | `content_type`、`content_md5`、`user_metadata`（只有这些头参与签名） | 签名与不设置相同 |

内部统一为 `metadata::WriteMetadata`（四个系统字段 + `UserMetadata`）：`insert_system_headers` 按 cache-control、content-encoding、content-type、content-md5 的顺序插入非空头（put 仍按原顺序与 content-disposition/expect 交错插入）；用户元数据由 `ClientRequest.user_metadata` 在原来追加 metadata 的位置（`do_request` 开头）追加，所以头的相对位置与改前相同；`to_body` 生成 `{"rawMeta":{...}}`。改前 set_metadata body 用 HashMap，多个 key 时顺序随机；现在按 key 排序（BTreeMap），单 key 时逐字节相同。

**3. 冲突行为（有测试）。** 用户元数据一律加前缀发送，与系统头不可能是同一个头：`user_metadata` 里的 `content-type` 发送为 `x-xiaomi-meta-content-type`，不影响 `.content_type()` 设置的 Content-Type，读取时也回到 `user_metadata`（`object_builders.rs::user_metadata_and_system_headers_never_conflict`、`metadata.rs` 单元测试）。6 个系统 meta key（见第 4 条）写入时没有类型化方法，只能作为 `UserMetadata` 的 key（如 `storage-class`）原样发送，读取时归入 `attributes`、不出现在 `user_metadata`；因此每个头仍只有一条写入路径。fds_cli 中 `--meta` 与同名专用参数同时给出仍报 `{key} supplied both as a flag and --meta`。

**4. 带 meta 前缀的系统字段映射表。**

| 线上 key | `ObjectAttributes` 字段 | 类型 |
| --- | --- | --- |
| `x-xiaomi-meta-content-length` | `meta_content_length` | `Option<u64>`（不是本次响应的 HTTP Content-Length；后者是 `content_length`） |
| `x-xiaomi-meta-server-side-encryption` | `server_side_encryption` | `Option<String>` |
| `x-xiaomi-meta-storage-class` | `storage_class` | `Option<StorageClass>` |
| `x-xiaomi-meta-ongoing-restore` | `ongoing_restore` | `Option<String>`（取值格式无协议依据，原样） |
| `x-xiaomi-meta-restore-expiry` | `restore_expiry` | `Option<String>`（同上） |
| `x-xiaomi-meta-hash-crc64ecma` | `crc64ecma` | `Option<String>`（同上） |

`x-xiaomi-multipart-upload-mode` 以及 `upload-time`、`last-checked`、`date`、`range`、`content-range`、`authorization` 不是 `x-xiaomi-meta-*`，不归入用户元数据，也没有类型化字段，只保留在 `entries()` 中。

**5. `GetObjectOutput::headers()`。** 保留，改为返回 crate 自有的借用视图 `ResponseHeaders<'_>`（`get(name) -> Option<&str>`、`get_all(name)`、`iter() -> (&str, &[u8])`、`len`、`is_empty`；`Debug` 只打印头名）。理由：fds_cli `object get --show-headers` 需要按收到顺序输出全部原始头（含非 UTF-8 值），删掉会违反 AC-7；类型化访问方法覆盖不了任意头。公开 API 中已没有 reqwest/http 的 `HeaderMap`。

**6. 三种线上格式改为内部。** 发送：`WriteMetadata::insert_system_headers`、`UserMetadata::append_to_headers`、`WriteMetadata::to_body`（均 `pub(crate)`）。basicInfo：`metadata::entry_list` 作为 `ObjectBasicInfo.metadata` 的 `deserialize_with`，保留原 `Aux/RawMeta/Entry` 结构，null/缺失与错误情形不变。响应头：`ObjectMetadata::from_headers`（`pub(crate)`），白名单与"最后一个值生效、非可见 ASCII 记为空串"语义不变。

**7. 线上格式不变的证明。**

- `tests/wire_snapshot.rs` 与 4 个 fixture、`tests/golden.rs` 与 `golden.json`：fixture 未改一字节（`git diff main -- crates/fds/tests/fixtures` 为空），只把调用改成新 API（`meta("content-type", ..)` → `.content_type(..)`，`meta("x-xiaomi-meta-k", ..)` → `.user_metadata(..)`）；set_metadata、put、init、complete、managed upload、presign 的快照全部通过。
- 新旧实现差分测试（临时把 main 上的旧 `metadata.rs` 编进 crate 作 `#[cfg(test)]` 模块，跑完后删除，未提交；脚本留在 scratchpad）：(a) 20000 组随机响应头（28 种头名含全部白名单、etag、非 meta 的 `x-xiaomi-*`、空后缀，11 种值含空串、非 UTF-8、重复头）：旧 `as_map()` 与新 `entries()` 完全相等；(b) 5012 组 entry-list JSON（含 null/缺失/类型错误的 12 个边界用例与随机大小写重复 key）：成功/失败一致，成功时 map 相等；(c) 20000 组随机写入（四个系统 key、各类 meta key、非法头名/值、非 ASCII）：set_metadata body 解析后的 JSON 与旧 `serialize()` 完全相等；请求头集合与旧 `append_to_headers` 相等、校验成败及错误 kind 一致，唯一差别是系统头的空值不再发送（14577 组可比较中 1648 组含此情形，见第 9 条）。
- 新增测试：`metadata.rs` 单元测试 10 个（大小写归一、头名/值在发送时校验、body 排序与空值、系统头跳过空值、响应头拆分与 13 个字段、缺失与非法值为 None、重复头取最后值、entry-list 拆分、entry-list null/错误）；`response_models.rs` 改为经 `ObjectBasicInfo` 解析 entry-list 并新增拆分用例；`object_builders.rs` 新增冲突用例和按传输方式校验用例。

**8. fds_cli 与 AC-7。** `MetadataArgs::build` 返回 CLI 自有的 `util::Metadata`（四个系统字段 + `UserMetadata`），`apply_metadata!` 把它交给 put/init/complete/upload/set-metadata 的类型化方法，presign 只传参与签名的 content-type、content-md5 和用户元数据（cache-control/content-encoding 本来就不参与签名，URL 不变）。`--meta` key 忽略大小写：四个系统头进类型化字段，`x-xiaomi-meta-*` 去前缀进 `UserMetadata`，其余报 `InvalidInput: Invalid metadata: {key}`（与改前错误文本相同）。head/get-metadata 输出 `entries()`，basic-info 的 `metadata` 输出 `entries()`，`--show-headers` 用 `ResponseHeaders::iter()`。

新旧二进制对比（改前二进制取自 main `f33af23` 构建；Python 桩服务返回固定应答且 Date 固定；比较退出码、stderr、stdout、发出的请求（方法、路径、除 date/authorization/host/user-agent 外的全部头、body）和写出的文件；presign 因 Expires 取当前时间，比较除 Expires/Signature 外的 URL，并用独立的 Python Galaxy-V2 实现分别校验新旧两个签名）共 40 个场景：

- 完全一致（35 个）：head（完整头含 6 个系统 meta key、重复头、非 ASCII、非法数值；404 → `null`；500 报错）、get-metadata（正常、404）、basic-info（大小写重复 key、空 key、未知 key、null 的 metadataBean、entry 为 null 报错）、`get --show-headers`；put（专用参数 + 用户 meta + content-disposition/expect、经 `--meta` 设置系统头并大小写混写、保留 meta key、重复用户 key、用户 key 名为 content-type、非 ASCII 值、`--md5` + `--file`、未知 key、参数冲突、缺 `=`、非法头值、非法头名）；set-metadata（多 key、单 key、非 ASCII 与换行、空、未知 key）；presign（PUT 带 content-type 与用户 meta、HEAD 带全部参数、GET、未知 key）；multipart init（正常、未知 key）、complete、upload（含 recovery 文件内容）、upload 未知 key。
- head、get-metadata 的 stdout 字节顺序不同但 JSON 相等：旧版用 HashMap 序列化，顺序本身随机（旧二进制自比两次即不同），现在按 key 排序；set-metadata 多 key 的 body 同理。
- 差异 5 个，均为第 9 条中用户已决定接受的变化：`put --content-type ""`（只少一个空请求头）、`put --meta date=x`（报 `Invalid metadata: date`）、`put --meta x-xiaomi-meta-=v` 与 `set-metadata --meta x-xiaomi-meta-=v`（报 `user metadata key must not be empty`）、`put --meta x-xiaomi-meta-x-xiaomi-meta-a=v`（报 `user metadata key "x-xiaomi-meta-a" must not include the x-xiaomi-meta- prefix`）；报错的场景退出码 1，且未发出请求。

**9. 行为变化与用户决定（roadmap revision 7）。**

1. 接受为 AC-7 例外，不增加 `upload_mode`：`--meta` 中既不是四个系统头、也不是 `x-xiaomi-meta-*` 的旧白名单 key（`content-length`、`last-modified`、`last-checked`、`upload-time`、`date`、`authorization`、`range`、`content-range`、`x-xiaomi-multipart-upload-mode`）改前被接受，现在报 `InvalidInput: Invalid metadata: {key}` 并以 1 退出。这些是传输头或服务端生成的头，新 SDK 没有写入它们的路径；`docs/open_api/README.md` 已注明 init 不能再发送 multipart 模式头。
2. 接受：系统头取空值时（如 `--content-type ""`），改前 CLI 经 metadata 发送空头 `content-type: `，现在与 SDK `PutObject` 原有文档一致不发送；CLI 输出不变，presign 签名不变（空值与缺失签名相同），只有 put/init/complete/upload 的请求少一个空头。set-metadata body 仍保留空值。
3. 已按决定实现：`UserMetadata` 在 `send()`/`build()` 时拒绝空 key（错误 `user metadata key must not be empty`）和已带 `x-xiaomi-meta-` 前缀的 key（大小写不敏感；key 存储为小写，错误 `user metadata key "{key}" must not include the x-xiaomi-meta- prefix`），返回 `InvalidInput` + `NotSent`；所有用到 user_metadata 的 builder 都生效，set_object_metadata 虽走 JSON body 也拒绝。实现为 `UserMetadata::check_keys`，由 `append_to_headers` 和 `WriteMetadata::to_body` 调用。测试：`metadata.rs::empty_and_prefixed_keys_are_rejected_on_every_transport`；`object_builders.rs::empty_and_prefixed_user_metadata_keys_are_never_sent`（4 种 key × 6 个 builder，桩服务断言 0 个请求）；fds_cli `commands.rs::empty_and_prefixed_user_metadata_keys_fail_before_sending`（`--meta x-xiaomi-meta-=v` 与 `--meta x-xiaomi-meta-X-Xiaomi-Meta-a=v` 在 put、set-metadata、presign、multipart init 上退出码 1、stdout 为空、stderr 为确切错误文本、无请求）。CLI 的 `--meta x-xiaomi-meta-=v`、`--meta x-xiaomi-meta-x-xiaomi-meta-a=v` 因此由成功变为报错，属于本决定接受的 AC-7 例外。

**10. 其余同步。** examples `copy_prefix.rs`、`upload_verify.rs` 改用 `attributes.content_length` 和 `UserMetadata`；README（上传选项表、对象信息与元数据一节含字段映射表、分片、预签名、GetObjectOutput 说明）、`docs/runtime.md`、`docs/cli.md`、`docs/open_api/README.md`、`docs/migration.md` 同步；README 中改动的 3 段代码临时编成 example 编译通过后删除。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/object-metadata`，用 `git log --grep` 查。

- 状态：实现与验证完成，未合并。执行记录第 9 条的三项已由用户在 revision 7 决定，第 3 项（拒绝空 key 和带前缀的 key）已按决定实现。
- `bash scripts/check.sh`（2026-11-08，本 worktree，revision 7 返工的最后一次代码改动之后运行，退出码 0）：fmt；clippy `-D warnings`；四种 feature 组合的 fds test/check；fds_cli test/check；`cargo doc -D warnings`；`fds_cli --help`；MSRV 1.86.0 三种组合的 test/check 及 fds_cli check；`check_coverage.py`（35/35 YAML、33/33 操作）；`check_docs.py`（114 个本地链接）；`git diff --check`。日志中 163 个 `test result: ok`，没有失败、warning 或 error 行。
- wire 快照与 golden：fixture 未改动；差分测试见执行记录第 7 条。
- CLI 新旧二进制对比：40 个场景，35 个完全一致，5 个差异均为第 9 条中用户已接受的变化；presign 3 个场景新旧签名都通过独立校验。
- 共享文件（合并时注意）：`crates/fds/src/{metadata,download,client,multipart,lib}.rs`、`object/{mod,put,multipart,metadata,presign,tests}.rs`；tests：`wire_snapshot.rs`、`golden.rs`、`object_builders.rs`、`response_models.rs`、`sdk_operations.rs`、`head_delete.rs`、`get_object_output.rs`、`client_builder.rs`；examples 2 个；fds_cli `util.rs`、`object.rs`、`output.rs`、`transfer.rs`、`smoke.rs`、`tests/commands.rs`；`README.md`、`docs/{runtime,cli,migration}.md`、`docs/open_api/README.md`。
- 未验证：`tests/live_acceptance.rs` 和 fds_cli `smoke`（需要真实 FDS）；6 个系统 meta key 与 `upload-time` 等在真实服务响应中的取值格式没有 live 证据，因此除 `storage_class` 和两个长度外都按原样字符串提供。
- 依赖变化：无。
- 协调者复验：确认分支没有改动 plan.json 和 roadmap.md，wire fixture 全部不变。按 revision 7 完成返工后，rebase 到最新 main，完整运行 `bash scripts/check.sh`，退出码 0。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul object-metadata

推进 Roadmap sdk-overhaul 的任务 object-metadata：ObjectMetadata 拆分系统字段与用户元数据。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/object-metadata.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：重写 crates/fds/src/metadata.rs，拆成类型化系统属性和 UserMetadata（自动加 x-xiaomi-meta- 前缀并校验），三种线上格式改为 crate 内部；put/multipart/set_metadata 的 builder 改用类型化方法和 .user_metadata()；head/get/basic_info 返回类型化属性；同步 fds_cli、tests、examples
验证：三种线上格式的 wire / golden 逐字节对比；冲突行为测试；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
