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

2026-11-08，分支 `feat/sdk-overhaul-cli-oauth-refresh`（worktree `.worktrees/cli-oauth-refresh`），基于 main `a023679`。

**1. 方案：首次显式交换 + `with_token` 预置。** `crates/fds_cli/src/config.rs` 的 `Connection::client()` 在 `--oauth` 时仍按原流程读取输入文件、用 `self.builder()`（含 `--auth-endpoint` 覆盖）构建 `FdsServiceClient`、生成 `ExchangeStorageToken`；先 `exchange.clone().send().await?` 交换一次，再用 `StorageTokenProvider::new(exchange).with_token(token)` 交给 `ClientBuilder::credentials_provider(..)`，提前量用 SDK 默认值 `DEFAULT_REFRESH_MARGIN`（5 分钟），不新增 CLI 参数。access key 分支不变（`StaticCredentials`）。`service.rs`、`main.rs` 未改动。

- 选择理由：首次交换与改动前是同一段代码、同一时机（在任何命令本地工作之前），失败时 `?` 直接返回交换的 `fds::Error`，stderr 文本、`--diagnostics` 输出、退出码都不经过映射，天然一致。若改为由 provider 在首个请求时交换，错误会被包成 `Unauthenticated: storage token exchange failed; the request was not sent: <交换错误>`，并推迟到命令本地工作（读文件、建 recovery 文件等）之后，两者都会改变输出，需要额外映射，不采用。
- 已知边界：`with_token` 预置的 token 有效期对 SDK 未知，按完整提前量判断，不套用"半寿命上限"。若首次交换得到的 token 剩余有效期不超过 5 分钟，第一个请求会立即再交换一次（之后由 provider 自己交换的 token 按半寿命上限刷新）。按 `docs/protocol.md` 的证据，真实 token 有效期约一周，实际不会触发；测试中用 3 秒 token 时会看到这次额外交换。
- 行为变化（revision 8 批准的 AC-7 例外）：token 进入刷新窗口时自动重新交换后继续；续期交换失败时，当前请求不发送，命令以 `error: Unauthenticated: storage token exchange failed; the request was not sent: <交换错误>`（token 未过期）或 `CredentialsExpired`（已过期）报错，退出码 1，与原来 token 过期时的退出码相同，文本不同。注意 SDK 不回退旧 token：在刷新窗口内（过期前 5 分钟）续期失败即报错，而原来该 token 仍可用到真正过期。
- `--oauth` 的 `--help` 说明从 "explicitly exchange once before the operation" 改为 "exchange before the operation and again before expiry"，因旧文字已不准确；参数本身不变。这是 `--help` 输出唯一的差异。

**2. 文档。** `docs/cli.md`"服务发现与 OAuth"中"每次 `--oauth` 调用显式交换一次，不自动刷新"改为新行为（操作前交换一次、失败即退出 1；默认 5 分钟提前量自动续期、不可从命令行修改；续期失败的报错种类与退出码；过期 token 不会发出）。README 中没有关于 CLI `--oauth` 不续期的说法（README 里"不自动续期"都指 SDK 的 `storage_token` 和 `oauth_inventory` 示例，仍然正确），未改动。`.claude/skills/fds-cli/SKILL.md` 不涉及 OAuth，未改动。

**3. 测试（`crates/fds_cli/tests/commands.rs`）。** 新增两个用例，交换 mock 每次签发不同 token（`storage-secret-N`）并记录过期时间，列举 mock 每页延迟 250 ms 并记录每个请求携带的 token 与到达时间：

- `oauth_reexchanges_a_short_lived_token_during_a_long_listing`：token 有效期 3 秒，`object list b --max-keys 1 --all` 共 16 页（≥ 4 秒），断言命令成功、输出 16 页、交换 ≥ 3 次、列举期间至少换过一次 token、每个请求携带的都是已签发且到达时未过期的 token，stdout/stderr 不含 token。
- `oauth_refresh_failure_stops_before_sending_and_exits_one`：首次交换签发 301.5 秒 token（1.5 秒后进入默认 5 分钟窗口），之后的交换返回 503；断言退出码 1、stderr 以 `error: ` 开头并含 "storage token exchange failed"、不含 token 与服务端 body，已输出的页数等于服务端收到的列举请求数（失败的那页没有发出），收到的 token 都未过期，续期交换只发生一次。
- 变异检查：把 `config.rs` 换回 main 的版本后，第一个用例失败（`error: CredentialsExpired: storage token expired; explicitly exchange a new token`），第二个用例失败（退出码 0，旧 CLI 不续期）；已恢复。新用例连续运行 3 次稳定通过（约 4.5 秒）。
- 原有 `discovery_needs_no_keys_and_oauth_exchange_authenticates_app_listing`（1 小时 token，mock 默认要求恰好 1 次交换）继续通过，说明正常有效期下预置的 token 被直接使用，没有额外交换。

