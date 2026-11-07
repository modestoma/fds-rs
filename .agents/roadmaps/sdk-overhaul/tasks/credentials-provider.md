# 可刷新的凭证 provider

- Task key：credentials-provider（Roadmap sdk-overhaul）
- 类型：feat
- 状态、依赖：以 plan.json 为准；分支 `feat/sdk-overhaul-credentials-provider` 存在即表示进行中

## 目标与范围

**问题：** 凭证在 Client 构造后就固定了（`crates/fds/src/oauth.rs` 的 `Credentials` 枚举、`ClientBuilder::credentials` / `storage_token`）：

- OAuth storage token 过期后，`validate()` 返回错误（error-model 之后是 CredentialsExpired），调用方只能重新交换 token、重建 Client，所有持有旧 Client 克隆的地方都要替换；
- 长时间运行的服务没有办法在 token 过期前自动刷新；
- AK/SK 也无法轮换。

参照 aws-sdk 的 `ProvideCredentials` 和 object_store 的 `CredentialProvider`：Client 持有一个 provider，每次请求时（或在缓存过期时）向它取凭证。

**要做：**

- 定义 `CredentialsProvider` trait（异步，返回 `Result<Credentials>`；如果 trait 里用 async fn 不满足对象安全，可以返回 `BoxFuture`），以及公开的 `Credentials` 类型（AccessKey 或 StorageToken，Debug 时脱敏）。
- 内置实现：`StaticCredentials`（AK/SK 或固定 token）；`StorageTokenProvider`，包装 `FdsServiceClient` 加 `StorageTokenRequest`，在过期前的一个可配置提前量内自动重新交换，并发请求共享同一次刷新（single-flight），交换失败时返回 Unauthenticated 或 CredentialsExpired。
- `ClientBuilder::credentials(..)` / `storage_token(..)` 作为静态 provider 的快捷方法保留；新增 `credentials_provider(impl CredentialsProvider)`。
- presign 需要 AK/SK：如果当前凭证是 token，返回 InvalidInput（现有行为），文档写清楚。
- 内部：`Client` 在 `do_request` 中获取凭证并缓存；`list_app_buckets` 要求 OAuth 凭证的判断改为基于 provider 当前返回的凭证类型。
- fds_cli 的配置加载（`config.rs`、`service.rs`）改用新 API，行为不变。

**不做：** 不实现从环境变量或配置文件自动发现凭证的链式 provider（如果需要，作为以后的 Issue）。

## 对应验收标准

AC-9（凭证部分），AC-7、AC-8（本任务范围内）。

## 验收条件与验证方法

- [ ] mock 测试：token 即将过期时，下一次请求前自动交换出新 token，请求使用新 token；并发 N 个请求只触发一次交换。
- [ ] 交换失败时，请求返回 Unauthenticated 或 CredentialsExpired，并且不发出带过期 token 的请求。
- [ ] 静态凭证的行为与改动前一致（签名 golden 测试通过）。
- [ ] Debug 和 tracing 输出不含凭证（沿用并扩展现有脱敏测试）。
- [ ] fds_cli 测试通过。

## 执行记录

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/credentials-provider`，用 `git log --grep` 查。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul credentials-provider

推进 Roadmap sdk-overhaul 的任务 credentials-provider：可刷新的凭证 provider。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/credentials-provider.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：在 crates/fds/src/oauth.rs（或新建 credentials 模块）定义 CredentialsProvider trait、公开的 Credentials、StaticCredentials 和 StorageTokenProvider（提前刷新 + single-flight）；client.rs 的 do_request 改为从 provider 获取凭证；ClientBuilder 增加 credentials_provider；fds_cli 的 config/service 适配
验证：自动刷新、single-flight、交换失败三类 mock 测试；签名 golden 测试；脱敏测试；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
