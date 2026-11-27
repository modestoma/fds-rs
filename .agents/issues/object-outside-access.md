# 设置对象外网访问

- 类型：feat
- 分支：`object-outside-access`（从 main 拉出，合回 main）
- 状态：用户已在真实服务上验证，提交并向 main 发起合并请求

## 需求

SDK 和 CLI 能读取对象的外网访问状态（`allowOutsideAccess`），但不能设置。用户要求增加设置外网访问的 API。

## 决定

- **协议依据**：原始 YAML 没有这项操作。官方 Java SDK `setObjectOutsideAccess` 与 Python SDK `set_object_outside_access` 一致：`PUT /{bucket}/{object}?setOutsideAccess=true|false`，不带 body，200 为成功。证据链接（固定 commit）记在 `docs/protocol.md#对象外网访问`。
- **只做对象级**：两份官方 SDK 也有 Bucket 级 `PUT /{bucket}?setOutsideAccess=...`，本次不做，留作后续。
- **API 形状**：`Client::set_object_outside_access(bucket, object, allow: bool)`，返回 `SetObjectOutsideAccess` builder，支持 `.timeout(..)`、`.retry(..)`，`send()` 返回 `Result<()>`。放在 `object/acl.rs`，与 `set_object_public` 相邻，文档说明它不是 ACL 授权。
- **重试归类为幂等写**：请求把开关设为给定值，重复执行结果相同。Java SDK 不重试它，SDK 也默认不重试，只在 `retry_idempotent_writes()` 或 `RetryMode::Enabled` 时重试。
- **CLI**：`fds_cli object set-outside-access <bucket> <object> <true|false>`，成功输出与 `set-public` 相同；值只接受 `true` / `false`，其他值属于参数错误（退出码 2）。
- **顺带修复 wire snapshot**：0.1.1 发版改了 crate 版本，fixtures 中仍是 `user-agent: fds-rs/0.1.0`，main 上 `wire_snapshot` 测试已失败（release 提交带 `[skip ci]`，CI 没有发现）。快照改为把 user-agent 中的 crate 版本归一化为 `fds-rs/<version>`，以后发版不再需要重写 fixtures。

## 改动

- `crates/fds/src/object/acl.rs`、`object/mod.rs`：新增 builder 和 `Client` 方法并导出。
- `crates/fds/src/retry.rs`：幂等写列表加入 `set_object_outside_access`。
- `crates/fds/tests/wire_snapshot.rs` 与 fixtures：user-agent 版本归一化；新增 true / false 两条请求快照。
- `crates/fds/tests/retries.rs`：加入幂等写重试矩阵；`object_builders.rs`：加入非法输入不发送的检查。
- `crates/fds_cli/src/object.rs`、`crates/fds_cli/tests/commands.rs`：新增命令和测试。
- `README.md`、`docs/cli.md`、`docs/protocol.md`：用法与协议依据。

## 验证

见执行记录。

## 执行记录
- 2026-11-27 `bash scripts/check.sh`（分支 object-outside-access）：fmt、clippy `-D warnings`、四种 feature 组合的测试与 check、fds_cli 测试、`cargo doc -D warnings` 通过（98 个 `test result: ok`，0 失败）。`cargo package` 在沙箱中无法访问 crates.io index 而中断，放开 index 网络后单独运行其余步骤：`cargo package`、MSRV 1.86 测试与 check（72 个 `test result: ok`）、`check_coverage.py`、`check_docs.py` 通过；`git diff --check` 发现 `docs/protocol.md` 末尾多一个空行，修正后通过。
- 2026-11-27 用户本地编译 `fds_cli`，对真实 FDS 服务执行 `object set-outside-access` 并确认可以设置。
- **未验证**：外网/内网的具体判定方式、调用所需权限仍没有文档。
