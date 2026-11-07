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

2026-11-07，分支 `feat/sdk-overhaul-fluent-listing-delete`（worktree `.worktrees/fluent-listing-delete`），基于 main `088995a`。与 fluent-bucket、fluent-multipart-manager 并行（revision 4）。

**1. 先锁定线上字节。** 迁移前在旧 API 上往 `crates/fds/tests/wire_snapshot.rs` 末尾追加一节（`serve_one`、`capture_all`、`listing_delete_requests`、测试 `listing_and_bulk_delete_match_the_wire_snapshot`），写入新的 fixture `tests/fixtures/wire_listing_delete.txt`，单独提交（WIP 第一笔）。没有改动已有的单对象快照代码和 `wire_object_ops.txt`；新 fixture 独立成文件，便于与另外两个并行任务合并。分页操作会发多个请求，`capture_all` 按顺序为每个请求接受一个新连接（应答带 `Connection: close`），记录请求行、按发送顺序的全部头和原始 body，归一化规则与单对象快照相同。覆盖：list 默认值（`delimiter=&maxKeys=1000&prefix=`）、全部选项（含需编码的 prefix 与 marker，体现查询参数排序）、空 marker（发送 `marker=`）、两页 `pages` 的续页请求、从 marker 开始的 `objects`（maxKeys=2）、`delete_objects`（`enableTrash=false`，含重复名与需编码名的 JSON body）、`delete_objects` trash（`enableTrash=true`）、前缀删除两页（list → delete → list → delete）、前缀删除 trash + 页大小 2。迁移后只改测试中的调用写法，fixture 一字节未改（迁移提交中 `wire_listing_delete.txt` 无改动），测试通过。

**2. 迁移后的公开 API（`fds::object`，沿用 `lib.rs` 的 "Operation builders" 约定和 `timeout_option!`、`ClientRequest::with_timeout`）。**
- `client.list_objects(bucket) -> ListObjects`：`.prefix(..)` `.delimiter(..)` `.max_keys(NonZeroU32)` `.marker(..)` `.timeout(d)`；`.send() -> Result<ObjectListing>`（单页）、`.into_pages() -> impl Stream<Item = Result<ObjectListing>> + Send + 'static`、`.into_objects() -> impl Stream<Item = Result<ObjectSummary>> + Send + 'static`。builder 持有 Client，流不再借用 client；timeout 对每页请求分别生效。`max_keys` 由类型保证非零，原来的运行时检查和 "max_keys must be positive" 错误已从 SDK 删除；`DEFAULT_LIST_OBJECTS_MAX_KEYS` 改为 `NonZeroU32`（`Option::unwrap` 在 const 中自 1.83 稳定，满足 MSRV 1.86）。
- `client.delete_objects(bucket) -> DeleteObjects`：`.keys(impl IntoIterator<Item: Into<String>>)`（追加，可多次调用，重复名原样保留）、`.move_to_trash()`（按约定是无参开关，不用 bool；不调用时发送 `enableTrash=false`）、`.timeout(d)`；`.send() -> Result<DeleteObjectsOutput>`。
- `client.delete_prefix(bucket, prefix) -> DeletePrefix`：`.page_size(NonZeroU32)`（即 maxKeys，默认 1000）、`.move_to_trash()`、`.timeout(d)`（每个 list / delete 请求分别生效）；`.send() -> Result<DeleteObjectsOutput>`。只用 bucket 和 prefix 选择对象：不带 delimiter、不接受起始 marker。
- 结果判定（rustdoc 写在 `DeleteObjects::send`、`DeletePrefix::send`、`DeleteObjectsOutput`、`Error::partial_delete_output` 上）：
  - `DeleteObjects`：服务给出合法失败数组即 `Ok`，`failures` 是逐项失败、`deleted` 是其余名字，`unconfirmed` 在 `Ok` 中恒为空。本地校验或发送前失败返回原错误（`Dispatch::NotSent`，不附带结果）。可能已发出之后的任何失败（错误状态、响应损坏或非法、失败项不在请求中）返回 `ErrorKind::OutcomeUnknown`，原因在 `source()`，`partial_delete_output().unconfirmed` 是整批名字。验收条件中的"二选一"选的是 OutcomeUnknown 错误：把 403、5xx 之类变成 `Ok` 会让调用方误以为删除已执行。空批次不请求、不做任何校验，返回空 `Ok`（保留原语义和测试）。
  - `DeletePrefix`：基于 `ListObjects::into_pages()` 实现，列一页、校验、删一页，再拉下一页（流是惰性的，请求顺序与原实现相同）。全部确认删除才返回 `Ok`；遇到第一个问题即停止，错误的 `partial_delete_output()` 带有此前所有页已确认的结果。
