# 文档、示例与版本号 0.1.0

- Task key：docs-release（Roadmap sdk-overhaul）
- 类型：docs
- 状态、依赖：以 plan.json 为准；分支 `docs/sdk-overhaul-docs-release` 存在即表示进行中

## 目标与范围

**要做：**

1. **版本号**：`crates/fds/Cargo.toml` 和 `crates/fds_cli/Cargo.toml` 的 version 从 0.2.0 改为 0.1.0（用户决定）；fds_cli 对 fds 的依赖版本同步修改；Cargo.lock 更新。
2. **README.md**（约 1000 行）：确认超时一节反映 upload-idle-timeout 后的最终语义（上传与下载都是按进度重置的空闲计时，不再提示"上传大文件需调大 read_timeout"）；所有示例改为流式 builder API、新的错误处理方式（kind / is_not_found）、新的返回值类型；删掉与已移除 API 相关的段落；补上超时语义（默认无整体超时）、重试策略（只读默认 + 可选幂等写）、凭证 provider 的说明。
3. **docs/**：
   - `docs/runtime.md`：超时、重试、Downloader 的 IO 模型；
   - `docs/protocol.md`、`docs/validation.md`、`docs/cli.md`：与新 API 和新行为对齐；
   - `docs/migration.md`：删除（用户决定不写迁移指南，现有内容是已过时的 0.2 迁移指南），并清理 `docs/README.md`、`docs/runtime.md`、`crates/fds/src/lib.rs` 中对它的引用；
   - `docs/open_api/`：只核对引用，不改协议资料。
4. **examples/**：确认 12 个示例都使用最终 API、能编译，并在 rustdoc 或 README 中被引用。
5. **crate 级 rustdoc**（`lib.rs`）：快速上手、builder 约定、错误处理、feature 说明；删除过时的说明，比如"POST object creation ... remain deferred"如果仍然成立，就挪到"未支持的功能"一节。
6. **脚本**：`scripts/check_docs.py`、`scripts/check_coverage.py` 跟随新 API 调整（如果它们检查特定的 API 名字）；`scripts/check.sh` 中不再需要的步骤（比如已删除的 feature 组合）同步更新。
7. **最终全量验证**：完整运行 `scripts/check.sh`，结果写进执行记录；为整个 Roadmap 的 AC-1 到 AC-10 逐条整理验证证据，供 `accept` 阶段使用。

**不做：** 不发布到 artifactory；不打 tag。

## 对应验收标准

AC-10（全部），AC-8（全量），并为 AC-1 到 AC-9 汇总验证证据。

## 验收条件与验证方法

- [ ] 两个 crate 的 version 都是 0.1.0。
- [ ] README 和 docs 中的代码片段都能对应到实际存在的 API（`check_docs.py` 通过，并人工抽查）。
- [ ] `scripts/check.sh` 全量通过；没跑的部分如实列出；live_acceptance 由用户本地执行，Agent 不运行。
- [ ] 在 `acceptance.md` 草稿中按 AC 整理证据（只整理，不代替用户验收）。

## 执行记录

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/docs-release`，用 `git log --grep` 查。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul docs-release

推进 Roadmap sdk-overhaul 的任务 docs-release：文档、示例与版本号 0.1.0。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/docs-release.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：版本号改为 0.1.0（两个 crate 加 fds_cli 对 fds 的依赖）；按最终 API 重写 README、docs/runtime.md、protocol.md、validation.md、cli.md 和 lib.rs 的 crate 文档；删除 docs/migration.md 及其引用；核对 examples；调整 check_docs.py/check_coverage.py/check.sh；完整运行 check.sh 并整理 AC 证据
验证：完整运行 scripts/check.sh；check_docs.py 通过；人工抽查 README 示例；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
