# delete_prefix 增加显式 marker/delimiter，CLI 改用 SDK

- Task key：delete-prefix-options（Roadmap sdk-overhaul）
- 类型：refactor
- 状态、依赖：以 plan.json 为准；分支 `refactor/sdk-overhaul-delete-prefix-options` 存在即表示进行中

## 目标与范围

**背景（fluent-listing-delete 交付时发现，2026-11-07 用户决定"SDK 加显式选项"）：** fds_cli 的 `object delete-prefix` 有 `--marker`（起始位置）和 `--delimiter`（`docs/cli.md`：只删当前层）两个参数，原实现总是发送 `marker=`。fluent-listing-delete 的 SDK `delete_prefix(bucket, prefix)` 按任务要求只用 bucket 和 prefix，于是 CLI 在 `crates/fds_cli/src/object.rs` 里保留了一份自己的分页删除循环（防护规则与 SDK 相同，起始 marker 也计入已见集合），形成和 SDK 重复的逻辑。

**要做：**

1. 给 SDK 的 `DeletePrefix` builder 增加显式选项 `.marker(..)`（起始 marker，计入 exact 守卫的已见集合）和 `.delimiter(..)`（只处理当前层），rustdoc 写清它们对删除范围的影响；默认（不调用）时行为与当前 SDK 完全一致。
2. 线上请求兼容：CLI 原来总是发送 `marker=`（即使未指定）。确认 SDK 在 CLI 需要时能产生逐字节相同的请求（例如 CLI 不传 `--marker` 时调用 `.marker("")`，或由 SDK 区分"未设置"和"空 marker"），并用 wire 快照证明 CLI 发出的请求序列与改动前一致。
3. fds_cli `delete-prefix` 改为直接调用 SDK，删除 CLI 内的重复循环；命令参数、输出、退出码和线上请求不变（AC-7）。
4. 保留 `MarkerGuard::exact` 语义（重复 marker 在提交该页删除前拒绝），包括"第二页回到起始 marker 时不删除"的 CLI 测试。
5. README / docs/cli.md 同步。

**不做：** 不改 `delete_objects` 的语义；不改响应模型字段类型。

## 对应验收标准

AC-4（删除 API 表达清楚）、AC-7（CLI 行为不变）、AC-8（本任务范围）。

## 验收条件与验证方法

- [ ] `delete_prefix(..).marker(..).delimiter(..)` 有 rustdoc 和测试；不调用时行为与改动前的 SDK 一致（现有分页防护测试全部通过）。
- [ ] fds_cli 中不再有独立的分页删除循环。
- [ ] 用改动前的 CLI 二进制和新二进制在同一组固定应答下对比 delete-prefix 场景（含 `--marker`、`--delimiter`、回到起始 marker、部分失败），退出码、stdout、stderr 和收到的请求一致。
- [ ] wire 快照不变或仅追加。

## 执行记录

2026-11-07，分支 `refactor/sdk-overhaul-delete-prefix-options`（worktree `.worktrees/delete-prefix-options`），基于 main `994d6ef`。与 fluent-multipart-manager 并行，未改动 `transfer.rs` 和分片相关文件。

**1. SDK（`crates/fds/src/object/delete.rs`）。** `DeletePrefix` 新增两个显式选项，沿用 "Operation builders" 约定：
- `.marker(m)`：首个列举请求带 `marker=m`（空串也照样发送），m 及之前的名字不列举也不删除。m 作为起始 marker 交给 `ListObjects::marker`，`into_pages()` 用 `MarkerGuard::exact(Some(m))`，所以 m 计入已见集合：后续页的 `nextMarker` 回到 m 时，在该页删除前以 ProtocolViolation 停止。不调用时不带 `marker` 参数（与改动前一致）。SDK 由 `Option<String>` 区分"未设置"和"空 marker"。
- `.delimiter(d)`：列举带 `delimiter=d`，服务归入 commonPrefixes 的名字既不删除也不深入，续页保持调用方的 delimiter；服务列为对象的名字照常删除（不再二次校验分组，与原 CLI 行为一致）。空串等同于不调用（`delimiter` 参数本来就总是发送）。
- `send` 的 rustdoc 改写删除范围一段（默认只按 bucket/prefix，两个选项各自如何缩小范围；页检查的已见集合包含起始 marker）；`Client::delete_prefix` 加了 `.delimiter("/")` 的示例。其余实现不变：默认路径的请求、页检查、错误种类与消息都与改动前一致（现有前缀删除防护测试未改动、全部通过）。

