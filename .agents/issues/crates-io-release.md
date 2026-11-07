# 发布到 crates.io 与自动化发版

- 类型：build / release
- 分支：`crates-io-release`（从 main 拉出，合回 main）
- 状态：实现完成，待用户评审与提交

## 需求

把 SDK 发布到 crates.io，用 GitHub Actions + semantic-release 自动更新版本号并发布。

## 决定

- **包名 `fds-rs`**（用户决定）。crates.io 上的 `fds` 已被无关项目占用。库名保持 `fds`，用户代码 `use fds::...` 不变；CLI 包名仍为 `fds_cli`。
- **许可证改为 MIT**（用户决定），原为 Apache-2.0。根目录和两个 crate 目录各放一份 LICENSE（cargo 只打包 crate 目录内的文件）。
- **仓库地址** `https://github.com/modestoma/fds-rs`（用户提供）。
- **不再使用内网 artifactory**：删除 `.cargo/config.toml`、`publish = ["artifactory"]` 和 `fds_cli` 依赖上的 `registry`；README 改为从 crates.io 安装。
- **版本统一**：`[workspace.package]` 定义 version 等公共字段，两个 crate 继承；`fds-rs` 作为 workspace 依赖声明，`fds_cli` 引用它。
- **0.x 版本规则**：breaking 升 minor，feat / fix / perf 升 patch，符合 Cargo 0.x 语义。
- **版本号修改用自写脚本** `scripts/set_version.py`，而不是 cargo-edit：除了 Cargo.toml，还要同步 README 与 docs 中的依赖和安装版本，`check_docs.py` 会校验它们一致。
- **crates.io 凭证用 Trusted Publishing**（OIDC），不在仓库中保存长期 token；首个版本需手动发布。
- 包内容通过 `include` 限定为 `src`、`examples`（仅 SDK）、`tests`、README、LICENSE；测试只引用 crate 内的 fixtures，打包后可独立运行。两个 crate 各有一份简短 README 供 crates.io 展示，完整手册仍是根 README。

## 改动

- `Cargo.toml`、`crates/*/Cargo.toml`、`Cargo.lock`：包名、许可证、元数据、workspace 继承、docs.rs 配置。
- `LICENSE`、`crates/fds/LICENSE`、`crates/fds_cli/LICENSE`、`crates/fds/README.md`、`crates/fds_cli/README.md`：新增。
- `README.md`、`docs/README.md`、`docs/cli.md`、`docs/validation.md`：安装方式、包名、`-p fds-rs`、许可证；新增 `docs/release.md`。
- `scripts/check.sh`：`-p fds` 改为 `-p fds-rs`，新增 `cargo package --workspace`；`scripts/check_docs.py`：从 workspace 读取版本并校验 `fds-rs` 依赖写法；新增 `scripts/set_version.py`；`scripts/benchmark_download.py` 注释中的包名。
- `.releaserc.json`、`package.json`、`package-lock.json`、`.github/workflows/ci.yml`、`.github/workflows/release.yml`：新增；`.gitignore` 忽略 `node_modules`。

## 验证

见下方执行记录。

## 需要用户完成

见 `docs/release.md` 的“首次发布准备”：建 GitHub 仓库并推送，手动发布 0.1.0，打 tag `v0.1.0`，为两个 crate 配置 Trusted Publishing。AGENTS.md 中“当前以本地 main 为基线，不推送”需要按实际情况调整。

## 执行记录

- 2026-11-08 `bash scripts/check.sh`（分支 crates-io-release，最后一次改动之后）：**退出码 0**，170 个 `test result: ok`，0 个失败，日志中没有 warning。新增的 `cargo package --workspace` 打包两个 crate 并独立编译通过。
- `cargo publish --locked --workspace --dry-run --allow-dirty`：`fds-rs`（85 个文件）、`fds_cli`（16 个文件）打包、验证均通过，上传按 dry run 中止。顺带把 Cargo.lock 中已被 yank 的 `chacha20` 0.10.1 升到 0.10.2。
- `scripts/set_version.py`：0.1.0 → 0.2.0 → 0.1.0 往返后 `check_docs.py` 通过，文件无残留差异。
- semantic-release 版本规则（在临时 clone 中 dry run，只启用 commit-analyzer）：`v0.1.0` 之后有 feat + fix → 0.1.1；再加 `feat!` → 0.2.0；没有 tag → 1.0.0（所以首次发布后必须打 `v0.1.0`）。
- **未验证**：GitHub Actions 两个 workflow 没有实际运行过（还没有远端仓库）；Trusted Publishing 和真正的 crates.io 上传要等用户完成首次发布准备。本地 `npm install` 时沙箱拒绝写入 `node_modules/tunnel/.idea/`，不影响 `package-lock.json`，CI 用 `npm ci` 全新安装。
