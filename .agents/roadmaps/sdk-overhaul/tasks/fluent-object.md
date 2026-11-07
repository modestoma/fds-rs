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

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/fluent-object`，用 `git log --grep` 查。

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
