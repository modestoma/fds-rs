# 内部实现去冗余：serde 样板、header、脱敏、HTTP 配置

- Task key：internal-cleanup（Roadmap sdk-overhaul）
- 类型：refactor
- 状态、依赖：以 plan.json 为准；分支 `refactor/sdk-overhaul-internal-cleanup` 存在即表示进行中

## 目标与范围

**问题（来自审视的"内部实现冗余"部分）：**

1. **serde 样板代码。** 几乎每个响应字段都写成 `#[serde(rename = "xxx", default, deserialize_with = "crate::de::null_default")]`，每个字段占 5 到 6 行，而且不少 rename 前后同名（比如 `rename = "etag"` 用在 `etag` 字段上，`rename = "prefix"` 用在 `prefix` 字段上）。涉及 `object/put.rs`、`object/listing.rs`、`object/metadata.rs`、`object/multipart.rs`、`bucket.rs`、`acl.rs` 等。
2. **header 构造重复。**
   - 把 metadata 转成 HeaderMap 的循环在 `client.rs:162` 和 `object/presign.rs:116`（`metadata_headers`）各写了一遍；
   - `HeaderValue::from_str(..).map_err(|e| FdsError::invalid(e.to_string()))` 在 `client.rs`、`object/mod.rs`、`object/basic.rs`、`object/presign.rs`、`bucket.rs` 等处重复约 10 次；
   - `object/mod.rs` 的 `insert_header_opt` 只处理 `&Option<String>`。
3. **脱敏逻辑分散在三处**，并且 `matches!(self.inner.credentials, Credentials::AccessKey(..))` 重复了 4 次：
   - `send()` 在 OAuth 模式下拆开 `reqwest::Response` 再重新组装，只为改写里面的 URL（`client.rs:329-349`，包含 `ResponseBuilderExt` 和 `.unwrap()`）；
   - `do_json_limited` 在非 AccessKey 模式下清空 request_id（`client.rs:231-237`）；
   - `redact_error` 在非 AccessKey 模式下清空 body、service_code、request_id（`client.rs:355`）。

   request id 本身不是秘密。URL 改写只是为了 `GetObjectOutput::into_raw()` 暴露的原始 response，而 `into_raw` 会在 typed-responses 中删掉。
4. **同类逻辑两套实现。** `ListObjectsRequest::pages()` 用 Brent 算法检测分页循环（`object/listing.rs:153`），`delete_objects_with_prefix` 却用一个不断增长的 HashSet 自己重新分页（`object/delete.rs:160`）。本任务先把循环检测抽成一个内部 helper，让两处共用；删除流程改为复用 `pages()` 放在 fluent-listing-delete 中做。
5. **绕开了已有封装。** `MultipartUpload::start` 只检查 bucket 和 object 非空（`multipart.rs:149`），没有调用 `validate_object`，bucket 名规则校验不一致。
6. **HTTP 客户端配置。**
   - 没有设置 User-Agent（`constant.rs` 的注释明确写着"Rust does not set a User-Agent"），建议设成 `fds-rs/<CARGO_PKG_VERSION>`，并允许追加应用标识；
   - `pool_max_idle_per_host(100)` 写死在代码里，改为 builder 可配置（保留默认值 100）。
7. **tokio feature 过多。** 作为库，`crates/fds/Cargo.toml` 不应强制开启 `rt-multi-thread`，`net`、`macros` 也要按实际用到的功能评估（`tokio::select!` 需要 `macros`，`JoinSet` 和 `spawn_blocking` 需要 `rt`）。只保留真正需要的 feature。
8. **重试随机源。** `RetryPolicy::delay` 用 `RandomState::new().hash_one(SEQUENCE)` 生成抖动（`retry.rs:53-54`），能用但可读性差。可以加注释说明原理，或者换成 `fastrand`（新增依赖要说明理由）。
9. 函数前的 🔴 注释和 "Ports `fds/xxx.go`" 一类注释只对移植阶段有意义。逐一保留协议相关的说明，删掉纯粹的移植痕迹。

**要做：** 逐项处理以上 1 到 9。serde 部分建议在容器上加 `#[serde(rename_all = "camelCase", default)]`，配合 `serde_with::DefaultOnNull`（通过 `#[serde_as]`）或一个 crate 内部的小宏，表达"null 视为默认值"。字段名和 camelCase 不一致的（例如 `name` 对应 `bucket_name`、`objects` 对应 `object_summaries`）继续用显式 rename。**线上 JSON 的兼容性必须保持不变**：缺失键、null、未知字段的处理行为和改动前一致。

