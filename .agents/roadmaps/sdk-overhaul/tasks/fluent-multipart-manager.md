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

2026-11-07，分支 `feat/sdk-overhaul-fluent-multipart-manager`（worktree `.worktrees/fluent-multipart-manager`），基于 main `088995a`，与 fluent-bucket、fluent-listing-delete 并行。

**1. 先锁定线上字节。** 迁移前在旧 API 上给 `tests/wire_snapshot.rs` 追加（不改原有 helper 和用例）`multipart_and_transfer_operations_match_the_wire_snapshot`，fixture 是新文件 `tests/fixtures/wire_multipart_ops.txt`（不碰 `wire_object_ops.txt`），单独提交为 WIP 第一笔。新增 `capture_sequence`：每个连接回一个脚本化响应，按到达顺序记录多个请求，归一化方式与对象快照相同。覆盖：init（无/有 metadata）、upload_part（bytes、content_md5、客户端 MD5、stream、文件 body，upload ID 含 `/`、空格、`+`）、complete（两片、metadata + expires、空列表）、abort；托管上传成功的 init → part1 → part2 → complete（含 metadata），托管上传失败的 init → part 503 → abort；Downloader 的 HEAD + 两个 Range GET、范围下载的 HEAD + GET。不开 manager feature 时只比较前缀。迁移后只改测试里的调用写法，fixture 一字节未改（迁移提交中 `git diff 4c53f73 -- crates/fds/tests/fixtures/` 为空），四种 feature 组合和 MSRV 下都通过。

**2. 手动分片（`fds::multipart`，定义在 `object/multipart.rs`）。**
- `client.create_multipart_upload(b, k).metadata(m).timeout(d).send() -> MultipartUploadSession`（`CreateMultipartUpload`）。会话绑定请求的 bucket/object，不用服务端回显的名字；响应缺少 upload ID 时为 `OutcomeUnknown`（原来只有托管上传检查，手动 init 返回空 ID）。
- `MultipartUploadSession`：私有字段 + `bucket()`/`object()`/`upload_id()`，`Clone`；`.upload_part(n, body)`、`.complete(parts)`、`.abort()` 返回与 Client 方法相同的 builder。
- 不依赖会话的入口（跨进程恢复）：`client.upload_part(b, k, upload_id, n, body).content_md5(..).timeout(..).send() -> UploadPartResponse`（`UploadPart`）、`client.complete_multipart_upload(b, k, upload_id, parts).metadata(..).expires(..).timeout(..).send() -> PutObjectResponse`（`CompleteMultipartUpload`）、`client.abort_multipart_upload(b, k, upload_id).timeout(..).send()`（`AbortMultipartUpload`）。
- part number 改为 `NonZeroU16`，`send` 时只保留上限检查（`MAX_PART_NUMBER = 10_000`，公开常量），失败为 `InvalidInput` + `NotSent`。`UploadPartResponse.part_number` 仍是 `i32`（返回值字段类型属 typed-responses）。
- init 响应改为 crate 内部的 lenient 结构（只读 `uploadId`）；`response_models.rs` 中它的用例移到 `object/multipart.rs` 的单元测试 `init_response_is_lenient`。
- 删除：`InitMultipartUploadRequest`、`InitMultipartUploadResponse`（公开类型）、`UploadPartRequest`、`CompleteMultipartUploadRequest` 及其 `send(&client)`；`InitMultipartUploadResponse::part/complete/abort(&client)`；`Client::init_multipart_upload(&req)`、`upload_part(req)`、`complete_multipart_upload(&req)`、`abort_multipart_upload(&init)` 的旧签名。

