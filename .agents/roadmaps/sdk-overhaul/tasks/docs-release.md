# 文档、示例与版本号 0.1.0

- Task key：docs-release（Roadmap sdk-overhaul）
- 类型：docs
- 状态、依赖：以 plan.json 为准；分支 `docs/sdk-overhaul-docs-release` 存在即表示进行中

## 目标与范围

**要做：**

1. **版本号**：`crates/fds/Cargo.toml` 和 `crates/fds_cli/Cargo.toml` 的 version 从 0.2.0 改为 0.1.0（用户决定）；fds_cli 对 fds 的依赖版本同步修改；Cargo.lock 更新。
2. **README.md**（约 1000 行）：确认超时一节反映 upload-idle-timeout 后的最终语义（上传与下载都是按进度重置的空闲计时，不再提示"上传大文件需调大 read_timeout"）；所有示例改为流式 builder API、新的错误处理方式（kind / is_not_found）、新的返回值类型；删掉与已移除 API 相关的段落；补上超时语义（默认无整体超时）、重试策略（只读默认 + 可选幂等写）、凭证 provider 的说明。
3. **docs/**：
   - `docs/runtime.md`：超时、重试、Downloader 的 IO 模型；
   - `docs/protocol.md`、`docs/validation.md`、`docs/cli.md`：与新 API 和新行为对齐；
   - `docs/migration.md`：删除（用户决定不写迁移指南，现有内容是已过时的 0.2 迁移指南），并清理 `docs/README.md`、`docs/runtime.md`、`crates/fds/src/lib.rs` 中对它的引用；
   - `docs/open_api/`：只核对引用，不改协议资料。
4. **examples/**：确认 12 个示例都使用最终 API、能编译，并在 rustdoc 或 README 中被引用。
5. **crate 级 rustdoc**（`lib.rs`）：快速上手、builder 约定、错误处理、feature 说明；删除过时的说明，比如"POST object creation ... remain deferred"如果仍然成立，就挪到"未支持的功能"一节。
6. **脚本**：`scripts/check_docs.py`、`scripts/check_coverage.py` 跟随新 API 调整（如果它们检查特定的 API 名字）；`scripts/check.sh` 中不再需要的步骤（比如已删除的 feature 组合）同步更新。
7. **最终全量验证**：完整运行 `scripts/check.sh`，结果写进执行记录；为整个 Roadmap 的 AC-1 到 AC-10 逐条整理验证证据，供 `accept` 阶段使用。

**不做：** 不发布到 artifactory；不打 tag。

## 对应验收标准

AC-10（全部），AC-8（全量），并为 AC-1 到 AC-9 汇总验证证据。

## 验收条件与验证方法

- [ ] 两个 crate 的 version 都是 0.1.0。
- [ ] README 和 docs 中的代码片段都能对应到实际存在的 API（`check_docs.py` 通过，并人工抽查）。
- [ ] `scripts/check.sh` 全量通过；没跑的部分如实列出；live_acceptance 由用户本地执行，Agent 不运行。
- [ ] 在 `acceptance.md` 草稿中按 AC 整理证据（只整理，不代替用户验收）。

## 执行记录

2026-11-08，分支 `docs/sdk-overhaul-docs-release`（worktree `.worktrees/docs-release`），基于 main `122581d`。

**1. 版本号。** `crates/fds`、`crates/fds_cli` 的 version 和 fds_cli 对 fds 的依赖版本改为 0.1.0，Cargo.lock 随之更新（只有两个包的版本行变化，没有依赖变化）。User-Agent 是 `fds-rs/<CARGO_PKG_VERSION>`，因此四个 wire 快照 fixture（`wire_object_ops.txt`、`wire_bucket_ops.txt`、`wire_listing_delete.txt`、`wire_multipart_ops.txt`）中的 `user-agent: fds-rs/0.2.0` 用 sed 逐行替换为 `0.1.0`，其余字节不变（没有用 `FDS_UPDATE_WIRE_SNAPSHOT` 重新生成）。

**2. README。** 版本 1.0.0 的说法全部改为 0.1.0（依赖示例、feature 示例、`cargo install` 命令），并如实说明通过内网 registry 分发；"请求与所有权"一节的旧 `Request::new(..).send(&client)` 描述改写为"调用约定与所有权"（builder 约定、`send()` 校验时机、`timeout` / `retry`、`build()`、托管传输、类型化返回值）；feature 表的核心一行补上凭据 provider 与可选幂等写重试；`StorageTokenProvider` 一段补充 `with_token` 不适用半寿命上限的已知边界；删除不存在的 `MIN_PART_SIZE` / `MAX_PART_SIZE` 常量说法；新增"全部示例程序"表，列出并链接 12 个 examples；许可证一节移到末尾并加入目录。其余各节（超时、重试、错误、元数据、凭据等）前面任务已按最终 API 更新，逐节核对后未改。30 个 Rust 代码块抽成临时 example（每块一个 `mod`），`cargo build -p fds --all-features --example` 编译通过后删除，未提交。另用脚本把 README 与 docs 中反引号内的标识符对照源码，只剩环境变量、片段内函数名和 Java 类型名。

**3. docs。** 删除 `docs/migration.md`；`docs/README.md` 删除其索引行，补充 rustdoc 入口；`docs/runtime.md` 删除迁移指南引用，改写请求生命周期（builder 约定、凭据 provider 每次尝试前取凭据），删除已移除的 raw response 说法，`expires_in` 改为 `build()` 时计算，"重试与超时"拆成"超时"（默认无整体期限、三阶段空闲超时、dispatch、`request_timeout` / `.timeout` 的覆盖范围、托管传输无单请求超时）和"重试"两节；`docs/protocol.md` 的 OAuth 一段补上 `StorageTokenProvider` 续期；`docs/validation.md` 把"五组 feature"改为实际的四组并说明 check_docs 的新检查；`docs/cli.md` 改正"SDK 不重试写操作"（CLI 的 `--retries` 不开启幂等写重试）并说明空闲超时语义。`docs/open_api/` 只核对了链接与 SDK 映射，与当前 API 一致，未改。

**4. examples。** 12 个示例都使用最终 API，在四种 feature 组合下编译。`migration.rs` 是旧迁移指南的配套样例，按用户删除迁移指南的决定改名为 `multipart_recovery.rs`（内容不变，只改说明和函数名）；`download.rs`、`quickstart.rs` 的 `GO_FDS_TEST_*` 与 `object_requests.rs`、`upload_multipart.rs` 的 `FDS_ACCESS_KEY` / `FDS_SECRET_KEY` 统一为 README 与 CLI 使用的 `FDS_ENDPOINT` / `FDS_ACCESS_KEY_ID` / `FDS_ACCESS_KEY_SECRET`（`download.rs`、`quickstart.rs` 不再默认 cnbj1 endpoint）；`oauth.rs` 注释指向 `StorageTokenProvider`。全部示例在 README 中链接，并在 crate rustdoc 的 Examples 一节列出。

**5. crate rustdoc（`lib.rs`）。** 新增 Quick start（构建 Client、上传、流式读取、按 `ErrorKind` 处理错误）、Errors、Timeouts, retries and credentials（含 `with_token` 已知边界）、Features、Examples、Unsupported features 各节；保留 Module layout 与 Operation builders；删除对 `docs/migration.md` 的引用，"POST object creation ... remain deferred" 移入 Unsupported features（同时列出归档解冻、分片复制、list-parts 恢复、未知长度上传、条件写、版本化读写）。

**6. 脚本。** `check_docs.py`：退役扫描增加 `migration.md`、旧错误类型、旧 `*Request` 结构体、`into_raw`、`_with_response`、`send(&client)`、`fds::download`、`GO_FDS_TEST`；新增检查：每个 `crates/fds/examples/*.rs` 都在 README 中链接，两个 crate 版本一致，README 与 docs 中的 `fds` / `fds_cli` 依赖版本和 `--version` 安装版本等于 crate 版本。`check_coverage.py` 不涉及改动的 API 名或文件，未改。`check.sh` 的步骤（四种 feature 组合、tracing 串行测试、MSRV）仍然都需要，未改。

**7. 验证（都实际运行）。**
- `python3 scripts/check_docs.py`、`python3 scripts/check_coverage.py`：通过。
- README 30 个 Rust 代码块的临时编译：通过（见第 2 条）。
- `bash scripts/check.sh`（最后一次代码与文档改动之后，提交后的工作区）：**退出码 0**。fmt、clippy `-D warnings`、四种 feature 组合的 fds test/check、fds_cli test/check、`cargo doc -D warnings`、`fds_cli --help`、MSRV 1.86.0 三种组合的 fds test/check 与 fds_cli check、check_coverage、check_docs、`git diff --check`；日志中 170 个 `test result: ok`、0 个失败。
- rustdoc 公开项审计：104 个公开项无重复名称，无内部模块、无旧命名、无 `reqwest::` 类型（详见 acceptance.md）。
- AC-1 到 AC-10 的证据整理在 `acceptance.md` 草稿中，只整理证据，不写验收结论。

**未验证。** `tests/live_acceptance.rs` 的 2 个 live 测试和 fds_cli `smoke`（需要真实 FDS 凭证，用户本地执行）；`cargo install` 从 artifactory 安装 0.1.0（未发布）。

**范围外的发现（未改动）。** `.claude/skills/fds-cli/SKILL.md` 仍让用户设置 `GO_FDS_TEST_ENDPOINT` 等变量（且 endpoint 不带 scheme），而 fds_cli 实际读取 `FDS_ENDPOINT`、`FDS_ACCESS_KEY_ID`、`FDS_ACCESS_KEY_SECRET` 并要求完整 origin；该文件不在本任务范围内，且 Agent 无写权限。

## 交付

合并方式：local，验证后快进合并进 main，不推送。不写 commit hash：本任务的提交信息末行是 `Roadmap: sdk-overhaul/docs-release`，用 `git log --grep` 查。

- 状态：实现与验证完成，分支上是 WIP 提交，等待协调者合并；未发布、未打 tag。
- 改动：`crates/fds/Cargo.toml`、`crates/fds_cli/Cargo.toml`、`Cargo.lock`、四个 wire fixture 的 user-agent 行；`README.md`；`docs/README.md`、`runtime.md`、`protocol.md`、`validation.md`、`cli.md`，删除 `docs/migration.md`；`crates/fds/src/lib.rs`；examples（`migration.rs` → `multipart_recovery.rs`，另 5 个改注释与环境变量名）；`scripts/check_docs.py`；`acceptance.md` 草稿；本文档。
- `bash scripts/check.sh`：退出码 0（详见执行记录第 7 条）。
- 未验证：live_acceptance 与 fds_cli smoke（用户本地执行）。
- 待用户决定：无。备注：example 改名与环境变量统一属于删除迁移指南后的清理，如不希望改名可回退；`.claude/skills/fds-cli/SKILL.md` 的环境变量说明已过时（范围外）。
- 协调者复验：wire fixture 的改动只有 91 行 `user-agent: fds-rs/0.2.0` → `0.1.0`（版本号连带变化），其余字节不变；两个 crate 的 version 都是 0.1.0；确认分支没有改动 plan.json 和 roadmap.md；完整运行 `bash scripts/check.sh`，退出码 0。

## 启动 prompt

在新会话里粘贴下面的内容即可开始或继续本任务。要点和验证由 Agent 在 plan 时填写，计划修订后同步更新。在 Codex 里把第一行的 `/roadmap-workflow` 换成 `$roadmap-workflow`。

```text
/roadmap-workflow run sdk-overhaul docs-release

推进 Roadmap sdk-overhaul 的任务 docs-release：文档、示例与版本号 0.1.0。
先读 .agents/roadmaps/sdk-overhaul/roadmap.md 和 tasks/docs-release.md；基线、合并方式、依赖和状态以 plan.json 为准。
要点：版本号改为 0.1.0（两个 crate 加 fds_cli 对 fds 的依赖）；按最终 API 重写 README、docs/runtime.md、protocol.md、validation.md、cli.md 和 lib.rs 的 crate 文档；删除 docs/migration.md 及其引用；核对 examples；调整 check_docs.py/check_coverage.py/check.sh；完整运行 check.sh 并整理 AC 证据
验证：完整运行 scripts/check.sh；check_docs.py 通过；人工抽查 README 示例；scripts/check.sh 中的全部检查（fmt、clippy -D warnings、四种 feature 组合的 fds 测试与 check、fds_cli 测试、cargo doc -D warnings、MSRV 1.86、check_coverage/check_docs、git diff --check）；缺网络或工具链的项如实标为未验证
只做这一个任务，交付后停下汇报；遇到超出任务范围的决定先记疑问再问我。
```
