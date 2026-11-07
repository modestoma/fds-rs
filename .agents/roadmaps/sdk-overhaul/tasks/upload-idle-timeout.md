# 上传请求按进度重置的空闲超时

- Task key：upload-idle-timeout（Roadmap sdk-overhaul）
- 类型：fix
- 状态、依赖：以 plan.json 为准；分支 `fix/sdk-overhaul-upload-idle-timeout` 存在即表示进行中

## 目标与范围

**问题（request-timeout 任务执行中实测发现）：** reqwest 0.12.28 的 `read_timeout` 在收到响应头之前，是一个从请求开始计时、只创建一次的期限（`PendingRequest` 内部的 `sleep`），上传请求体的进展不会重置它。实测：只设 `read_timeout(300ms)`、不设整体超时，上传 8 MiB，服务端持续缓慢读取，约 301ms 时以超时失败。因此在默认配置（`read_timeout` 50s）下：

- 任何"发送请求体 + 等待响应头"总耗时超过 50s 的 `put_object`、`upload_part`（以及其他带大 body 的写操作）都会失败，即使上传一直在推进；
- 去掉 300s 整体超时（request-timeout 任务）只解决了下载；AC-1 的上传部分仍未满足；
- 目前只在 rustdoc / README 中如实说明，并建议调大 `read_timeout`，但"调大"同时会放宽对卡死连接的检测。

**要做：** 让上传请求的超时语义变成"按进度重置的空闲计时"，与下载一致：

1. **上传阶段**：请求体每成功交给传输层一个 chunk 就重置空闲计时；超过 `read_timeout`（或单独的 `write_timeout` / upload idle 配置，命名和是否拆分由任务决定并记录）没有进展才判定超时。实现思路：用一个共享的"最后进展时间"（例如 `Arc<AtomicU64>` 或 `tokio::sync::watch`），由 `UploadBody::prepare` 产出的 body stream 在每个 chunk 被拉取时更新；`send()` 用一个 watchdog future 与请求 future `select`，watchdog 按"最后进展时间 + 空闲阈值"睡眠并在超时时取消请求。
2. **等待响应头阶段**：请求体发送完毕后，等待响应头的时间按同一空闲阈值从"最后一次上传进展"开始计算。
3. **读取响应体阶段**：保持现有的读空闲语义。
4. 上传请求不能再受 reqwest 的固定期限 `read_timeout` 约束。可行方案包括：对上传请求使用一个不设 `read_timeout` 的内部 reqwest Client（共享配置，连接池分开），或全部请求都改由 SDK 自己的空闲计时管理、不再使用 reqwest 的 `read_timeout`。方案由任务决定并在执行记录中说明取舍（连接池复用、代码复杂度、下载路径是否受影响）。
5. 超时错误映射为 error-model 定义的 `ErrorKind::Timeout`，并正确设置 dispatch（上传途中超时为 Unknown，未发出为 NotSent）。
6. 更新 rustdoc 与 README：删除"上传大文件需调大 read_timeout"的提示，改为描述新的空闲语义。
7. `fds_cli` 若暴露相关超时配置，保持参数和行为兼容（新增配置项可选）。

**不做：** 不改重试策略；不改 connect_timeout；不改单请求整体超时（request-timeout 已完成）的语义。

## 对应验收标准

AC-1（上传部分）：默认配置下，持续推进的上传不因总耗时超过 read_timeout 而失败；卡死（无进展）的上传在空闲阈值后以 Timeout 失败。AC-8（本任务范围）。

## 验收条件与验证方法

- [ ] 新增测试（本地慢速读取的 TCP server，缩短阈值模拟）：空闲阈值 200ms，服务端每 50ms 读取一小块、总上传耗时远超 200ms，`put_object` 成功。
- [ ] 服务端停止读取超过阈值时，`put_object` 以 `ErrorKind::Timeout` 失败，dispatch 为 Unknown。
- [ ] 上传完成后服务端迟迟不返回响应头超过阈值时，以 Timeout 失败。
- [ ] 下载路径的现有读空闲测试（`read_timeout_enforces_header_and_stream_body_stalls` 等）继续通过。
- [ ] 变异验证：去掉进度重置后，"持续推进的上传"测试失败。
- [ ] rustdoc / README 与新语义一致。

## 执行记录

2026-11-07，执行 Agent，worktree `.worktrees/upload-idle-timeout`，分支 `fix/sdk-overhaul-upload-idle-timeout`。

**方案选择：全部请求改由 SDK 自管空闲计时，不再给 reqwest 配置 `read_timeout`（单一 Client、单一连接池）。** 与"上传用不设 `read_timeout` 的第二个内部 Client"比较：

