# AGENTS.md

## 项目

小米 FDS 对象存储的异步 Rust SDK（`crates/fds`），以及基于它的命令行工具（`crates/fds_cli`）。技术栈为 Tokio + reqwest。

## 验证

本地验证以 `scripts/check.sh` 为准，包括 fmt、clippy `-D warnings`、各 feature 组合的测试与 check、fds_cli 测试、`cargo doc -D warnings`、MSRV 1.86、文档与覆盖率脚本、`git diff --check`。项目当前没有 CI。

## Git 提交

提交消息中禁止写入 `Co-Authored-By:` 或其他类似的署名、协作归属标记。

## Roadmap 工作流

本仓库的 Roadmap 记录在 `.agents/roadmaps/<roadmap-id>/`，单个 Issue 记录在 `.agents/issues/`，都纳入 Git。
需求、决定、任务状态以这些文件为准；plan.json 只通过 roadmap-workflow skill 的 `scripts/roadmap.py` 修改。
所有任务分支都从基线分支拉出，并合回该基线；依赖任务合入并验证后才开始下游任务。
受保护分支（只能通过合并请求合入，Agent 不提交、不推送）：无。当前以本地 main 为基线，不推送。
