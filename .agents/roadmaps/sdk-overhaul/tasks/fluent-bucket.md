# Bucket 与 OAuth 操作迁移到流式 builder

- Task key：fluent-bucket（Roadmap sdk-overhaul）
- 类型：feat
- 状态、依赖：以 plan.json 为准；分支 `feat/sdk-overhaul-fluent-bucket` 存在即表示进行中

## 目标与范围

**问题：** bucket 相关 API 同样存在三种调用风格混用、`pub` 字段加 `Default` 的请求结构体、命名问题：

- `CreateBucketRequest`、`SetBucketAclRequest`、`MigrateBucketRequest`、`GetLifecycleConfigRequest`、`AccessLog` 是 `pub` 字段加 `Default` 的结构体；
- `does_bucket_exist`（应为 `bucket_exists`），`storage_class_type`（字段名冗余）；
- `CreateBucketRequest.org_id: String` 用空字符串表示"不传"，应改为 `Option`；
- `MigrateBucketRequest` 的 `org_id` / `team_id` 是必填项，却放在 `Default` 结构体里，可能被构造成空值；
- `LifecycleRule::from_json` / `LifecycleConfig::from_json` 是对 serde 的简单包装，没有必要单独提供；
- OAuth 部分：`client.list_app_buckets()`；`FdsServiceClient::new` / `from_builder` / `get_endpoint` / `exchange_storage_token(&StorageTokenRequest)`，其中 `StorageTokenRequest::new(app_id, oauth_app_id, token, provider).xiaomi_mac(..)` 的参数全是位置参数。

**要做：**

- 按 fluent-object 确立的模式迁移：`create_bucket`、`delete_bucket`、`bucket_exists`、`get_bucket_info`、`list_buckets`、`list_authorized_buckets`、`migrate_bucket`、`get_bucket_acl`、`set_bucket_acl`、`get/set_lifecycle_config`、`set_lifecycle_rule`、`get/set_access_log`、`list_app_buckets`。
- `FdsServiceClient` 的 `get_endpoint` 和 `exchange_storage_token` 也改成 builder 形式，`StorageTokenRequest` 的必填字段走 builder 参数，可选的 mac 参数走链式方法。
- 删除 `from_json` 包装（用户直接用 serde_json）。
- `fds_cli/src/bucket.rs`、`fds_cli/src/service.rs`（如果涉及），以及 tests（`head_delete.rs`、`pagination_options.rs`、`sdk_operations.rs`、`oauth_discovery.rs`、`live_acceptance.rs` 中 bucket / OAuth 相关部分）和 examples（`oauth.rs`、`oauth_inventory.rs`、`migration.rs`）同步修改。

**不做：** 字段类型化（`creation_time`、`used_space` 等由 typed-responses 负责）；凭证刷新（credentials-provider 负责）。

- 并行说明（revision 4）：本任务与另外两个 fluent 迁移任务并行执行。共享的 tests、examples、fds_cli tests、README 和 wire 快照文件可能同时被改：只改与本任务操作相关的调用点和内容，往 wire 快照里追加条目时不要改动其他条目，便于协调者在合并时解决冲突。

## 对应验收标准

AC-4（bucket 与 OAuth 部分），AC-7、AC-8（本任务范围内）。

## 验收条件与验证方法

- [ ] bucket 和 OAuth 操作只有流式 builder 一种写法，没有 `pub` 字段加 `Default` 的请求结构体。
- [ ] 可选参数用 `Option` 或 builder 方法表达，不再用空字符串表示缺省。
- [ ] wire 测试证明 create_bucket（包括 acl 头和 storageClass、orgId 查询参数）、migrate、lifecycle、access log、token 交换的请求逐字节一致。
- [ ] fds_cli 测试通过。

## 执行记录

2026-11-07，分支 `feat/sdk-overhaul-fluent-bucket`（worktree `.worktrees/fluent-bucket`），基于 main `2787298`。与 fluent-listing-delete、fluent-multipart-manager 并行执行（用户已批准），共享文件只改 bucket / OAuth 相关的调用点。

