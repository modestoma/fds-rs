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

## 对应验收标准

AC-6（元数据部分），AC-7、AC-8（本任务范围内）。

## 验收条件与验证方法

- [ ] 系统属性都是类型化字段，用户元数据单独存放；公开 API 中没有 `get_content_length` 一类不一致的访问方法，也没有静默吞错误的 setter。
- [ ] 三种线上格式的 wire 测试和 golden 测试与改动前逐字节一致，包括 set_metadata 的请求体、basic_info 的解析、HEAD 响应头的解析、put 请求头。
- [ ] 系统头和用户元数据的冲突（比如用户元数据里出现 content-type）有明确行为，并有测试覆盖。
- [ ] 公开 API 中没有 reqwest 的 `HeaderMap`。
- [ ] fds_cli 的 metadata 相关命令输出不变。

## 执行记录

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/object-metadata`，用 `git log --grep` 查。

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