- 连接池：双 Client 会把上传与其他请求分进两个池（PUT 之后的 GET、带 JSON body 的 ACL / complete 等请求不能复用彼此的连接，空闲连接上限与 TLS 握手翻倍，cslb 解析器也要共享或复制）；单 Client 保持现状。
- 代码复杂度：两种方案都必须实现"按进度重置的发送 watchdog"和"响应 body 的读空闲包装"（第二个 Client 没有 `read_timeout`，上传回执的读取同样需要 SDK 自己限时），双 Client 还要额外维护第二个 Client 的构建和按 body 路由。单 Client 方案更少。
- 下载路径：单 Client 方案把下载 body 的读空闲计时从 reqwest 的 `ReadTimeoutBody` 换成 SDK 的 `IdleBody`。语义按 reqwest 0.12.28 源码逐项对齐：只在 poll 等待期间计时（调用者两次读取之间的时间不计入），每收到一个 frame 重置；`size_hint` / `is_end_stream` 透传（`into_raw().content_length()` 不变）。超时错误是 `io::ErrorKind::TimedOut`，reqwest 的 `is_timeout()` 沿 source 链识别，`Error::body_failure` 仍映射为 `Timeout` + `Sent`。现有下载读空闲测试全部通过。

**空闲阈值：沿用 `read_timeout`，不新增配置。** 理由：一个"I/O 空闲超时"覆盖上传、等待响应头、读 body 三个阶段，概念上统一；默认 50s 对上传同样合理；不增加公开 API（public-surface 任务尚未执行），`fds_cli --read-timeout-ms` 自动获得新语义、参数不变；以后确有需要再拆出独立的上传空闲配置不会破坏兼容。名称中的 "read" 与实际覆盖上传有出入，已在 rustdoc 和 README 中写明。

实现：

- `crates/fds/src/idle.rs`（新增，私有模块）：
  - `ProgressClock`：一次尝试共享的"最后进展时间"（`tokio::time::Instant` 起点 + `AtomicU64` 纳秒偏移，`fetch_max` 单调）；`stalled(idle)` 按"最后进展 + idle"睡眠，醒来时若期间有新进展则继续等待。
  - `TrackedBody`：请求 body stream 包装，每次被传输层拉取时最多给出 64 KiB（`Bytes::split_to`，零拷贝），并记录进展。切片是必要的：内存 body 原本是单个 chunk，hyper 只拉取一次就在内部慢慢写出，期间看不到任何进展（变异验证证实了这一点）。用户流的大 chunk 同样被切分。
  - `IdleBody`：响应 body 的读空闲包装（见上）。`Sleep` 只分配一次，用 `reset` 重置。
- `crates/fds/src/upload.rs`：`UploadBody::prepare` 增加 `clock` 参数，内存和流两种来源都包进 `TrackedBody`；原有的长度、提前 EOF、超长 chunk 检查不变。
- `crates/fds/src/client.rs`：
  - `with_credentials` 不再调用 reqwest 的 `read_timeout`（加注释说明原因）；connect timeout、可选整体超时、单请求 `.timeout()` 不变。
  - `send()`：在准备 body 前创建 `ProgressClock`；`tokio::select!`（biased）让 `rb.send()` 与 `clock.stalled(read_timeout)` 竞争，watchdog 先到则丢弃发送 future（中止该次尝试）并返回 `SendFailure::IdleTimeout`。无 body 的请求没有进展事件，等价于原来"从请求开始计时到响应头"的期限。
  - 收到响应头后统一重建 response：body 包进 `IdleBody`，保留 status、headers、extensions；URL 显式保留（access key 时原样，OAuth / 匿名时照旧去掉 query）。
  - `SendFailure::IdleTimeout` 在 `do_request` 中与 reqwest 超时一样，只对可重放的只读请求按重试策略重试；写操作仍不重试。
- `crates/fds/src/error.rs`：新增 `Error::idle_timeout()`：`ErrorKind::Timeout`，dispatch `Unknown`（响应头之前，请求可能已到达服务端），source 为 `io::ErrorKind::TimedOut`。watchdog 无法可靠区分"请求头是否已写出"，所以一律保守地标为 Unknown；未发出的情况（建连失败 / 建连超时）仍由 reqwest 的 connect 错误映射为 `NotSent`，逻辑未改。`ErrorKind::Timeout` 文档补充了空闲超时。
- 文档：`config.rs` 中 `ClientBuilder::read_timeout`、`ClientConfiguration::read_timeout`、`ClientBuilder::request_timeout` 的 rustdoc，`PutObjectRequest::timeout` 的 rustdoc，README 配置表与说明段落（删除"大文件上传需要调大 read timeout"，改为三阶段空闲语义和 dispatch 说明），`docs/cli.md` 中 `--read-timeout-ms` 示例的注释。
- 依赖：`crates/fds/Cargo.toml` 新增直接依赖 `http-body = "1"`（实现 `http_body::Body` 以透传 `size_hint`；该 crate 1.0.1 本来就在依赖树中，`Cargo.lock` 只多了 fds 条目下的一行，没有新的包或版本变化）。
- `fds_cli`：无代码改动，参数和 `--help` 输出不变。

测试：