**1. 先锁定线上字节。** 迁移前在旧 API 上扩展 `crates/fds/tests/wire_snapshot.rs`，新增测试 `bucket_and_oauth_operations_match_the_wire_snapshot` 和 fixture `tests/fixtures/wire_bucket_ops.txt`，单独提交（WIP 第一笔）。沿用 fluent-object 的裸 TCP 抓包和归一化规则（只归一化 host 端口、date、Galaxy-V2 签名）；为了覆盖 OAuth，把抓包函数拆成 `capture_at`（给出 origin，可返回值）和原来的 `capture`，`authorization: OAuth` 原样保留（不含秘密，测试 token 是固定值）。对象快照的已有条目和 `wire_object_ops.txt` 没有改动。覆盖 23 个请求：create_bucket（无选项；acl 头 `PUBLIC_READ,AUTHENTICATED_WRITE` + `storageClass=STANDARD_IA` + 需要编码的 `orgId`；空 acl 列表与空 org_id 都不发送）、bucket_exists（200 / 404）、delete_bucket、get_bucket_info、list_buckets、list_authorized_buckets、migrate_bucket（编码的 orgId / teamId）、get_bucket_acl、set_bucket_acl（add / delete）、get_lifecycle_config（无 rule id 时 `lifecycle=`、有 rule id）、set_lifecycle_config（多 action，含带尾随空格的 `"abortIncompleteMultipartUpload "` 和不带空格的 Other key）、set_lifecycle_rule、get_access_log、set_access_log、get_endpoint、exchange_storage_token（QQ；XiaoMi + MAC）、list_app_buckets（用快照里交换得到的 token，覆盖 appId / storageAccessToken 查询参数）。迁移后只改测试里的调用写法（并在 create_bucket 全选项和 QQ 交换上加了 `.timeout(..)`，timeout 不产生线上字节），fixture 一字节未改（迁移提交前 `git diff --cached --quiet -- crates/fds/tests/fixtures` 成立），测试通过。

**2. 沿用 fluent-object 的 builder 约定。** `timeout_option!` 从 `object/mod.rs` 移到新的 crate 内部模块 `macros.rs`（`lib.rs` 里 `#[macro_use]` 最先声明），对象、bucket、OAuth 共用；`bucket/mod.rs` 新增 `bucket_builder!`（只需 bucket 的 builder：结构体、构造函数、timeout）。所有新 builder：持有 `Client` 克隆、字段私有、不实现 `Default`、`#[must_use]`、有 `timeout()`、`send(self)` 里先做本地校验（`InvalidInput` + `NotSent`）。`lib.rs` 的 "Operation builders" 一节更新为单对象、bucket、OAuth 已迁移，并补充"可选项不调用即不设置，不用空字符串表示未设置"。

**3. 迁移后的公开 API。** `fds::bucket`（`bucket.rs` 拆为 `bucket/{mod,basic,acl,config}.rs`，模型留在 `mod.rs`）：
- `create_bucket(b)` → `.acl(impl IntoIterator<Item = BucketCannedAcl>)` `.storage_class(StorageClass)` `.org_id(v)` `.timeout(d)` `.send() -> ()`（`CreateBucket`；`org_id` 是 `Option<String>`，空值不发送，与对象 `version_id` 一致）
- `bucket_exists(b).send() -> bool`（`BucketExists`，原 `does_bucket_exist`）；`delete_bucket(b).send()`（`DeleteBucket`）；`get_bucket_info(b).send() -> GetBucketInfoResponse`（`GetBucketInfo`）
- `list_buckets().send()`、`list_authorized_buckets().send() -> ListBucketsResponse`（`ListBuckets`、`ListAuthorizedBuckets`）
- `migrate_bucket(b, org_id, team_id).send()`（`MigrateBucket`；两者必填，按原值发送，空值也发送，与旧行为一致）
- `get_bucket_acl(b).send() -> AccessControlList`（`GetBucketAcl`）；`set_bucket_acl(b, acl).action(AclAction).send()`（`SetBucketAcl`）
- `get_lifecycle_config(b).rule_id(id).send() -> LifecycleConfig`（`GetLifecycleConfig`；不设 rule id 时发 `lifecycle=`）；`set_lifecycle_config(b, config).send()`、`set_lifecycle_rule(b, rule).send()`（`SetLifecycleConfig`、`SetLifecycleRule`，模型按值传入）
- `get_access_log(b).send() -> AccessLog`、`set_access_log(b, access_log).send()`（`GetAccessLog`、`SetAccessLog`）

