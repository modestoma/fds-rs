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

2026-11-08，分支 `feat/sdk-overhaul-credentials-provider`（worktree `.worktrees/credentials-provider`），基于 main `d4bd8f6`。任务文档写于迁移之前，按当前代码核对：内部 `oauth::Credentials` 枚举（AccessKey / OAuth / Anonymous）、`ClientBuilder::credentials` / `storage_token`、`client.rs` 的 `Redaction`、`ErrorKind::CredentialsExpired` / `Unauthenticated` 都已存在；上传空闲计时在 `send()` 里开始。

**1. 公开 API（新模块 `fds::credentials`，每个类型只有这一个路径）。**

- `trait CredentialsProvider: Send + Sync { fn provide_credentials(&self) -> CredentialsFuture<'_>; }`，`CredentialsFuture<'a> = Pin<Box<dyn Future<Output = Result<Credentials>> + Send + 'a>>`，对象安全；`Arc<T>` 有转发实现。
- `Credentials`：不透明结构体，`Credentials::access_key(id, secret)`、`Credentials::storage_token(StorageAccessToken)`，访问器 `access_key_id()`、`storage_access_token()`；`Debug` 只输出种类（token 另输出 `expires_at`），不输出 AK ID、secret、app ID、token。
- `StaticCredentials::new(Credentials)`（另有 `From<Credentials>`）。
- `StorageTokenProvider::new(ExchangeStorageToken)`，`.with_token(token)` 预置已交换的 token，`.refresh_margin(Duration)`；常量 `DEFAULT_REFRESH_MARGIN` = 5 分钟。
- `ClientBuilder::credentials_provider(impl CredentialsProvider + 'static)`；`credentials(..)` / `storage_token(..)` 保留为 `StaticCredentials` 的快捷方式，三个 setter 最后一个生效。传入 `StaticCredentials` 时（通过 `Any` 识别）按静态凭证处理，构建时校验，与快捷方式完全相同。
- `oauth::ExchangeStorageToken` 增加 `#[derive(Clone)]`，供 provider 重复交换（Debug 仍脱敏）。

**2. 刷新策略。** token 剩余有效期 ≤ 提前量时视为需要刷新；对 provider 自己交换的 token，提前量最多取其有效期的一半（避免短期 token 每次请求都交换），`with_token` 预置的 token 有效期未知，按完整提前量。single-flight：`tokio::sync::Mutex` 包住交换，`std::sync::Mutex` 保存缓存和"已完成交换次数"；等待者拿到锁后若发现有交换在等待期间完成，就直接用其结果（成功用新 token，失败返回同种类错误），不再交换。provider 的 clone 共享缓存与 single-flight。交换失败：缓存 token 已过期 → `CredentialsExpired`，否则（还没有 token，或 token 未过期但在刷新窗口内）→ `Unauthenticated`；`Dispatch::NotSent`，执行交换的请求的错误以交换错误为 `source()`（交换走匿名通道，已脱敏）；失败不缓存，下一次请求重新交换。没有采用"刷新失败但 token 未过期时继续用旧 token"的回退，与 aws-sdk 的 lazy cache、object_store 的 TokenCache 一致。取消执行交换的请求会取消交换，下一个等待者重新发起。

**3. 请求管线（`client.rs`）。** `ClientInner.credentials` 改为 `Auth { Static(Credentials), Provider(Arc<dyn CredentialsProvider>), Anonymous }`。`do_request` 在**每次尝试前**（含重试的 sleep 之后）调用 `prepare_attempt`：取凭证（provider 来源的每次校验，错误种类与构建时相同）、检查 `requires_storage_token`、再次检查 token 过期、把 `appId`/`storageAccessToken` 加进该次的 query 并构建 URL；`send()` 用该次凭证在设置 Date 之后签名或设置 `Authorization: OAuth`。凭证获取在 `send()` 之前，不计入上传空闲计时。第一次尝试的错误先后顺序与改前相同（凭证/过期 → URL → 元数据头）。为了不在 await 期间持有非 `Sync` 的 body，`prepare_attempt` 只借用 bucket/object/query。匿名通道行为不变。

**4. 脱敏。** `Redaction` 由该次尝试实际使用的凭证决定（access key → 保留诊断，其余 → 脱敏），`do_json_limited` 通过 `do_request_redacted` 拿到成功那次尝试的策略处理 body 错误。

**5. presign 与 `list_app_buckets`。** presign 的 `build()` 是同步的，只能用**静态** access key（`credentials` 或 `StaticCredentials`）；storage token 或任何其他 provider 返回 `ErrorKind::ConfigInvalid`。注意：任务说明写的是 InvalidInput，但改前代码实际返回 `ConfigInvalid`（消息 "Galaxy-V2 presigning requires access key credentials"），本任务按"保持现有行为"保留 `ConfigInvalid` 和原消息，并在 rustdoc / README 写明。`list_app_buckets` 改为在每次尝试时按 provider 当次返回的凭证判断，不是 token 则 `ConfigInvalid`、不发送。副作用：对 access-key Client 同时设置非法 `.timeout()` 时，先报 InvalidInput（超时校验）而不是 ConfigInvalid，属于两个本地错误的先后顺序变化。

