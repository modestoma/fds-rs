# 收缩公开 API 面并清理死代码

- Task key：public-surface（Roadmap sdk-overhaul）
- 类型：refactor
- 状态、依赖：以 plan.json 为准；分支 `refactor/sdk-overhaul-public-surface` 存在即表示进行中

## 目标与范围

**问题（来自审视的"API 设计 · 公开面过大"部分）：**

1. `pub mod constant`（`crates/fds/src/constant.rs`）暴露了大量协议内部常量，包括：
   - `TEMP_FILE_PREFIX`、`FILE_PERM_MODE`（Go 版本遗留，本 crate 中未使用或只在内部使用）；
   - `VERSION = "0.9.0"`，这是 Go SDK 的版本号，对 Rust 用户有误导；
   - `MAX_PART_SIZE = 5GiB`、`MIN_PART_SIZE = 5MiB`，与 `MultipartOptions::new` 的 64 MiB 单片上限矛盾；
   - `URL_*_SUFFIX`、`MODE_*`、全部 `HTTP_HEADER_*`。
2. `pub mod encoding` 暴露了 `go_query_escape`、`QueryValues`（字段也是 `pub`）、`format_rfc822`、`parse_range` / `HttpRange`。其中 `parse_range` / `HttpRange` / `format_rfc822` 在 src、tests、examples 和 fds_cli 中**都没有使用**，而且功能和 `ByteRange` 重复。
3. `pub mod auth` 暴露了 `sign()` 和 `HeaderLookup`：签名是实现细节，公开出去等于把协议细节变成 semver 承诺。
4. 自定义的 `HttpMethod` 枚举（`constant.rs:63`）可以直接换成 `http::Method`。presign 需要 method，接受 `http::Method` 即可。
5. 同一类型有多条导出路径：既有 `pub mod object` / `bucket` / `acl` / `download` / `upload` / `multipart` / `oauth` / `retry` / `metadata` / `config` / `client` / `error`，`lib.rs` 又把它们平铺 re-export 到根下，用户能写 `fds::object::PutObjectRequest`，也能写 `fds::PutObjectRequest`。
6. `pub use tokio_util::sync::CancellationToken`（`multipart.rs:9`）把第三方类型从 SDK 根导出，形成隐式的版本耦合。
7. `impl AsRef<Client> for Client`（`client.rs:37`）只是为了服务 `Downloader::new(impl AsRef<Client>)`，属于不必要的公开 impl。
8. `bucket.rs` 中的 `ACTION_*` 常量和 `LifecycleAction = HashMap<String, LifecycleBaseItem>` 是字符串加 map 的弱类型设计（包括 Go 遗留的带尾随空格的值 `"abortIncompleteMultipartUpload "`）。本任务只把它们收进模块内部，或改成枚举加 serde rename，线上格式保持不变。

**要做：**

- `constant`、`encoding`、`auth`、`de` 改为 `pub(crate)`；删除死代码（`parse_range`、`HttpRange`、`format_rfc822`、未使用的常量、`VERSION`）；`MAX_PART_SIZE` / `MIN_PART_SIZE` 要么删除，要么和 MultipartOptions 的限制统一（任务里说明依据）。
- 用 `http::Method` 替换 `HttpMethod`（内部 ClientRequest 也一样）。
- 确定公开模块布局：根下只导出 `Client`、`ClientBuilder`、`Error`、`ErrorKind`、`Result` 等核心类型；操作相关的 builder 和模型放在少量公开模块下（例如 `fds::types` 或按领域划分），每个类型**只有一条**导出路径。把这条约定写进 `lib.rs` 的模块文档，后续 fluent-* 任务照此执行。
- `CancellationToken`：改为接受 `tokio_util` 的 token，但不从根 re-export；或者改为接受任意 `Future<Output = ()>` 作为取消信号（推荐，不暴露第三方类型）。由任务决定并记录。
- 删除 `impl AsRef<Client> for Client`，`Downloader::new` 改为接受 `Client`。
- lifecycle action 改为枚举，线上值不变。
- `fds_cli`、tests、examples 同步修改。

**不做：** 不改操作的调用形态，不改返回值模型。

## 对应验收标准

AC-5（全部），AC-7、AC-8（本任务范围内）。

## 验收条件与验证方法

