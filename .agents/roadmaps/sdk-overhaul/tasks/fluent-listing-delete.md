# 列举与批量删除迁移到流式 builder

- Task key：fluent-listing-delete（Roadmap sdk-overhaul）
- 类型：feat
- 状态、依赖：以 plan.json 为准；分支 `feat/sdk-overhaul-fluent-listing-delete` 存在即表示进行中

## 目标与范围

**问题：**

1. `ListObjectsRequest` 是 `pub` 字段加 `Default` 的结构体；`max_keys: i32` 靠运行时检查正数（`object/listing.rs:47`、`:198`），应该用 `u32` 或 `NonZeroU32`；`pages(self, &Client)` / `objects(self, &Client)` 需要传 client 进去。
2. `delete_objects(bucket, names: &[String], put_to_trash: bool)`（`object/delete.rs:82`）：
   - bool 参数在调用处看不出含义；
   - `&[String]` 太死板，调用方常常手里是 `Vec<&str>` 或者迭代器；
   - 部分失败当作 `Err(DeleteObjectsError)` 返回。aws-sdk-s3 的 `DeleteObjects` 把部分失败作为 `Ok` 的输出（输出里带 `errors` 列表），因为部分失败是这个操作的正常结果之一。
3. `delete_objects_with_prefix(&ListObjectsRequest, bool)` 自己重新实现了分页和循环检测（HashSet 版），没有复用 `pages()`；而且接收整个 `ListObjectsRequest`，`delimiter` 和 `max_keys` 等字段对删除有什么影响，接口上看不出来。
4. `DeleteObjectFailure` 的 `Debug` 输出是 `"(details redacted)"`，错误码和描述都被隐藏，排查困难（object name 也许敏感，但 error_code 不敏感）。

**要做：**

- `client.list_objects("bucket").prefix(..).delimiter(..).max_keys(NonZeroU32 或 u32).marker(..)`，提供 `.send()`（单页）、`.into_pages()`（Stream<Item = Result<ObjectListing>>）、`.into_objects()`（Stream<Item = Result<ObjectSummary>>）。
- `client.delete_objects("bucket").keys(impl IntoIterator<Item = impl Into<String>>).trash(Trash::Enabled 或 Disabled)`（或者 `.move_to_trash(bool)` 一类意义清楚的方法），`send()` 返回 `Result<DeleteObjectsOutput>`：`deleted`、`failures`、`unconfirmed` 都放在 Ok 里；只有整个批次没发出去或无法判定时才返回 Err（用 error-model 的 dispatch 判断）。
- 注意（internal-cleanup 交接）：分页循环检测已抽成 `object/listing.rs` 的 `MarkerGuard`，有 `new()`（Brent，供 `pages()`）和 `exact()`（记住全部 marker，供删除）两种模式。删除流程改为复用 `into_pages()` 时，必须保留 `exact` 语义：重复的 marker 要在提交该页删除前立即拒绝（`tests/head_delete.rs::prefix_long_cycle_and_initial_marker_are_checked_before_delete`），否则同一页的删除可能被重复提交。
- `client.delete_prefix("bucket", "prefix").trash(..).page_size(..)`：基于 `into_pages()` 实现，只使用 bucket 和 prefix（去掉对 delimiter 的隐式依赖）；中途出错时，已经确认的结果要能拿到（作为错误上下文，或者通过一个"部分完成"的输出返回，由任务决定并在 rustdoc 写清楚）。
- `DeleteObjectFailure` 的 Debug 输出 error_code，object name 是否输出由任务决定并说明理由。
- fds_cli（`src/object.rs` 中的 ls / rm / rm --prefix 等命令）以及 tests（`pagination_options.rs`、`head_delete.rs` 等）、examples（`pagination.rs`、`copy_prefix.rs`）同步修改。

**不做：** ObjectSummary 等模型的字段类型化（typed-responses 负责）。

- 并行说明（revision 4）：本任务与另外两个 fluent 迁移任务并行执行。共享的 tests、examples、fds_cli tests、README 和 wire 快照文件可能同时被改：只改与本任务操作相关的调用点和内容，往 wire 快照里追加条目时不要改动其他条目，便于协调者在合并时解决冲突。

## 对应验收标准

AC-4（列举与删除部分），AC-6（删除的部分失败作为 Ok 输出），AC-7、AC-8（本任务范围内）。

## 验收条件与验证方法

- [ ] `max_keys` 用类型系统保证为正数，相关运行时检查删除。
- [ ] 删除的部分失败返回 `Ok(output)`，`output.failures` 非空；批次没发出（NotSent）返回 Err，kind 为 InvalidInput 或 Transport；发出后结果未知返回 `OutcomeUnknown`，或在 Ok 里给出 `unconfirmed`（二选一，rustdoc 写清楚）。
- [ ] `delete_prefix` 复用分页 Stream，循环 marker、缺失 marker、越出 prefix 的条目等现有防护测试全部保留并通过。
- [ ] wire 测试证明 list 和 delete 的请求逐字节一致（包括查询参数顺序和 enableTrash 的值）。
- [ ] fds_cli 的删除和列举命令输出不变。

## 执行记录

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/fluent-listing-delete`，用 `git log --grep` 查。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul fluent-listing-delete

推进 Roadmap sdk-overhaul 的任务 fluent-listing-delete：列举与批量删除迁移到流式 builder。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/fluent-listing-delete.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：crates/fds/src/object/listing.rs 改为 client.list_objects(bucket) builder，提供 send/into_pages/into_objects，max_keys 改为 NonZeroU32；object/delete.rs 的 delete_objects 改为 keys(IntoIterator)+trash 选项，部分失败作为 Ok 输出，基于 dispatch 判定；delete_prefix 复用 into_pages；同步 fds_cli/src/object.rs、pagination_options.rs、head_delete.rs、examples
验证：部分失败、NotSent、OutcomeUnknown 三种分支的测试；分页防护测试；wire 逐字节对比；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
