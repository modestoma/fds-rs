# FDS Rust SDK 设计重构（0.1.0） 整体验收

- 基线分支：main
- 验收 revision：以 plan.json 的 acceptance 为准
- 证据草稿：由 docs-release 任务整理（2026-11-08），只汇总各任务执行记录与本任务的实际运行结果，不代替用户验收。测试名均已对照当前代码核对存在。

| AC | 实现入口 | 实际验证与证据 | 未验证项 / 遗留问题 |
| --- | --- | --- | --- |
| AC-1 | request-timeout、upload-idle-timeout | `tests/client_builder.rs` 的本地慢速 server 测试（默认无整体超时、空闲超时、单请求超时）；变异验证；docs-release 全量 check.sh | 未在真实 FDS 上跑超过 300 s / 50 s 的真实大对象传输，只用缩短阈值的 loopback 模拟；只在 macOS 上运行 |
| AC-2 | downloader-io（fluent-multipart-manager 迁移入口） | current_thread tick 检测测试、checkpoint 节流测试、既有 `download_safety` / `manager` 测试；变异前后对比 | 没有真实 FDS + 真实磁盘的性能基准 |
| AC-3 | error-model（后续任务沿用） | `tests/error_model.rs` 真实 HTTP 映射、dispatch、脱敏测试；`error.rs` 单元测试 | 真实服务的错误状态 / service code 形态没有 live 证据 |
| AC-4 | fluent-object、fluent-bucket、fluent-listing-delete、fluent-multipart-manager、delete-prefix-options | 四份 wire 快照迁移前后逐字节不变；各 builder 的发送前校验测试；命名检查；docs-release 的 rustdoc 公开项审计；12 个 examples 与 doctest 编译 | 需要用户评审 rustdoc（AC 的"用户"部分） |
| AC-5 | public-surface、cslb-internal | rustdoc 公开项审计（无重复路径、无内部模块）；clippy / `cargo doc -D warnings`；docs-release 复核 | 需要用户评审 rustdoc |
| AC-6 | typed-responses、object-metadata、fluent-listing-delete | 模型反序列化表驱动测试（转换、缺失、非法值）；`GetObjectOutput` 访问方法与适配器测试；presign golden；元数据新旧实现差分测试 | 真实服务响应字段（如 `maxKeys` 类型、系统 meta 取值格式）没有 live 证据 |
| AC-7 | 每个改动 fds_cli 的任务；例外见下文 | `cargo test -p fds_cli --all-features`（21 个用例）；typed-responses、object-metadata、cli-oauth-refresh 的新旧二进制对比 | fds_cli `smoke` 与真实 FDS 上的命令由用户本地执行；fds_cli 测试只在 stable 运行（MSRV 只做 check） |
| AC-8 | baseline-green、docs-release | docs-release 在 worktree 中完整运行 `bash scripts/check.sh`，结果见下文 | `tests/live_acceptance.rs` 由用户本地执行 |
| AC-9 | credentials-provider、idempotent-retry、cli-oauth-refresh | `tests/credentials_provider.rs`（12 个用例）、`tests/retries.rs`（8 个用例）、fds_cli OAuth 续期用例 | 真实 OAuth 服务上的 token 续期、真实服务对重复幂等写的行为没有 live 证据 |
| AC-10 | docs-release | `check_docs.py`（链接、退役 API 扫描、12 个示例均被 README 链接、文档中的依赖版本 = 0.1.0）；README 30 个 Rust 代码块临时编译通过；两个 crate 版本 0.1.0 | 需要用户评审 README、docs |

## 证据明细

### AC-1 大对象传输不被默认整体超时打断

