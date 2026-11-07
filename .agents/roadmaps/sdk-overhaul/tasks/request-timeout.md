# 取消默认整体超时，支持单请求超时

- Task key：request-timeout（Roadmap sdk-overhaul）
- 类型：fix
- 状态、依赖：以 plan.json 为准；分支 `fix/sdk-overhaul-request-timeout` 存在即表示进行中

## 目标与范围

**问题。** `ClientConfiguration::for_endpoint` 默认设了 `request_timeout = 300s`（`crates/fds/src/config.rs:33`），`Client::with_credentials` 把它传给 reqwest 的 `ClientBuilder::timeout()`（`crates/fds/src/client.rs:128`）。reqwest 的这个超时从连接开始一直计到**响应体读完**，所以：

- 用 `get_object` 流式下载一个几 GB 的对象，或者用 `put_object` 上传一个大文件，只要超过 5 分钟就会被强行中断，即使传输一直在正常进行；
- `copy_to()` 和 `chunk()` 这类流式消费 API 的文档没有说明这个限制，用户只会看到一个莫名其妙的 timeout。

aws-sdk-s3 和 object_store 默认都不设整体超时，只靠连接超时和读空闲超时（每次读取有进展就重置）防止请求卡死。

**要做：**

1. 默认不设整体超时：`request_timeout` 改为 `Option<Duration>`，默认 `None`；`ClientBuilder::request_timeout` 保留，用来显式开启；`ClientConfiguration::request_timeout()` 改为返回 `Option<Duration>`。
2. 保留并写清 `connect_timeout`（默认 50s）和 `read_timeout`（默认 50s，读空闲超时）的语义。
3. 支持单个请求覆盖超时：在内部 `ClientRequest` 上加一个可选的 timeout 字段，用 reqwest 的 `RequestBuilder::timeout` 生效。公开入口先在 `GetObjectRequest` 和 `PutObjectRequest` 上加 `.timeout(Duration)`，后续 fluent-* 任务会统一推广到所有 builder。
4. 在 rustdoc 中说明：开启整体超时后，它覆盖流式响应体的读取过程。
5. 同步修改 `fds_cli`：`crates/fds_cli/src/config.rs` 的 `request_timeout_ms` 配置项保留原有含义（设置了才生效），CLI 行为不变。

**不做：** 不改重试逻辑；不改 read/connect 超时的默认值。

## 对应验收标准

AC-1（全部），AC-7、AC-8（本任务范围内）。

## 验收条件与验证方法

- [ ] 默认构建的 Client 没有整体超时：用 mockito 或本地慢速 server 返回一个分块缓慢、累计耗时超过配置阈值的响应体（测试里用缩短的阈值模拟），默认配置下读完整个响应体不报错。
- [ ] 显式设置 `request_timeout` 后，同样的慢响应会以超时错误结束。
- [ ] 单请求 `.timeout()` 生效，并且优先于 Client 级别的配置。
- [ ] 读空闲超时依然生效：服务端停止发送数据超过 `read_timeout` 时报错。
- [ ] `crates/fds/tests/client_builder.rs` 中关于超时的校验（非零、可表示）调整为适配 Option 语义。
- [ ] `fds_cli` 测试通过。

## 执行记录

2026-11-07，执行 Agent，worktree `.worktrees/request-timeout`，分支 `fix/sdk-overhaul-request-timeout`。