- **`MarkerGuard::exact` 语义保留**：`ListObjects` 新增 crate 内部开关 `exact_markers()`，`into_pages()` 据此选用 `MarkerGuard::exact`（否则 `MarkerGuard::new`），marker 检查发生在该页交给调用方之前，所以重复的 marker 在提交该页删除前就被拒绝。测试 `prefix_long_cycle_is_checked_before_delete` 构造 a→b→c→d→b：Brent 此时只记得 d 和 c，会放过重复的 b；把 `exact_markers()` 临时去掉后该测试失败（p/5 被删除），恢复后通过。原测试中的"初始 marker"部分不再适用于 SDK（`delete_prefix` 不接受 marker），改由 fds_cli 测试覆盖（见第 5 点）。
- **PartialFailure 的处理（保留，理由如下）**：单批 `delete_objects` 不再产生 `PartialFailure`。`ErrorKind::PartialFailure` 和 `Error::partial_delete_output()` 保留，只服务于 `delete_prefix`：某一批出现逐项失败时，`delete_prefix` 停止、不再列举后续页（与原行为一致），此时结果是不完整的（prefix 下还有没处理的对象），不能作为 `Ok` 返回，否则调用方无法区分"全部处理完"和"中途停下"；`partial_delete_output()` 同时也是 OutcomeUnknown 时取得 `unconfirmed` 的途径。两处 rustdoc 已改写为新的生产者范围。错误文本（`bulk deletion failed for some objects`，context 为该批的 deleted / failed 数量）保持不变。
- `DeleteObjectFailure` 的 `Debug` 改为 `DeleteObjectFailure { error_code: "AccessDenied", .. }`：error_code 只在形如普通标识符（≤64 个 ASCII 字母数字或 `_-.`）时输出，否则显示 `<withheld>`，与 `ResponseDetails` 对 service code 的过滤一致（把过滤函数提成 `error::printable_code` 共用）。object name 和 error_description 不输出：对象名是用户数据，常含人名、路径等，`Debug` 容易进日志；description 是服务端自由文本，可能回显对象名。需要时直接读字段。`DeleteObjectsOutput` 的 `Debug` 仍只输出三个计数。

**3. 删除的旧入口。** `ListObjectsRequest`（pub 字段 + `Default` 的结构体）及其 `new/prefix/delimiter/max_keys/marker/send(&client)/pages(&client)/objects(&client)`；`Client::list_objects(&ListObjectsRequest)`；`Client::delete_objects(bucket, &[String], bool)`；`Client::delete_objects_with_prefix(&ListObjectsRequest, bool)`。

**4. 测试。** `head_delete.rs`：单批部分失败的断言改为 `Ok`（含 Debug 不泄露 description 和对象名）；重复名测试的失败分支改为 `Ok`；`prefix_long_cycle_and_initial_marker_are_checked_before_delete` 改为 `prefix_long_cycle_is_checked_before_delete`（见上，echo 的 evil scope 与 page_size=7、trash 的查询串都精确匹配）；其余前缀删除防护测试（逐页保留进度、空页继续与部分失败即停、缺失 marker 与越出 prefix、重复 marker、调用方 scope、空名字、空 bucket）只改调用写法、断言不变。新增 `bulk_delete_outcomes_follow_dispatch`（连接被拒 → Transport + NotSent + 无结果；逐项失败 → Ok；503 → OutcomeUnknown + Sent + 不可重试 + 整批 unconfirmed）、`delete_builders_validate_before_sending`（DeleteObjects 的空 bucket / 空 key / 零 timeout，DeletePrefix 的空 bucket / 零 timeout，都在发送前返回 InvalidInput + NotSent；endpoint 无人监听，发出去会是 Transport）、`delete_failure_debug_shows_only_plain_error_codes`。`pagination_options.rs`：调用改写，`max_keys(0)` 的运行时检查不复存在，换成 ListObjects 的空 bucket、零 timeout（`send` 与 `into_objects` 两条路径）都是 InvalidInput + NotSent。`object_builders.rs`、`error_model.rs`、`wire.rs`、`live_acceptance.rs` 只改调用写法（`object_builders.rs` 中 `max_keys(0)` 一项换成空 bucket）。`head_delete.rs` 的 bucket 校验测试里去掉了不再需要的 `object::*` 导入（clippy `-D warnings`）。