- 实现：request-timeout 取消默认 300 s 整体超时（`request_timeout` 默认 `None`），新增单请求 `.timeout(Duration)`；upload-idle-timeout 让 `read_timeout` 成为按进度重置的空闲超时，覆盖上传 body、等待响应头、读响应 body 三个阶段（`crates/fds/src/idle.rs`）。
- 自动测试（`crates/fds/tests/client_builder.rs`）：`default_client_reads_slow_body_beyond_total_threshold`、`read_timeout_enforces_header_and_stream_body_stalls`、`total_timeout_expires_despite_continuing_body_progress`、`per_request_timeout_overrides_client_configuration`、`put_object_request_timeout_applies_while_awaiting_response`、`per_request_timeout_is_validated`、`progressing_upload_outlasts_idle_threshold`、`stalled_upload_times_out_with_unknown_dispatch`、`missing_answer_after_upload_times_out`；`idle.rs` 单元测试 `tracked_body_splits_chunks_and_resets_the_clock`。
- 变异验证（任务记录）：去掉单请求超时、给默认 Client 加 200 ms 整体超时、去掉上传进度重置、去掉 64 KiB 切片，对应测试都失败，验证后恢复。
- 未验证：真实网络上的长时间传输；Linux 上的 socket 缓冲差异（测试 body 16 MiB，远大于常见缓冲）。

### AC-2 Downloader 不阻塞 tokio runtime

- 实现：downloader-io 把文件 IO、fsync、恢复摘要、加锁与发布移入 `spawn_blocking`，只在发布前 `sync_all` 一次，checkpoint 按"首片 / 每 64 片 / 每 2 秒 / 失败时"节流。
- 自动测试：`tests/download_safety.rs::current_thread_runtime_keeps_ticking_during_download_and_resume`（原实现恢复阶段最大 tick 间隔 335 ms 失败，新实现约 2.5–3 ms，阈值 100 ms）、`throttled_checkpoint_still_resumes_exactly_the_missing_parts`；`manager/download.rs` 单元测试 `checkpoint_throttle_policy`、`checkpoint_writes_are_throttled`、`abandoned_part_writes_finish_before_tracker_wait_returns`；既有 `download_safety.rs`、`manager.rs` 测试未改动并通过（checkpoint 与 rename 语义不变）。
- 未验证：真实 FDS 与真实磁盘的性能基准（`scripts/benchmark_download.py` 只测本地 HTTP 源的 RSS）。

### AC-3 单一错误类型

- 实现：error-model 把 `ManagerError`、`MultipartUploadError`、`DeleteObjectsError` 合并进不透明的 `fds::Error`（`kind()`、`dispatch()`、`response()`、`is_not_found()`、`is_retryable()`、`is_outcome_unknown()`、`partial_delete_output()`）；idempotent-retry 让 `is_retryable()` 与重试循环共用判定（revision 8 决定）。
- 自动测试：`tests/error_model.rs` 的 `rejected_statuses_map_to_kinds`、`protocol_violations_are_classified`、`caller_and_configuration_errors_are_distinct`、`dispatch_tracks_whether_the_request_was_sent`、`debug_is_diagnostic_and_omits_credentials`、`error_stays_small_without_result_large_err`；`error.rs` 单元测试（状态码映射表、Display/Debug 脱敏、传输错误去 URL）；`tests/credentials_provider.rs::redaction_follows_the_credentials_each_request_used`、`debug_output_never_contains_credentials`。
- 未验证：真实服务的错误响应形态（映射测试使用本地 mock）。

### AC-4 每个操作只有一种流式 builder 写法

- 实现：fluent-object、fluent-bucket、fluent-listing-delete、fluent-multipart-manager 把全部操作迁移为 `client.<op>(必填).<可选>().send().await`，删除旧 `*Request` 结构体、`send(&client)`、`xxx_with_response`、`does_*` / `generate_*`；delete-prefix-options 增加显式 `.marker()` / `.delimiter()`（revision 5）。约定写在 `crates/fds/src/lib.rs` 的 "Operation builders" 一节。
- 线上字节不变：每个迁移任务先在旧 API 上生成 wire 快照（`tests/wire_snapshot.rs` 的 `single_object_operations_match_the_wire_snapshot`、`bucket_and_oauth_operations_match_the_wire_snapshot`、`listing_and_bulk_delete_match_the_wire_snapshot`、`multipart_and_transfer_operations_match_the_wire_snapshot`），迁移后 fixture 逐字节不变（协调者复验）。docs-release 因版本号回退把四个 fixture 中 `user-agent: fds-rs/0.2.0` 改为 `fds-rs/0.1.0`，其余字节未动。
- 发送前校验：`tests/object_builders.rs::every_object_builder_validates_names_and_timeout_before_sending`、`direct_client_entry_points_validate_before_transport`、`tests/bucket_builders.rs::every_bucket_builder_validates_bucket_and_timeout_before_sending`、`oauth_builders_validate_before_sending`、`tests/download_safety.rs::invalid_options_fail_before_lock_and_network`。
- 命名：fluent-object 检查 rustdoc 条目中没有 `ACL`、`URL`、`does_`、`generate_`、`send_with_response`；docs-release 复核见 AC-5 的公开项审计。
- examples 与 doctest：12 个 examples 在四种 feature 组合下由 `cargo check --all-targets` 编译，doctest 由 `cargo test` 运行（check.sh）。
- 用户待做：评审 rustdoc 与 examples。

