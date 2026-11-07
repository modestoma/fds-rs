# 发布指南

SDK 以 `fds-rs`、CLI 以 `fds_cli` 发布到 crates.io，两个包共用 workspace 版本号，一起发布。合入 `main` 的提交由 GitHub Actions 运行 semantic-release：根据提交信息计算下一个版本，更新版本号和 CHANGELOG，打 tag，发布到 crates.io，并创建 GitHub Release。

## 提交信息与版本号

提交信息使用 [Conventional Commits](https://www.conventionalcommits.org/zh-hans/) 格式。1.0 之前按 Cargo 的 0.x 语义计算版本：

| 提交 | 示例 | 版本变化 |
| --- | --- | --- |
| 不兼容改动 | `feat!: ...`，或正文含 `BREAKING CHANGE:` | 0.1.3 → 0.2.0 |
| 新功能 | `feat(object): ...` | 0.1.3 → 0.1.4 |
| 修复与性能 | `fix: ...`、`perf: ...` | 0.1.3 → 0.1.4 |
| 其他 | `docs`、`refactor`、`test`、`chore`、`ci` 等 | 不发布 |

规则写在 `.releaserc.json` 的 `releaseRules` 中。进入 1.0 时删掉这两条规则，恢复默认规则（breaking 升 major、feat 升 minor）。

使用 squash merge 时，PR 标题就是最终的提交信息，需要同样符合上述格式。

## 自动发布流程

`.github/workflows/release.yml` 在 push 到 `main` 时运行：

1. 调用 `ci.yml` 执行 `scripts/check.sh`，通过后才进入发布。
2. `rust-lang/crates-io-auth-action` 用 GitHub OIDC 换取短期 crates.io token（Trusted Publishing，仓库中不保存长期 token）。
3. semantic-release 分析上一个 tag 之后的提交；有需要发布的提交时：
   - `scripts/set_version.py` 修改 workspace 版本、`fds-rs` workspace 依赖版本，以及 README 与 `docs/` 中的依赖和安装版本；`cargo update --workspace` 刷新 `Cargo.lock`；`check_docs.py` 复核版本一致；
   - 更新 `CHANGELOG.md`，提交 `chore(release): X.Y.Z [skip ci]` 并推回 `main`，打 tag `vX.Y.Z`；
   - `cargo publish --workspace` 按依赖顺序先发布 `fds-rs`，再发布 `fds_cli`；
   - 创建 GitHub Release，正文为本次 release notes。

tag 在 `cargo publish` 之前推送。如果发布失败，tag 和 release 提交已经存在，修复问题后在该 tag 上手动执行 `cargo publish --locked --workspace`，不要删除 tag 重来。

本地检查版本规则可以执行 `npx semantic-release --dry-run --no-ci`（需要 Node.js 24.15 及以上，先 `npm ci`）。

## 首次发布准备

以下步骤只做一次，按顺序执行。

1. **创建 GitHub 仓库并推送。** 仓库为 `https://github.com/modestoma/fds-rs`，推送 `main`。
2. **设置 Actions 权限。** 仓库 Settings → Actions → General → Workflow permissions 保持默认即可，workflow 文件里已按 job 声明 `contents: write` 和 `id-token: write`。以后如果给 `main` 开启分支保护并禁止直接推送，`GITHUB_TOKEN` 无法推回 release 提交，需要改用 GitHub App token，或把 Actions 加入允许绕过的名单。
3. **手动发布 0.1.0。** Trusted Publishing 只能为已存在的 crate 配置，所以第一版需要手动发布：
   - 在 crates.io 用 GitHub 登录，验证邮箱，在 Account Settings → API Tokens 创建一个带 `publish-new` 权限的 token，用完即删；
   - 在干净的 `main` 上执行：

     ```sh
     cargo login
     bash scripts/check.sh
     cargo publish --locked --workspace --dry-run
     cargo publish --locked --workspace
     ```

4. **打 tag `v0.1.0` 并推送。** semantic-release 以这个 tag 为起点计算下一个版本；没有它会把下一次发布当作首个版本，直接发 `1.0.0`。

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

5. **配置 Trusted Publishing。** 分别在 crates.io 上 `fds-rs` 和 `fds_cli` 的 Settings → Trusted Publishing 中添加 GitHub 配置：仓库 owner `modestoma`，仓库名 `fds-rs`，workflow 文件名 `release.yml`，environment 留空。配置后删除第 3 步创建的 token。

之后合入 `main` 的 `feat` / `fix` / `perf` 提交都会自动发布新版本。

## 相关文件

| 文件 | 作用 |
| --- | --- |
| `.releaserc.json` | semantic-release 插件、版本规则、release 提交包含的文件 |
| `package.json`、`package-lock.json` | 锁定 semantic-release 及插件版本，只用于发布 |
| `scripts/set_version.py` | 发布时统一修改版本号 |
| `.github/workflows/ci.yml` | PR 检查，也被发布流程调用 |
| `.github/workflows/release.yml` | 自动发布 |