**不做：** 不改字段的类型（`i64` 改 `u64`、时间改 DateTime 由 typed-responses 负责）；不改公开 API 的形态。

## 对应验收标准

AC-8（本任务范围内）；为 AC-6 打基础。没有直接对用户可见的 AC，本任务对外行为不变。

## 验收条件与验证方法

- [ ] 所有响应模型的反序列化测试保持通过；新增覆盖"字段缺失 / 字段为 null / 未知字段"的表驱动测试，每个模型至少一条。
- [ ] metadata 转 header 只剩一个 helper；`HeaderValue::from_str(..).map_err(..)` 的重复调用被一个 helper 或 `From` 实现替代。
- [ ] 脱敏策略集中在一个地方（比如 `Client::redaction_policy()` 或 credentials 上的一个方法），`matches!(credentials, AccessKey)` 不再到处重复；现有脱敏测试全部通过。
- [ ] 分页循环检测只有一份实现。
- [ ] 请求带上 `User-Agent: fds-rs/<version>`，有 wire 测试断言。
- [ ] `cargo tree -e features -p fds` 确认 tokio 不再强制开启 `rt-multi-thread`；默认 feature 和全部 feature 下都能编译。
- [ ] 签名 golden 测试不受 User-Agent 影响（UA 不参与签名，需要确认）。

## 执行记录

2026-11-07，分支 `refactor/sdk-overhaul-internal-cleanup`（worktree `.worktrees/internal-cleanup`），基于 main `65bd4f7`。逐项按当前代码重新核对后处理：