`fds::oauth`：
- `FdsServiceClient::new` / `from_builder` 不变（构造函数）。
- `service.get_endpoint(region, service).timeout(d).send() -> String`（`GetEndpoint`）
- `service.exchange_storage_token(app_id, oauth_app_id, access_token, provider).xiaomi_mac(open_id, key, algorithm).timeout(d).send() -> StorageAccessToken`（`ExchangeStorageToken`；Debug 只显示 provider；校验顺序保持"输入 → HTTPS origin"，之后才校验 timeout；仍是单次尝试）
- `client.list_app_buckets().timeout(d).send() -> AppBuckets`（`ListAppBuckets`；先检查 OAuth 凭证，`ConfigInvalid` + `NotSent`）

**4. 删除。** `CreateBucketRequest`、`SetBucketAclRequest`、`MigrateBucketRequest`、`GetLifecycleConfigRequest`、`StorageTokenRequest` 及它们的 `send(&client)`；`client.create_bucket(&req)` 等接收请求结构体的方法、`does_bucket_exist` 和其他位置参数的 async 方法；`LifecycleRule::from_json`、`LifecycleConfig::from_json`，以及只被它们使用的 `Error::json_input`（crate 内部）。`AccessLog`、`LifecycleConfig`、`LifecycleRule`、`AccessControlList` 是请求体 / 响应共用的 serde 模型，保留公开字段和 `Default`，作为 set_* builder 的必填参数按值传入；响应模型的字段名和类型（`GetBucketInfoResponse.storage_class_type` 等）不动，由 typed-responses 负责。

**5. 测试。** 新增 `tests/bucket_builders.rs`：`every_bucket_builder_validates_bucket_and_timeout_before_sending`（12 个带 bucket 的 builder 在空 bucket、零 timeout、不可表示的 timeout 下都返回 `InvalidInput` + `NotSent`，同样的 builder 输入合法时到达 transport；两个根列举的零 timeout）、`bucket_timeout_bounds_a_silent_server`（timeout 真正生效）、`oauth_builders_validate_before_sending`（get_endpoint 空 / 空白选择器和零 timeout；exchange 的缺字段、XiaoMi 缺 MAC、非 XiaoMi 带 MAC、零 timeout，Debug 不含 token；非回环 HTTP origin 为 `ConfigInvalid`；access-key client 调 list_app_buckets 为 `ConfigInvalid`）、`list_app_buckets_validates_timeout_before_sending`。builder 和 `send()` 的 future 都断言为 `Send`。其余 tests（wire、client_builder、error_model、head_delete、live_acceptance、oauth_discovery、pagination_options、retries、sdk_operations、client.rs 内的脱敏测试、bucket 模块单测）只改调用写法，断言不变；`lifecycle_rule_from_json_days_float` 改名为 `lifecycle_rule_days_are_float`，改用 `serde_json::from_slice`。

**6. fds_cli、examples、文档。** `fds_cli/src/bucket.rs`、`service.rs`、`config.rs`、`smoke.rs` 改用新写法，命令参数、输出和退出码不变：`service.rs` 的 `read_exchange` 改为返回 `ExchangeInput`（仍在读取时校验 provider，错误文本不变），再由 `ExchangeInput::exchange(&service)` 生成 SDK builder；`set-lifecycle` / `set-lifecycle-rule` 读文件改用 `util::read_json_redacted`，解析失败仍是与原 `from_json` 相同的 SDK `InvalidInput` 错误（只含解析位置，不回显输入）；smoke 输出里的检查标签 `does_bucket_exist` 是 CLI 输出文本，保持原样。examples `oauth.rs`、`oauth_inventory.rs` 改用新写法（`migration.rs` 不涉及 bucket / OAuth，未改）。README 中受影响的代码片段与相邻说明（快速开始、Bucket 创建/查询/列举、迁移和删除、Bucket ACL、canned ACL、生命周期、访问日志、服务发现、OAuth 交换、配置表中的 `.timeout` 说明）和 `docs/open_api/README.md` 的映射列同步修改。`docs/migration.md` 留给 docs-release 删除，未改。