**2. fds_cli（`crates/fds_cli/src/object.rs`）。** `delete-prefix` 改为 `client.delete_prefix(bucket, prefix).delimiter(--delimiter 或 "").marker(--marker 或 "").page_size(..)[.move_to_trash()]`，删除了 CLI 自己的分页循环 `delete_prefix` 函数及随之无用的 `HashSet`、`object::*` 导入。逐字节一致的办法：不给 `--marker` 时显式调用 `.marker("")`，与原实现一样总发送 `marker=`；delimiter 原来就总是发送（空串）。输出仍走 `output::delete_outcome`：`Ok` 直接输出，`Err` 取 `partial_delete_output()` 作为 `output`。
- 唯一需要适配的是错误文本：原 CLI 对两类页检查（名字越出 prefix / 为空，nextMarker 缺失 / 重复）统一报 `invalid prefix deletion page or pagination marker`，而当前 SDK 分别报 `prefix deletion page lists a name outside the prefix` 和 `listing has missing or cyclic nextMarker`。为不改 SDK 默认行为，CLI 用 `page_check_error` 把"kind 为 ProtocolViolation 且没有 response"的错误换回原文本。依据：这次运行里其他 ProtocolViolation 都带 response（client 的空 body / 超限 / 非法 JSON），批删的协议错误会变成 OutcomeUnknown；新增 CLI 测试锁定非法 JSON 列举仍保留 SDK 原错误。
- PartialFailure：SDK 的错误多了 `Dispatch::Sent`，但 `Display` 不含 dispatch，context（该批 deleted / failed 数）与文本相同，输出一致。

**3. 测试。** `head_delete.rs` 新增 `prefix_marker_and_delimiter_narrow_the_run_and_guard_the_start`（`marker("start") + delimiter("/") + page_size 7 + trash`，查询串精确匹配，commonPrefixes 不删除，第二页回到 start 时不删除）和 `prefix_marker_is_sent_only_when_set_even_if_empty`（不调用时精确匹配无 marker 的查询串，`.marker("")` 时带 `marker=`）。变异检查：把 `MarkerGuard::exact` 的已见集合改为不含起始 marker 后，新 SDK 测试和 CLI 测试 `delete_prefix_keeps_listing_flags_and_stops_on_a_repeated_marker` 都失败，恢复后通过。`fds_cli/tests/commands.rs`：原测试的 stderr 断言从 `contains("ProtocolViolation")` 收紧为整行精确匹配；新增 `delete_prefix_without_flags_sends_an_empty_marker`（`delimiter=&marker=&maxKeys=1000&prefix=p%2F`，第二页越出 prefix 不删除）和 `delete_prefix_keeps_the_client_error_for_a_broken_listing`。

**4. wire 快照。** `wire_snapshot.rs` 的 `listing_delete_requests` 末尾追加 `delete_prefix empty marker`、`delete_prefix marker delimiter`（含需编码的 marker 和续页）两例；用 `FDS_UPDATE_WIRE_SNAPSHOT=1` 只运行该测试重新生成，`wire_listing_delete.txt` 仅在末尾追加 61 行、0 行删除，其他 fixture 未动。

**5. 新旧 CLI 二进制对比。** 改动前在本 worktree（HEAD = main `994d6ef`）构建旧 `fds_cli`，改动后构建新的，两者放在同名路径下（避免 usage 中的程序名不同），用 Python 本地 HTTP 桩按同一组固定应答逐一运行 40 个场景，对比退出码、stdout、stderr 和服务端收到的请求（方法、带查询串的路径、body），help 文本中环境变量显示的随机端口统一替换为 `<port>`。40/40 一致。场景：两页成功（不带 flag）、单页 trash、空列举、空的 truncated 页、只带 `--marker` 两页、`--marker ""`、需编码的 prefix 与 marker、只带 `--delimiter`（含 commonPrefixes 和只有 commonPrefixes 的 truncated 页）、`--delimiter ""`、全部 flag 第二页回到起始 marker、首页即回到起始 marker、nextMarker 为空串、相邻重复、长循环（不带 / 带 `--marker`，后者回到起始 marker）、缺 nextMarker、越出 prefix（首页 / 带 flag 的第二页 / 同页 marker 也非法）、空名字、第二页部分失败、带 flag 的部分失败、失败项不在请求中、第二页列举 500、列举 403 + `--diagnostics`、列举 404、列举非法 JSON（含 `--diagnostics`）、列举空 body、删除 500、删除非法 body、删除时连接被断开、页检查失败 + `--diagnostics`、`--max-keys 0`、`--max-keys=-1 --marker s`、`--max-keys -1`（clap 报错）、空 bucket（不带 / 带 flag）、`delete-prefix --help`、缺 prefix 参数。