### AC-5 公开面收缩

- 实现：public-surface 让 `constant`、`encoding`、`auth` 等实现模块私有，每个公开类型只保留一条导出路径（根只导出 `Client`、`ClientBuilder`、`ClientConfiguration`、`RetryPolicy`、`RetryMode`、`Error`、`ErrorKind`、`Dispatch`、`ResponseDetails`、`Result`），删除死代码；cslb-internal 把 `fds::cslb` 改为私有并删除不可达的负载均衡工具（revision 3）。
- 任务记录：public-surface 解析 `target/doc/fds/all.html`，公开项按名称去重无重复；cslb-internal 确认 rustdoc 中没有 `cslb` 模块与类型。
- docs-release 复核：见下文"docs-release 实际运行"中的公开项审计。
- 用户待做：评审 rustdoc。

### AC-6 返回值类型化

- 实现：typed-responses 把时间改为 `DateTime<Utc>`、大小改为 `u64`、可能缺失的字段改为 `Option`，`GetObjectOutput` 增加类型化访问方法、`into_stream()` / `into_async_read()`，删除 `into_raw()`，presign 返回 `url::Url`；object-metadata 把 `ObjectMetadata` 拆成 `attributes` 与 `user_metadata`；fluent-listing-delete 让单批删除的逐项失败作为 `Ok(output)` 返回。`PutObjectOutput` 按 revision 6 保留四个字段。
- 自动测试：`tests/response_models.rs`（`typed_fields_convert`、`optional_fields_distinguish_missing_from_empty`、`invalid_times_sizes_and_part_numbers_are_rejected`、`invalid_typed_values_are_protocol_violations`、`object_metadata_entry_list_splits_attributes_and_user_metadata` 等）；`tests/get_object_output.rs`（6 个用例）；`tests/golden.rs::golden_presigned_urls`；object-metadata 的新旧实现差分测试（20000 组随机响应头，临时代码未提交）。
- 未验证：真实服务上 `PutObjectOutput` 字段的出现情况、`maxKeys` 的实际 JSON 类型、系统 meta key 的取值格式。

### AC-7 fds_cli 行为不变（含例外）

- 自动测试：`cargo test -p fds_cli --all-features`，`crates/fds_cli/tests/commands.rs` 共 21 个用例，在每个任务和 docs-release 的 check.sh 中运行。
- 新旧二进制对比（任务记录，Python 固定应答服务）：typed-responses 4 套合法应答 68/68 一致，非法应答按例外改为报错；object-metadata 40 个场景中 35 个一致，5 个差异均为 revision 7 例外；cli-oauth-refresh 45 个场景 0 差异，`--help` 仅 `--oauth` 说明一行不同。
- 已批准的例外清单（决定记录）：
  1. revision 5：`multipart init` 收到不含 uploadId 的响应时，由打印空 uploadId 并以 0 退出改为报 OutcomeUnknown 并以 1 退出。
  2. revision 6：服务端返回负数大小、负数或超出范围的时间戳时报 ProtocolViolation 并以 1 退出（`multipart upload` 为 OutcomeUnknown / CompletionUnknown）；`multipart complete --parts` 文件中 partNumber 为 0、负数或大于 65535，或 partSize 为负数时，读文件阶段即报错退出。
  3. revision 7：`--meta` 不再接受既非系统头、也非 `x-xiaomi-meta-*` 的旧白名单 key（content-length、last-modified、last-checked、upload-time、date、authorization、range、content-range、x-xiaomi-multipart-upload-mode），报 InvalidInput 并以 1 退出；`--meta x-xiaomi-meta-=v`、`--meta x-xiaomi-meta-x-xiaomi-meta-a=v` 这类退化 key 由 SDK 报错（用例 `empty_and_prefixed_user_metadata_keys_fail_before_sending`）。同一 revision 还决定系统头传空值时不再发送空头（CLI 输出不变）。
  4. revision 8：`--oauth` 由 token 过期时报错退出改为自动续期（用例 `oauth_reexchanges_a_short_lived_token_during_a_long_listing`、`oauth_refresh_failure_stops_before_sending_and_exits_one`）。
