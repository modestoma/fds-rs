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

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/public-surface`，用 `git log --grep` 查。

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