- `crates/fds/tests/client_builder.rs` 新增本地 TCP 上传 server（`upload_server`，监听 socket 的接收缓冲设为 16 KiB，被 accept 的 socket 继承），空闲阈值 `THRESHOLD = 200ms`，body 16 MiB，外层 10s `tokio::time::timeout` 兜底：
  - `progressing_upload_outlasts_idle_threshold`：服务端每 50ms 读 1 MiB，读完后再等 100ms 才返回响应头；单个 16 MiB 内存 chunk 的 `put_object` 成功，总耗时实测约 955ms（断言 > 400ms）。等响应头的 100ms 叠加在约 850ms 的上传之后，也验证了"等待响应头从最后一次上传进展起算"。
  - `stalled_upload_times_out_with_unknown_dispatch`：流式 body（16 个 1 MiB chunk），服务端读 256 KiB 后停止读取；以 `ErrorKind::Timeout`、`Dispatch::Unknown` 失败，实测约 202ms。
  - `missing_answer_after_upload_times_out`：1 MiB body 被很快读完，服务端不返回响应头；以 `Timeout`、`Unknown` 失败，实测约 203ms。
- `crates/fds/src/idle.rs` 单元测试 `tracked_body_splits_chunks_and_resets_the_clock`（暂停时钟）：大 chunk 切成 64 KiB、空 chunk 被跳过、每次拉取重置空闲计时、最后一次拉取后满 idle 才判定停滞。
- `crates/fds/tests/object_builders.rs` 的 `output_preserves_errors_and_raw_escape_hatch` 补充断言：重建后的 raw response 仍有 `content_length() == Some(2)` 和原 URL。
- 下载路径现有测试 `read_timeout_enforces_header_and_stream_body_stalls`（等响应头卡住、读 body 卡住）、`default_client_reads_slow_body_beyond_total_threshold` 等全部通过，未修改。
- 变异验证（验证后均已恢复）：
  - 去掉 `TrackedBody` 中的 `clock.touch()`（不再按进度重置）：`progressing_upload_outlasts_idle_threshold` 失败（约 200ms 时 Timeout），其余 13 个通过。
  - 去掉 64 KiB 切片（整块交给 hyper）：同一测试失败，说明切片对内存 body 是必要的。
- 稳定性：`cargo test -p fds --all-features --test client_builder` 并行模式连续 10 次全部通过（单次约 0.97s）；同一测试二进制 4 个进程并发、共 3 轮（12 次）全部通过；`cargo test -p fds --all-features` 并行模式连续 3 次全部通过。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/upload-idle-timeout`，用 `git log --grep` 查。

实际验证（都在 worktree 中运行）：

- `bash scripts/check.sh`：退出码 0。包括 fmt、clippy `-D warnings`、四种 feature 组合的 `cargo test -p fds -- --test-threads=1` 和 `cargo check`、`fds_cli` 测试与 check、`cargo doc -D warnings`、`fds_cli --help`、MSRV 1.86（三种 feature 组合的 test/check，以及 fds_cli check）、`check_coverage.py`、`check_docs.py`、`git diff --check`。日志中 135 个 `test result` 全部为 0 failed。之后只修改了本任务文档的"执行记录"和"交付"两节（不在 check 覆盖范围内），又单独运行了 `git diff --check`。
- `cargo test -p fds --all-features --test client_builder`：14 个测试通过；并行模式连续 10 次全部通过；4 进程并发 3 轮（12 次）全部通过。
- `cargo test -p fds --all-features`（并行模式）：连续 3 次全部通过。
- 变异验证：去掉进度重置、去掉 64 KiB 切片，两种情况下 `progressing_upload_outlasts_idle_threshold` 都失败；均已恢复。
- 未验证：`tests/live_acceptance.rs` 需要真实 FDS 凭证，按决定由用户本地执行；真实网络上超过 50s 的大对象上传没有实际跑过，只用 200ms 阈值的本地慢速 server 模拟；只在 macOS 本机 loopback 上跑过，Linux 的 socket 缓冲大小不同，但测试 body（16 MiB）远大于常见缓冲上限。
- 协调者复验：审查 diff（单 Client 方案、发送 watchdog、`IdleBody` 替代 reqwest 的 read_timeout、重试只覆盖可重放的只读请求），接受新增 `http-body` 直接依赖（依赖树中原本就有，版本不变）；在任务 worktree 中完整运行 `bash scripts/check.sh`，退出码 0。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul upload-idle-timeout

推进 Roadmap sdk-overhaul 的任务 upload-idle-timeout：上传请求按进度重置的空闲超时。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/upload-idle-timeout.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：client.rs 的 send() 为带 body 的请求加入按进度重置的 watchdog（upload.rs 的 body stream 在每个 chunk 被拉取时更新最后进展时间）；上传请求不再受 reqwest 固定期限 read_timeout 约束（例如上传用不设 read_timeout 的内部 Client，或全部改为 SDK 自管空闲计时，方案写进执行记录）；超时映射 ErrorKind::Timeout + dispatch；更新 config.rs rustdoc 与 README
验证：新增慢速读取 server 测试（持续推进成功 / 上传卡住超时 / 等响应头超时）与变异验证；下载读空闲测试保持通过；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