- 其他已记录、非例外的差异：`--version` 输出随版本号变为 0.1.0；`--diagnostics` JSON 中的 `kind` 改为 ErrorKind 名（error-model 记录，退出码不变）。
- 未验证：fds_cli `smoke` 和各命令对真实 FDS 的运行（用户本地执行）；fds_cli 测试只在 stable 上运行。

### AC-8 check.sh 全部通过

- baseline-green 修复新工具链下的 clippy 失败后建立全绿基线；之后每个任务交付和协调者复验都完整运行过 check.sh（退出码 0，见各任务文档）。
- docs-release 的最终运行见下文。
- 未运行：`tests/live_acceptance.rs` 的 2 个 `#[ignore]` live 测试（需要真实凭证，用户本地执行），check.sh 中它们只编译不运行。

### AC-9 凭证刷新与幂等写重试

- 凭证：credentials-provider 新增 `fds::credentials`（`CredentialsProvider`、`StaticCredentials`、`StorageTokenProvider`），Client 在每次请求尝试前取凭据。测试 `tests/credentials_provider.rs` 的 12 个用例，含 `token_close_to_expiry_is_exchanged_before_the_next_request`、`first_request_exchanges_and_short_tokens_refresh_at_half_lifetime`、`concurrent_requests_share_one_exchange`、`failed_refresh_of_an_expired_token_never_sends_it`、`every_attempt_signs_with_the_credentials_current_at_that_attempt`。cli-oauth-refresh 让 `--oauth` 使用该 provider（CLI 用例见 AC-7）。
- 已知边界（cli-oauth-refresh 交付备注，已写入 lib.rs 与 README）：`StorageTokenProvider::with_token` 预置的 token 有效期未知，按完整提前量判断，不适用半寿命上限。
- 重试：idempotent-retry 新增 `RetryPolicy::retry_idempotent_writes()` 和单请求 `.retry(RetryMode)`；非幂等写没有 `.retry` 方法且永不重试。测试 `tests/retries.rs` 的 8 个用例，含 `writes_are_single_attempt_unless_idempotent_writes_are_enabled`、`per_request_mode_overrides_the_policy`、`writes_streams_and_success_decode_errors_are_never_replayed`、`idempotent_put_replays_the_same_bytes_and_uses_the_shared_rule`、`idempotent_write_is_retried_after_a_dropped_connection`、`delete_prefix_retries_only_its_listing`。HTTP 500 不重试等取舍见 revision 8。
- 未验证：真实 OAuth 服务的续期；真实服务对重复 put / delete / upload_part / set_metadata 的行为。

### AC-10 文档与版本号

- 版本：`crates/fds`、`crates/fds_cli` 均为 0.1.0，fds_cli 对 fds 的依赖同步，Cargo.lock 已更新。
- 文档：README 全部示例对应最终 API（30 个 Rust 代码块临时编成 example，`cargo build -p fds --all-features` 编译通过后删除）；`docs/runtime.md`、`protocol.md`、`validation.md`、`cli.md` 与最终行为对齐；`docs/migration.md` 已删除，引用已清理；crate 级 rustdoc 重写。
- `scripts/check_docs.py`：本地链接与锚点、退役 API / 文件扫描、12 个 examples 均被 README 链接、README 与 docs 中的依赖 / 安装版本等于 crate 版本。
- 用户待做：评审 README、docs 与 rustdoc。

## docs-release 实际运行

（见 `tasks/docs-release.md` 的执行记录，以下为摘要）