**3. 托管上传（`fds::multipart`）。**
- `client.upload_file(b, k, path).part_size(u64).concurrency(usize).cleanup(CleanupPolicy).metadata(m).start() -> MultipartUpload`（`UploadFile`），然后 `upload.upload(cancel)`。
- 形态决定：保留"会话 + 运行一次"两步。`dropping_future_keeps_session...` 等安全语义要求 future 被丢弃后仍能读进度，所以会话必须在运行之外存活；取消信号仍按 public-surface 的决定作为 `upload(cancel)` 的参数（`Future<Output = ()>`，可以是借用 token 的 `token.cancelled()`，放进会话会引入生命周期参数），没有做成 builder 上的 `cancel_on`。终结方法叫 `start()` 而不是 `send()`，因为它返回待运行的会话而不是上传结果；约定写进 `lib.rs` 的 "Operation builders"。
- 恢复能力写进 `MultipartUpload` 的 rustdoc（"Recovering after a failure"一节，含示例）：`progress().session` 就是 `MultipartUploadSession`，按状态补片（第 n 片 = 文件偏移 `(n-1) × part_size`）、complete 或 abort；`MultipartProgress.upload` 改名为 `session`。
- 选项校验移到 `start()`：名字 → 选项（每片 1 B–64 MiB、并发 1–64、乘积 ≤ 256 MiB）→ 打开文件 → 非空、≤ 10000 片 → init。选项错误不打开文件、不发请求。
- 删除：`MultipartOptions`（含位置参数的 `MultipartOptions::new`）、`MultipartUpload::start(&client, request, path, options)`。

**4. Downloader（`fds::manager`）。**
- `client.download_file(b, k, path).range(r).part_size(u64).concurrency(usize).resume().request_limiter(l).send()`（`DownloadFile`）；`resume()` 按约定是无参开关。默认 1 MiB、4 并发、不续传。名字和选项在 `send()` 中、加锁和发请求前校验（`InvalidInput` + `NotSent`），与托管上传统一。
- HEAD 复用 `object::basic` 新抽出的 `head_request()`（`HeadObject`/`ObjectExists` 也改用它，行为不变）：只读、按 RetryPolicy 重试；下载只接受 200，404 由统一的状态映射得到 `NotFound`（带 `ResponseDetails`），`is_not_found()` 为真。线上字节不变（快照覆盖）。
- `request_limiter` 不再接收 governor 类型：新增 SDK 自己的 `RequestLimiter::per_second(NonZeroU32)`（突发上限等于速率，克隆共享预算，手写 `Debug`），内部仍用 governor。理由：公开签名里出现 `governor::DefaultDirectRateLimiter` 会把 governor 的版本绑进 SDK 公开 API（应用必须用同一个 0.6），也违反"不暴露第三方类型"。fds_cli 因此不再直接依赖 governor（`Cargo.lock` 中 fds_cli 的依赖列表少一行，没有版本变化）。只提供 `per_second` 一个构造函数，足够覆盖 CLI 的 `--requests-per-second`。
- 下载 IO 实现未改：内部 `Downloader`（私有）保留执行逻辑，只把入参从 `&DownloadRequest` 改为 bucket/object/range/path。
- 删除：`Downloader`（公开）、`DownloadOptions`、`DownloadOptionsBuilder`、`DownloadRequest`、`DownloadRequest::send(&Downloader)`。
- 两个管理器都没有单请求 `timeout`（它们发很多请求，依赖连接和空闲超时），在 `lib.rs` 约定里写明这个例外。

