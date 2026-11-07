# fds_cli --oauth 改用 StorageTokenProvider 自动续期

- Task key：cli-oauth-refresh（Roadmap sdk-overhaul）
- 类型：feat
- 状态、依赖：以 plan.json 为准；分支 `feat/sdk-overhaul-cli-oauth-refresh` 存在即表示进行中

## 目标与范围

**背景（credentials-provider 交付时提出，2026-11-08 用户决定"改为自动续期"）：** credentials-provider 已在 SDK 中提供 `fds::credentials::StorageTokenProvider`（过期前按提前量自动重新交换、single-flight、失败时不发过期 token）。fds_cli 的 `--oauth` 目前仍在操作前显式交换一次 token，并以静态凭证构建 Client，长时间运行的命令（大文件上传/下载、批量删除、分页列举等）在 token 过期后报错退出。

**要做：**

1. fds_cli 的 `--oauth` 改为用 `StorageTokenProvider::new(<交换 builder>)` 构建 Client（`credentials_provider(..)`），不再在 CLI 里手动交换后传静态 token；如有需要保留"首个请求前先交换一次以尽早报告凭证错误"的体验（例如 `with_token` 预置首次交换结果），由任务决定并说明。
2. 交换失败的错误信息、退出码与原来保持一致（首次交换失败时 stderr / 退出码不变）。
3. 这项变化本身是 AC-7 的例外（token 过期时由"报错退出 1"变为"自动续期后继续"），已由用户批准；其余参数、输出、退出码不变。
4. `docs/cli.md`、README 中关于 `--oauth`"不自动续期"的说法改为新行为。

**不做：** 不新增 CLI 参数（提前量用 SDK 默认值；若确有必要新增参数，先停下回报）。

## 对应验收标准

AC-9（凭证刷新在 CLI 中生效）、AC-7（除已批准的例外外行为不变）、AC-8（本任务范围）。

## 验收条件与验证方法

- [ ] CLI 测试：mock 交换端点返回短期 token，长命令（例如分页列举多页或多分片传输）执行过程中 token 进入刷新窗口，CLI 自动重新交换并成功完成；服务端未收到过期 token。
- [ ] 首次交换失败时，stderr 与退出码与改动前一致（新旧二进制对比）。
- [ ] 非 `--oauth` 模式的命令行为完全不变。
- [ ] 文档同步。

## 执行记录

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/cli-oauth-refresh`，用 `git log --grep` 查。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul cli-oauth-refresh

推进 Roadmap sdk-overhaul 的任务 cli-oauth-refresh：fds_cli --oauth 改用 StorageTokenProvider 自动续期。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/cli-oauth-refresh.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：fds_cli/src/config.rs（及 service.rs 如涉及）把 --oauth 改为 credentials_provider(StorageTokenProvider::new(exchange builder))，必要时 with_token 预置首次交换结果以保持首个错误时机；保持首次交换失败的输出与退出码；补短期 token 自动续期的 CLI 测试；同步 docs/cli.md 与 README
验证：短期 token 自动续期的 CLI 测试；首次交换失败场景的新旧二进制对比；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