- `bash scripts/check.sh`（2026-11-08，docs-release worktree，最后一次代码与文档改动之后）：**退出码 0**。覆盖 fmt；clippy `--workspace --all-features --all-targets -D warnings`；四种 feature 组合（默认、`--no-default-features`、`--all-features`、`--no-default-features --features cslb`）的 `cargo test -p fds -- --test-threads=1` 与 `cargo check --all-targets`；`cargo test -p fds_cli --all-features` 与 check；`RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps`；`fds_cli --help`；MSRV 1.86.0 三种组合的 fds test/check 与 fds_cli check；`check_coverage.py`（35/35 YAML、33/33 操作）；`check_docs.py`（121 个本地链接、12 个示例已链接、文档版本 0.1.0、退役扫描通过）；`git diff --check`。日志中 170 个 `test result: ok`，0 个失败；被忽略的只有 live_acceptance 的 2 个 live 测试（每个跑 fds 测试的组合各 2 个）和 1 个本来就标 `ignore` 的私有宏 doctest。
- rustdoc 公开项审计（解析 `target/doc/fds/all.html`）：104 个公开项，按名称去重无重复；公开模块只有根、`acl`、`bucket`、`credentials`、`manager`、`multipart`、`oauth`、`object`，没有 `constant`、`encoding`、`auth`、`cslb`；没有 `ACL` / `URL` 命名、`*Request` 结构体、`HttpMethod` / `HttpRange` / `parse_range`；`Client` 的 64 个方法中没有 `does_*`、`generate_*`、`*_with_response`；所有 rustdoc 页面中没有 `reqwest::` 类型。
- 未运行：`tests/live_acceptance.rs`、fds_cli `smoke`（需要真实 FDS 凭证，用户本地执行）。

## 组合验证（协调者，2026-11-08）

19 个任务全部合入后，在 main 最新提交（`docs: align docs and examples with the final API; version 0.1.0`）上，于主工作目录完整运行 `bash scripts/check.sh`，退出码 0。日志中共 170 个 `test result: ok`，没有失败，也没有 warning 或 error 行。被忽略的用例如下：

- `live_object_roundtrip` 和 `live_oauth_discovery_and_read`：需要 `FDS_LIVE=1` 和真实凭证；
- `de::lenient` 的 doctest：私有宏，本来就标了 `ignore`。

自动检查已覆盖 AC-1 至 AC-3、AC-6、AC-8、AC-9 的自动部分，以及 AC-4、AC-5、AC-7、AC-10 的自动部分。

## 需要用户亲自确认的项

1. 评审 rustdoc：运行 `cargo doc -p fds --all-features --open`，确认 API 形态和命名（AC-4、AC-5）。
2. 评审 README 和 `docs/`（AC-10）。
3. 在本地用真实凭证运行 `FDS_LIVE=1 cargo test -p fds --test live_acceptance -- --ignored`，以及 `fds_cli smoke`（AC-7、AC-8，以及 AC-1、AC-6、AC-9 的真实服务部分）。
4. 确认 AC-7 的例外清单（决定记录 revision 5、6、7、8），确认无误后再给出验收结论。

## 已知遗留（不阻塞验收）

- `StorageTokenProvider::with_token` 预置的 token 不适用半寿命上限。真实 token 有效期约一周，实际不受影响；已写入 README 和 rustdoc。
- fds_cli 的 `delete-prefix` 中有一层页检查错误文本映射，用来保持旧输出。如果以后 SDK 统一了这两条错误文本，这层映射可以删除。
- `.claude/skills/fds-cli/SKILL.md` 仍要求设置 `GO_FDS_TEST_*` 变量，endpoint 也不带 scheme，和 fds_cli 实际读取的 `FDS_*` 变量、要求完整 origin 不一致。该文件不在本 Roadmap 范围内，Agent 也没有写权限。
- Windows 下的写入分支，以及非 unix、非 windows 平台的 fallback，都没有编译验证。

## 用户结论

2026-11-08 · revision 8 · 用户在会话中回复"验收通过"。结论：通过。验收基线为本地 main，未推送、未发布。"需要用户亲自确认的项"由用户自行完成，Agent 不掌握其执行结果。