- `crates/fds/src/config.rs`：`ClientConfiguration.request_timeout` 改为 `Option<Duration>`，默认 `None`；`ClientConfiguration::request_timeout()` 返回 `Option<Duration>`。`ClientBuilder::request_timeout(Duration)` 保留，用来显式开启，仍然校验非零、可表示。校验逻辑抽成 `pub(crate) fn validate_timeout`，与单请求超时共用。rustdoc 写明：默认没有整体超时；开启后覆盖 `chunk` / `copy_to` / `bytes` 的流式读取；按每次尝试计算，不覆盖全部重试；单请求 `timeout` 优先。`read_timeout` 的文档写明了它的实际语义（见下方"发现"）。
- `crates/fds/src/client.rs`：`with_credentials` 只在 `Some` 时调用 reqwest 的 `ClientBuilder::timeout`，同时去掉已经无用的 `#[allow(unused_mut)]`。内部 `ClientRequest` 新增 `timeout: Option<Duration>`；`do_request` 把它传给 `send()`，`send()` 用 `RequestBuilder::timeout` 生效。reqwest 0.12.28 的单请求 timeout 覆盖 Client 级别的配置，并且一直计到响应体读完，这一点已对照源码确认。
- 公开 API：`GetObjectRequest` 新增公开字段 `pub timeout: Option<Duration>`，以及 `pub fn timeout(self, Duration) -> Self`。这个结构体的字段原本都是公开的，所以新字段也设为公开，仓库里所有结构体字面量都补了 `timeout: None`。`PutObjectRequest` 新增私有字段和 `pub fn timeout(self, Duration) -> Self`。两者在 `get_object` / `put_object` 中都会先校验（为零或不可表示时返回 `FdsError::Invalid`），校验在发起网络请求之前完成。
- `crates/fds/src/download.rs`：`GetObjectOutput` 的文档说明了读空闲超时和可选整体超时对流式读取的影响。
- `crates/fds_cli`：`config.rs` 原本就只在设置了 `--request-timeout-ms` 时才调用 `request_timeout`，逻辑不变，只加了一行普通注释，`--help` 输出不变。`object.rs`、`smoke.rs` 中的 `GetObjectRequest` 字面量补了 `timeout: None`。
- `README.md`：配置表中 `request_timeout` 的默认值从"300 秒"改为"无"，并补充了 `read_timeout` 在上传时的语义，避免文档与实现不一致。
- 测试（`crates/fds/tests/client_builder.rs`）：
  - `limits_and_effective_configuration`：默认值断言改为 `None`，显式设置后断言 `Some(d)`；零值和 `Duration::MAX` 仍然被拒绝。
  - `read_timeout_enforces_header_and_stream_body_stalls`：去掉 `.request_timeout(10s)`，改为在默认（没有整体超时）配置下验证读空闲超时，覆盖等待响应头和读 body 两种卡住的情况。
  - 新增本地 TCP 慢速 server（`slow_body_server`，每 40ms 发 1 字节，共 12 字节，约 480ms），用缩短的阈值 `THRESHOLD = 200ms` 模拟：
    - `default_client_reads_slow_body_beyond_total_threshold`：默认配置下能完整读完，耗时大于阈值。
    - `total_timeout_expires_despite_continuing_body_progress`（改写为使用同一个 server）：`request_timeout(THRESHOLD)` 后同样的慢响应以超时结束。
    - `per_request_timeout_overrides_client_configuration`：默认 Client 加上 `.timeout(THRESHOLD)` 会超时；Client 30s 加单请求 `THRESHOLD` 会超时；Client `THRESHOLD` 加单请求 30s 能读完。
    - `put_object_request_timeout_applies_while_awaiting_response`：`PutObjectRequest::timeout` 生效（read_timeout 设为 30s，以排除干扰）。
    - `per_request_timeout_is_validated`：Get 和 Put 的单请求 timeout 为零或 `Duration::MAX` 时，都返回 `FdsError::Invalid`。
  - 每个测试都有 5s 的外层 `tokio::time::timeout` 兜底；`client_builder` 并行模式连续跑了 8 次，全部通过，单次约 0.9s。
  - 变异验证：让 `send()` 忽略单请求 timeout 时，两个单请求测试失败；让 Client 默认带 200ms 整体超时时，默认慢速测试失败。验证后已恢复。
- 依赖与 Cargo.lock 没有变化。

**发现（需要用户决定，未在本任务中修改）**：reqwest 0.12.28 的 `read_timeout`，在响应头到达之前，是从请求开始计时的单一期限（`PendingRequest` 里只创建一次的 `sleep`），上传请求体的进展不会重置它。用临时探针实测：只设 `read_timeout(300ms)`、不设整体超时，上传 8 MiB，服务端持续缓慢读取，结果在约 301ms 时以 `FdsError::Http` 超时失败。所以按默认配置（read_timeout 50s），任何"发送 body 加等待响应头"超过 50s 的 PUT / upload_part 仍然会失败。AC-1 的上传部分只取消了 300s 整体超时这一层限制，还没有完全满足。修复需要改动读超时机制（例如上传请求不用 reqwest 的 read_timeout，改由 SDK 自己实现按进度重置的空闲计时），这超出本任务"不改 read/connect 超时"的范围。本任务只把 rustdoc 和 README 改成如实描述这一行为。探针是临时代码，没有提交。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/request-timeout`，用 `git log --grep` 查。

实际验证（都在 worktree 中运行）：

- `bash scripts/check.sh`：退出码 0。包括 fmt、clippy `-D warnings`、四种 feature 组合的 `cargo test -p fds -- --test-threads=1` 和 `cargo check`、`fds_cli` 测试与 check、`cargo doc -D warnings`、`fds_cli --help`、MSRV 1.86（三种 feature 组合的 test/check，以及 fds_cli check）、`check_coverage.py`、`check_docs.py`、`git diff --check`。第一次完整运行后只改了文档注释和 README，之后又完整运行了一次，退出码仍然是 0。
- `cargo test --locked -p fds --test client_builder`：11 个测试通过，并行模式连续 8 次全部通过。
- 未验证：`tests/live_acceptance.rs` 需要真实的 FDS 凭证，按决定由用户本地执行；超过 300s 的真实大对象传输没有在真实环境中跑过，只用缩短阈值的本地慢速 server 模拟。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul request-timeout

推进 Roadmap sdk-overhaul 的任务 request-timeout：取消默认整体超时，支持单请求超时。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/request-timeout.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：改 crates/fds/src/config.rs 的 request_timeout 为 Option 默认 None；client.rs 的 with_credentials 只在 Some 时调用 reqwest timeout；ClientRequest 增加可选单请求 timeout 并在 send 中用 RequestBuilder::timeout；GetObjectRequest/PutObjectRequest 增加 .timeout()；同步 fds_cli config.rs
验证：新增慢速响应测试（默认无整体超时 / 显式超时生效 / 单请求覆盖 / 读空闲超时）；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