**5. 测试。** 新增：`download_safety.rs::head_is_retried_and_missing_object_is_not_found`（无重试时 HEAD 503 直接返回 ServerError；配置 RetryPolicy 后 HEAD 503 被重试、下载完成；404 时 `is_not_found()`、状态 404、不发 GET、目标不变、无锁）、`invalid_options_fail_before_lock_and_network`；`multipart_upload.rs::failed_run_is_recovered_manually_from_progress`（第 2 片失败 → 进度为 Failed、1 份回执、再次 `upload` 被拒 → 用 `progress.session` 补第 2 片、按序 complete；再用持久化的 b/k/upload_id 走 `client.upload_part`（片号 10000）和 `client.abort_multipart_upload`）、`multipart_builders_validate_before_sending`（12 个用例：空名、空 upload ID、片号 10001、零 timeout 都是 `InvalidInput` + `NotSent`）、`initiation_without_upload_id_is_outcome_unknown`；`resource_limits_and_empty_file_fail_before_network` 扩展为"选项先于文件校验"。`download_safety.rs` 的 `request_preserves_non_utf8_path_without_lossy_conversion`（访问已删除的 pub 字段）改为实际下载到非 UTF-8 文件名：成功则文件名逐字节一致，文件系统拒绝时为 Io，且不会出现有损转换后的文件名（macOS 上走 Io 分支）。`limiter_controls_part_request_admission` 改用 `RequestLimiter::per_second(1)`，断言 ≥ 900 ms。其余 `download_safety.rs`、`multipart_upload.rs`、`manager.rs`、download.rs 单元测试的安全语义断言不变，只改构造写法。`object_builders.rs`、`retries.rs`、`upload_bodies.rs`、`pagination_options.rs`、`response_models.rs` 只改分片相关的调用点。

**6. fds_cli、examples、文档。** `transfer.rs`：download 改用 `download_file`，`--requests-per-second` 用 `RequestLimiter::per_second`；`multipart init/part/complete/abort` 改用新入口（part/complete/abort 直接传 `--upload-id`），`--number` 仍是 clap 的 `i32 range(1..)`，再转 `NonZeroU16`（>65535 时给出与 SDK 相同的 InvalidInput 信息）；`multipart upload` 改用 `upload_file(..).start()`。`smoke.rs` 只改下载和分片两处调用（以及不再需要的 `manager::*`/`multipart::*` glob 导入）。examples：`upload_multipart.rs`、`download.rs`、`download_memory.rs`、`migration.rs`；`upload_verify.rs` 不涉及分片或 Downloader，未改。README 的依赖说明（去掉 governor）、feature 表、手动分片、托管上传、并发下载、限流四节以及两处 Downloader 提及；`docs/runtime.md` 的重试范围、分片会话与恢复、下载文件与断点；`docs/open_api/README.md` 的分片四行与 Download manager 行。README "请求与所有权" 一节仍是旧的 `Request::new(...).send(&client)` 通用描述（其中提到 `upload_part`），涉及多个任务，留给协调者或 docs-release 统一改。`docs/migration.md` 按 roadmap 在 docs-release 删除，未改。

**7. CLI 行为差异（如实记录）。** 命令行参数、stdout 格式和退出码不变，`fds_cli` 测试全部通过。
- 返工（协调者按 AC-7 要求）：第一版把 `multipart upload` 的选项校验交给 SDK 的 `start()`，导致非法 `--part-size`/`--concurrency` 时先创建了 `--recovery` 文件。现已恢复原顺序：`transfer.rs` 的 `validate_upload_options` 在创建 recovery 文件前做本地校验，规则和错误（`InvalidInput`，"invalid multipart resource limits"）与 SDK 相同，乘法用 u64，避免 32 位平台溢出。单元测试 `transfer::tests::upload_limits_match_the_sdk` 对 8×8 组边界值比对 CLI 预检和 SDK `start()` 的结论（源文件不存在：选项非法得 InvalidInput，合法得 Io，均不发请求），防止两边规则漂移。CLI 测试 `invalid_managed_upload_options_fail_before_the_recovery_file`：5 组非法选项都以 1 退出，stderr 含原错误信息，不创建 recovery 文件，不发请求。临时去掉预检后该测试失败（"recovery file created for 0 x 1"），恢复后通过。
- 仍保留、等用户决定的差异：`multipart init` 收到不含 uploadId 的响应时，原来打印空 `uploadId` 并以 0 退出，现在报 OutcomeUnknown 并以 1 退出。