**4. 新旧二进制对比。** 用 main（`a023679` 的 `config.rs`）和本分支分别构建 `fds_cli`，放在同名路径下（避免 clap usage 中的程序名不同），对同一个 Python 固定应答服务逐场景运行，比较退出码、stdout、stderr 字节一致：45 个场景，0 差异。覆盖：首次交换返回 401、403、404、429、500、503、非 JSON、空 body、缺 token、空 token、已过期 token、非法 expireTime（超范围、字符串）；401 + `object list --all`；503/403/非 JSON + `--diagnostics`；503 + `object put --file <不存在>`（错误先后顺序）；503 + `download`、XiaoMi MAC 输入、`--auth-endpoint`、`--retries 3`；`--request-timeout-ms` 超时；连接被拒；本地错误（输入文件缺失、非法 JSON、未知 provider、`--oauth` 与 AK 环境变量并用、与 `--access-key-id` 冲突（clap，退出码 2）、非 HTTPS 交换 origin、缺 endpoint）；交换成功后的 `bucket list-app`、`object list --all`、`object get`、`object put`、`object presign`（ConfigInvalid）、`service exchange` 成功与 503；非 `--oauth` 的 AK 模式 `object list --all`、`object get`、`object put`、`bucket list`、`list-app`（ConfigInvalid）、缺 secret、缺凭证。`--help` 仅 `--oauth` 说明一行不同（见上）。另用同一服务确认续期失败场景：新二进制输出 `error: Unauthenticated: storage token exchange failed; the request was not sent: ServerError: FDS rejected the request (HTTP 503)`、退出 1；旧二进制在 token 过期后输出 `error: CredentialsExpired: storage token expired; explicitly exchange a new token`、退出 1（即批准的例外）。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/cli-oauth-refresh`，用 `git log --grep` 查。

- 状态：实现与验证完成，分支上一笔带 Roadmap 末行的提交，未合并。
- `bash scripts/check.sh`（2026-11-08，本 worktree，最后一次代码改动之后运行，退出码 0）：fmt；clippy `-D warnings`；四种 feature 组合的 fds test/check；fds_cli test（含两个新用例）/check；`cargo doc -D warnings`；`fds_cli --help`；MSRV 1.86.0 三种组合的 fds test/check 及 fds_cli check；`check_coverage.py`（35/35 YAML、33/33 操作）；`check_docs.py`（117 个本地链接）；`git diff --check`。日志中 170 个 `test result: ok`，没有失败，没有 warning / error 行。之后只改了本任务文档，另行运行 `git diff --check` 通过。
- 新旧二进制对比：45 个固定应答场景（首次交换失败、本地错误、交换成功后的命令、非 `--oauth` 命令）退出码、stdout、stderr 全部一致；`--help` 仅 `--oauth` 说明一行不同。
- 改动文件：`crates/fds_cli/src/config.rs`、`crates/fds_cli/tests/commands.rs`、`docs/cli.md`、本任务文档。SDK、`service.rs`、`main.rs`、README、wire fixture 未改动；无依赖变化，Cargo.lock 不变。
- 未验证：真实 FDS 服务上的 token 续期（需要真实 OAuth 凭证，`live_acceptance.rs` 也不覆盖 CLI）；fds_cli 测试只在 stable 上运行（check.sh 对 MSRV 只做 fds_cli check）；续期发生在多分片上传/并发下载过程中的场景没有单独的 CLI 用例（与分页列举走同一个 provider 与请求管线，SDK 的 single-flight 并发用例已覆盖）。
- 待用户决定：无。备注：`with_token` 预置的首个 token 按完整 5 分钟提前量判断，剩余有效期不超过 5 分钟时第一个请求会多交换一次；真实 token 约一周有效，不影响实际使用，如需消除可另行讨论（例如 SDK 让预置 token 也适用半寿命上限）。
- 协调者复验：确认分支没有改动 SDK、plan.json 和 roadmap.md，完整运行 `bash scripts/check.sh`，退出码 0。备注中提到的边界（`with_token` 预置的 token 不适用半寿命上限）不影响实际使用，没有另开任务，在 docs-release 的设计说明里提一句即可。

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
