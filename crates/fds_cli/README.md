# fds_cli

基于 [fds-rs](https://crates.io/crates/fds-rs) 的小米 FDS 对象存储命令行工具，覆盖 Bucket 管理、对象读写、分片上传、并发下载、预签名 URL、OAuth 与端到端 `smoke` 验证。

```sh
cargo install fds_cli --locked
fds_cli --help
```

连接参数通过 `FDS_ENDPOINT`、`FDS_ACCESS_KEY_ID`、`FDS_ACCESS_KEY_SECRET` 环境变量或同名命令行参数提供。完整命令见 [CLI 使用手册](https://github.com/modestoma/fds-rs/blob/main/docs/cli.md)。

最低 Rust 版本 1.86。许可证：MIT。
