# 分片上传与 Downloader 迁移到流式 builder

- Task key：fluent-multipart-manager（Roadmap sdk-overhaul）
- 类型：feat
- 状态、依赖：以 plan.json 为准；分支 `feat/sdk-overhaul-fluent-multipart-manager` 存在即表示进行中

## 目标与范围

**问题：**

1. 手动分片 API：`InitMultipartUploadRequest`、`CompleteMultipartUploadRequest` 是 `pub` 字段加 `Default`；`UploadPartRequest` 有私有字段但 `send(&client)` 需要传 client；`part_number: i32` 靠运行时检查正数，应该用 `NonZeroU16`（FDS 上限 10000）或 `u32`；`abort_multipart_upload(&InitMultipartUploadResponse)` 要求传入整个 init 响应。
2. `MultipartUpload`：
   - `start(&Client, InitMultipartUploadRequest, path, MultipartOptions)` 参数是位置参数；
   - `MultipartOptions::new(part_size, concurrency, cleanup)` 也是位置参数，和 `DownloadOptions::builder()` 风格不一致；
   - `upload(&mut self, &CancellationToken)` 与 public-surface 对取消信号的决定保持一致。
3. `Downloader`：
   - `Downloader::new(impl AsRef<Client>, DownloadOptions)`，`DownloadRequest` 是 `pub` 字段的结构体，`DownloadRequest::send(&Downloader)`；
   - `download()` 中的 HEAD 请求是手写的 `ClientRequest`（`manager/download.rs` 约 :314）：没有标 `read_only`，所以配置了重试也不会重试；404 也只返回普通错误；没有复用 `head_object`；
   - `DownloadOptionsBuilder::build()` 返回 `ManagerError`（error-model 合并后变为统一错误）。
4. 两个管理器的选项风格不统一：上传用 `MultipartOptions::new(...)` 加校验，下载用 builder 加 `build()` 校验，命名（part_size、concurrency、resume / cleanup）也应该统一。

**要做：**

- 手动分片：`client.create_multipart_upload(b, k).metadata(..).send()` 返回 `MultipartUploadSession`（或沿用 init 响应类型名，由任务决定），会话上提供 `.upload_part(n, body)`、`.complete(parts)`、`.abort()` 这些 builder；另外提供不依赖会话对象的入口 `client.upload_part(b, k, upload_id, n, body)`，用于跨进程恢复。`part_number` 改为强类型。
- `MultipartUpload`：`client.upload_file(b, k, path).part_size(..).concurrency(..).cleanup(..).metadata(..).cancel_on(signal)` 返回可检查进度的会话，或者直接 `.send()`（需要保留"失败后可以检查进度并手动恢复"的能力，形态由任务定并写进 rustdoc）。
- `Downloader`：`client.download_file(b, k, path).range(..).part_size(..).concurrency(..).resume(..).rate_limiter(..).send()`，或者保留 `Downloader` 作为可复用的配置对象，但入口统一为 builder。HEAD 改为复用 `head_object`（read_only，可重试），404 映射为 NotFound。
- 两个管理器的选项命名、校验时机（统一在 builder 设置或 `send()` 时校验）和错误 kind 保持一致。
- fds_cli（`src/transfer.rs`）以及 tests（`multipart_upload.rs`、`manager.rs`、`download_safety.rs`、`retries.rs`、`object_builders.rs`、`upload_bodies.rs` 相关部分）、examples（`upload_multipart.rs`、`download.rs`、`download_memory.rs`、`upload_verify.rs`）同步修改。

**不做：** 下载的 IO 实现（downloader-io 已经完成）；返回值字段类型化。

- 并行说明（revision 4）：本任务与另外两个 fluent 迁移任务并行执行。共享的 tests、examples、fds_cli tests、README 和 wire 快照文件可能同时被改：只改与本任务操作相关的调用点和内容，往 wire 快照里追加条目时不要改动其他条目，便于协调者在合并时解决冲突。

## 对应验收标准

AC-4（分片与管理器部分），AC-7、AC-8（本任务范围内）。

## 验收条件与验证方法

- [ ] 分片相关的 part number 用强类型保证范围，运行时检查删除或只保留上限检查。
- [ ] 上传和下载管理器的入口和选项风格统一，没有位置参数的 options 构造函数。
- [ ] Downloader 的 HEAD 请求复用 head_object：配置重试后 HEAD 遇到 503 会重试（新增测试）；对象不存在时返回 `is_not_found()`。
- [ ] 分片上传的"失败后检查进度，手动恢复"能力保留，有测试覆盖。
- [ ] `download_safety.rs` 和 `multipart_upload.rs` 的安全语义测试全部通过。
- [ ] fds_cli 的上传和下载命令行为不变。

## 执行记录

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/fluent-multipart-manager`，用 `git log --grep` 查。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul fluent-multipart-manager

推进 Roadmap sdk-overhaul 的任务 fluent-multipart-manager：分片上传与 Downloader 迁移到流式 builder。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/fluent-multipart-manager.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：crates/fds/src/object/multipart.rs 改为 create_multipart_upload builder + 会话对象（upload_part/complete/abort），part_number 改为强类型；crates/fds/src/multipart.rs 的 MultipartUpload 和 manager/download.rs 的 Downloader 统一为 builder 入口和一致的选项命名；Downloader 的 HEAD 复用 head_object；同步 fds_cli/src/transfer.rs、相关 tests 和 examples
验证：新增 Downloader 的 HEAD 重试与 404 测试；分片恢复测试；fds_cli 传输命令测试；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