**6. fds_cli（AC-7）。** `config.rs` 改用 `Credentials` + `credentials_provider(StaticCredentials::new(..))`，`--oauth` 仍在操作前显式交换一次、不自动刷新（与 `docs/cli.md` "每次 `--oauth` 调用显式交换一次，不自动刷新" 一致），参数、输出、退出码不变；`service.rs` 使用的交换 builder 接口未变，无需改动。是否让 CLI 改用 `StorageTokenProvider` 自动续期属于 CLI 行为变化，未做，列为待用户决定。

**7. 依赖。** tokio 增加 `sync` feature（single-flight 需要）；原来 `cslb` feature 额外启用的 `tokio/sync` 随之去掉。Cargo.lock 不变。

**8. 文档。** rustdoc：模块文档与示例、trait 文档含可编译的自定义 provider 示例、`ClientBuilder` 三个 setter、`PresignObject`、`ListAppBuckets::send`、`FdsServiceClient`。README：配置选项表、"刷新与轮换凭据"小节、预签名与 OAuth 示例中"不自动续期"的说法；`docs/migration.md` OAuth 行。

**9. 测试。** 新增 `crates/fds/tests/credentials_provider.rs`（12 个用例）：临近过期自动交换且请求使用新 token、旧 token 未被发出；提前量外复用、可配置；首次请求交换与短期 token 按半寿命刷新；16 个并发请求（两个 Client 共用 provider 的 clone）只交换 1 次；首次交换 401/403/503 → Unauthenticated、业务请求 0 次、失败不缓存；token 过期后刷新失败 → 4 个并发请求全部 CredentialsExpired、只交换 1 次、过期 token 未被发出；每次尝试（含重试）用当时的 AK 签名、用当时的 token 重建 URL；按每个请求的凭证脱敏（含 JSON body 错误）；`list_app_buckets` 依据当次凭证；presign 只接受静态 AK，`StaticCredentials` 与快捷方式生成的 URL 逐字节相同；静态 provider 构建时校验、动态 provider 每次尝试校验；各类型 Debug 不含凭证。扩展 `client.rs` 的 tracing 隔离测试，加入 `StorageTokenProvider` 的交换与请求（共 5 个请求，不出现 `private-`、`storageAccessToken`、`Galaxy-V2` 等）。变异检查：去掉 single-flight 的等待者判断后并发用例失败；去掉过期检查并改为失败回退旧 token 后过期用例失败；两处已恢复。wire 快照 fixture 与 golden 未改动且通过。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/credentials-provider`，用 `git log --grep` 查。

- 状态：实现与验证完成，分支上一笔带 Roadmap 末行的提交，未合并。
- `bash scripts/check.sh`（2026-11-08，本 worktree，最后一次代码改动之后运行，退出码 0）：fmt；clippy `-D warnings`；四种 feature 组合的 fds test/check；fds_cli test/check；`cargo doc -D warnings`；`fds_cli --help`；MSRV 1.86.0 三种组合的 test/check 及 fds_cli check；`check_coverage.py`（35/35 YAML、33/33 操作）；`check_docs.py`（116 个本地链接）；`git diff --check`。日志中 170 个 `test result: ok`，没有失败，没有 warning / error 行；新增的 `credentials_provider.rs` 在 stable 4 种与 MSRV 3 种组合中都运行并通过。之后只改了本任务文档，另行运行 `git diff --check` 通过。
- wire 快照 fixture 与签名 golden：未改动，全部通过。
- 共享文件（合并时注意）：`crates/fds/src/client.rs`（请求管线、`Auth`、`Redaction`）、`config.rs`、`oauth.rs`（删除内部 `Credentials`，`ExchangeStorageToken: Clone`，`ListAppBuckets`）、`object/presign.rs`、`lib.rs`、新文件 `credentials.rs`；`crates/fds/Cargo.toml`（tokio `sync`）；新测试 `tests/credentials_provider.rs`；fds_cli `config.rs`；`README.md`、`docs/migration.md`。idempotent-retry 若修改 `do_request` 的重试循环，会与本任务相邻：重试前的 `prepare_attempt` 必须保留。
- 未验证：`tests/live_acceptance.rs`（需要真实凭证，由用户本地执行），真实服务上的 token 刷新没有 live 证据；fds_cli 没有新增自动刷新路径，未做新旧二进制对比（CLI 凭证流程未变，`cargo test -p fds_cli` 通过）。
- 依赖变化：tokio 增加 `sync` feature，Cargo.lock 不变。
- 待用户决定：(1) fds_cli 的 `--oauth` 是否改用 `StorageTokenProvider` 自动续期（会改变长时间命令在 token 过期时的行为与退出码，需要 AC-7 例外；当前保持静态 token）；(2) presign 对 token 返回的是 `ConfigInvalid`（现有行为），任务说明写的是 InvalidInput，本任务按现有行为保留，如需改为 InvalidInput 请确认。
- 协调者复验：确认分支没有改动 plan.json 和 roadmap.md，wire fixture 全部不变；完整运行 `bash scripts/check.sh`，退出码 0。presign 遇到 token 时保留现有的 `ConfigInvalid`：凭证类型不对属于客户端配置问题，任务文档里写的 InvalidInput 只是泛指。fds_cli `--oauth` 是否改为自动续期，已作为待答问题 `cli-oauth-refresh` 交给用户；当前保持静态 token（行为不变）。

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