- [ ] `cargo doc` 生成的公开项中不再有 `constant`、`encoding`、`auth` 模块，也没有 `HttpMethod`、`HttpRange`、`parse_range`、`format_rfc822`、`VERSION`。
- [ ] 每个公开类型只有一条导出路径（检查 `lib.rs` 的 re-export，`cargo doc` 中不出现重复条目）。
- [ ] `rustc -W unreachable_pub` 或 clippy 不报未使用的公开项（如果启用这条 lint 会产生大量噪音，也可以人工核对后在执行记录里说明）。
- [ ] lifecycle 的 golden / wire 测试证明序列化后的 JSON 和改动前逐字节一致，包括带尾随空格的 action key。
- [ ] 签名的 golden 测试（`tests/golden.rs`）在 `auth` 改为 crate 内部后仍然可以运行：如果它依赖 `fds::auth::sign`，就把它改成单元测试或 `#[doc(hidden)]` 的测试入口，并说明原因。

## 执行记录

2026-11-07，分支 `refactor/sdk-overhaul-public-surface`（worktree `.worktrees/public-surface`），基于 main `01aa086`。

**公开模块布局（约定已写进 `crates/fds/src/lib.rs` 的 "Module layout" 一节）：**

- 根只导出客户端与错误核心：`Client`、`ClientBuilder`、`ClientConfiguration`、`RetryPolicy`、`Error`、`ErrorKind`、`Dispatch`、`ResponseDetails`、`Result`。
- 操作的输入、输出和模型按所作用的资源放进唯一的公开领域模块：
  - `fds::object`：单对象操作、列举、批量删除、预签名，以及 `ObjectMetadata`、`UploadBody`、`GetObjectOutput`、`ByteRange`、`StorageClass`（`StorageClass` 从 `constant.rs` 移到 `object/mod.rs`）；
  - `fds::bucket`：bucket 操作、lifecycle、access log；
  - `fds::acl`：bucket 和对象共用的 ACL 模型；
  - `fds::multipart`：底层分片请求（原先在 `object` 下导出的 Init/UploadPart/Complete 请求与响应）和托管的 `MultipartUpload`；
  - `fds::oauth`；`fds::manager`（`manager` feature，内部 `download` 子模块改为私有）；`fds::cslb`（`cslb` feature）。
- 定义所在的实现模块（`client`、`config`、`error`、`retry`、`download`、`metadata`、`upload`）全部私有，只通过上面一条路径导出；不 re-export 第三方 crate。`error.rs` 原来的模块文档移到 `Error` 类型文档上。后续 fluent-* 任务新增的 builder 放进对应领域模块，同样只有一条路径。

**改为内部或删除的项目：**

- `constant`、`encoding`、`auth`、`de`（以及 `client` 等实现模块）改为私有 `mod`。在 crate 根，私有 `mod` 与 `pub(crate) mod` 等价；模块内的 `pub` 项也改成了 `pub(crate)`。
- 删除：`encoding::parse_range`、`HttpRange`（整个 `range.rs`）、`format_rfc822`、从未被实现的 `ToQuery` / `ToHeaders` trait（整个 `header.rs`）、`QueryValues` 的公开字段和未用的 `is_empty`；常量 `VERSION`、`MAX_PART_SIZE`、`MIN_PART_SIZE`、`FILE_PERM_MODE`、`TEMP_FILE_PREFIX`、`URL_COM/NET/CDN_SUFFIX`、`MODE_MULTI_BLOB`、`MODE_DIRECT`；`HttpMethod`；`bucket::ACTION_*` 常量和 `LifecycleAction` map 别名；`impl AsRef<Client> for Client`；`multipart` 中对 `CancellationToken` 的 re-export。`encoding` 的 golden 向量测试保留，原来用 `format_rfc822` 生成的值改成字面量 `"01 Jan 18 01:01 UTC"`。
- `MAX_PART_SIZE` / `MIN_PART_SIZE` 选择删除，没有与 `MultipartOptions` 统一。依据：两者在 src、tests、examples、fds_cli 中都没有使用；数值是 Go SDK 对服务端限制的描述（5 GiB / 5 MiB），而 `MultipartOptions::new` 的 64 MiB 上限是本地内存资源限制（文档写明"不是服务端约束"），两者含义不同；服务端真实限制没有经过验证，不应作为常量承诺。
- `auth::sign` / `HeaderLookup` 改为 `pub(crate)`。`tests/golden.rs` 原来调用 `fds::auth::sign`，签名向量这部分移到 `auth.rs` 的单元测试 `auth::tests::golden_signatures`（通过 `include_str!` 读取同一份 `tests/fixtures/golden.json`，共 8 组）。选单元测试而不是 `#[doc(hidden)]` 入口，是为了不在公开 API 里留后门；绝对 URL 和预签名向量仍在 `tests/golden.rs` 里走公开 API。