**6. 文档。** README 前缀删除一段、`docs/runtime.md` 的 `delete_prefix` 一段（原文"不接受起始 marker"已过时）、`docs/cli.md` 删除与恢复一节补充 `--marker` / `--delimiter` 与 SDK 选项的对应、空 `marker=`、起始 marker 计入循环检查和页检查失败的错误文本。命令示例不变。

**7. 依赖。** 无变化，Cargo.lock 未改。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/delete-prefix-options`，用 `git log --grep` 查。

- 状态：实现与验证完成，分支上是 WIP 提交，等待协调者合并。
- `bash scripts/check.sh`（2026-11-07，本 worktree，退出码 0）：fmt；clippy `-D warnings`；四种 feature 组合的 fds 测试与 check；fds_cli test 和 check；`cargo doc -D warnings`；MSRV 1.86.0 下三种组合的 test/check 及 fds_cli check；`check_coverage.py`（35/35 YAML、33/33 操作）；`check_docs.py`（114 个本地链接，retired 扫描通过）；`git diff --check`。日志中 156 个 `test result: ok`，没有失败、warning 或 error 行。新增的两个 SDK 测试和 `listing_and_bulk_delete_match_the_wire_snapshot` 在 7 轮（stable 4 种组合 + MSRV 3 种）中都运行并通过，三个 CLI delete-prefix 测试通过。
- 新旧 CLI 二进制对比：40/40 场景一致（见执行记录第 5 点）。
- wire 快照：只在 `wire_listing_delete.txt` 末尾追加，已有内容与其他 fixture 不变。
- 共享文件改动（供合并时解决冲突）：`crates/fds/tests/head_delete.rs`（在 `empty_prefix_does_not_allow_empty_object_names` 前插入两个测试）、`crates/fds/tests/wire_snapshot.rs` 与 `wire_listing_delete.txt`（末尾追加）、`crates/fds_cli/tests/commands.rs`（改一处断言、末尾追加两个测试）、`README.md`、`docs/runtime.md`、`docs/cli.md`（各一段）。
- 未验证：live 测试和 fds_cli `smoke`（需要真实 FDS，未运行；本任务未改动它们）。
- 依赖变化：无。
- 待用户决定：无。备注：CLI 页检查错误文本靠"ProtocolViolation 且无 response"识别并换回原文本；如果以后希望 SDK 自己用统一文本，可在后续任务调整 SDK 消息后去掉这层映射。
- 协调者复验：rebase 到含 fluent-multipart-manager 的 main 时没有冲突。wire fixture 只追加了 61 行，没有删除任何行；完整运行 `bash scripts/check.sh`，退出码 0。CLI 中的 `page_check_error` 文本映射保留，作为已知的小技术债：如果以后 SDK 统一了页检查的错误文本，这层映射可以删除。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul delete-prefix-options

推进 Roadmap sdk-overhaul 的任务 delete-prefix-options：delete_prefix 增加显式 marker/delimiter，CLI 改用 SDK。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/delete-prefix-options.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：给 object/delete.rs 的 DeletePrefix 加 marker()/delimiter()（起始 marker 计入 MarkerGuard::exact 已见集合），保证 CLI 场景下请求逐字节不变（含原来总发 marker=）；fds_cli/src/object.rs 的 delete-prefix 改为直接调用 SDK 并删除重复循环；用新旧 CLI 二进制对比输出与请求
验证：新旧 CLI 二进制在固定应答下的 delete-prefix 场景对比；分页防护测试；wire 快照；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