**8. 依赖。** fds_cli 去掉直接依赖 governor；Cargo.lock 只少了 fds_cli 依赖列表中的 `"governor"` 一行，没有新增或升级。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/fluent-multipart-manager`，用 `git log --grep` 查。

- 状态：实现与验证完成，分支上是 WIP 提交（第一笔只有旧 API 下的分片/管理器 wire 快照，第二笔是迁移），等待协调者压成一笔并按顺序与另两个并行任务合并。
- `bash scripts/check.sh`（2026-11-07，本 worktree，退出码 0）：fmt；clippy `--workspace --all-features --all-targets -D warnings`；四种 feature 组合的 `cargo test -p fds -- --test-threads=1` 和 `cargo check --all-targets`；fds_cli test 和 check；`cargo doc -D warnings`；`fds_cli --help`；MSRV 1.86.0 下三种组合的 test/check 及 fds_cli check；`check_coverage.py`（35/35 YAML、33/33 操作）；`check_docs.py`（114 个本地链接，retired 扫描通过）；`git diff --check`。日志中有 149 个 `test result: ok`，没有失败，也没有 warning / error 行（第一次完整运行时，无 manager feature 的组合里 `wire_snapshot.rs` 出现了 `tmp_dir` 未使用的 warning，加上 `#[cfg(feature = "manager")]` 后又完整重跑了一遍）。新增的 wire、HEAD 重试/404、手动恢复测试在 7 轮（stable 4 种 + MSRV 3 种）中都运行并通过。
- 返工后（协调者 rebase 到含 fluent-bucket、fluent-listing-delete 的 main，HEAD `91d2d6a` 之上修 CLI 选项校验顺序）再次完整运行 `bash scripts/check.sh`，退出码 0，156 个 `test result: ok`，无失败、无 warning / error 行；新增的 `upload_limits_match_the_sdk` 与 `invalid_managed_upload_options_fail_before_the_recovery_file` 都运行并通过。
- wire 字节一致：`FDS_UPDATE_WIRE_SNAPSHOT=1` 只在旧 API 上运行过一次生成 `wire_multipart_ops.txt` 并单独提交；迁移后不带该变量运行，两个 fixture 都无改动。
- 共享文件（并行合并时需要注意）：`tests/wire_snapshot.rs`（只在文件末尾追加）、新 fixture `tests/fixtures/wire_multipart_ops.txt`；`tests/object_builders.rs`（`multipart_helpers_bind_identity_and_preserve_receipts` 与 `direct_client_entry_points_validate_before_transport` 中分片两项）、`tests/pagination_options.rs`（complete 一段）、`tests/response_models.rs`、`tests/retries.rs`、`tests/upload_bodies.rs`、`tests/download_safety.rs`、`tests/manager.rs`、`tests/multipart_upload.rs`；examples 四个；`fds_cli/src/smoke.rs`（下载、分片两处与 glob 导入）、`fds_cli/Cargo.toml`、`Cargo.lock`；`README.md`、`docs/runtime.md`、`docs/open_api/README.md`；`crates/fds/src/lib.rs`（模块布局两行、"Operation builders" 首段与新增一条）；`crates/fds/src/object/basic.rs`（抽出 `head_request`）和 `object/mod.rs`（一行 `pub(crate) use`）。
- 未验证：`tests/live_acceptance.rs` 的 live 测试（需要真实凭证，由用户本地执行）；fds_cli `smoke` 命令需要真实 FDS，未运行（只改了调用写法，编译通过）。
- 依赖变化：fds_cli 不再直接依赖 governor（Cargo.lock 少一行）。
- 待用户决定：`multipart init` 收到不含 uploadId 的响应时以 1 退出（原来以 0 退出并打印空 ID），见执行记录第 7 条；协调者正在等用户决定。
- 协调者复验：rebase 到含 fluent-bucket、fluent-listing-delete 的 main 时，lib.rs、migration.rs、pagination_options.rs、docs/open_api/README.md 出现冲突，保留两边的迁移结果解决。之后编译无告警，完整运行 `bash scripts/check.sh`，退出码 0；确认分片 wire fixture 迁移前后不变，其他 fixture 未被改动。按协调者返工要求，恢复了"非法选项不创建 recovery 文件"的 CLI 行为。`multipart init` 收到不含 uploadId 的响应时报错退出，按用户决定（revision 5）作为 AC-7 的例外接受。

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