**`HttpMethod` → `http::Method`：** `GeneratePresignedURLRequest` 的 `method` 字段、`new` / `expires_in` 参数，以及内部 `ClientRequest.method` 都改成 `http::Method`（与 reqwest 0.12 的 `Method` 是同一类型，`send` 直接传入，不再转换）。`Method` 不是 `Copy`，重试循环里每次 attempt 克隆一次。`fds` 不 re-export `http`，调用方自己依赖 `http = "1"`；`fds_cli` 为此新增 `http = "1"` 依赖，Cargo.lock 只多了 `fds_cli → http` 这一条依赖边（http 1.x 早已在锁文件中，没有引入新包）。CLI 的 `parse_method` 仍只接受原来的 5 个方法名，行为不变。

**取消信号（最终方案）：** `MultipartUpload::upload` 改为 `upload<C: Future<Output = ()>>(&mut self, cancel: C)`，不暴露任何第三方类型。内部把它 `fuse()` 后 pin 住，在分片循环和 completion 的 `select!` 中轮询，循环结束后用 `now_or_never()` 代替原来的 `is_cancelled()`。语义与原 token 一致：信号完成后返回 `Cancelled`（completion 进行中则为 `OutcomeUnknown`），并照常执行 `CleanupPolicy`。不需要取消时传 `std::future::pending()`；tokio-util 用户传 `token.cancelled()`，超时可传 `tokio::time::sleep(..)`。`fds_cli upload --multipart` 改为直接传入一个"等待 Ctrl-C 后打印提示"的 async 块，提示文本和顺序与原来相同。

**`Downloader::new`：** 参数改为 `Client`（不再是 `impl AsRef<Client>`）；`tests/manager.rs` 原来传 `Arc<Client>`，改为直接传 `Client`。

**Lifecycle action 枚举：** 新增 `bucket::LifecycleAction`（`#[non_exhaustive]`），变体为 `AbortIncompleteMultipartUpload`（线上值 `"abortIncompleteMultipartUpload "`，保留尾随空格）、`Expiration`、`NonCurrentVersionExpiration` 和 `Other(String)`。`LifecycleRule.action` 改为 `BTreeMap<LifecycleAction, LifecycleBaseItem>`，字段名和 JSON key `actions` 不变。Serialize / Deserialize / Eq / Ord / Hash 都按线上字符串手写实现，没有用 derive 加 `serde(rename)`，原因有两点：（1）未知 key，包括不带空格的 `"abortIncompleteMultipartUpload"`，必须经 `Other` 原样往返，否则 get → set 会改变线上值；（2）按字符串排序后，多个 action 的输出顺序与 Go `encoding/json` 对 map key 排序的结果一致（原来的 `HashMap` 顺序不确定）。逐字节测试 `bucket::tests::lifecycle_wire_bytes_round_trip` 的期望字节先在**改动前的代码**上跑通，再在新代码上验证，覆盖 3 个已知 key、不带空格的变体、未知 key、空 map 以及 `{"rules":[...]}` 整体；另外 `lifecycle_typed_actions_match_wire_bytes` 用枚举构造的规则与同一批字节比对，`lifecycle_multiple_actions_use_sorted_go_key_order` 固定多 action 时的顺序。

**CSLB resolver 结论：** `CslbResolver`（实现 `reqwest::dns::Resolve`）改为 `pub(crate)`，reqwest 类型不再出现在公开 API 中；Client 仍在 `cslb` feature 下自动安装它，可配置的仍只有 `ClientBuilder::cslb_max_nodes`。`fds::cslb` 中的其他负载均衡构件（`LoadBalancer`、`Service`、`Strategy`、`Group`、`Metrics` 等）README 明确称为"独立工具"，本任务保持公开；是否整个模块改为内部（届时 `HashedStrategy`、`StaticService` 等会变成死代码需要删除）留给用户决定，见交付中的待决问题。

**同步修改：** tests、examples、fds_cli 的导入全部改为新路径（fds_cli 原有的 `use fds::*` 改为按需 glob 对应领域模块）；README 中受影响的代码片段（各段 `use fds::{..}` 的路径、预签名的 `Method`、托管分片的取消 future、lifecycle 枚举、`Downloader::new(client.clone(), ..)`、CSLB 说明）和 `docs/protocol.md` 中关于 `encoding::parse_range` 的一句。`docs/migration.md` 留给 docs-release 删除，本任务没有修改。

