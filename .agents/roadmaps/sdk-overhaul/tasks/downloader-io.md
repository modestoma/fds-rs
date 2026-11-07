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

2026-11-07，分支 `perf/sdk-overhaul-downloader-io`。只改了 `crates/fds/src/manager/download.rs`、`crates/fds/tests/download_safety.rs`，`Cargo.toml` / `Cargo.lock` 没有变化（`tokio_util::task::TaskTracker` 来自已有的 tokio-util 0.7.18 `rt` feature）。公开 API 没有改动。

**写入管线。** 暂存文件在校验通过后只打开一次，句柄存进 `Destination`（`OnceLock<File>`），所有分片都通过这个句柄做偏移写入（unix 用 `FileExt::write_all_at`，windows 用 `seek_write` 循环），不再按路径重复打开，校验和写入之间也就没有替换窗口。每个 worker 在 async 侧把网络 chunk 攒成最多 `WRITE_BATCH = 256 KiB` 的一批，交给一个被跟踪的 `spawn_blocking`，在 blocking 线程里写入并更新 MD5。每个分片同一时刻最多只有一批在写，下一批要等上一批写完、取回 MD5 状态后才提交，所以写入顺序和摘要都是确定的。每个 worker 最多占用一批缓冲、一个正在写的批和一个 chunk 的内存，与分片大小无关；默认 1 MiB 分片只需要 4 次 blocking 提交，不会每个 chunk 起一个任务。没有选有界 channel 加常驻 blocking 写入端的方案：那样网络慢的时候每个分片会一直占着一个 blocking 线程。

**fsync 与 checkpoint 策略。** 去掉了每个分片的 `sync_data`，整个下载只在发布前做一次 `sync_all`。checkpoint 只在 resume 开启（并且有强 ETag）时写：第一个分片完成时立即写一次，之后在"又完成 `CHECKPOINT_EVERY_PARTS = 64` 个分片"或"距上次写入超过 `CHECKPOINT_INTERVAL = 2 s`"任一条件满足时再写；下载失败（worker 出错、panic、提交失败）时，在所有 blocking 写入结束后做最后一次 best-effort 写入，把已完成的分片全部记下来。成功时不再额外写，因为发布后 checkpoint 会被删除。取这两个值的理由：默认 1 MiB 分片在 100 MB/s 下约每秒 1.6 次 checkpoint（原实现是每秒约 100 次 checkpoint fsync 加 100 次 sync_data）；进程被杀或 future 被取消时最多丢失 64 MiB 或 2 秒的进度，重新下载的代价很小；checkpoint 最大可达约 40 MB（100 万分片），按时间节流能限制这类大 checkpoint 的写入频率。去掉了原来 workers 启动前的那次初始 checkpoint，由"第一个完成的分片立即写"代替。checkpoint 在 blocking 线程里序列化：用 `mem::take` 移过去再移回来，不在 async 侧克隆。安全性说明：checkpoint 里的分片数据现在不保证已经落盘，但恢复时本来就会对本地字节重新算 MD5，断电后不一致的分片会被重新下载，所以 checkpoint 的身份校验和"不信任未验证的本地数据"这两条语义不变。

**放进 spawn_blocking 的操作。** `Destination::acquire`（canonicalize 和创建锁文件）；`prepare`（读 checkpoint、symlink 和硬链接检查、打开文件、`hash_part`、`set_len`）；分片写入；checkpoint 写入；`commit`（`sync_all`、rename、删除 checkpoint）；`release`（关闭句柄、删除锁文件）。用 grep 确认过：`std::fs`、`std::io` 的阻塞调用只出现在这些同步函数里，这些函数只会在 blocking 闭包内被调用。async 侧剩下的同步操作只有一处，见下文的 `Abandon`。

**取消与锁。**
- 所有 blocking 闭包都持有 `Arc<Destination>`，锁文件在最后一个持有者释放时才删除。因此被 abort 的 worker 留下的 blocking 写入一定先于锁释放结束，新 owner 拿到锁之后不会再有旧写入落到 `.tmp`。
- 分片写入通过 `TaskTracker` 跟踪。`run_workers` 出错时先 `abort_all` 并等所有 worker 退出，然后 `download_locked` 执行 `tracker.close(); tracker.wait().await`，等所有 blocking 写入结束后才写最后一次 checkpoint、才释放锁。正常返回（成功或出错）时，锁文件在 `download` 返回前已经在 blocking 线程中删除。
- future 被取消时，`Destination` 的 `Drop` 不在 runtime 线程上 unlink，而是把关闭句柄和删除锁交给 `spawn_blocking`；如果 runtime 正在关闭、闭包没机会执行，闭包被丢弃时仍会同步完成清理。
- 取消不会留下后台 rename：提交前创建 `Abandon` guard，future 被丢弃时它在 mutex 下把 `abandoned` 置位；`commit` 在 `sync_all` 之前和 rename 之前检查这个标志，并且在同一个 mutex 下执行 rename。所以 future 的 drop 返回之后，rename 要么已经完成，要么永远不会发生。代价是 drop 最多等待一次进行中的 rename（只改元数据的系统调用）。这是 async 侧唯一的同步等待，有意保留。
- symlink 和硬链接的拒绝、checkpoint 身份校验、64 MiB 读取上限、100 万分片上限保持不变。

