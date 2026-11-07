# Downloader 文件 IO 移出 async 线程并减少 fsync

- Task key：downloader-io（Roadmap sdk-overhaul）
- 类型：perf
- 状态、依赖：以 plan.json 为准；分支 `perf/sdk-overhaul-downloader-io` 存在即表示进行中

## 目标与范围

**问题。** `crates/fds/src/manager/download.rs` 里的文件操作全部用同步的 `std::fs`，而整个 crate 里没有一处 `spawn_blocking` 或 `block_in_place`。这些阻塞调用直接跑在 tokio worker 线程上：

- `download_part` 中的 `file.write_all(bytes)`（约 :286）和每个分片结束时的 `file.sync_data()`（约 :294）；
- 恢复下载时的 `hash_part`（:217），会把已下载的分片整个读一遍再算 MD5；
- `run_workers` 每完成一个分片就调用一次 `checkpoint.dump()`（约 :463），每次都 `create_new`、写入、`sync_all`、再 rename；
- `download()` 中的 `Destination::acquire`、`canonicalize`、`set_len`、`sync_all`、`rename` 等。

后果：并发 4、每片 1 MiB（默认配置）时，每秒可能要 fsync 几十次，worker 线程被磁盘 IO 卡住，同一 runtime 上的其他任务（包括本 SDK 的其他请求）延迟飙升；在 current_thread runtime 上问题更严重。

**要做：**

1. 分片写入：每个分片用 `spawn_blocking` 打开文件，在指定偏移处写入（unix 下用 `std::os::unix::fs::FileExt::write_at`，windows 下用 `seek_write`，或在 blocking 线程里 seek + write）。网络 chunk 先通过有界 channel 交给 blocking 写入端，或者按 chunk 批量交给 `spawn_blocking`，避免每个 chunk 都起一个 blocking 任务。
2. fsync 策略：去掉每个分片的 `sync_data`，只在发布前做一次 `sync_all`。checkpoint 只在开启 resume 时写，并且按"每 N 个分片或每隔 T 秒"节流（具体取值由任务自定并记录），下载结束或出错时再写最后一次。
3. 断点校验（`hash_part`）、`Destination::acquire`、`set_len`、最终的 rename 和清理，统一放进 `spawn_blocking`。
4. 保持现有的安全语义：只有持有锁的 future 能发布目标文件；取消不会留下仍在后台运行的 rename；拒绝 symlink 和硬链接的暂存文件；checkpoint 的身份校验不变。这些语义在 `crates/fds/tests/download_safety.rs` 中已有测试，必须继续通过。
5. 注意取消语义：`spawn_blocking` 的任务无法被 abort。要保证 `run_workers` 出错调用 `abort_all` 后，所有 blocking 写入都已结束，才能释放 `Destination`（锁文件）。

**不做：** 不改 Downloader 的公开 API（由 fluent-multipart-manager 负责）；不改 HEAD 请求的实现方式（由 fluent-multipart-manager 负责）。

## 对应验收标准

AC-2（全部），AC-8（本任务范围内）。

## 验收条件与验证方法

- [ ] 新增测试：在 current_thread runtime 上并发跑 Downloader 下载（mock server 提供多个分片）和一个周期性 tick 的任务，断言 tick 的最大间隔在合理范围内（比如小于某个阈值；阈值由任务定，并说明为什么不会误报）。
- [ ] `download.rs` 的 async 上下文中不再直接调用 `std::fs` 或 `std::io` 的阻塞操作（代码审查，可以用 grep 辅助确认）。
- [ ] 开启 resume 时 checkpoint 写入次数按节流策略减少，有测试覆盖"中断后恢复"仍然正确。
- [ ] `download_safety.rs`、`manager.rs` 的现有测试全部通过，包括 worker panic 不发布目标文件、锁文件被清理。

## 执行记录

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/downloader-io`，用 `git log --grep` 查。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul downloader-io

推进 Roadmap sdk-overhaul 的任务 downloader-io：Downloader 文件 IO 移出 async 线程并减少 fsync。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/downloader-io.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：只改 crates/fds/src/manager/download.rs（必要时加私有 helper 模块）；分片写入走 spawn_blocking 加偏移写（FileExt::write_at / seek_write），网络 chunk 用有界 channel 交给 blocking 写入端；fsync 只在发布前做一次；checkpoint 节流；abort 后要等 blocking 写入结束才释放锁
验证：新增 current_thread 下的 runtime 阻塞检测测试和 checkpoint 节流后的恢复测试；cargo test -p fds --features manager -- --test-threads=1；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
