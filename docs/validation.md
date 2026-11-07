# 验证指南

本文提供可重复执行的检查入口，不保存每轮结果。日常调用见 [SDK 手册](../README.md)，命令行操作见 [CLI 手册](cli.md)。

## 本地检查

在仓库根目录执行：

```sh
python3 scripts/check_docs.py
python3 scripts/check_coverage.py
rustup toolchain install 1.86.0 --profile minimal
bash scripts/check.sh
```

完整脚本执行格式、严格 Clippy、五组 feature 测试/目标编译、rustdoc、CLI help，以及 Rust 1.86 的核心/默认/全功能检查。测试串行执行，避免 tracing subscriber 测试互相干扰；Cargo 可能下载依赖，但普通测试不访问真实 FDS。

文档检查验证本地链接与锚点；API 索引检查验证 35 份 YAML 的文件覆盖、33 项操作映射和 SHA-256。它们不证明请求行为或线上兼容性。

## 真实对象往返

普通 `cargo test` 不访问 FDS。仅在具备专用 scratch Bucket 和对应凭据、明确准备执行
上传与精确删除后运行以下命令。不要在共享业务 Bucket 执行 smoke CLI 或其他 mutation 样例。

```sh
export FDS_LIVE=1
export FDS_ENDPOINT=https://YOUR-REGION-ENDPOINT
export FDS_LIVE_BUCKET=YOUR-SCRATCH-BUCKET
# 使用安全环境注入 FDS_ACCESS_KEY_ID / FDS_ACCESS_KEY_SECRET；不要写入命令历史。
cargo test --locked -p fds --test live_acceptance live_object_roundtrip -- --ignored --test-threads=1 --nocapture
```

每次测试在固定命名空间下生成 PID + 纳秒的独立对象名，覆盖 slash、中文、空格、加号和百分号。
验证 PUT、HEAD、GET 字节、206 Range、Bucket 裸 GET 与空/非空 prefix 列举。
无论操作成功与否均尝试删除本次精确 key，不删 Bucket、不作 prefix 清理、不修改 ACL。
日志只打印清理所需 key；进程终止或删除失败时按此 key 手动清理。
这是常规 smoke，不覆盖所有失败状态或最终一致性延迟；环境限制应记录为失败/待复核。

## OAuth 与发现验证

另行注入 `FDS_DISCOVERY_ORIGIN`、`FDS_REGION`、`FDS_SERVICE`、
`FDS_EXPECTED_DISCOVERED_ENDPOINT`（精确发现结果）、`FDS_ENDPOINT`（明确允许发送凭据的服务）、
`FDS_APP_ID`、`FDS_OAUTH_APP_ID`、`FDS_OAUTH_ACCESS_TOKEN`、`FDS_OAUTH_PROVIDER`。
XiaoMi 还需 `FDS_OAUTH_OPEN_ID`、`FDS_OAUTH_MAC_KEY`、`FDS_OAUTH_MAC_ALGORITHM`。
用预先创建的小文本 fixture 设置 `FDS_LIVE_BUCKET`、`FDS_OAUTH_FIXTURE_KEY`、
`FDS_OAUTH_FIXTURE_TEXT`，勿用大对象或敏感内容。

```sh
FDS_LIVE=1 cargo test --locked -p fds --test live_acceptance live_oauth_discovery_and_read -- --ignored --test-threads=1
```

断言 discovery 精确返回、交换 token 尚未过期、app Bucket 列举及 OAuth 读取字节。
不输出 token/内容，不自动切换 endpoint，不写对象，但交换 token 可能创建或续期凭据；provider token 的刷新与过期场景另行人工验收。

## 专项前提与证据

| 项目 | 前提和验收 | 当前自动化覆盖 |
| --- | --- | --- |
| OAuth / discovery | 按 [OAuth 协议](protocol.md#服务发现与-oauth)配置启用地区、appId、provider token；验证 exchange 到期时间与当前时钟、绑定 appId 列 Bucket 与对象读取、过期失败和显式 refresh；凭据和 URL query 不入日志 | live_oauth_discovery_and_read 只读存储数据，包含 token 交换 |
| CDN prefetch / refresh | 专用对象、CDN 已开通、所有者权限；200 只记录提交成功，另从 CDN 读取确认可见性 | 本地 wire 已覆盖；live smoke 不执行 CDN mutation |
| 归档恢复 | 专用归档对象、明确 200/202 body 与可识别的 409 discriminator | API 暂缓；不能把任意 409 当解冻中 |
| MULTI_BLOB / part-copy | 明确 mode init、源编码/Range、complete 的端到端服务契约及权限 | API 暂缓；不调用猜测协议 |
| POST 新对象 | 确认 body/form 编码、带/不带 key 的覆盖与唯一性规则 | API 暂缓 |
| ACL / lifecycle / migration / accessLog | 单独的隔离 Bucket、保存/恢复原设置、准确授权和服务支持 | 保留原 SDK API；不放入通用 smoke 自动修改 |

归档与 multipart-copy 的证据门槛见 [协议核查](protocol.md#分片复制)；
OAuth 前提见 [认证说明](protocol.md#服务发现与-oauth)。只有获得前提和授权后才开展专项真实验收。
每次记录日期、提交 SHA、工具链、endpoint/region、非敏感配置、执行命令、实际结果、
请求 ID 和清理结果。未执行填“未执行”，缺少权限/服务能力填“阻塞”，不填通过。

## CLI smoke

CLI 使用 `FDS_ENDPOINT`、`FDS_ACCESS_KEY_ID`、`FDS_ACCESS_KEY_SECRET`，见 [CLI 准备](cli.md#构建与配置)。

```sh
./target/debug/fds_cli smoke "$B"
./target/debug/fds_cli smoke "$B" --skip-multipart --skip-download
```

`B` 必须是专用可写测试 Bucket。smoke 在 `fds-smoke-<pid>-<纳秒时间戳>/` 下检查 PUT/GET/Range、存在性、元数据、复制、rename、列举、basicInfo、对象 ACL、URL、下载和分片。它会将测试对象设为公开可读；不能用于敏感对象或共享业务前缀。它不是所有 SDK 能力的完整验收，不覆盖 OAuth、CDN 和各项 Bucket 配置。

`--skip-multipart` 跳过约 6 MiB 的分片步骤，`--skip-download` 跳过下载器；`--keep` 保留对象。默认按本轮精确对象名尝试清理；进程中断、分片失败或清理失败可能留下对象/会话。只对本轮已确认的精确 key 做清理，不删除通用 `fds-smoke-` 前缀。退出码 0 表示执行项全部通过，非零表示有失败；未执行项不算通过。

## 本地下载基准

```sh
cargo build --locked -p fds --example download_memory
python3 scripts/benchmark_download.py target/debug/examples/download_memory
```

Python 在独立进程提供 64 MiB 本地 HTTP 数据源；macOS `/usr/bin/time -l` 记录 Rust 客户端 peak RSS，脚本校验长度和 MD5。结果受构建 profile、连接、缓存与本机影响，不代表真实 FDS/TLS、跨平台、断点规模或固定内存上限，也不代表上传吞吐性能。