**7. 依赖。** 无变化，Cargo.lock 未改。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/fluent-bucket`，用 `git log --grep` 查。

- 状态：实现与验证完成，分支上是 WIP 提交（第一笔只有旧 API 下的 bucket / OAuth wire 快照，第二笔是迁移，第三笔是本文档），等待协调者压成一笔后合并。
- `bash scripts/check.sh`（2026-11-07，本 worktree，退出码 0）：fmt；clippy `--workspace --all-features --all-targets -D warnings`；四种 feature 组合的 `cargo test -p fds -- --test-threads=1` 和 `cargo check --all-targets`；fds_cli test 和 check；`cargo doc -D warnings`；`fds_cli --help`；MSRV 1.86.0 下三种组合的 test/check 及 fds_cli check；`check_coverage.py`（35/35 YAML、33/33 操作）；`check_docs.py`（114 个本地链接，retired 扫描通过）；`git diff --check`。日志中有 156 个 `test result: ok`，没有失败，也没有 warning / error 行。`wire_snapshot`（对象 + bucket/OAuth 两个测试）与 `bucket_builders` 在 7 轮（stable 4 种组合 + MSRV 3 种）中都运行并通过。
- wire 字节一致：`FDS_UPDATE_WIRE_SNAPSHOT=1` 只在旧 API 上运行过一次生成 `wire_bucket_ops.txt` 并单独提交（`wire_object_ops.txt` 同时被重写但内容不变，git 无改动）；迁移后不带该变量运行（含 check.sh 中的 7 轮），两个 fixture 在迁移提交中都无改动。
- 命名检查：`RUSTDOCFLAGS='-D warnings' cargo doc -p fds --all-features --no-deps` 后，`target/doc/fds` 中没有任何包含 `does_` 的文件或文本，方法锚点里没有 `does_` / `generate_` / `from_json`；源码中没有 `pub fn does_`、旧请求结构体和 `fn from_json`（grep 核对）。新 builder 没有 `pub` 字段，也没有 `derive(Default)`（grep 核对）。
- 未验证：`tests/live_acceptance.rs` 的 2 个 live 测试（需要真实凭证，由用户本地执行；本任务只改了其中的调用写法，编译通过）；fds_cli `smoke` 和各 bucket / service 命令对真实 FDS 的运行未做。
- 依赖变化：无。
- 待用户决定：无。可选评审点：`AccessLog` 仍是公开字段 + `Default` 的模型（get 的返回值、set 的必填参数，`bucket_name` 按原样发送），没有改成 set_access_log 的链式选项；`migrate_bucket` 的空 org/team 仍按原样发送，未加本地非空校验。
- 协调者复验：确认 bucket/OAuth 的 wire fixture 在迁移提交前后逐字节不变，对象的 fixture 未被改动。分支 rebase 到最新 main（revision 4 事件记录）后，完整运行 `bash scripts/check.sh`，退出码 0。保守处理的两点（`AccessLog` 仍是公开字段的 serde 模型；`migrate_bucket` 不做非空校验）保留现状，如需调整由后续任务或用户决定。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul fluent-bucket

推进 Roadmap sdk-overhaul 的任务 fluent-bucket：Bucket 与 OAuth 操作迁移到流式 builder。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/fluent-bucket.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：按 fluent-object 写下的 builder 约定迁移 crates/fds/src/bucket.rs、acl.rs、oauth.rs 的操作；does_bucket_exist 改名为 bucket_exists；org_id 改为 Option；StorageTokenRequest 必填字段走参数；删除 from_json；同步 fds_cli/src/bucket.rs、相关 tests 和 examples
验证：wire 测试对比 bucket / OAuth 请求逐字节一致；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
