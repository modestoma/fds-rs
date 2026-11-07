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

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/request-timeout`，用 `git log --grep` 查。

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
