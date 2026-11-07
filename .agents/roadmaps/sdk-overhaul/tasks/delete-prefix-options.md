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

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/delete-prefix-options`，用 `git log --grep` 查。

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