**新增测试。**
- `download_safety.rs::current_thread_runtime_keeps_ticking_during_download_and_resume`：在 current_thread runtime 上，一个 2 ms 周期的 tick 任务与 32 × 1 MiB 的下载并发。第一次下载在最后一片失败（覆盖分片写入和 checkpoint），第二次恢复（对约 31 MiB 本地数据算 MD5，然后写入、fsync、rename），断言两次的最大 tick 间隔都小于 100 ms。用原实现实测为 58 ms 和 335 ms（恢复阶段测试失败），新实现多次实测约 2.5–3 ms。阈值是预期值的 30 倍以上，负载较高的机器上也不会误报，同时仍低于原实现恢复阶段卡顿的三分之一。
- `download_safety.rs::throttled_checkpoint_still_resumes_exactly_the_missing_parts`：12 片、并发 1，第 8 片失败。断言 checkpoint 恰好记录了 0..8 片（来自最后一次写入），恢复时 0..8 片的请求数为 0，8..12 片各请求 1 次，内容正确，checkpoint 和锁都已清理。
- `download.rs` 单元测试：`checkpoint_throttle_policy` 用注入的时间点确定性地验证"第一片 / 每 N 片 / 每 T 秒 / 无新进度不写 / resume 关闭不写"；`checkpoint_writes_are_throttled` 用 cfg(test) 计数器验证 40 片成功下载只写 1–2 次（原实现 41 次），在第 20 片失败时写 2–3 次；`abandoned_part_writes_finish_before_tracker_wait_returns` 在一批写入还在进行时丢弃 writer（模拟 abort），断言 `tracker.wait()` 返回时写入已完整落盘，且 blocking 闭包已释放 `Destination`。
- 现有的 `download_safety.rs`、`manager.rs` 测试以及 `panic_does_not_publish` 都没有修改，全部通过。

**验证（都实际运行过）。**
- `cargo test --locked -p fds --test download_safety --test manager`、`cargo test -p fds --features manager --lib manager`：全部通过，tick 测试连续 3 次的最大间隔约 2.5–3.1 ms。
- `cargo clippy --locked --workspace --all-features --all-targets -- -D warnings`：通过。
- `bash scripts/check.sh`：退出码 0，覆盖 fmt、clippy、四种 feature 组合的 fds 测试与 check、fds_cli 测试与 check、`cargo doc -D warnings`、`fds_cli --help`、MSRV 1.86（三种 feature 组合的测试与 check、fds_cli check）、check_coverage、check_docs、`git diff --check`。

**未验证。**
- windows 的 `seek_write` 分支：本机没有安装 windows target，没有编译过。非 unix 且非 windows 平台的 fallback 返回 `Unsupported`。
- 没有用真实 FDS 和真实磁盘做性能基准；`tests/live_acceptance.rs` 按决定记录由用户在本地执行。

**遗留（超出本任务的文件范围，需要协调者决定）。** `docs/runtime.md`（"每片 sync_data 后保存检查点"，"同步 IO、fsync、恢复摘要会占运行时线程……建议多线程运行时"）和 `README.md`（"同步文件 IO、fsync 和恢复摘要可能占用运行时线程，建议多线程 Tokio"）描述的还是旧行为，本任务没有修改。

- 协调者补充：`docs/runtime.md`（下载文件与断点一节）和 `README.md` 原先写着"每片 sync_data 后保存检查点""同步 IO 会占运行时线程，建议多线程运行时"，已经和新实现不符。这属于纯事实性更正，由协调者在本任务中改为新的写入、fsync 和 checkpoint 语义；`check_docs.py` 通过。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/downloader-io`，用 `git log --grep` 查。

当前分支上有 WIP 提交，由协调者压成一笔。交付内容：`crates/fds/src/manager/download.rs`（写入管线、fsync / checkpoint 节流、取消与锁语义）和 `crates/fds/tests/download_safety.rs`（2 个新集成测试），另加 `download.rs` 中的 3 个新单元测试。`scripts/check.sh` 全绿（退出码 0）。
- 协调者复验：任务分支 rebase 到最新 main（已包含 request-timeout）后，完整运行 `bash scripts/check.sh`，退出码 0。

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