1. **serde 样板（已做）。** 没有引入 serde_with，改用 crate 内部的声明宏 `crate::de::lenient!`（`de.rs`）：它只给结构体的每个字段追加 `#[serde(deserialize_with = "crate::de::null_default")]`，容器属性由各模型显式写出：`#[serde(rename_all = "camelCase", default)]`。只有线上名不是字段名 camelCase 的字段保留显式 rename（`name`→`bucket_name`/`object_name`、`objects`、`metadataBean`、`storageClass`、`enableSSE`、`numObjects`、`actions`、`accessControlList`）。不用 serde_with 的理由：零新依赖、Cargo.lock 不变、不受 serde_with 新版本 MSRV 影响；宏复用原来的 `null_default`，所以 null / 缺失 / 未知键的行为逐字节不变。转换的宽松模型：`PutObjectResponse`、`ObjectBasicInfo`、`ObjectSummary`、`ObjectListing`、`InitMultipartUploadResponse`、`GetBucketInfoResponse`、`ListBucketsResponse`、`LifecycleRule`、`LifecycleConfig`、`AccessLog`、`Owner`、`GrantKey`、`AccessControlList`。所有容器级 `default` 的模型都是 derive 的 `Default`，与原字段级 `default` 等价。严格模型保持严格（缺失或 null 报错）：`UploadPartResponse` 只加 `rename_all = "camelCase"`、去掉同名 rename；`LifecycleBaseItem` 去掉同名 `rename = "days"`；`Grant` 的 grantee 仍单独宽松，permission/type 仍必填；`CopyObjectResponse`、`DeleteObjectFailure`（snake_case 键）、`AppBucket(s)`、`ObjectMetadata` 内部的 `Aux` 未动。没有改任何字段类型。
2. **header 构造（已做）。** metadata → header 只剩 `ObjectMetadata::append_to_headers`（`metadata.rs`），`do_request` 与预签名共用，删掉 `presign.rs` 的 `metadata_headers`。新增 `client::header_value(&str) -> Result<HeaderValue>`（非法字节为 InvalidInput），替换 client（Date）、`object/mod.rs`、`object/basic.rs`（Range）、`bucket.rs`（`x-xiaomi-meta-acl`，原来是 `.parse().map_err(..)`）、`upload.rs`（Content-MD5）、`manager/download.rs`（Range）中的重复转换。剩下唯一的 `HeaderValue::from_str` 在 helper 里，以及 Authorization（保留 `config_invalid` 且 `set_sensitive`，语义不同）。Date、MD5 hex、整数 Range 原来的错误种类是 Unexpected / 自定义消息，这些值总是合法 ASCII，分支不可达，统一后不影响对外行为。`insert_header_opt` 改为接受 `Option<&str>`，`put_object` 的 6 个可选头改成一个循环。
3. **脱敏（已做；部分已由 error-model 解决）。** `do_json_limited` 清空 request_id 的逻辑已由 error-model 删除（统一走 `redact_error`）。剩下的两处判断（`send()` 改写响应 URL、`redact_error`）集中到 `client.rs` 的私有 `Redaction` 策略：`Redaction::for_credentials` 用穷举 `match` 决定 Keep（access key）/ Redact（OAuth、Anonymous），`response_url()` 与 `error()` 两个方法分别执行；`matches!(credentials, AccessKey(..))` 不再出现。`send()` 中重组响应时的 `.unwrap()` 改为 `url_extension()`，失败时退回 reqwest 的占位 URL（更保守）。OAuth 下仍清空 request_id：现有测试（`oauth_discovery.rs`）明确断言这一点，按"对外行为不变"保留；URL 改写在 typed-responses 删除 `into_raw` 后可一并去掉。
4. **分页循环检测（已做，含一处需要知会下游的取舍）。** 抽成 `object/listing.rs` 的 `MarkerGuard`，`pages()` 与 `delete_objects_with_prefix` 共用同一个 `advance()`。原来两处的语义不同：`pages()` 用 Brent（内存有界，长环延迟检测），删除流程用不断增长的 HashSet（任何重复立即拒绝，且在提交该页删除之前）。现有测试 `head_delete.rs::prefix_long_cycle_and_initial_marker_are_checked_before_delete` 断言后者：start→a→b→start 的环必须在删除第 3 页之前发现。只用 Brent 会让该测试失败，并让破坏性操作重复提交删除，所以 `MarkerGuard` 提供两种模式：`new()`（Brent，供 `pages()`）和 `exact()`（在同一检测逻辑上额外记住全部 marker，供删除）。两处对外行为都不变。新增单元测试 `object::tests::marker_guard_bounded_and_exact_modes`。**提示 fluent-listing-delete：** 删除流程改为复用 `pages()` 时，需要让它使用 `exact` 模式，否则会削弱上述保证。
5. **`MultipartUpload::start`（已做）。** 改为调用 `crate::object::validate_object`，空 bucket 的报错消息变为 `bucket must be non-empty`（种类仍为 InvalidInput）。
6. **HTTP 客户端配置（已做）。** 每个请求带 `User-Agent: fds-rs/<CARGO_PKG_VERSION>`（`config.rs` 的 `USER_AGENT`，通过 `reqwest::ClientBuilder::user_agent` 设置）。新增公开 API：`ClientBuilder::user_agent_suffix(..)`（追加应用标识，空格分隔，必须是非空可打印 ASCII，否则 `ConfigInvalid`）、`ClientBuilder::pool_max_idle_per_host(usize)`（默认 100），以及 `ClientConfiguration::user_agent()` / `pool_max_idle_per_host()` 两个 getter。UA 不参与签名：签名在我们自己的 HeaderMap 上完成，之后 reqwest 才加默认头；而且 Galaxy-V2 只签 Content-MD5、Content-Type、Date 和 `x-xiaomi-*`。新增 `auth::tests::user_agent_is_not_signed`；`auth::tests::golden_signatures` 与 `tests/golden.rs` 未改动并通过。README 配置表与 `docs/runtime.md` 中"固定为 100"一句同步更新。
7. **tokio feature（已做）。** `crates/fds/Cargo.toml`：tokio 从 `rt-multi-thread, macros, fs, io-util, sync, time, net` 改为 `rt, macros, fs, io-util, time`；`sync`、`net` 只在 `cslb` 中用到（`tokio::sync::Mutex`、`lookup_host`），改为 `cslb = [.., "tokio/sync", "tokio/net"]`；顺带把 tokio-util 的 `rt`（只有 manager 的 `TaskTracker` 用）移到 `manager = [.., "tokio-util/rt"]`。`rt` 是必需的（`spawn_blocking`、`JoinSet`、`yield_now`、`tokio::spawn`），`macros` 用于 `select!`。dev-dependencies 仍是 `full`。`cargo tree --locked -e features,no-dev -p fds -i tokio` 在 `--no-default-features`、默认、`--all-features` 下都不再出现 `rt-multi-thread`（`net`、`sync` 仍会由 reqwest/hyper-util 传递开启，不是本 crate 强制）。另外单独验证了 `cargo check -p fds --lib --no-default-features --features cslb` 和 `--features manager`。
8. **重试抖动（已做，加注释）。** 抽成 `retry::jitter_random()`，注释说明 `RandomState::new()` 的 SipHash key 来自按线程从 OS 取种、每次新建都会扰动的 key，对全局计数器求哈希得到每次调用都不同、分布均匀的值；足够用于抖动，不是密码学随机源。没有引入 fastrand。
9. **移植注释（已做）。** 删除各模块的 "Ports `fds/xxx.go`"、🔴 标记（保留"字节敏感"的说明文字）、类型/函数文档里的 `(Go \`Xxx\`)` 与 `auth.go:xxx`、`object.go:302-322` 等行号引用（client、auth、constant、encoding、metadata、acl、bucket、object/*、manager、cslb/*）。保留与协议或兼容性有关的说明：Go `url.QueryEscape` / `url.Values.Encode()` 的编码规则、lifecycle 尾随空格是 Go SDK 的线上值、签名向量取自 Go SDK、CSLB 错误文本与 Go 一致等。

**新增测试：** `crates/fds/tests/response_models.rs`，表驱动覆盖全部响应模型（宽松 13 个、严格 7 个、`ObjectMetadata`）。宽松模型逐一检查：全部键缺失 = Default、全部键为 null = Default、追加未知键结果不变、每个键"缺失"与"null"结果相同、去掉任一键结果改变（证明每个线上名都被识别）、该键的 snake_case 拼写被当作未知键。严格模型检查必填键缺失或为 null 时报错、未知键被忽略。这套测试**先在改动前的代码上跑通**，再在新代码上通过。另外用一个临时测试（未提交）在改动前后各导出一次 Serialize 模型（LifecycleRule/Config、AccessLog、Owner、GrantKey、AccessControlList、UploadPartResponse、LifecycleBaseItem）的 Debug、序列化结果和 Default 序列化结果，两次输出 `diff` 完全一致。UA 的 wire 测试：`tests/wire.rs::every_request_identifies_the_sdk_in_user_agent`（access key 请求、带 suffix 的请求、匿名 discovery 通道都断言 `user-agent`）；`tests/client_builder.rs` 补充 UA/连接池默认值、自定义值和非法 suffix。

**依赖：** 没有新增依赖，Cargo.lock 无变化（`git diff 65bd4f7 -- Cargo.lock` 为空）。fds_cli、examples 无需改动。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/internal-cleanup`，用 `git log --grep` 查。

- 状态：实现与验证完成，分支上是 WIP 提交，等待协调者压成一笔后合并。
- `bash scripts/check.sh`（2026-11-07，本 worktree，退出码 0）：fmt；clippy `--all-features --all-targets -D warnings`；四种 feature 组合的 `cargo test -p fds -- --test-threads=1` 和 `cargo check --all-targets`；fds_cli test 和 check；`cargo doc -D warnings`；`fds_cli --help`；MSRV 1.86.0 下三种组合的 test/check 及 fds_cli check；`check_coverage.py`（35/35 YAML、33/33 操作）；`check_docs.py`（114 个本地链接，retired 扫描通过）；`git diff --check`。日志中 142 个 `test result: ok`，没有失败。
- 其他实际运行：`cargo tree --locked -e features,no-dev -p fds -i tokio`（三种 feature 组合，无 `rt-multi-thread`）；`cargo check --locked -p fds --lib --no-default-features --features cslb` / `--features manager`；改动前后的响应模型测试与序列化快照对比。
- 未验证：`tests/live_acceptance.rs` 的 2 个 live 测试（需要 `FDS_LIVE=1` 和真实凭证）；`de::lenient` 的 doctest 是 `ignore`（宏是私有的，示例无法在 doctest 中编译）。
- 依赖变化：无，Cargo.lock 未变。
- 待用户决定：无。知会下游：fluent-listing-delete 让删除流程复用 `pages()` 时须保留 `MarkerGuard::exact` 的"重复 marker 在提交该页前立即拒绝"语义（见执行记录第 4 项）。
- 协调者复验：审查 diff（`lenient!` 宏只追加原有的 `null_default`、容器级 `rename_all`/`default`、`MarkerGuard` 两种模式、`Redaction` 策略、UA 与 tokio feature）。rebase 到最新 main（revision 3 事件记录）后，完整运行 `bash scripts/check.sh`，退出码 0。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul internal-cleanup

推进 Roadmap sdk-overhaul 的任务 internal-cleanup：内部实现去冗余：serde 样板、header、脱敏、HTTP 配置。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/internal-cleanup.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：响应模型改用容器级 rename_all + default + DefaultOnNull（或内部宏），线上兼容性不变；client.rs 中合并 metadata→header helper、HeaderValue 错误转换、脱敏策略（删掉 do_json_limited 中清空 request_id 的逻辑）；分页循环检测抽成内部 helper；MultipartUpload::start 改用 validate_object；设置 UA、pool_max_idle_per_host 可配置；精简 tokio features；清理移植注释
验证：新增缺失 / null / 未知字段的反序列化表驱动测试和 UA 的 wire 测试；cargo tree -e features -p fds 检查 tokio feature；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
