# 建立流式 builder 模式并迁移对象基础操作

- Task key：fluent-object（Roadmap sdk-overhaul）
- 类型：feat
- 状态、依赖：以 plan.json 为准；分支 `feat/sdk-overhaul-fluent-object` 存在即表示进行中

## 目标与范围

**问题（来自审视的"API 设计"部分，第 1 到 4 点）：**

1. **三种调用风格并存**：`PutObjectRequest::new(..).send(&client)`；`client.get_object(&req)` / `client.put_object(req)`；`client.head_object(bucket, object)`。参数所有权也不一致：`put_object` 按值传，`get_object` 按引用传。
2. **请求结构体风格不一**：`GetObjectRequest`、`CopyObjectRequest`、`SetObjectMetadataRequest`、`GetObjectACLRequest`、`SetObjectACLRequest`、`GeneratePresignedURLRequest` 是 `pub` 字段加 `#[derive(Default)]`，可以构造出 bucket 为空的请求；`PutObjectRequest` 是私有字段加 builder。
3. **命名带着 Go 的习惯**：`GetObjectACLRequest` / `SetObjectACLRequest`（应为 `Acl`）、`GeneratePresignedURLRequest`（应为 `Url`）、`does_object_exist`（应为 `object_exists`）、`generate_absolute_object_url`。
4. **重复 API**：`copy_object` 和 `copy_object_with_response`，`CopyObjectRequest::send` 和 `send_with_response`；`does_object_exist` 和 `head_object` 功能重叠；`get_object_metadata` 和 `head_object` 也重叠（两者区别在于是否带 `?metadata` 查询参数，这个区别要在文档里写清楚，或者合并）。
5. **bool 参数**：`GeneratePresignedURLRequest.cdn: bool`，改为 `.cdn()` 这样的 builder 方法，或者 `Endpoint::Cdn` 枚举。

**要做（目标形态，参照 aws-sdk-s3 的 fluent builder）：**

```rust
let out = client.get_object("bucket", "key").range(r).timeout(d).send().await?;
let meta = client.head_object("bucket", "key").send().await?;   // Option<..>
let exists = client.object_exists("bucket", "key").await?;      // 或 head_object().send().await?.is_some()
client.put_object("bucket", "key", body).content_type("text/plain").send().await?;
let receipt = client.copy_object(("sb", "sk"), ("tb", "tk")).send().await?;
let url = client.presign_object("bucket", "key").method(Method::GET).expires_in(d).cdn().build()?;
```

- 建立统一的 builder 模式：
  - builder 持有 `Client`（clone 开销低）和必填参数，字段全部私有，可选项通过链式方法设置，`send(self)` 消费 builder；
  - 本地校验（bucket / object 合法性等）在 `send()` 时统一做，返回 `InvalidInput`；
  - 所有 builder 都支持 `.timeout(Duration)`，替代 request-timeout 任务里临时加在 Get/Put 上的入口。
- 把这套模式写进 `lib.rs` 或一个内部模块的文档，作为 fluent-bucket、fluent-listing-delete、fluent-multipart-manager 的参照。
- 迁移本任务负责的对象操作：get、put（包括 `from_file`）、head / exists、copy（只返回回执）、rename、restore、prefetch、refresh、delete（单个对象）、get / set metadata、get_basic_info、get / set acl、set_public、presign、absolute URL。
- 删除旧入口：`XxxRequest::send(&client)`、`client.xxx(&req)`、以及被替代的位置参数方法。
- 命名统一为 `Acl`、`Url`，去掉 `does_` / `generate_` 一类前缀。
- `fds_cli`（主要是 `crates/fds_cli/src/object.rs`）、受影响的 tests（`object_builders.rs`、`copy_receipt.rs`、`cache_operations.rs`、`upload_bodies.rs`、`wire.rs`、`retries.rs`、`client_builder.rs`、`oauth_discovery.rs`、`sdk_operations.rs`、`live_acceptance.rs` 中的相关部分）和 examples 同步修改。

**不做：** bucket、listing、delete_objects、multipart、Downloader 的迁移（分别由后续三个任务负责）；返回值字段类型化（typed-responses 负责）。