**未纳入本任务、留给后续任务的公开面问题（只记录，没有改动）：** `GetObjectOutput::into_raw()` 返回 `reqwest::Response`（typed-responses 任务提供 Stream / AsyncRead 适配时一起处理）；`DownloadOptionsBuilder::request_limiter` 接受 `Arc<governor::DefaultDirectRateLimiter>`，同样是第三方类型（fluent-multipart-manager 任务处理）；`ClientConfiguration::endpoint` 返回 `url::Url`。

**验证（均在本 worktree 实际运行）：**

- 公开项审计：`RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --all-features --no-deps` 通过；解析 `target/doc/fds/all.html` 得到 90 个公开项，按名称去重**没有重复**，其中没有 `constant`、`encoding`、`auth`、`HttpMethod`、`HttpRange`、`parse_range`、`format_rfc822`、`VERSION`、`CancellationToken`、`CslbResolver`、`MAX/MIN_PART_SIZE`、`ACTION_*`、`HeaderLookup`。
- `cargo rustc --locked -p fds --all-features --lib -- -W unreachable-pub`：0 条警告。clippy `-D warnings` 也没有 dead_code 或未使用项的报错。
- `cargo test -p fds --lib bucket`（lifecycle 逐字节测试，改动前后都跑过）、`cargo test -p fds --lib golden`（`auth::tests::golden_signatures`、`encoding::query::tests::encode_golden_vector`）、`cargo test -p fds --test golden`（`golden_absolute_urls`、`golden_presigned_urls`）：全部通过。
- README 中 29 个 rust 代码块：抽成一个临时 example（`mod snippet_N { .. }`），用 `cargo build -p fds --all-features --example` 编译通过后删除。该文件没有提交。
- `bash scripts/check.sh` 全部通过，结果见交付一节。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/public-surface`，用 `git log --grep` 查。

- 状态：实现与验证完成，分支上是 WIP 提交，等待协调者压成一笔后合并。
- `bash scripts/check.sh`（2026-11-07，本 worktree，退出码 0）：fmt；clippy `--all-features --all-targets -D warnings`；四种 feature 组合（默认、`--no-default-features`、`--all-features`、`--no-default-features --features cslb`）的 `cargo test -p fds -- --test-threads=1` 和 `cargo check --all-targets`；`cargo test -p fds_cli --all-features` 和 check；`cargo doc -D warnings`；`fds_cli --help`；MSRV 1.86.0 下三种组合的 test 和 check，以及 fds_cli check；`check_coverage.py`（35/35 YAML、33/33 操作）；`check_docs.py`（114 个本地链接，retired 扫描通过）；`git diff --check`。日志中共有 135 个 `test result: ok`，没有失败。依赖 mockito 的本地端口测试这次都实际运行并通过。
- 被忽略、未验证的测试：`tests/live_acceptance.rs` 中的 2 个 live 测试需要 `FDS_LIVE=1` 和真实凭证，按决定记录由用户本地执行；`de::null_default` 的 doctest 本来就是 `ignore`。本任务只确认 `live_acceptance.rs` 改了导入路径后能编译。
- 依赖变化：`fds_cli` 新增 `http = "1"`；Cargo.lock 只在 `fds_cli` 的依赖列表中多了 `"http"` 一行。
- 待用户决定：`fds::cslb` 中除 resolver 以外的独立负载均衡工具是否也改为内部（这样会删掉 `HashedStrategy`、`StaticService` 等没有被 Client 使用的代码，README 中"独立工具位于 `fds::cslb`"一句也要删除）。本任务保持现状。
- 协调者复验：审查 lib.rs 的模块布局（根下 9 个导出 + 7 个公开领域模块，实现模块私有），在任务 worktree 中完整运行 `bash scripts/check.sh`，退出码 0。cslb 独立工具是否改为内部，已作为待答问题 `cslb-public-tools` 交给用户决定。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul public-surface

推进 Roadmap sdk-overhaul 的任务 public-surface：收缩公开 API 面并清理死代码。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/public-surface.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：lib.rs 收缩 pub mod 并确定唯一导出路径约定（写进模块文档）；constant.rs/encoding/auth.rs 改为 pub(crate) 并删除死代码；HttpMethod 换成 http::Method；CancellationToken 不再从根 re-export；删除 AsRef<Client> impl；bucket.rs 的 lifecycle action 改为枚举（线上值不变）；同步 tests/examples/fds_cli
验证：cargo doc 检查公开项；golden.rs 与 lifecycle 序列化逐字节对比测试；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