**5. fds_cli（命令参数、输出、退出码不变）。**
- `object list`：改用 builder；`--max-keys` 仍是 `i32`，由 `positive_max_keys` 转为 `NonZeroU32`，非正数报与原来相同的 `InvalidInput: max_keys must be positive`；仍总是发送 marker（未给时为空串），线上请求不变。
- `object delete-many`：改用 builder。SDK 现在把逐项失败放进 `Ok`，`output::delete_outcome` 遇到非空 `failures` 时仍按错误格式输出 `{"output","unconfirmed","not_submitted"}`、以同样的 `PartialFailure` 文本退出 1。
- `object delete-prefix`：命令有 `--delimiter`（只删当前层，`docs/cli.md` 有记载）和 `--marker`，且原实现总是发送 `marker=`；而 SDK 的 `delete_prefix` 按任务要求只用 bucket 和 prefix。为保持命令行为和线上请求不变，CLI 在 `object.rs` 里自己分页（`list_objects(..).send()` + `delete_objects`），沿用同样的防护：名字非空且在 prefix 内，truncated 页的 nextMarker 非空且与本次所有 marker（含起始 marker）不同，否则不删该页。这是暂定做法，见"待用户决定"。
- `smoke`：list 与 cleanup 删除改用 builder；cleanup 出现逐项失败时仍报失败（输出文本与原来相同）。
- 新增 `commands.rs::delete_prefix_keeps_listing_flags_and_stops_on_a_repeated_marker`（`--delimiter / --marker start / --max-keys 7 / --trash`，第二页回到起始 marker 时不删除）。
- 另外用迁移前的提交 `4f42bad` 构建旧二进制，与新二进制在同一组固定应答（Python 本地 HTTP 桩）下逐一对比 25 个场景的退出码、stdout、stderr 和收到的请求（方法、路径含查询串、body），全部一致：delete-many 成功 / trash 部分失败 / 500 / 失败项不在请求中 / 非法 body / 空名字 / 空 bucket；delete-prefix 两页成功 / 空页 / 第二页部分失败 / 带全部 flag 回到起始 marker / 长循环 / 缺 marker / 越出 prefix / 空名字 / 第二页列举 500 / 删除 500 / `--max-keys 0` / 空 bucket；list 单页 / `--all` 两页 / marker 循环 / 403 / `--max-keys -1` / `--diagnostics --max-keys 0`。已知未覆盖的边角：空 bucket 同时 `--max-keys 0` 时，旧版先报 bucket 错误，新版先报 max_keys 错误。

**6. 文档。** README 的列举与分页、单对象/批量/前缀删除两节和错误类型表中的 `PartialFailure` 行；`docs/runtime.md` 的分页与批量删除一节；`docs/open_api/README.md` 的映射列；`lib.rs` 的 "Operation builders" 首段（列举、批量删除已迁移）和 `object` 模块文档。改过的 README 代码片段用临时 example 编译检查通过后删除了该文件。`docs/cli.md` 不变（CLI 行为不变）；`docs/migration.md` 留给 docs-release 删除，未改。