## 对应验收标准

AC-4（对象基础操作部分），AC-7、AC-8（本任务范围内）。

## 验收条件与验证方法

- [ ] 本任务范围内的每个对象操作只有一种 `client.xxx(..)...send()` 写法；旧的 Request 结构体和 `client.xxx(&req)` 已删除。
- [ ] 公开 builder 没有 `pub` 字段，也没有 `Default`（避免构造出无效请求）。
- [ ] 命名检查：公开 API 中不再有 `ACL`、`URL`、`does_`、`generate_`、`send_with_response`。
- [ ] wire 测试证明迁移前后请求的方法、路径、查询参数、请求头、请求体逐字节一致（尤其是 put、copy、presign、acl）。
- [ ] `lib.rs` 或相关模块文档写清 builder 约定。
- [ ] fds_cli 测试通过，CLI 行为不变。

## 执行记录

2026-11-07，分支 `feat/sdk-overhaul-fluent-object`（worktree `.worktrees/fluent-object`），基于 main `c61dc57`。

**1. 先锁定线上字节。** 迁移前先在旧 API 上新增 `crates/fds/tests/wire_snapshot.rs`，单独提交（WIP 第一笔）。它用裸 TCP 监听器逐个接收请求，记录请求行、按发送顺序的全部请求头和原始 body，写入 `tests/fixtures/wire_object_ops.txt`。只归一化三处每次运行必然不同的值：`host` 的端口、`date`、`authorization` 中的签名（签名由已记录的 method、URL、头和该 date 决定，签名算法本身由 Go golden 向量固定）。presign 没有时钟，URL 连同 Signature 整串比较。覆盖 24 个请求和 8 个 URL：get（含 range + timeout）、head（200）、exists（404）、get_object_metadata、put（全部选项 + metadata + expires、空字符串选项不发送、流 body + 预计算 MD5、bytes 自动 MD5、文件 body）、copy、rename、restore、prefetch、refresh、delete、set_object_metadata、basic_info、get_object_acl（无版本 / 有版本 / 空版本不发送）、set_object_acl（add / delete + versionId）、set_object_public；presign 的 GET / HEAD / CDN / PUT+metadata / DELETE+CDN+metadata，absolute URL 的对象 / bucket / 根。对象名用 `dir/a b+c.txt` 覆盖编码。迁移后只改测试里的调用写法，fixture 文件一字节未改（第二笔提交里 `git diff --cached --quiet -- crates/fds/tests/fixtures/wire_object_ops.txt` 成立），测试通过。

**2. builder 约定（写进 `lib.rs` 的 "Operation builders" 一节，`object` 模块文档引用它）。**
- `Client` 方法只接收必填参数（bucket、object、body、ACL 等，名字用 `impl Into<String>`），返回以操作命名的 builder 类型，定义在该操作的领域模块里（如 `object::GetObject`）。
- builder 持有 `Client` 的克隆和参数；字段全部私有，不实现 `Default`，只能从 `Client` 方法创建；`#[must_use]`。
- 可选项是按值传入、按值返回的链式方法；开关是无参方法（如 `PresignObject::cdn()`），不用 bool 参数。
- `send(self)` 消费 builder；本地校验（bucket/object 名、header 值、timeout）都在 `send` 里、发送之前做，失败为 `ErrorKind::InvalidInput` + `Dispatch::NotSent`。
- 每个会发请求的 builder 都有 `timeout(Duration)`。
- 不发请求的操作以 `build()` 结尾、没有 timeout（`presign_object`）；没有可选项的本地操作是普通方法（`absolute_object_url`）。
- 内部实现：`object/mod.rs` 的 `timeout_option!`（共享的 `timeout` 方法和文档）与 `object_builder!`（只需 bucket + object 的 builder：结构体、构造函数、timeout；`send` 各自手写）；`ClientRequest::with_timeout` 统一校验并设置单请求超时。后续 fluent 任务可复用这两个宏。

