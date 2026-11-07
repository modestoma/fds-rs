# fds-rs

[![crates.io](https://img.shields.io/crates/v/fds-rs.svg)](https://crates.io/crates/fds-rs) [![docs.rs](https://img.shields.io/docsrs/fds-rs)](https://docs.rs/fds-rs)

FDS（File Data Storage）对象存储的异步 Rust SDK，基于 Tokio 和 reqwest，支持 Bucket 管理、对象读写、分页、ACL、分片上传、断点下载、预签名 URL 和 OAuth。

```sh
cargo add fds-rs
```

包名是 `fds-rs`，库名是 `fds`：

```rust
use fds::Client;

async fn upload() -> fds::Result<()> {
    let client = Client::builder()
        .endpoint("https://fds.example.com") // 替换为实际 endpoint
        .credentials("ACCESS_KEY_ID", "ACCESS_KEY_SECRET")
        .build()?;
    client.put_object("bucket", "hello.txt", b"hello".to_vec()).send().await?;
    Ok(())
}
```

| feature | 默认开启 | 说明 |
| --- | --- | --- |
| `manager` | 是 | 并发下载、断点续传与请求限流 |
| `cslb` | 否 | 通过内部 CSLB DNS resolver 解析主机 |

最低 Rust 版本 1.86。完整中文调用手册见 [GitHub README](https://github.com/modestoma/fds-rs#readme)，API 文档见 [docs.rs](https://docs.rs/fds-rs)。

许可证：MIT。