**7. 依赖。** 无变化，Cargo.lock 未改。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/fluent-listing-delete`，用 `git log --grep` 查。

- 状态：实现与验证完成，分支上是 WIP 提交（第一笔只有旧 API 下的 wire 快照，第二笔是迁移），等待协调者合并。
- `bash scripts/check.sh`（2026-11-07，本 worktree，退出码 0）：fmt；clippy `--workspace --all-features --all-targets -D warnings`；四种 feature 组合的 `cargo test -p fds -- --test-threads=1` 和 `cargo check --all-targets`；fds_cli test 和 check；`cargo doc -D warnings`；`fds_cli --help`；MSRV 1.86.0 下三种组合的 test/check 及 fds_cli check；`check_coverage.py`（35/35 YAML、33/33 操作）；`check_docs.py`（114 个本地链接，retired 扫描通过）；`git diff --check`。日志中有 149 个 `test result: ok`，没有失败，也没有 warning / error 行。`listing_and_bulk_delete_match_the_wire_snapshot`、`prefix_long_cycle_is_checked_before_delete`、`delete_builders_validate_before_sending` 在 7 轮（stable 4 种组合 + MSRV 3 种）中都运行并通过。
- wire 字节一致：`FDS_UPDATE_WIRE_SNAPSHOT=1` 只在旧 API 上运行过一次生成 `wire_listing_delete.txt` 并单独提交；迁移后不带该变量运行，两份 fixture 都无改动。
- 共享文件改动（供合并时解决冲突）：`tests/wire_snapshot.rs`（只在文件末尾追加一节）、新 fixture `tests/fixtures/wire_listing_delete.txt`、`tests/head_delete.rs`（bucket 校验测试里只动了 `object::*` 导入和 `delete_prefix` 一行）、`tests/pagination_options.rs`（只动列举部分）、`tests/object_builders.rs`、`tests/error_model.rs`、`tests/wire.rs`、`tests/live_acceptance.rs`、`fds_cli/tests/commands.rs`（末尾追加一个测试）、`fds_cli/src/output.rs`（只动删除结果输出）、`fds_cli/src/smoke.rs`（list 与 cleanup 删除两处）、examples（quickstart、migration、pagination、oauth_inventory、copy_prefix）、`README.md`、`docs/runtime.md`、`docs/open_api/README.md`、`crates/fds/src/lib.rs`（首段一句）、`crates/fds/src/error.rs`（PartialFailure / partial_delete_output 文档与 `printable_code`）、`crates/fds/src/constant.rs`（一个常量）、`crates/fds/src/object/mod.rs`（导出与模块文档首句）。
- 未验证：`tests/live_acceptance.rs` 的 live 测试（需要真实凭证，由用户本地执行；本任务只改了其中的列举调用写法，编译通过）；fds_cli `smoke` 需要真实 FDS，未运行。
- 依赖变化：无。
- 待用户决定：fds_cli `object delete-prefix` 的 `--delimiter` / `--marker` 与 SDK `delete_prefix` "只用 bucket 和 prefix" 的冲突。暂定做法是 CLI 自己分页（行为、输出、线上请求完全不变，但 CLI 里有一份与 SDK 相似的分页删除循环）。可选：(a) 保持暂定做法；(b) 给 `DeletePrefix` 增加显式的 `.marker(..)` / `.delimiter(..)` 选项，CLI 直接调用 SDK（SDK 接口偏离"只用 bucket 和 prefix"，但影响写在方法上，不再是隐式依赖）；(c) 删掉 CLI 的 `--delimiter` / `--marker`，CLI 直接调用 SDK（命令行参数变化，违背 AC-7，且不带 marker 时少发一个空的 `marker=`）。
- 协调者复验：rebase 到含 fluent-bucket 的 main 时，lib.rs、oauth_inventory.rs、head_delete.rs、live_acceptance.rs、wire.rs 出现冲突，都是两边各自迁移了同一处调用，保留两边的迁移结果解决。之后编译无告警，完整运行 `bash scripts/check.sh`，退出码 0；确认列举与删除的 wire fixture 迁移前后不变，其他 fixture 未被改动。CLI `delete-prefix` 的 `--marker`/`--delimiter` 处理方式已作为待答问题 `cli-delete-prefix` 交给用户决定，当前合入的是方案 (a)。

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