**3. 迁移后的公开 API（全部在 `fds::object`，builder 名在括号内）。**
- `get_object(b, k)` → `.range(ByteRange)` `.timeout(d)` `.send() -> GetObjectOutput`（`GetObject`）
- `head_object(b, k).send() -> Option<ObjectMetadata>`（`HeadObject`）；`object_exists(b, k).send() -> bool`（`ObjectExists`，同一个 HEAD）
- `put_object(b, k, body)` → `.cache_control/.content_disposition/.content_encoding/.content_type/.content_md5/.expect/.metadata/.expires/.timeout` `.send() -> PutObjectResponse`（`PutObject`）
- `copy_object((sb, sk), (tb, tk)).send() -> CopyObjectResponse`（`CopyObject`，只返回回执）
- `rename_object(b, src, dst)`、`restore_object(b, k)`、`prefetch_object(b, k)`、`refresh_object(b, k)`、`delete_object(b, k)` → `.send() -> ()`（`RenameObject`、`RestoreObject`、`PrefetchObject`、`RefreshObject`、`DeleteObject`）
- `get_object_metadata(b, k).send() -> ObjectMetadata`（GET `?metadata`，404 是错误）、`set_object_metadata(b, k, metadata).send()`、`get_object_basic_info(b, k).send() -> ObjectBasicInfo`（`GetObjectMetadata`、`SetObjectMetadata`、`GetObjectBasicInfo`）。`head_object` 与 `get_object_metadata` 的区别写进两者的 rustdoc 和 README 表格。
- `get_object_acl(b, k).version_id(v).send() -> AccessControlList`、`set_object_acl(b, k, acl).action(AclAction).version_id(v).send()`、`set_object_public(b, k).send()`（`GetObjectAcl`、`SetObjectAcl`、`SetObjectPublic`）
- `presign_object(b, k).method(Method).expires_at(t) | .expires_in(d).cdn().metadata(m).build() -> Result<String>`（`PresignObject`；method 默认 GET；expires_at / expires_in 必须设置其一，否则 `InvalidInput`；`expires_in` 在 `build()` 时取当前时间计算；检查顺序保持原来的"有效期 → 名字 → 凭证"）。返回类型仍是 `String`，改为 `Url` 留给 typed-responses。
- `absolute_object_url(b, k) -> Result<String>`

**4. 删除。** `GetObjectRequest`、`PutObjectRequest`（含 `PutObjectRequest::from_file`）、`CopyObjectRequest`、`SetObjectMetadataRequest`、`GetObjectACLRequest`、`SetObjectACLRequest`、`GeneratePresignedURLRequest` 及它们的 `send(&client)` / `generate(&client)`；`copy_object_with_response`、`send_with_response`、`does_object_exist`、`generate_presigned_url`、`generate_absolute_object_url`；`client.xxx(&req)` 和位置参数的 async 方法。`PutObjectRequest::from_file` 没有新的等价入口：它只是 `UploadBody::from_file(path).await?` 加 `new` 的组合，现在写成 `client.put_object(b, k, UploadBody::from_file(path).await?)`，文件 body 的 wire 字节由快照覆盖。`CopyObjectResponse`、`PutObjectResponse`、`ObjectBasicInfo` 的名字和字段不变（typed-responses 范围）。

**5. 测试。** 新增 `object_builders.rs::every_object_builder_validates_names_and_timeout_before_sending`：18 个 builder 对空 bucket、空 object、零 timeout 都在发送前返回 `InvalidInput` + `NotSent`（endpoint 指向无人监听的端口，发出去就会是 Transport）；`presign_builder_keeps_milliseconds_and_validates_lifetimes` 覆盖缺少有效期、非法有效期、`expires_in` 在 build 时计算、未配置 CDN 时报 `ConfigInvalid`；`object/tests.rs` 新增 `presign_requires_a_valid_expiration`。其余 tests（wire、object_builders、copy_receipt、cache_operations、upload_bodies、retries、client_builder、oauth_discovery、sdk_operations、error_model、head_delete、pagination_options、golden、live_acceptance）只改调用写法，断言不变；`copy_receipt.rs::legacy_entry_point_validates_body` 改名为 `empty_receipt_is_outcome_unknown`（旧的"返回 `()` 的入口"已不存在，断言不变）。`client_builder.rs` 的 `get_slow_body` 参数由请求结构体改为 `Option<Duration>`。

