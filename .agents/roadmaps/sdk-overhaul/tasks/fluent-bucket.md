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

## 对应验收标准

AC-4（bucket 与 OAuth 部分），AC-7、AC-8（本任务范围内）。

## 验收条件与验证方法

- [ ] bucket 和 OAuth 操作只有流式 builder 一种写法，没有 `pub` 字段加 `Default` 的请求结构体。
- [ ] 可选参数用 `Option` 或 builder 方法表达，不再用空字符串表示缺省。
- [ ] wire 测试证明 create_bucket（包括 acl 头和 storageClass、orgId 查询参数）、migrate、lifecycle、access log、token 交换的请求逐字节一致。
- [ ] fds_cli 测试通过。

## 执行记录

尚未开始。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/fluent-bucket`，用 `git log --grep` 查。

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
