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

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/upload-idle-timeout`，用 `git log --grep` 查。

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