**6. fds_cli、examples、文档。** `fds_cli/src/object.rs`、`smoke.rs` 改用新写法，命令参数、输出和退出码不变；smoke 输出里的检查标签（如 `does_object_exist`）是 CLI 输出文本，保持原样。`fds_cli/tests/commands.rs` 的 presign 期望值改用新 builder 计算。examples（quickstart、object_requests、upload_stream、upload_verify、copy_prefix、migration）改用新写法。README 中受影响的代码片段与相邻说明（上传、文件上传、读取、Range、元数据、复制、删除、ACL、预签名、CDN、OAuth、超时），`docs/open_api/README.md` 的映射列，`config.rs` / `download.rs` 中指向 `GetObjectRequest::timeout` 的文档链接同步修改。`docs/migration.md` 是要在 docs-release 删除的 0.2 迁移指南，没有改。

**7. 依赖。** 无变化，Cargo.lock 未改。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/fluent-object`，用 `git log --grep` 查。

- 状态：实现与验证完成，分支上是 WIP 提交（第一笔只有旧 API 下的 wire 快照，第二笔是迁移），等待协调者压成一笔后合并。
- `bash scripts/check.sh`（2026-11-07，本 worktree，退出码 0）：fmt；clippy `--workspace --all-features --all-targets -D warnings`；四种 feature 组合的 `cargo test -p fds -- --test-threads=1` 和 `cargo check --all-targets`；fds_cli test 和 check；`cargo doc -D warnings`；`fds_cli --help`；MSRV 1.86.0 下三种组合的 test/check 及 fds_cli check；`check_coverage.py`（35/35 YAML、33/33 操作）；`check_docs.py`（114 个本地链接，retired 扫描通过）；`git diff --check`。日志中有 149 个 `test result: ok`，没有失败，也没有 warning / error 行。`wire_snapshot` 与 `every_object_builder_validates_names_and_timeout_before_sending` 在 7 轮（stable 4 种组合 + MSRV 3 种）中都运行并通过。
- wire 字节一致：`FDS_UPDATE_WIRE_SNAPSHOT=1` 只在旧 API 上运行过一次生成 fixture 并单独提交；迁移后不带该变量运行（含 check.sh 中的 7 轮），fixture 在迁移提交中无改动。
- 命名检查：`RUSTDOCFLAGS='-D warnings' cargo doc -p fds --all-features --no-deps` 后，`target/doc/fds` 的条目文件名中没有 `ACL`、`URL`、`does_`、`generate_`、`send_with_response`；方法锚点只剩本任务范围外的 `does_bucket_exist`（fluent-bucket 负责改为 `bucket_exists`）。源码 `pub` 声明的 grep 结果相同。对象 builder 的结构体没有 `pub` 字段，也没有 `derive(Default)`（grep 核对）。
- 未验证：`tests/live_acceptance.rs` 的 2 个 live 测试（需要真实凭证，由用户本地执行；本任务只改了其中的调用写法，编译通过）；fds_cli `smoke` 命令需要真实 FDS，未运行。
- 依赖变化：无。
- 待用户决定：无。
- 协调者复验：确认 wire 快照 fixture 在迁移提交前后逐字节不变；builder 没有 `pub` 字段，也没有 `Default`。分支用 `rebase --onto` 移到改写历史后的新 main（文件内容与旧基线完全相同），之后完整运行 `bash scripts/check.sh`，退出码 0。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul fluent-object

推进 Roadmap sdk-overhaul 的任务 fluent-object：建立流式 builder 模式并迁移对象基础操作。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/fluent-object.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：新建 builder 基础设施（builder 持有 Client 和必填参数，私有字段，send(self)），迁移 crates/fds/src/object/{basic,put,metadata,acl,presign,delete(仅单对象)}.rs 的操作；命名改为 Acl/Url/object_exists；copy 只返回回执；删除旧 Request + client.xxx(&req) 入口；把约定写进模块文档；同步 tests/examples/fds_cli/src/object.rs
验证：wire 测试对比迁移前后请求逐字节一致；检查公开 API 命名；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
