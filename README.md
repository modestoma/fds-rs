# FDS Rust SDK

小米 FDS 对象存储的异步 Rust SDK，基于 Tokio 和 reqwest，支持 Bucket 管理、对象读写、分页、ACL、分片上传、断点下载、预签名 URL 和 OAuth。

适用于 Rust 服务端、命令行和原生客户端；当前不提供浏览器/WASM 接口或 Kotlin/Swift 封装。终端应用不要内置长期 Access Key Secret，可使用服务端签发的短期预签名 URL，或已开通的 OAuth 认证。

本文是 `fds` 1.0.0 的完整调用入口。示例函数只有被调用时才执行请求，服务权限与地区支持以实际 FDS 环境为准。
## 目录

- [依赖与运行环境](#依赖与运行环境)
- [客户端配置与通用调用约定](#客户端配置与通用调用约定)
- [Bucket 管理](#bucket-管理)
- [对象上传](#对象上传)
- [对象读取与范围下载](#对象读取与范围下载)
- [对象信息与元数据](#对象信息与元数据)
- [对象列举与分页](#对象列举与分页)
- [复制、重命名与误删恢复](#复制重命名与误删恢复)
- [单对象、批量与前缀删除](#单对象批量与前缀删除)
- [Bucket 与对象 ACL](#bucket-与对象-acl)
- [手动分片上传](#手动分片上传)
- [托管文件分片上传](#托管文件分片上传)
- [并发下载与断点续传](#并发下载与断点续传)
- [预签名与普通 URL](#预签名与普通-url)
- [生命周期与访问日志](#生命周期与访问日志)
- [CDN 预取与刷新](#cdn-预取与刷新)
- [服务发现与 OAuth](#服务发现与-oauth)
- [错误处理、超时与重试](#错误处理超时与重试)
- [CSLB 负载均衡](#cslb-负载均衡)
- [组合功能示例](#组合功能示例)
- [能力边界与参考文档](#能力边界与参考文档)
- [Workspace 与 CLI](#workspace-与-cli)

## 依赖与运行环境

仓库声明的最低 Rust 版本为 1.86。`fds` 1.0.0 已发布到公司内网 Cargo 源，需要在可访问内网的环境中下载。

在项目的 `.cargo/config.toml` 或全局 `~/.cargo/config.toml` 中添加以下配置，保留已有配置：

```toml
[registries.artifactory]
index = "sparse+https://pkgs.d.xiaomi.net/artifactory/api/cargo/cargo-local/index/"
credential-provider = "cargo:token"
```

在项目 `Cargo.toml` 中添加依赖：

```toml
[dependencies]
fds = { version = "1.0.0", registry = "artifactory" }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "fs", "io-util", "time"] }
bytes = "1"
futures-util = "0.3"
chrono = { version = "0.4", default-features = false, features = ["clock", "std"] }
```

按用到的类型添加依赖：字节流示例需要 `bytes`/`futures-util`，UTC 过期时间需要 `chrono`。组合示例文件另列依赖。SDK 的传递依赖不能代替应用自己的直接依赖声明。

| feature | 默认开启 | 功能与选择 |
| --- | --- | --- |
| 核心 SDK | 始终 | Bucket、对象、OAuth、手动及托管分片、流式读写、读取重试 |
| `manager` | 是 | `Client::download_file`（`DownloadFile`）、断点与请求限流（`RequestLimiter`） |
| `cslb` | 否 | 让 Client 通过内部 CSLB DNS resolver 解析主机；不增加公开模块或类型 |

只用核心：`fds = { version = "1.0.0", registry = "artifactory", default-features = false }`。
启用 CSLB：`fds = { version = "1.0.0", registry = "artifactory", features = ["cslb"] }`。
独立 CLI 操作见 [CLI 手册](docs/cli.md)。

运行 `cargo check` 验证依赖解析和构建。如果提示需要认证，配置环境变量 `CARGO_REGISTRIES_ARTIFACTORY_TOKEN`，值为 `Bearer <TOKEN>`，使用具有读取权限的 Artifactory 令牌。不要将令牌写入仓库。网络、证书或版本不存在的问题需要分别排查。

## 客户端配置与通用调用约定

### 从应用配置构建客户端

SDK 不主动读取环境变量。以下是应用层读取凭据、创建并复用 Client 的完整入口；变量名由应用决定，本手册统一使用 `FDS_ENDPOINT`、`FDS_ACCESS_KEY_ID`、`FDS_ACCESS_KEY_SECRET`。

```rust,no_run
use fds::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder()
        .endpoint(std::env::var("FDS_ENDPOINT")?)
        .credentials(
            std::env::var("FDS_ACCESS_KEY_ID")?,
            std::env::var("FDS_ACCESS_KEY_SECRET")?,
        )
        .build()?;
    let buckets = client.list_buckets().send().await?;
    println!("可见 Bucket 数量：{}", buckets.buckets.len());
    Ok(())
}
```

`build()` 校验配置并创建 HTTP 客户端，不校验远端凭据有效性。endpoint 是完整的 HTTP(S) origin，例如 `https://cnbj1.fds.api.xiaomi.com`，允许端口；不能携带 Bucket 路径、用户信息、query 或 fragment。生产凭据应使用 HTTPS。Bucket 名称需满足服务的命名规则，SDK 不完整校验这些业务规则。Bucket 和 object key 分开传入，key 使用原始字符串，如 `logs/设备 A.txt`；SDK 负责编码，不要先百分号编码。

Client 内部共享配置、凭据 provider 和连接池，可廉价 `clone()` 后交给多个异步任务。不要每次请求都重建客户端。配置通过 `client.configuration()` 的同名 getter 读取；修改配置时创建新 Client，旧 clone 保持原配置。需要轮换凭据时使用 `credentials_provider`（见[刷新与轮换凭据](#刷新与轮换凭据)），Client 每次请求尝试前向 provider 取当前凭据，无需重建。

### 配置选项

| builder 方法 | 默认值 | 作用与限制 |
| --- | --- | --- |
| `endpoint(origin)` | 必填 | FDS 请求 origin |
| `credentials(id, secret)` | 必填，或使用 OAuth token / provider | access key 认证；两者须非空。`StaticCredentials` 的快捷方式 |
| `storage_token(token)` | 无 | 使用已交换的 storage token，不自动续期。`StaticCredentials` 的快捷方式 |
| `credentials_provider(provider)` | 无 | 每次请求尝试前（含重试）向 `fds::credentials::CredentialsProvider` 取凭据；三个凭据 setter 最后一个生效 |
| `cdn_endpoint(origin)` | 无 | 仅生成 CDN 预签名 URL 时使用，不改变普通 SDK 请求入口 |
| `connect_timeout(Duration)` | 50 秒 | 建连，包括 TLS |
| `read_timeout(Duration)` | 50 秒 | 所有请求的 I/O 空闲超时，只限制无进展的时长：上传时每传出一块 body（最多 64 KiB）重置；等待响应头从最后一次上传进展开始计时（无 body 的请求从请求开始计时）；读响应 body 时每次读到数据重置 |
| `request_timeout(Duration)` | 无 | 显式开启单次请求总期限，覆盖响应 body 的流式消费；对象、Bucket 与 OAuth 操作 builder 的 `.timeout(Duration)` 可按请求覆盖 |
| `pool_idle_timeout(Duration)` | 90 秒 | 空闲连接在池中保留的时间 |
| `tcp_keepalive(Duration)` | 30 秒 | 整数秒，1–32767；实际探测由操作系统调度 |
| `pool_max_idle_per_host(usize)` | 100 | 连接池每主机最多保留的空闲连接数；0 表示不保留空闲连接；不是并发上限 |
| `user_agent_suffix(text)` | 无 | 追加在 `fds-rs/<版本>` 之后、以空格分隔的应用标识，须为非空的可打印 ASCII；User-Agent 不参与签名 |
| `enable_md5_calculate(bool)` | false | 为内存 body 计算十六进制 Content-MD5；文件/流另见上传章节 |
| `retry_policy(RetryPolicy)` | 不重试 | 默认只对显式分类的读取生效；`.retry_idempotent_writes()` 另行开启幂等写重试 |
| `cslb_max_nodes(usize)` | 20 | 仅 `cslb` feature；必须大于零，限制每个主机保留的 DNS 节点数 |

超时参数均须非零且可表示。默认不设总期限，大对象上传和下载只要持续有进展就不会被打断，卡住的连接由 connect timeout 和 read timeout 发现：对端停止读取上传 body、上传完成后迟迟不返回响应头、或响应 body 停止到达，超过 read timeout 即以 `ErrorKind::Timeout` 失败。收到响应头之前超时的错误 `dispatch()` 为 `Dispatch::Unknown`（请求可能已到达服务端），读 body 时超时为 `Dispatch::Sent`。开启总期限后，它覆盖从建连、上传 body 到 `chunk()` / `copy_to()` / `bytes()` 读完响应的整个过程。

```rust,no_run
use fds::{Client, RetryPolicy};
use std::time::Duration;

fn configured_client(endpoint: &str, id: &str, secret: &str) -> fds::Result<Client> {
    Client::builder()
        .endpoint(endpoint)
        .credentials(id, secret)
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(60))
        .request_timeout(Duration::from_secs(1800))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(30))
        .retry_policy(RetryPolicy::new(
            3, Duration::from_millis(200), Duration::from_secs(5),
        )?)
        .build()
}
```

### 请求与所有权

通常使用 `Request::new(...).option(...).send(&client).await`。也可把请求传给对应 Client 方法：读取及控制类多接收 `&Request`；`put_object` 和 `upload_part` 接收拥有 body 的请求值，发送后不能复用。`.send()` 本身也消耗请求构造器。`generate()` 只在本地生成 URL，不是异步请求。

单次请求成功后，后续步骤失败不会回滚已经发生的上传、复制、授权或删除。业务流程需要自行保留进度和处理部分完成状态。

## Bucket 管理

### 创建、查询和列举

```rust,no_run
use fds::Client;
use fds::object::StorageClass;

async fn bucket_operations(client: &Client, bucket: &str) -> fds::Result<()> {
    client
        .create_bucket(bucket)
        .storage_class(StorageClass::new(StorageClass::STANDARD))
        .org_id("your-org-id")
        .send()
        .await?;

    let exists = client.bucket_exists(bucket).send().await?;
    let info = client.get_bucket_info(bucket).send().await?;
    let owned = client.list_buckets().send().await?;
    let authorized = client.list_authorized_buckets().send().await?;
    println!("存在：{exists}，对象数：{}，字节数：{}", info.object_count, info.used_space);
    println!("所有桶：{}，授权桶：{}", owned.buckets.len(), authorized.buckets.len());
    Ok(())
}
```

`storage_class`、`org_id` 和 `acl` 都是可选的链式方法，不调用即不发送，依服务默认；`org_id("")` 同样不发送。创建时的 canned ACL 见 ACL 章节。不要用“先 HEAD 不存在再创建”推导原子创建：检查和创建之间仍有竞争。

`bucket_exists` 只将 200/404 转成 true/false；403、网络错误等继续返回错误。`BucketInfo` 提供 `bucket_name`、`creation_time`（`Option<DateTime<Utc>>`，由毫秒时间戳转换）、`object_count`、`used_space`（均为 `u64`）、`storage_class`、`allow_outside_access`、`enable_sse`；服务端缺省或返回 null 的字段为 `None`、0、`false` 或空串，负数或非法时间戳返回 `ErrorKind::ProtocolViolation`。`BucketList` 包含 `owner` 和 `buckets`（元素为 `BucketInfo`）。

`StorageClass` 是保留服务端原始字符串的包装，使用 `new()`/`as_str()`（也实现 `Display`）；常量有 `STANDARD`、`STANDARD_IA`、`ARCHIVE`，后者的实际字符串为 `Archive`。服务端返回的大小写不稳定（如 `STANDARD` 与 `Standard`），判断类别用不区分 ASCII 大小写的 `is_standard()`、`is_standard_ia()`、`is_archive()`；`==` 比较的是原始字符串。类型存在不保证目标地区支持该存储类别。

### 迁移归属和删除

```rust,no_run
use fds::Client;

async fn move_ownership(client: &Client, bucket: &str, org: &str, team: &str) -> fds::Result<()> {
    client.migrate_bucket(bucket, org, team).send().await
}
async fn remove_empty_bucket(client: &Client, bucket: &str) -> fds::Result<()> {
    client.delete_bucket(bucket).send().await
}
```

`migrate_bucket` 的 org 和 team 是必填参数，按原值发送；它调整组织/团队归属，不是复制 Bucket 数据到另一个 endpoint。`delete_bucket` 不自动清空对象；是否允许删除由服务判断。

## 对象上传

### 内存数据与上传选项

```rust,no_run
use bytes::Bytes;
use fds::Client;
use fds::object::UserMetadata;

async fn upload_text(client: &Client, bucket: &str, key: &str) -> fds::Result<()> {
    let mut metadata = UserMetadata::new();
    metadata.insert("source", "report-job"); // 发送为 x-xiaomi-meta-source
    let output = client
        .put_object(bucket, key, Bytes::from_static(b"hello FDS\n"))
        .content_type("text/plain; charset=utf-8")
        .cache_control("private, max-age=60")
        .content_disposition("attachment; filename=report.txt")
        .user_metadata(metadata)
        .expires(chrono::Utc::now() + chrono::Duration::minutes(10))
        .send()
        .await?;
    println!("写入对象：{}", output.object_name);
    Ok(())
}
```

PUT 是创建或覆盖同名对象，不提供“不存在才写”的条件写入口。body 支持 `Vec<u8>`、`Bytes`、`UploadBody`；`Vec` 移入 Bytes，不复制整个 buffer。结果 `PutObjectOutput` 包含 `bucket_name`、`object_name`、`previous_version_id`（`Option<String>`，服务端未返回时为 `None`）、`access_key_id`、`signature`、`expires`（`Option<DateTime<Utc>>`，下载签名的过期时间，不是对象 TTL）、`outside_access`；不要把整个结果直接写入日志，因为它含签名信息。

| 选项 | 含义 |
| --- | --- |
| `content_type` | MIME；文件上传不会自动推断 |
| `cache_control` | 缓存控制头 |
| `content_disposition` | 下载显示方式/文件名 |
| `content_encoding` | 声明已有编码，如 gzip；SDK 不压缩 body |
| `content_md5` | 显式 Content-MD5，沿用此 SDK 的十六进制协议约定 |
| `expect` | 显式 Expect 头；不提供上传业务重试保证 |
| `user_metadata` | `UserMetadata` 用户元数据，作为 `x-xiaomi-meta-*` 头发送 |
| `expires` | 上传响应签名的 UTC 毫秒到期时间；不设置对象 TTL |

系统头只能通过专用 setter 设置，用户元数据只能通过 `user_metadata` 设置，同一个头不存在两条设置路径。专用 setter 的空值不发送。`UserMetadata` 的 key 不带 `x-xiaomi-meta-` 前缀，由 SDK 统一转小写并加前缀。校验在 `send()`（presign 为 `build()`）时进行，失败返回 `InvalidInput` 且不发送请求：所有操作都拒绝空 key 和已带 `x-xiaomi-meta-` 前缀（不区分大小写）的 key；以请求头发送的操作还要求头名和头值合法，`set_object_metadata` 的 JSON body 不做头校验。用户 key 即使叫 `content-type`，也只会发送为 `x-xiaomi-meta-content-type`，不会影响 Content-Type。

### 文件上传

```rust,no_run
use fds::Client;
use fds::object::UploadBody;
use std::path::Path;

async fn upload_file(client: &Client, bucket: &str, key: &str, path: &Path) -> fds::Result<()> {
    let body = UploadBody::from_file(path).await?;
    client
        .put_object(bucket, key, body)
        .content_type("application/octet-stream")
        .send()
        .await?;
    Ok(())
}
```

`UploadBody::from_file().await` 打开普通文件并读取长度，此时还未发请求。发送时按 64 KiB 块读取；保持文件不变直到调用结束。它不计算文件 MD5，也不自动切换为分片上传。零字节文件可以普通 PUT，托管分片要求非空文件。

### 已知长度的异步流

```rust,no_run
use bytes::Bytes;
use fds::Client;
use fds::object::UploadBody;

async fn upload_stream(client: &Client, bucket: &str, key: &str) -> fds::Result<()> {
    let stream = futures_util::stream::iter([
        Ok::<Bytes, std::io::Error>(Bytes::from_static(b"hello ")),
        Ok(Bytes::from_static(b"world")),
    ]);
    let body = UploadBody::from_stream(stream, 11);
    client
        .put_object(bucket, key, body)
        .content_type("text/plain")
        .send()
        .await?;
    Ok(())
}
```

流必须满足 `Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static`，并声明精确长度。提前结束或某块超过剩余长度会失败；达到长度后不再轮询，因此不检测额外尾随数据。未知长度没有自动收集或落盘入口；应用需先确定长度或显式设计暂存流程。

`UploadBody::len()`/`is_empty()` 可查询声明长度；`try_clone()` 仅为内存 Bytes 返回共享副本，文件/流返回 None。传输 Content-Length 由 body 产生，不能用 metadata 覆盖它。

开启 `enable_md5_calculate(true)` 后：内存 body 自动计算 MD5；文件/流必须显式 `.content_md5(...)`，否则发送前报错。关闭时也可以显式提供校验和。SDK 不验证调用者声明的 MD5 与文件/流一致。

上传默认不自动重试；只有 Bytes body 的 `put_object` / `upload_part` 在显式开启幂等写重试后会重放，文件和流 body 永不重试（见[重试和整体期限](#重试和整体期限)）。取消 future 会释放 SDK 持有的 body，但远端可能已接收甚至完成写入；调用者自建的后台流生产任务仍需自行取消。更详细的背压约束见 [上传 body](docs/runtime.md#流式上传与读取)。

## 对象读取与范围下载

### 小对象：收集到内存

```rust,no_run
use fds::Client;

async fn read_text(client: &Client, bucket: &str, key: &str) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = client.get_object(bucket, key).send().await?.bytes().await?;
    Ok(String::from_utf8(bytes.to_vec())?)
}
```

`bytes()` 收集剩余响应体，没有应用级大小上限。仅对已知可接受大小的对象使用；如果先调用过 `chunk()`，它不会重新返回已消费内容。

### 流式消费与写入文件

```rust,no_run
use fds::Client;
use tokio::io::AsyncWriteExt;

async fn stream_to_file(client: &Client, bucket: &str, key: &str, path: &std::path::Path) -> fds::Result<u64> {
    let output = client.get_object(bucket, key).send().await?;
    println!("HTTP 状态：{}，长度：{:?}", output.status(), output.content_length());
    let mut file = tokio::fs::File::create(path).await?;
    let written = output.copy_to(&mut file).await?;
    file.flush().await?;
    Ok(written)
}

async fn consume_chunks(client: &Client, bucket: &str, key: &str) -> fds::Result<u64> {
    let mut output = client.get_object(bucket, key).send().await?;
    let mut total = 0;
    while let Some(chunk) = output.chunk().await? {
        total += chunk.len() as u64;
        // 在这里解析、计算摘要或发送给下游；无需累计全部 body。
    }
    Ok(total)
}

async fn decode_lines(client: &Client, bucket: &str, key: &str) -> fds::Result<usize> {
    use tokio::io::AsyncBufReadExt;
    let output = client.get_object(bucket, key).send().await?;
    // 交给任何接受 AsyncRead 的解析器；错误是包着 fds::Error 的 io::Error。
    let mut lines = tokio::io::BufReader::new(output.into_async_read()).lines();
    let mut count = 0;
    while lines.next_line().await?.is_some() {
        count += 1;
    }
    Ok(count)
}
```

`copy_to` 不 flush、不 shutdown，也不回滚 writer；示例的 `File::create` 会截断已有文件，错误时可能留下部分内容。需要失败保留旧目标、原子替换、续传时使用后文的 `download_file`。

`GetObjectOutput` 的 `status()` 返回 `http::StatusCode`，`is_partial()` 判断是否为 206。类型化的头部访问方法有 `content_length()`、`content_type()`、`etag()`（原样，含引号）、`last_modified()`（`DateTime<Utc>`）和 `content_range()`（`ContentRange`，提供 `start()`、`end()`（含）与 `complete_length()`）；头部缺失或无法解析时返回 `None`，不会让下载失败，原始值仍可通过 `headers()` 读取。`metadata()` 返回从同一组响应头解析的 `ObjectMetadata`。`headers()` 返回 `ResponseHeaders` 只读视图（`get`、`get_all`、`iter`），用于诊断和 SDK 未建模的头。

除 `chunk()`、`bytes()`、`copy_to()` 外，`into_stream()` 把剩余响应体转成 `Stream<Item = fds::Result<Bytes>>`，`into_async_read()` 转成 `AsyncRead`（失败时返回 `std::io::Error`：超时为 `TimedOut`，其余为 `Other`，`into_inner()` 后可 downcast 回 `fds::Error`）。两者都是 `Send + Unpin + 'static`，读取同样受 read timeout 和总期限约束。公开 API 不暴露 reqwest 类型。

### Range

```rust,no_run
use fds::{Client, Error, ErrorKind};
use fds::object::ByteRange;

async fn read_first_kib(client: &Client, bucket: &str, key: &str) -> fds::Result<bytes::Bytes> {
    // 此示例要求对象至少有 1024 字节，并要求服务返回对应范围。
    let output = client
        .get_object(bucket, key)
        .range(ByteRange::inclusive(0, 1023)?)
        .send()
        .await?;
    if !output.is_partial() {
        return Err(Error::new(ErrorKind::ProtocolViolation, "服务未返回范围响应"));
    }
    let range = output.content_range();
    if !range.is_some_and(|r| (r.start(), r.end()) == (0, 1023)) {
        return Err(Error::new(ErrorKind::ProtocolViolation, "Content-Range 与请求不匹配"));
    }
    let bytes = output.bytes().await?;
    if bytes.len() != 1024 {
        return Err(Error::new(ErrorKind::ProtocolViolation, "范围响应长度不匹配"));
    }
    Ok(bytes)
}
```

| 构造 | HTTP Range |
| --- | --- |
| `ByteRange::inclusive(0, 1023)?` | `bytes=0-1023`，含两端 |
| `ByteRange::from(1024)` | `bytes=1024-` |
| `ByteRange::suffix(512)?` | `bytes=-512` |
| `"bytes=0-1023".parse::<ByteRange>()?` | 解析单范围字符串 |

闭区间 start 不能大于 end，suffix 不能为零，不支持多范围。普通 `get_object` 在请求 Range 时接受 200 和 206：200 可能代表服务器忽略 Range，因此调用者需检查状态与范围。`download_file` 会严格校验响应范围、长度与一致性。

## 对象信息与元数据

这三个读取入口不能互相替代：

| 入口 | 返回 | 含义 |
| --- | --- | --- |
| `head_object(bucket, key).send()` | `Result<Option<ObjectMetadata>>` | 真正 HEAD；404 → None，200 → 元数据；`object_exists` 是同一请求的 bool 形式 |
| `get_object_metadata(bucket, key).send()` | `Result<ObjectMetadata>` | GET `?metadata`，从响应头提取元数据；404 是错误 |
| `get_object_basic_info(bucket, key).send()` | `Result<ObjectBasicInfo>` | GET `?basicInfo`，JSON 包含 size（`u64`）、upload_time（`Option<DateTime<Utc>>`）、storage_class、metadata 等 |

```rust,no_run
use fds::{Client, Error, ErrorKind};
async fn inspect_and_set_metadata(client: &Client, bucket: &str, key: &str) -> fds::Result<()> {
    let Some(head) = client.head_object(bucket, key).send().await? else {
        return Err(Error::new(ErrorKind::NotFound, "对象不存在"));
    };
    let wire_length = head.attributes.content_length
        .ok_or_else(|| Error::new(ErrorKind::ProtocolViolation, "HEAD 缺少合法的 Content-Length"))?;
    println!("Content-Length：{wire_length}");
    let info = client.get_object_basic_info(bucket, key).send().await?;
    println!("basicInfo 大小：{}", info.size);
    let current = client.get_object_metadata(bucket, key).send().await?;
    let _content_type: Option<&str> = current.attributes.content_type.as_deref();

    // 显式提交完整的目标元数据，不把 set 当作自动合并补丁。
    let mut user = current.user_metadata;
    user.insert("owner", "upload-service");
    client
        .set_object_metadata(bucket, key)
        .content_type("application/octet-stream")
        .user_metadata(user)
        .send()
        .await?;
    Ok(())
}
```

读取结果 `ObjectMetadata` 分为两部分：`attributes`（`ObjectAttributes`，类型化的只读系统属性）和 `user_metadata`（`UserMetadata`，去掉前缀的 `x-xiaomi-meta-*`）。它只用于读取，写入一律使用各 builder 的专用 setter 与 `user_metadata`。`set_object_metadata` 提交的是完整的目标元数据：需要保留已有自定义字段时由应用显式带上；读取再写入没有并发更新保护，也不要把只读系统属性原样写回。

| `ObjectAttributes` 字段 | 来源（响应头或 basicInfo 条目） |
| --- | --- |
| `content_type`、`content_encoding`、`cache_control`、`content_md5`、`etag` | 同名头，原样字符串 |
| `content_length: Option<u64>` | `content-length`：HEAD 为对象大小，GET 为本次响应体长度 |
| `last_modified: Option<DateTime<Utc>>` | `last-modified`（IMF-fixdate） |
| `storage_class: Option<StorageClass>` | `x-xiaomi-meta-storage-class` |
| `server_side_encryption` | `x-xiaomi-meta-server-side-encryption` |
| `crc64ecma` | `x-xiaomi-meta-hash-crc64ecma` |
| `meta_content_length: Option<u64>` | `x-xiaomi-meta-content-length`，不是本次响应的 HTTP Content-Length |
| `ongoing_restore`、`restore_expiry` | `x-xiaomi-meta-ongoing-restore`、`x-xiaomi-meta-restore-expiry` |

上表中带 `x-xiaomi-meta-` 前缀的 6 个 key 是系统属性，读取时不会出现在 `user_metadata` 中；写入时它们只能作为 `UserMetadata` 的 key（如 `storage-class`）原样发送。字段缺失为 `None`；数值或日期无法解析时也为 `None`，不会让请求失败。`ObjectMetadata::entries()` 按 key 顺序返回收到的全部原始条目（响应头取协议元数据头与全部 `x-xiaomi-meta-*`，不含 ETag；basicInfo 取全部条目），仅用于展示和诊断。

三种线上格式都是 SDK 内部实现：SET 发送 `{"rawMeta":{...}}` map，basicInfo 接收 `rawMeta.entry` 数组，HEAD/GET/`?metadata` 从响应头解析。

## 对象列举与分页

### 单页与目录式浏览

```rust,no_run
use fds::Client;
use std::num::NonZeroU32;

async fn list_directory(client: &Client, bucket: &str) -> fds::Result<()> {
    let page_size = NonZeroU32::new(100).unwrap();
    let page = client
        .list_objects(bucket)
        .prefix("reports/")
        .delimiter("/")
        .max_keys(page_size)
        .send()
        .await?;
    for object in page.objects {
        println!("{}\t{}", object.object_name, object.size);
    }
    for prefix in page.common_prefixes {
        println!("目录前缀：{prefix}");
    }
    if page.truncated {
        // 将服务端 next_marker 原样传回；不是 offset 或自行计算的 key。
        let _next = client
            .list_objects(bucket)
            .prefix("reports/")
            .delimiter("/")
            .max_keys(page_size)
            .marker(page.next_marker)
            .send()
            .await?;
    }
    Ok(())
}
```

默认 `max_keys = 1000`；类型是 `NonZeroU32`，零在编译期就无法传入。`ObjectListing` 包含对象数组、common_prefixes、truncated、next_marker 等分页信息。`ObjectListing` 的对象数组字段是 `objects`，`max_keys` 是服务端回显的 `u32`。`ObjectSummary` 包含 `object_name`、`size`（`u64`）、`etag`、`owner`、可选 UTC `last_modified`（RFC 3339 字符串）和可选 UTC `upload_time`（由毫秒时间戳转换）。负数大小或非法时间戳会让整页返回 `ErrorKind::ProtocolViolation`。

prefix 是字符串前缀，没有真实目录实体。要遍历前缀下全部对象通常省略 delimiter；设为 `/` 后，下一层分组出现在 common_prefixes，SDK 不递归进入这些分组。

### 自动遍历页或对象

```rust,no_run
use fds::Client;
use futures_util::{pin_mut, StreamExt};

async fn sum_prefix(client: &Client, bucket: &str, prefix: &str) -> fds::Result<u64> {
    let objects = client.list_objects(bucket).prefix(prefix).into_objects();
    pin_mut!(objects);
    let mut total = 0u64;
    while let Some(object) = objects.next().await {
        let object = object?;
        total = total.checked_add(object.size)
            .ok_or_else(|| fds::Error::new(fds::ErrorKind::Unexpected, "累计大小溢出"))?;
    }
    Ok(total)
}

async fn count_pages(client: &Client, bucket: &str) -> fds::Result<usize> {
    let pages = client.list_objects(bucket).into_pages();
    pin_mut!(pages);
    let mut count = 0;
    while let Some(page) = pages.next().await {
        let _page = page?;
        count += 1;
    }
    Ok(count)
}
```

`into_pages()`/`into_objects()` 消费 builder，返回 `Send + 'static` 的惰性流，创建时不请求、不预取下一页；`timeout` 对每一页的请求分别生效；错误后终止。truncated 但 next_marker 缺失或相邻重复时报协议错误；较长循环用常数空间检测，可能先输出部分重复页再报错。objects 会展开每页对象，保留一页响应和常数个 marker，不保证对象去重。列举不是事务快照，并发新增/删除时不要假定清单代表某个精确时刻的完整集合。

## 复制、重命名与误删恢复

```rust,no_run
use fds::Client;

async fn copy_and_rename(client: &Client, src_bucket: &str, dst_bucket: &str) -> fds::Result<()> {
    let receipt = client
        .copy_object((src_bucket, "incoming/a.bin"), (dst_bucket, "backup/a.bin"))
        .send()
        .await?;
    assert_eq!(receipt.bucket_name, dst_bucket);
    assert_eq!(receipt.object_name, "backup/a.bin");
    client
        .rename_object(dst_bucket, "backup/a.bin", "backup/final.bin")
        .send()
        .await?;
    Ok(())
}
async fn restore_deleted(client: &Client, bucket: &str, key: &str) -> fds::Result<()> {
    client.restore_object(bucket, key).send().await
}
```

复制是服务端操作，不会把源文件下载到调用进程。支持分别指定源/目标 Bucket 和 key；仍使用此 Client 的目标服务入口，不能据此假设能跨地区或跨独立 FDS 服务复制。没有条件防覆盖保证，也没有把复制和 rename 包装为事务。

`copy_object(..).send()` 返回 `CopyObjectOutput`，并执行完整回执校验：必须 HTTP 200、完整有效 JSON、必填 `bucketName` / `objectName` 与请求目标完全一致。回执最多 64 KiB（含额外字段和空白），空体、截断、缺字段、目标不符、超限及超时均失败；读取受 Client 的 `request_timeout` 和 `read_timeout` 约束。

Copy 不自动重试。请求可能已到达服务后的任何失败（错误状态、回执缺失或不符、响应体中断）都是 `ErrorKind::OutcomeUnknown`，表示未取得有效匹配回执，**目标可能已写入，不能据此认定复制未执行，也不能盲目重试**；可通过 `source()` 检查原因，HTTP 诊断仍可通过 `status()` / `response()` 读取。应由业务结合目标身份、版本或内容证据核对结果，仅检查目标存在不能证明它来自本次 Copy。`dispatch()` 为 `Dispatch::NotSent` 的失败（参数校验、签名、连接被拒）原样返回。

`rename_object` 在同一 Bucket 中改名。`restore_object` 是误删恢复（`?restore`），是否仍可恢复取决于服务保留策略；它不是归档解冻。

## 单对象、批量与前缀删除

单个对象调用 `client.delete_object(bucket, key).send().await?`。批量删除用 `client.delete_objects(bucket).keys(names)`，`keys` 接受任何能转换成 `String` 的迭代器（`["a", "b"]`、`Vec<String>`、`&[String]` 等），可多次调用追加；一次 `send()` 发送一个批次，不自动把任意长数组切成符合服务限制的小批次。

```rust,no_run
use fds::{Client, Dispatch, ErrorKind};
use fds::object::DeleteObjectsOutput;
use std::num::NonZeroU32;

async fn delete_selected(client: &Client, bucket: &str, keys: &[String])
    -> fds::Result<DeleteObjectsOutput>
{
    let result = client
        .delete_objects(bucket)
        .keys(keys)
        .move_to_trash()
        .send()
        .await;
    match result {
        // 逐项失败也在 Ok 里：检查 failures。
        Ok(output) => {
            for failure in &output.failures {
                eprintln!("删除失败：{} {}", failure.object_name, failure.error_code);
            }
            Ok(output)
        }
        Err(error) => {
            if error.kind() == ErrorKind::OutcomeUnknown {
                let unknown = error.partial_delete_output().map_or(0, |o| o.unconfirmed.len());
                eprintln!("批次结果未知（{unknown} 项），可能已删除：{error}");
            } else if error.dispatch() == Dispatch::NotSent {
                eprintln!("批次未提交：{error}");
            }
            // 将完整 error 交还上层保存和决策，不在这里盲目重发。
            Err(error)
        }
    }
}

async fn delete_prefix(client: &Client, bucket: &str, prefix: &str)
    -> Result<DeleteObjectsOutput, Box<dyn std::error::Error>>
{
    // 这是应用自己的删除范围约束，SDK 允许空 prefix。
    if prefix.is_empty() {
        return Err("删除范围必须显式指定".into());
    }
    Ok(client
        .delete_prefix(bucket, prefix)
        .page_size(NonZeroU32::new(100).unwrap())
        .move_to_trash()
        .send()
        .await?)
}
```

调用 `move_to_trash()` 发送 `enableTrash=true`，不调用时发送 false；不要把它理解成 SDK 提供可恢复保证。空批次不发请求，直接返回空的 `Ok`。

| 结果 | 如何使用 |
| --- | --- |
| `Ok(output).deleted` | 按合法服务失败数组推导的成功项，并非逐个 HEAD 再检查 |
| `Ok(output).failures` | 服务返回的 object_name、error_code、error_description；HTTP 200 也可能有逐项失败，这是批量删除的正常结果，不是错误 |
| `ErrorKind::OutcomeUnknown` | 批次可能已发出但结果无法确认（错误状态、响应损坏、失败项不在请求中）；`partial_delete_output().unconfirmed` 列出整批名字，可能实际已删除 |
| `error.dispatch()` | `NotSent` 表示批次没有发出（本地校验、连接被拒），此时没有附带结果 |
| `source()` | `OutcomeUnknown` 的底层原因 |

`DeleteObjectFailure` 的 `Debug` 只输出 error_code（形如普通标识符时），不输出对象名和描述，避免对象名进入日志；需要时直接读字段。

前缀删除默认只按 bucket 和 prefix 选择对象（不带 delimiter，嵌套“目录”下的对象也会删除），逐页列举、逐页删除。两个显式选项会缩小范围：`.marker(m)` 从 m 之后开始，m 及之前的对象不列举也不删除（首个列举请求带 `marker`，空串也照样发送；不调用时不带该参数）；`.delimiter("/")` 只删当前层，服务归入 commonPrefixes 的名字既不删除也不深入（SDK 信任服务的分组）。提交一页之前会检查：名字非空且以 prefix 开头；truncated 页的 next_marker 存在且与本次运行见过的所有 marker 都不同，起始 marker 也算在内（记住全部 marker，任何长度的循环都会在重复删除之前停下）。全部确认删除才返回 `Ok`；遇到第一个问题即停止并返回错误，`partial_delete_output()` 带有此前所有页已确认的结果：某批出现逐项失败时 kind 为 `PartialFailure`（后续页不再列举），批次结果未知时为 `OutcomeUnknown`，列举失败或检查不通过时为对应错误。尚未列举的对象不会出现在结果中。它会累计已确认结果，因此结果内存随删除数量增长；海量清理可由应用分页、分批持久记录进度。

## Bucket 与对象 ACL

ACL 的 `Add` 是默认增量添加，`Delete` 删除指定授权项；没有 Replace action。把从本地 Vec 删除 grant 后的列表发为 Add，不会撤销远端原授权。

```rust,no_run
use fds::Client;
use fds::acl::{AccessControlList, AclAction, Grant, GrantKey, GrantPermission, GrantType};

fn read_grant(user_id: &str) -> AccessControlList {
    let mut acl = AccessControlList::default();
    acl.add_grant(Grant {
        grantee: GrantKey { id: user_id.into(), display_name: String::new() },
        permission: GrantPermission::Read,
        grant_type: GrantType::User,
    });
    acl
}

async fn grant_read(client: &Client, bucket: &str, key: &str, user_id: &str) -> fds::Result<()> {
    client.set_bucket_acl(bucket, read_grant(user_id)).send().await?;
    client.set_object_acl(bucket, key, read_grant(user_id)).send().await?;
    let _bucket_acl = client.get_bucket_acl(bucket).send().await?;
    let _object_acl = client.get_object_acl(bucket, key).send().await?;
    Ok(())
}

async fn revoke_object_read(client: &Client, bucket: &str, key: &str, user_id: &str) -> fds::Result<()> {
    client
        .set_object_acl(bucket, key, read_grant(user_id))
        .action(AclAction::Delete)
        .send()
        .await
}
```

`GrantType` 为 User/Group；`GrantPermission` 为 Read、Write、ReadObjects、SsoWrite、FullControl。不同资源、用户、服务权限可能限制可用组合。ACL 结果还包含 owner。对象 GET/SET ACL 请求都有 `.version_id(...)`；其余对象读写入口不能因此推断支持版本寻址。服务文档中的“不能删除自身 ACL”限制仍需遵守。

`client.set_object_public(bucket, key).send().await?` 添加 `ALL_USERS` Group 的 Read 授权，意味着公开可读。只有业务明确需要匿名访问时才使用。

创建桶的 canned ACL 使用独立枚举，可设置多个值：

```rust,no_run
use fds::Client;
use fds::acl::BucketCannedAcl;

async fn create_public_read_bucket(client: &Client, bucket: &str) -> fds::Result<()> {
    client
        .create_bucket(bucket)
        .acl([BucketCannedAcl::PublicRead])
        .send()
        .await
}
```

可选值：PublicRead、PublicWrite、PublicReadObjects、PublicSsoWrite、AuthenticatedRead、AuthenticatedWrite、AuthenticatedSsoWrite。空列表不发送 canned ACL 头。常规私有业务不要为了生成 URL 而开放全桶 ACL，优先按需签名访问。

## 手动分片上传

适合调用者自行管理分片来源、调度、回执和恢复流程。步骤是 create → 一个或多个 part → complete；放弃已知会话时显式 abort。普通文件上传优先考虑下一节托管接口。

```rust,no_run
use fds::Client;
use fds::multipart::{MultipartUploadSession, UploadPartOutput};
use fds::object::PutObjectOutput;
use std::num::NonZeroU16;

// 返回会话给调用者保存；随后执行分片，不把会话藏在一个会丢失上下文的函数内。
async fn begin(client: &Client, bucket: &str, key: &str) -> fds::Result<MultipartUploadSession> {
    // 会话绑定调用者传入的 bucket/key；响应缺少 upload ID 时返回 OutcomeUnknown。
    client.create_multipart_upload(bucket, key).send().await
}

async fn send_part(session: &MultipartUploadSession, number: NonZeroU16, bytes: bytes::Bytes)
    -> fds::Result<UploadPartOutput>
{
    session.upload_part(number, bytes).send().await
}

async fn finish(session: &MultipartUploadSession, mut receipts: Vec<UploadPartOutput>)
    -> fds::Result<PutObjectOutput>
{
    receipts.sort_by_key(|part| part.part_number);
    session.complete(receipts)
        .expires(chrono::Utc::now() + chrono::Duration::minutes(10))
        .send().await
}

async fn abandon(session: &MultipartUploadSession) -> fds::Result<()> {
    session.abort().send().await
}
```

调用顺序示例：保存 `begin()` 返回的会话（至少持久化 `bucket()`、`object()`、`upload_id()`）；按从 1 开始的编号调用 `send_part()`，每收到一份回执就保存；确认分片编号、长度、ETag 和总数满足预期后把回执交给 `finish()`。回执为 `UploadPartOutput { part_number: NonZeroU16, etag: String, part_size: u64 }`，序列化格式与服务端返回、complete 请求体中的 `{"partNumber","etag","partSize"}` 一致。手动 complete 按调用者提供的集合序列化，不替调用者检查完整性或自动排序。

create 与 complete 支持 `.content_type(...)`、`.cache_control(...)`、`.content_encoding(...)`、`.content_md5(...)` 和 `.user_metadata(...)`（`upload_file` 同样支持，并在 create 与 complete 时各发送一次），complete 另支持 `.expires(...)`；四个 builder 都支持 `.timeout(...)`。part 支持和 PUT 一样的 UploadBody，可显式 `.content_md5(...)`。跨进程恢复时不需要会话对象：`client.upload_part(bucket, key, upload_id, number, body)`、`client.complete_multipart_upload(bucket, key, upload_id, parts)` 和 `client.abort_multipart_upload(bucket, key, upload_id)` 直接接收持久化的 upload ID，会话上的同名方法返回同样的 builder。

分片编号是 `NonZeroU16`（不能为 0），`send` 时再检查不超过 `fds::multipart::MAX_PART_NUMBER`（10,000），否则返回 `InvalidInput` 且不发送。服务端分片尺寸、数量和地区限制需单独满足；库内 `MIN_PART_SIZE`/`MAX_PART_SIZE` 常量不代表所有接口都自动检查，更不能当成各地区统一约束。

初始化超时可能已创建远端会话但没有收到 ID。part 失败可能已被接收。complete 失败可能已经完成对象：保留 upload ID、回执和目标身份，先核对远端结果，再决定下一步；不要在 complete 错误后自动 abort 或重发。SDK 没有 list-parts 或通用的跨进程续传协调接口。

## 托管文件分片上传

`client.upload_file(bucket, key, path)` 打开一个非空普通文件，限制并发和缓冲，收集回执并按编号 complete。它属于核心 SDK，不依赖 manager feature。`start()` 校验选项和文件、初始化一次并返回 `MultipartUpload` 会话；`upload(cancel)` 运行一次上传。

```rust,no_run
use fds::multipart::{CleanupPolicy, MultipartUpload};
use fds::object::PutObjectOutput;
use fds::Client;
use std::future::Future;

async fn prepare_large_file(client: &Client, bucket: &str, key: &str, path: &std::path::Path)
    -> fds::Result<MultipartUpload>
{
    client
        .upload_file(bucket, key, path)
        .part_size(8 * 1024 * 1024)
        .concurrency(4)
        .cleanup(CleanupPolicy::Preserve)
        .start()
        .await
}

// session 由上层持有；错误返回后仍可读取 progress，不会随本函数丢失。
async fn run_upload(session: &mut MultipartUpload, cancel: impl Future<Output = ()>)
    -> fds::Result<PutObjectOutput>
{
    let result = session.upload(cancel).await;
    if result.is_err() {
        let progress = session.progress();
        eprintln!("状态 {:?}，已确认 {}/{} 片", progress.state,
            progress.parts.len(), progress.total_parts);
    }
    result
}
```

调用者先 await `prepare_large_file` 并保存会话，再调用 `run_upload(&mut session, cancel)`。取消信号是任意 `Future<Output = ()>`：完成即请求合作式取消，SDK 仍会执行清理策略并返回最终状态；不需要取消时传 `std::future::pending()`，使用 tokio-util 的 `CancellationToken` 时传 `token.cancelled()`。失败后从仍存活的 session 读取并持久化进度，随后决定恢复。

默认 8 MiB、4 并发、Preserve。`start()` 在打开文件和发送任何请求前校验：每片 1 字节至 64 MiB，并发 1–64，二者乘积不超过 256 MiB，违反时返回 `InvalidInput`；随后要求文件非空且最多 10,000 片。非最后一片仍需满足服务要求；不要因本地允许 1 字节就按 1 字节部署。payload 缓冲约束为 `part_size × concurrency`，另有 HTTP、TLS、文件和响应模型开销，不是进程 RSS 上限。

| 状态 | 含义与处理 |
| --- | --- |
| Ready | 已初始化，尚未上传 |
| Uploading | 上传中；future 被丢弃可能留下此状态，需要显式核对 |
| Failed | 分片/文件/完成前取消等错误；Preserve 保留会话 |
| Aborted | 显式清理策略的 abort 得到成功响应 |
| CompletionUnknown | complete 可能已发送或返回无效结果；先核对，禁止盲目清理 |
| Completed | complete 响应有效且对象身份匹配 |

`progress()` 包含 `session`（`MultipartUploadSession`，含 bucket/key/upload_id）、`parts`、`total_parts`、`file_size`、`state`。这是会话快照，不是可直接导入的自动恢复格式。`upload(&mut self, ...)` 只允许运行一次；失败后不能再次调用它来隐式续传，而是用 `progress().session` 的 `upload_part` 补传缺失分片（第 n 片对应文件偏移 `(n - 1) × part_size`），再用全部回执 `complete`，或 `abort` 放弃。完整示例见 `MultipartUpload` 的 rustdoc。

`CleanupPolicy::AbortOnFailure` 仅在完成前错误/合作式取消时尝试一次 abort；返回的是原始错误，abort 也失败时其文本附加在 `error.context()` 的 `cleanup_error` 项中。complete 可能已发出后的失败统一为 `ErrorKind::OutcomeUnknown`。`Preserve` 是默认策略。合作式取消要调用 token.cancel() 后继续 await；直接 timeout/drop future 不执行异步清理，整个 session Drop 也不发请求。源文件在全会话期间必须保持不变。

可运行基础示例见 [upload_multipart.rs](crates/fds/examples/upload_multipart.rs)，详细状态行为见 [重试与分片](docs/runtime.md#分片会话与恢复)。

## 并发下载与断点续传

需要 `manager`（默认开启）。`client.download_file(bucket, key, path)` 返回 builder，选项链式设置，`send()` 执行一次下载。

```rust,no_run
use fds::Client;

async fn download_safely(client: &Client, bucket: &str, key: &str, path: &std::path::Path)
    -> fds::Result<()>
{
    client
        .download_file(bucket, key, path)
        .part_size(4 * 1024 * 1024)
        .concurrency(4)
        .resume()
        .send()
        .await
}
```

默认每片 1 MiB、4 并发、不续传、无限流。分片字节数和并发须大于零，在 `send()` 中、加锁和发送请求前校验，违反时返回 `InvalidInput`（与托管上传相同）；最大分片数为 1,000,000，太多时应增大片尺寸。范围下载使用 `.range(ByteRange::from(...))`，只将选中范围写入目标文件，文件开头对应选中范围的起点。

执行时先发送与 `head_object` 相同的 HEAD 获取对象长度：它是只读请求，按 Client 的 `RetryPolicy` 重试；对象不存在时返回 `is_not_found()` 为真的错误。具有强 ETag 时，逐片检查相同 ETag、206、Content-Range 和精确长度；没有强 ETag 时使用单个流式 Range 请求，不复用断点。HTTP 200 忽略 Range 会报错。

下载先写同目录暂存文件，全部成功并同步后替换目标，失败保留旧目标。父目录必须存在且可写。同目标使用 `.download.lock` 合作式排他；还会使用 `.tmp`、`.download.bp`、`.download.bp.new` 等路径，调用者不能同时使用这些附属路径。中断后对同一路径重新调用并开启 `resume()`，下载器校验对象身份和本地分片摘要，损坏或不匹配分片重新下载。进程被杀死可能留下锁，确认无下载进程后才移除。

取消直接丢弃 download future；worker 结束前仍持有锁，立即重试可能暂时报 AlreadyExists。文件 IO、fsync 和恢复摘要都在阻塞线程中执行，不占用 async 运行时线程。它不承诺跨程序目录防篡改或目录 fsync 级断电持久性；详见 [下载安全](docs/runtime.md#下载文件与断点)。

### 请求启动限流

```rust,no_run
use fds::manager::RequestLimiter;
use std::num::NonZeroU32;

async fn limited_download(client: &fds::Client, path: &std::path::Path) -> fds::Result<()> {
    let limiter = RequestLimiter::per_second(NonZeroU32::new(5).unwrap());
    client
        .download_file("bucket", "key", path)
        .concurrency(4)
        .request_limiter(limiter.clone())
        .send()
        .await
}
```

每个分片请求启动消耗一个 permit，限的是请求频率，不是字节带宽。`per_second(n)` 允许最多 n 个请求的突发，并非严格每 1/n 秒一次；`RequestLimiter` 的克隆共享同一预算，可让多个下载共用。限流器是 SDK 自己的类型，应用不需要直接依赖限流库。

## 预签名与普通 URL

```rust,no_run
use fds::Client;
use http::Method;
use std::time::Duration;
use url::Url;

fn download_link(client: &Client, bucket: &str, key: &str) -> fds::Result<Url> {
    client
        .presign_object(bucket, key)
        .expires_in(Duration::from_secs(600))
        .build()
}

fn upload_link(client: &Client, bucket: &str, key: &str) -> fds::Result<Url> {
    client
        .presign_object(bucket, key)
        .method(Method::PUT)
        .expires_in(Duration::from_secs(600))
        .content_type("application/octet-stream")
        .build()
}

fn public_link(client: &Client, bucket: &str, key: &str) -> fds::Result<Url> {
    client.absolute_object_url(bucket, key)
}
```

预签名仅支持静态 access key Client（`credentials` 或 `StaticCredentials`），OAuth Client 或自定义 provider 返回 `ConfigInvalid`。生成操作不联网、不验证对象存在或凭据是否有效。`expires_in` 的有效期从调用 `build()` 时计算，不是实际交付 URL 时，至少 1 ms；绝对时间用 `.expires_at(DateTime<Utc>)`。两者必须设置其一，method 默认 GET。

接收方必须使用签名时的 HTTP method 与参与签名的头；例如上传示例使用相同 Content-Type。参与签名的头由 `.content_type(...)`、`.content_md5(...)` 和 `.user_metadata(...)` 设置。SDK 返回 `url::Url`，其 `as_str()` / `Display` 就是签名时的完整字符串；原样使用，不要再用 `Url` 的 setter 修改 query 或重复编码 Signature。`absolute_object_url` 同样返回 `Url`。该协议 Expires 是毫秒时间戳，HEAD 预签名还附带 metadata 子资源。URL 包含临时访问凭证，应经受控渠道交付，不放公开日志。

`.cdn()` 请求 CDN 地址，必须在构建 Client 时显式设置 `cdn_endpoint(origin)`；未配置会返回错误，不会从 FDS 主机自动推导。普通 SDK 数据请求始终使用 FDS endpoint。CDN 地址是否已开通由服务环境决定。

普通 URL 不含认证，也不会把对象设为公开；匿名访问仍需权限。空 object 可生成 Bucket URL，Bucket/object 都空可生成 endpoint 根 URL。

## 生命周期与访问日志

### 生命周期规则

```rust,no_run
use fds::bucket::{LifecycleAction, LifecycleBaseItem, LifecycleConfig, LifecycleRule};
use fds::Client;
use std::collections::BTreeMap;

async fn configure_lifecycle(client: &Client, bucket: &str) -> fds::Result<()> {
    let action = BTreeMap::from([(LifecycleAction::Expiration, LifecycleBaseItem { days: 30.0 })]);
    let rule = LifecycleRule {
        id: "expire-temporary-reports".into(), prefix: "temporary-reports/".into(),
        enabled: true, action,
    };
    // 配置指定规则。
    client.set_lifecycle_rule(bucket, rule.clone()).send().await?;
    let _selected = client
        .get_lifecycle_config(bucket)
        .rule_id(&rule.id)
        .send()
        .await?;
    let _all = client.get_lifecycle_config(bucket).send().await?;

    // 整体提交 rules；仅在确实要管理完整配置时调用。
    let config = LifecycleConfig { rules: vec![rule] };
    client.set_lifecycle_config(bucket, config).send().await?;
    Ok(())
}
```

`LifecycleRule.action` 是 `LifecycleAction` 到 `{ days: f64 }` 的有序 map，序列化字段名为 `actions`，多个 action 按线上字符串排序输出。两者都实现 serde 的 `Serialize`/`Deserialize`，从 JSON 读取直接用 `serde_json::from_slice`；完整配置形式为 `{"rules":[...]}`。不调用 `.rule_id(..)` 时读取完整配置。

`LifecycleAction` 还有 `NonCurrentVersionExpiration` 和 `AbortIncompleteMultipartUpload`；后者的线上值保留了协议兼容字符串的尾随空格（`"abortIncompleteMultipartUpload "`），不带空格的 key 会作为 `Other` 原样往返，不能假定等价。SDK 不验证天数业务范围，也没有独立 delete-lifecycle-rule 方法；禁用可提交同 ID 的 `enabled: false` 规则，具体更新/删除语义以服务为准。expiration 会影响匹配对象的数据保留，请按业务策略选择前缀和天数。

### 访问日志

```rust,no_run
use fds::Client;
use fds::bucket::AccessLog;

async fn enable_logs(client: &Client, bucket: &str, log_bucket: &str) -> fds::Result<()> {
    let config = AccessLog {
        bucket_name: bucket.into(), enabled: true,
        log_bucket_name: log_bucket.into(), log_prefix: "fds-access/".into(),
    };
    client.set_access_log(bucket, config).send().await?;
    let _effective = client.get_access_log(bucket).send().await?;
    Ok(())
}
async fn disable_logs(client: &Client, bucket: &str) -> fds::Result<()> {
    let mut config = client.get_access_log(bucket).send().await?;
    config.bucket_name = bucket.into();
    config.enabled = false;
    client.set_access_log(bucket, config).send().await
}
```

保证路径参数 bucket 与配置的 bucket_name 一致。日志目标 Bucket、写入权限、交付延迟需在服务端配置和确认；返回成功只表示配置请求被接受。

## CDN 预取与刷新

```rust,no_run
use fds::Client;

async fn request_prefetch(client: &Client, bucket: &str, key: &str) -> fds::Result<()> {
    client.prefetch_object(bucket, key).send().await
}
async fn request_refresh(client: &Client, bucket: &str, key: &str) -> fds::Result<()> {
    client.refresh_object(bucket, key).send().await
}
```

预取用于请求缓存对象；刷新用于请求失效更新。使用对象所属 FDS endpoint，不向 CDN URL 调用这些 SDK 方法。两者都是 mutation，不自动重试，返回 `Result<()>`，HTTP 200 只表示请求确认，没有任务 ID 或完成等待接口。需要对象 owner、目标地区和 CDN 服务支持；若流程依赖 CDN 已生效，要另外读取 CDN 内容核验。协议依据见 [CDN 说明](docs/protocol.md#cdn-预取与刷新)。

## 服务发现与 OAuth

### 显式发现 endpoint

```rust,no_run
use fds::{Client, Error, ErrorKind};
use fds::oauth::FdsServiceClient;

async fn discover_client(discovery_origin: &str, region: &str, service_name: &str,
    allowed_hosts: &[String], id: &str, secret: &str) -> fds::Result<Client>
{
    let discovery = FdsServiceClient::new(discovery_origin)?;
    let host = discovery.get_endpoint(region, service_name).send().await?;
    if !allowed_hosts.contains(&host) {
        return Err(Error::new(ErrorKind::ConfigInvalid, "发现主机不在应用允许列表"));
    }
    Client::builder().endpoint(format!("https://{host}"))
        .credentials(id, secret).build()
}
```

必须显式提供 discovery origin、非空 region 和 service；返回 JSON 主机名字符串，不是带 scheme 的 URL。SDK 只检查主机名格式，不验证其可信性、可达性或可用地区，不自动切换已有 Client。应用验证返回主机后再绑定凭据。

`FdsServiceClient::from_builder(Client::builder().endpoint(...).request_timeout(...))` 可复用超时/连接设置；builder 上的凭据会被丢弃，此客户端专用于发现和交换。

### 交换 storage token 并访问对象

```rust,no_run
use fds::Client;
use fds::oauth::{FdsServiceClient, OAuthProvider};

async fn oauth_read(exchange_origin: &str, object_origin: &str, app_id: &str,
    oauth_app_id: &str, provider_token: &str, bucket: &str, key: &str) -> fds::Result<u64>
{
    let service = FdsServiceClient::new(exchange_origin)?;
    let token = service
        .exchange_storage_token(app_id, oauth_app_id, provider_token, OAuthProvider::QQ)
        .send()
        .await?;
    let expires_at = token.expires_at();
    let client = Client::builder().endpoint(object_origin).storage_token(token).build()?;
    let _app_buckets = client.list_app_buckets().send().await?;
    let mut output = client.get_object(bucket, key).send().await?;
    let mut count = 0;
    while let Some(chunk) = output.chunk().await? {
        count += chunk.len() as u64;
    }
    println!("读取 {count} 字节，token 到期时间 {expires_at}");
    Ok(count)
}
```

provider 为 XiaoMi、QQ、Sina、RenRen。app ID、OAuth app ID、provider token 和 provider 是 `exchange_storage_token` 的必填参数；XiaoMi 还需在 `.send()` 前链式追加 `.xiaomi_mac(open_id, mac_key, algorithm)`，三者非空；algorithm 原样交给服务，不在本地计算 MAC。Weixin 尚无可调用枚举。provider token 由应用通过相应授权流程获取，SDK 不提供用户登录流程。

`StorageAccessToken` 提供 `app_id()`、`expires_at()`、`is_expired()` 和敏感访问器 `token()`；过期时间是 UTC 绝对时间，接口不提供任意字符串手动构造 token 的公开入口。交换始终只尝试一次。token 绑定 appId，`list_app_buckets` 自动使用该 appId，返回 `AppBucketList { owner, buckets }`；`AppBucket` 含 `bucket_name`、`org_id`、`creation_time`（`DateTime<Utc>`）、`used_space`、`object_count`（`u64`），每个字段都必须出现。

OAuth Client 普通对象 API 共用 token 认证；`credentials`、`storage_token` 与 `credentials_provider` 最后一个 setter 生效。`storage_token` 设置的 token 在构建及每次发送前检查过期，不会自动续期；需要自动续期时改用下面的 `StorageTokenProvider`。

### 刷新与轮换凭据

`fds::credentials` 定义 `CredentialsProvider` trait（对象安全，返回 `CredentialsFuture`）和公开的 `Credentials`（`Credentials::access_key(id, secret)` 或 `Credentials::storage_token(token)`，Debug 不输出密钥和 token）。Client 在每次请求尝试前（包括重试）向 provider 取凭据，并用该次的凭据生成 Date 和签名，或把 token 放进该次的 URL；错误诊断是否脱敏也按该次请求实际使用的凭据决定。

```rust,no_run
use fds::Client;
use fds::credentials::StorageTokenProvider;
use fds::oauth::{FdsServiceClient, OAuthProvider};
use std::time::Duration;

fn refreshing_client(exchange_origin: &str, object_origin: &str, app_id: &str,
    oauth_app_id: &str, provider_token: &str) -> fds::Result<Client>
{
    let service = FdsServiceClient::new(exchange_origin)?;
    let provider = StorageTokenProvider::new(
        service.exchange_storage_token(app_id, oauth_app_id, provider_token, OAuthProvider::QQ),
    )
    .refresh_margin(Duration::from_secs(600));
    Client::builder().endpoint(object_origin).credentials_provider(provider).build()
}
```

- `StaticCredentials`：固定凭据，`credentials` / `storage_token` 即其快捷方式；构建时校验。
- `StorageTokenProvider`：首次请求时交换 token（也可用 `.with_token(token)` 预置已交换的 token）；token 剩余有效期不超过提前量时重新交换。提前量默认 5 分钟（`DEFAULT_REFRESH_MARGIN`），可用 `.refresh_margin(..)` 修改；对 provider 自己交换的 token，提前量最多取其有效期的一半，避免短期 token 每次请求都交换。并发请求共享同一次交换（single-flight），provider 的 clone 共享缓存。交换失败时，等待该次交换的请求都在发送前失败：缓存的 token 已过期返回 `CredentialsExpired`，否则返回 `Unauthenticated`，交换错误作为 `source()`；失败不缓存，下一次请求会重新交换；过期 token 永远不会被发出。交换本身仍只尝试一次。provider 的 OAuth access token 本身需要刷新时，由应用实现 `CredentialsProvider`。
- 自定义 provider 返回的凭据在每次尝试前校验，无效时该请求在发送前失败，错误种类与构建时相同。
- 预签名是同步的本地操作，只能使用静态 access key；storage token 或其他 provider 返回 `ConfigInvalid`。`list_app_buckets` 要求 provider 当次返回的是 storage token，否则返回 `ConfigInvalid`，请求不发出。
- SDK 不从环境变量或配置文件自动发现凭据。

token 传输要求 HTTPS，仅 HTTP loopback IP 允许本地测试。OAuth 错误会丢弃服务端诊断字符串；不要记录 provider token、MAC key、完整认证 URL。应用开通、region 和 Bucket 权限前提见 [OAuth 说明](docs/protocol.md#服务发现与-oauth)。

## 错误处理、超时与重试

### 区分错误种类

所有 API 都返回 `fds::Result<T>`，错误类型只有一个 `fds::Error`。它是不透明结构体：`kind()` 给出 `ErrorKind` 分类，`dispatch()` 说明请求是否已到达服务，`response()` 给出 HTTP 状态和诊断信息，`source()` 链保留底层原因。

```rust,no_run
use fds::{Dispatch, ErrorKind};

fn report_error(error: &fds::Error) {
    match error.kind() {
        ErrorKind::NotFound => eprintln!("对象或 Bucket 不存在"),
        ErrorKind::PermissionDenied | ErrorKind::Unauthenticated => eprintln!("权限或认证失败"),
        ErrorKind::CredentialsExpired => eprintln!("storage token 已过期，需重新交换"),
        ErrorKind::InvalidInput | ErrorKind::ConfigInvalid => eprintln!("本地参数或配置无效"),
        ErrorKind::OutcomeUnknown => eprintln!("结果未知，先核对再决定是否重试"),
        _ if error.is_retryable() => eprintln!("暂时性失败，可以重试：{error}"),
        _ => eprintln!("其他失败：{error}"),
    }
    if let Some(response) = error.response() {
        // service_code/request_id/diagnostic_body 是服务原文，按应用规则脱敏后再使用。
        eprintln!("HTTP {}，诊断截断 {}", response.status(), response.body_truncated());
    }
    if error.dispatch() == Dispatch::NotSent {
        eprintln!("请求没有发出");
    }
}
```

| ErrorKind | 含义 |
| --- | --- |
| `InvalidInput` / `ConfigInvalid` | 调用参数无效 / Client 配置（endpoint、凭证、超时、重试策略、CDN）无效或不适用于该操作 |
| `NotFound` / `Unauthenticated` / `PermissionDenied` / `Conflict` / `PreconditionFailed` / `RateLimited` | HTTP 404 / 401 / 403 / 409 / 412 / 429；对象在下载中途变化也是 `PreconditionFailed` |
| `ServerError` | HTTP 5xx |
| `CredentialsExpired` | storage token 在本地检查时已过期 |
| `ProtocolViolation` | 服务响应不合约定：空 JSON、非法 JSON、超限响应、回执或分页 marker 与请求不符 |
| `Timeout` / `Transport` / `Io` / `Cancelled` | 超时 / 连接或传输失败 / 本地 IO / 合作式取消 |
| `OutcomeUnknown` | Copy、分片 complete、批量删除等变更可能已生效，禁止盲目重试 |
| `PartialFailure` | 前缀删除因某批出现逐项失败而停止，已确认结果见 `partial_delete_output()`（单批 `delete_objects` 的逐项失败在 `Ok` 里） |
| `Unexpected` | 其他 HTTP 状态（其余 4xx、未列入成功的 2xx/3xx）和 SDK 内部错误 |

`ErrorKind` 为 non_exhaustive，match 必须留兜底分支。`is_retryable()` 对 RateLimited、HTTP 502/503/504、Timeout 和收到响应前的 Transport（无论是否已发出）返回 true，与 SDK 重试循环使用同一条判定规则；HTTP 500 和读响应 body 时的 Transport 失败返回 false；`is_not_found()`、`is_outcome_unknown()` 是对应 kind 的简写。`status()`（`http::StatusCode`）、`service_code()`、`request_id()` 是便利访问器；`ResponseDetails` 还含最多 8 KiB 的 `diagnostic_body()` 和 `body_truncated()`。

HTTP status 与服务 error code 是两套字段。JSON 操作即使 HTTP 200，也可能因空体或非法 JSON 失败；权限错误不能转换成“对象不存在”。SDK 禁止自动跟随重定向，未列入操作成功状态的 2xx 也会失败。更多状态规则见 [响应处理](docs/runtime.md#响应与错误)。

`fds::Error` 的 Display 包含 kind、消息、HTTP 状态和形如标识符的 service code；Debug 另含 dispatch、上下文和 source 链（逐项按 Display 输出）。两者都不输出 AK/SK、签名、storage token、带凭证的 URL、request ID 或原始响应体；OAuth/匿名模式下不保留 service code、request ID 和响应体。但调用者自己的错误文本、metadata、ACL、key、原始响应和 PutObjectOutput 等并非统一脱敏对象。不要把任意 SDK 模型整体打印当成安全日志策略。

### 重试和整体期限

`RetryPolicy::new(max_retries, initial_delay, max_delay)` 中 max_retries 不含首次尝试，允许 0–100；初始等待大于零且不超过最大等待，最大等待不超过 60 秒。默认 max_retries=0。

启用后，默认仅明确安全的读取在响应交付前的连接/发送/超时错误和 429、502、503、504 可重试，判定规则与 `Error::is_retryable()` 相同。HTTP 500 不重试：它是服务端的通用失败，对同一请求往往是确定性的，重复相同请求多半只会耗尽重试预算并在故障期间加重负载；需要时由调用者按 `status()` 自行重试。采用有随机抖动的指数退避；Retry-After 作为等待下限，超过 max_delay 时放弃重试。JSON 解码错误、成功响应之后的 body 断流、上传流错误均不重放。

幂等写默认也不重试，需显式开启：`RetryPolicy::new(..)?.retry_idempotent_writes()` 对整个 Client 生效，单个请求可用 `.retry(RetryMode::Enabled)` 开启或 `.retry(RetryMode::Disabled)` 关闭（重试次数和等待仍来自 Client 的 RetryPolicy，未配置重试时不会重试）。幂等写包括 Bytes body 的 `put_object` 和 `upload_part`（托管上传的分片也因此可重试）、`delete_object`、`set_object_metadata`、`set_lifecycle_config`、`set_access_log`；文件和流 body 永不重放。重复执行的最终状态相同，但仍可被观察到：前一次已删除时 `delete_object` 的重试可能返回 NotFound；版本化 bucket 中重复 `put_object` 可能多出一个版本；两次尝试之间其他写入者的修改可能被覆盖。

复制、rename、restore、ACL 修改（Add 重复授权或 Delete 不存在授权的服务端语义未成文）、`set_lifecycle_rule`、批量删除、bucket 创建/删除/迁移、分片 init/complete/abort、token exchange 和 CDN 操作永不自动重试，也没有 `.retry(..)` 方法。前缀删除仅列举步骤可以重试。不能仅凭 HTTP 动词判断重复业务操作安全。

客户端 request_timeout 是每次请求的期限；完整下载 body 消费和所有重试的外层期限可以这样设置：

```rust,no_run
use fds::Client;
use std::time::Duration;

async fn read_with_deadline(client: &Client, bucket: &str, key: &str)
    -> Result<bytes::Bytes, Box<dyn std::error::Error>>
{
    let bytes = tokio::time::timeout(Duration::from_secs(30), async {
        client.get_object(bucket, key).send().await?.bytes().await
    }).await??;
    Ok(bytes)
}
```

这里仍是小对象读取。若将 timeout 用于写入，超时只意味着停止等待，不证明远端没有执行；托管上传需要清理时使用传给 `upload` 的合作式取消 future。

## CSLB 负载均衡

启用 `cslb` feature 后，构建 Client 就会安装 DNS resolver；可用 `.cslb_max_nodes(20)` 调整每个主机保留的节点上限。

它在 DNS 解析时轮换首选地址并附带备选 IP，以 600 秒 TTL 刷新。HTTP 连接池复用不会逐请求重新选节点；节点数不是 HTTP 并发数。集成 resolver 没有连接失败反馈回调，不能视为自动故障摘除系统；无节点时会回退系统 DNS。

`cslb` feature 只影响 Client 的 DNS 解析：resolver 是 crate 内部实现，SDK 不提供 `fds::cslb` 模块或独立的负载均衡类型，唯一的配置入口是 `cslb_max_nodes`。

## 组合功能示例

以下三个完整程序位于 examples，可复制到业务工程并按自己的错误持久化、权限与凭据来源调整。源码中的每个网络步骤都有明确边界，运行会连接真实服务；运行前请配置对应凭据和资源。

### 1. 文件上传、HEAD 大小检查、流式摘要校验与分享链接

源码：[upload_verify.rs](crates/fds/examples/upload_verify.rs)。

流程：流式计算本地文件 MD5 → PUT 文件与自定义摘要元数据 → HEAD 检查长度 → GET 分块计算 MD5 → 比较长度和摘要 → 生成 10 分钟 GET 预签名 URL。

```sh
cargo run --locked -p fds --example upload_verify -- BUCKET UNIQUE_KEY LOCAL_FILE
```

读取 `FDS_ENDPOINT`、`FDS_ACCESS_KEY_ID`、`FDS_ACCESS_KEY_SECRET`。额外直接依赖 `md-5 = "0.10"`，库名为 `md5`。MD5 在此仅用于意外传输/内容差异检查，不作为抗篡改认证。文件从计算摘要到上传完成必须保持不变；使用业务唯一 key 避免覆盖和并发改写。验证失败不会删除远端对象。程序成功时只将预签名 URL 写到 stdout，诊断写 stderr；stdout 应交给受控接收方，勿收集到公开日志。

### 2. 分页扫描、服务端复制与目标大小核对

源码：[copy_prefix.rs](crates/fds/examples/copy_prefix.rs)。

流程：逐页遍历源 prefix → 校验返回 key 范围 → 映射目标 prefix → 服务端 Copy → HEAD 目标大小 → 输出完成计数。

```sh
cargo run --locked -p fds --example copy_prefix -- SRC_BUCKET SRC_PREFIX DST_BUCKET DST_PREFIX
```

使用相同的三项 FDS 环境变量。复制串行执行，不下载文件，不自动删除源对象。拒绝同桶重叠前缀，防止复制结果再次进入源清单。目标 key 可能被覆盖，应使用新的目标命名空间；失败时已复制对象保留。大小相等不是内容一致证明，源清单也不是快照；需要严格迁移一致性时冻结源写入并增加内容校验和业务清单。

### 3. OAuth 交换、应用 Bucket 校验与对象清单统计

源码：[oauth_inventory.rs](crates/fds/examples/oauth_inventory.rs)。

流程：QQ provider token 换 storage token → 构建绑定 appId 的 Client → 确认所选 Bucket 位于 app Bucket 返回列表 → 分页统计该前缀对象数与总字节数。

```sh
cargo run --locked -p fds --example oauth_inventory -- BUCKET PREFIX
```

环境变量：`FDS_OAUTH_ORIGIN`（交换入口）、`FDS_ENDPOINT`（对象入口）、`FDS_APP_ID`、`FDS_OAUTH_APP_ID`、`FDS_PROVIDER_TOKEN`。两处 origin 都由应用明确配置。程序不修改 Bucket 或对象，但 token 交换仍可能创建/续期凭据。示例使用 `storage_token`，没有自动刷新；长时间统计若 token 到期会返回错误。需要自动续期时可改用 `StorageTokenProvider`（见[刷新与轮换凭据](#刷新与轮换凭据)）。

更多单功能程序：[流上传](crates/fds/examples/upload_stream.rs)、[分页](crates/fds/examples/pagination.rs)、[对象请求](crates/fds/examples/object_requests.rs)、[下载管理器](crates/fds/examples/download.rs)、[托管分片](crates/fds/examples/upload_multipart.rs)。既有示例的环境变量名称可能不同，以各文件入口说明为准。

## 能力边界与参考文档

当前不提供 POST 创建、归档恢复、multipart copy、typed MULTI_BLOB、list-parts 自动恢复、未知长度上传、条件写防覆盖或通用版本化读写。协议常量或源 YAML 中出现某个名字不代表已有对应可调用功能。

日常调用以本文为准。补充资料统一见 [文档导航](docs/README.md)，原始接口及 SDK 映射见 [API 索引](docs/open_api/README.md)。

## 许可证

Apache-2.0。

## Workspace 与 CLI

根目录是纯 workspace；`crates/fds` 是 SDK，`crates/fds_cli` 是独立验证工具，包名和二进制名均为 `fds_cli`。

从内网源安装 CLI：先将上面的 registry 配置添加到全局 `~/.cargo/config.toml`，然后执行：

```sh
cargo install fds_cli --version 1.0.0 --registry artifactory --locked
fds_cli --help
```

默认安装到 `~/.cargo/bin`（自定义 `CARGO_HOME` 时为 `$CARGO_HOME/bin`），确保该目录在 `PATH` 中。下载认证方式与 SDK 相同。

本地开发可使用 `cargo run -p fds_cli -- --help` 或 `cargo install --path crates/fds_cli --locked`。不再提供 SDK 的 `cli` feature 或旧命令兼容层。完整命令与 SDK 映射见 [CLI 使用手册](docs/cli.md)。
