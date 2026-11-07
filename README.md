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

按用到的类型添加依赖：字节流示例需要 `bytes`/`futures-util`，UTC 过期时间需要 `chrono`。下面的下载限流示例还需要 `governor = "0.6"`；组合示例文件另列依赖。SDK 的传递依赖不能代替应用自己的直接依赖声明。

| feature | 默认开启 | 功能与选择 |
| --- | --- | --- |
| 核心 SDK | 始终 | Bucket、对象、OAuth、手动及托管分片、流式读写、读取重试 |
| `manager` | 是 | `Downloader`、下载选项、断点与请求限流 |
| `cslb` | 否 | 为 Client 安装 CSLB DNS resolver；提供独立负载均衡工具 |

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
    let buckets = client.list_buckets().await?;
    println!("可见 Bucket 数量：{}", buckets.buckets.len());
    Ok(())
}
```

`build()` 校验配置并创建 HTTP 客户端，不校验远端凭据有效性。endpoint 是完整的 HTTP(S) origin，例如 `https://cnbj1.fds.api.xiaomi.com`，允许端口；不能携带 Bucket 路径、用户信息、query 或 fragment。生产凭据应使用 HTTPS。Bucket 名称需满足服务的命名规则，SDK 不完整校验这些业务规则。Bucket 和 object key 分开传入，key 使用原始字符串，如 `logs/设备 A.txt`；SDK 负责编码，不要先百分号编码。

Client 内部共享配置、凭据和连接池，可廉价 `clone()` 后交给多个异步任务。不要每次请求都重建客户端。配置通过 `client.configuration()` 的同名 getter 读取；修改配置或轮换凭据时创建新 Client，旧 clone 保持原配置。

### 配置选项

| builder 方法 | 默认值 | 作用与限制 |
| --- | --- | --- |
| `endpoint(origin)` | 必填 | FDS 请求 origin |
| `credentials(id, secret)` | 必填，或使用 OAuth token | access key 认证；两者须非空 |
| `cdn_endpoint(origin)` | 无 | 仅生成 CDN 预签名 URL 时使用，不改变普通 SDK 请求入口 |
| `connect_timeout(Duration)` | 50 秒 | 建连，包括 TLS |
| `read_timeout(Duration)` | 50 秒 | 每次响应读取的最大等待，取得进展后重置 |
| `request_timeout(Duration)` | 300 秒 | 单次请求总期限，包含响应 body 消费 |
| `pool_idle_timeout(Duration)` | 90 秒 | 空闲连接在池中保留的时间 |
| `tcp_keepalive(Duration)` | 30 秒 | 整数秒，1–32767；实际探测由操作系统调度 |
| `enable_md5_calculate(bool)` | false | 为内存 body 计算十六进制 Content-MD5；文件/流另见上传章节 |
| `retry_policy(RetryPolicy)` | 不重试 | 只对显式分类的读取生效 |
| `cslb_max_nodes(usize)` | 20 | 仅 `cslb` feature；必须大于零，限制每个主机保留的 DNS 节点数 |

超时参数均须非零且可表示。慢速大文件传输应同时评估 read timeout 和总 timeout，不能只调大连接超时。

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
use fds::{Client, CreateBucketRequest, StorageClass};

async fn bucket_operations(client: &Client, bucket: &str) -> fds::Result<()> {
    CreateBucketRequest::new(bucket)
        .storage_class(StorageClass::new(StorageClass::STANDARD))
        .org_id("your-org-id")
        .send(client).await?;

    let exists = client.does_bucket_exist(bucket).await?;
    let info = client.get_bucket_info(bucket).await?;
    let owned = client.list_buckets().await?;
    let authorized = client.list_authorized_buckets().await?;
    println!("存在：{exists}，对象数：{}，字节数：{}", info.object_num, info.used_space);
    println!("所有桶：{}，授权桶：{}", owned.buckets.len(), authorized.buckets.len());
    Ok(())
}
```

`storage_class`、`org_id` 和 `acl` 都可省略；省略时依服务默认。创建时的 canned ACL 见 ACL 章节。不要用“先 HEAD 不存在再创建”推导原子创建：检查和创建之间仍有竞争。

`does_bucket_exist` 只将 200/404 转成 true/false；403、网络错误等继续返回错误。`GetBucketInfoResponse` 提供 `bucket_name`、`creation_time`、`object_num`、`used_space`、`storage_class_type`、`allow_outside_access`、`enable_sse`。`ListBucketsResponse` 包含 `owner` 和 `buckets`。

`StorageClass` 是保留服务端原始字符串的包装，使用 `new()`/`as_str()`；常量有 `STANDARD`、`STANDARD_IA`、`ARCHIVE`，后者的实际字符串为 `Archive`。类型存在不保证目标地区支持该存储类别。

### 迁移归属和删除

```rust,no_run
use fds::{Client, MigrateBucketRequest};

async fn move_ownership(client: &Client, bucket: &str, org: &str, team: &str) -> fds::Result<()> {
    MigrateBucketRequest::new(bucket, org, team).send(client).await
}
async fn remove_empty_bucket(client: &Client, bucket: &str) -> fds::Result<()> {
    client.delete_bucket(bucket).await
}
```

`migrate_bucket` 调整组织/团队归属，不是复制 Bucket 数据到另一个 endpoint。`delete_bucket` 不自动清空对象；是否允许删除由服务判断。

## 对象上传

### 内存数据与上传选项

```rust,no_run
use bytes::Bytes;
use fds::{Client, ObjectMetadata, PutObjectRequest};

async fn upload_text(client: &Client, bucket: &str, key: &str) -> fds::Result<()> {
    let mut metadata = ObjectMetadata::new();
    metadata.set("x-xiaomi-meta-source", "report-job")?;
    let output = PutObjectRequest::new(bucket, key, Bytes::from_static(b"hello FDS\n"))
        .content_type("text/plain; charset=utf-8")
        .cache_control("private, max-age=60")
        .content_disposition("attachment; filename=report.txt")
        .metadata(metadata)
        .expires(chrono::Utc::now() + chrono::Duration::minutes(10))
        .send(client).await?;
    println!("写入对象：{}", output.object_name);
    Ok(())
}
```

PUT 是创建或覆盖同名对象，不提供“不存在才写”的条件写入口。body 支持 `Vec<u8>`、`Bytes`、`UploadBody`；`Vec` 移入 Bytes，不复制整个 buffer。结果 `PutObjectResponse` 包含 `bucket_name`、`object_name`、`access_key_id`、`signature`、`expires`、`previous_version_id`、`outside_access`；不要把整个结果直接写入日志，因为它含签名信息。

| 选项 | 含义 |
| --- | --- |
| `content_type` | MIME；文件上传不会自动推断 |
| `cache_control` | 缓存控制头 |
| `content_disposition` | 下载显示方式/文件名 |
| `content_encoding` | 声明已有编码，如 gzip；SDK 不压缩 body |
| `content_md5` | 显式 Content-MD5，沿用此 SDK 的十六进制协议约定 |
| `expect` | 显式 Expect 头；不提供上传业务重试保证 |
| `metadata` | ObjectMetadata 中的请求元数据 |
| `expires` | 上传响应签名的 UTC 毫秒到期时间；不设置对象 TTL |

同一 HTTP 头不要同时通过 metadata 和专用 setter 设置；内部可能追加成重复头，并非可靠的覆盖关系。

### 文件上传

```rust,no_run
use fds::{Client, PutObjectRequest};
use std::path::Path;

async fn upload_file(client: &Client, bucket: &str, key: &str, path: &Path) -> fds::Result<()> {
    PutObjectRequest::from_file(bucket, key, path).await?
        .content_type("application/octet-stream")
        .send(client).await?;
    Ok(())
}
```

`from_file().await` 打开普通文件并读取长度，此时还未发请求。发送时按 64 KiB 块读取；保持文件不变直到调用结束。它不计算文件 MD5，也不自动切换为分片上传。零字节文件可以普通 PUT，托管分片要求非空文件。

### 已知长度的异步流

```rust,no_run
use bytes::Bytes;
use fds::{Client, PutObjectRequest, UploadBody};

async fn upload_stream(client: &Client, bucket: &str, key: &str) -> fds::Result<()> {
    let stream = futures_util::stream::iter([
        Ok::<Bytes, std::io::Error>(Bytes::from_static(b"hello ")),
        Ok(Bytes::from_static(b"world")),
    ]);
    let body = UploadBody::from_stream(stream, 11);
    PutObjectRequest::new(bucket, key, body)
        .content_type("text/plain")
        .send(client).await?;
    Ok(())
}
```

流必须满足 `Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static`，并声明精确长度。提前结束或某块超过剩余长度会失败；达到长度后不再轮询，因此不检测额外尾随数据。未知长度没有自动收集或落盘入口；应用需先确定长度或显式设计暂存流程。

`UploadBody::len()`/`is_empty()` 可查询声明长度；`try_clone()` 仅为内存 Bytes 返回共享副本，文件/流返回 None。传输 Content-Length 由 body 产生，不能用 metadata 覆盖它。

开启 `enable_md5_calculate(true)` 后：内存 body 自动计算 MD5；文件/流必须显式 `.content_md5(...)`，否则发送前报错。关闭时也可以显式提供校验和。SDK 不验证调用者声明的 MD5 与文件/流一致。

上传不会自动重试。取消 future 会释放 SDK 持有的 body，但远端可能已接收甚至完成写入；调用者自建的后台流生产任务仍需自行取消。更详细的背压约束见 [上传 body](docs/runtime.md#流式上传与读取)。

## 对象读取与范围下载

### 小对象：收集到内存

```rust,no_run
use fds::{Client, GetObjectRequest};

async fn read_text(client: &Client, bucket: &str, key: &str) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = GetObjectRequest::new(bucket, key).send(client).await?.bytes().await?;
    Ok(String::from_utf8(bytes.to_vec())?)
}
```

`bytes()` 收集剩余响应体，没有应用级大小上限。仅对已知可接受大小的对象使用；如果先调用过 `chunk()`，它不会重新返回已消费内容。

### 流式消费与写入文件

```rust,no_run
use fds::{Client, GetObjectRequest};
use tokio::io::AsyncWriteExt;

async fn stream_to_file(client: &Client, bucket: &str, key: &str, path: &std::path::Path) -> fds::Result<u64> {
    let output = GetObjectRequest::new(bucket, key).send(client).await?;
    println!("HTTP 状态：{}", output.status());
    let mut file = tokio::fs::File::create(path).await?;
    let written = output.copy_to(&mut file).await?;
    file.flush().await?;
    Ok(written)
}

async fn consume_chunks(client: &Client, bucket: &str, key: &str) -> fds::Result<u64> {
    let mut output = GetObjectRequest::new(bucket, key).send(client).await?;
    let mut total = 0;
    while let Some(chunk) = output.chunk().await? {
        total += chunk.len() as u64;
        // 在这里解析、计算摘要或发送给下游；无需累计全部 body。
    }
    Ok(total)
}
```

`copy_to` 不 flush、不 shutdown，也不回滚 writer；示例的 `File::create` 会截断已有文件，错误时可能留下部分内容。需要失败保留旧目标、原子替换、续传时使用后文的 Downloader。

`GetObjectOutput` 还提供 `headers()`、`metadata()` 和 `into_raw()`；后者转移出 `reqwest::Response`，供需要底层能力的调用者使用。

### Range

```rust,no_run
use fds::{ByteRange, Client, FdsError, GetObjectRequest};

async fn read_first_kib(client: &Client, bucket: &str, key: &str) -> fds::Result<bytes::Bytes> {
    // 此示例要求对象至少有 1024 字节，并要求服务返回对应范围。
    let output = GetObjectRequest::new(bucket, key)
        .range(ByteRange::inclusive(0, 1023)?)
        .send(client).await?;
    if output.status() != 206 {
        return Err(FdsError::invalid("服务未返回范围响应"));
    }
    let range = output.headers().get("content-range").and_then(|v| v.to_str().ok());
    if !range.is_some_and(|v| v.starts_with("bytes 0-1023/")) {
        return Err(FdsError::invalid("Content-Range 与请求不匹配"));
    }
    let bytes = output.bytes().await?;
    if bytes.len() != 1024 {
        return Err(FdsError::invalid("范围响应长度不匹配"));
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

闭区间 start 不能大于 end，suffix 不能为零，不支持多范围。普通 `get_object` 在请求 Range 时接受 200 和 206：200 可能代表服务器忽略 Range，因此调用者需检查状态与范围。Downloader 会严格校验响应范围、长度与一致性。

## 对象信息与元数据

这三个读取入口不能互相替代：

| 入口 | 返回 | 含义 |
| --- | --- | --- |
| `head_object(bucket, key)` | `Result<Option<ObjectMetadata>>` | 真正 HEAD；404 → None，200 → 元数据 |
| `get_object_metadata(bucket, key)` | `Result<ObjectMetadata>` | GET `?metadata`，从响应头提取元数据 |
| `get_object_basic_info(bucket, key)` | `Result<ObjectBasicInfo>` | GET `?basicInfo`，JSON 包含 size、upload_time、存储类别、metadata 等 |

```rust,no_run
use fds::{Client, FdsError, ObjectMetadata, SetObjectMetadataRequest};

async fn inspect_and_set_metadata(client: &Client, bucket: &str, key: &str) -> fds::Result<()> {
    let Some(head) = client.head_object(bucket, key).await? else {
        return Err(FdsError::invalid("对象不存在"));
    };
    let wire_length = head.get("content-length")
        .ok_or_else(|| FdsError::invalid("HEAD 缺少 Content-Length"))?;
    println!("Content-Length：{wire_length}");
    let info = client.get_object_basic_info(bucket, key).await?;
    println!("basicInfo 大小：{}", info.size);
    let current = client.get_object_metadata(bucket, key).await?;
    let _content_type = current.get_content_type();

    // 显式提交完整的目标元数据，不把 set 当作自动合并补丁。
    let mut desired = ObjectMetadata::new();
    desired.set_content_type("application/octet-stream");
    desired.set("x-xiaomi-meta-owner", "upload-service")?;
    SetObjectMetadataRequest::new(bucket, key, desired).send(client).await?;
    Ok(())
}
```

`ObjectMetadata::set` 将 key 转成小写，只接受预定义字段或 `x-xiaomi-meta-*`。`get` 忽略大小写，返回 `Option<&str>`，可区分缺失与空字符串；`iter`/`as_map` 借用数据。需要保留已有自定义字段时由应用显式构造目标集合；读取再写入没有并发更新保护，也不要把所有只读响应头原样写回。

`set_content_length`/`get_content_length` 对应 `x-xiaomi-meta-content-length`，不是实际 HTTP `content-length`。HEAD 的真实长度应使用 `get("content-length")`。ETag 不在 ObjectMetadata 白名单中；读取 GET 的 ETag 使用 `output.headers().get("etag")`，不能假定 `head_object` 的 metadata 保留 ETag。

`serialize()` 输出 SET 所需的 `{"rawMeta":{...}}`；Deserialize 读取 basicInfo 的 `rawMeta.entry` 数组；`from_headers()` 从 HTTP 头筛选字段。这三种形式用途不同，不是对称 JSON 编解码接口。

## 对象列举与分页

### 单页与目录式浏览

```rust,no_run
use fds::{Client, ListObjectsRequest};

async fn list_directory(client: &Client, bucket: &str) -> fds::Result<()> {
    let page = ListObjectsRequest::new(bucket)
        .prefix("reports/")
        .delimiter("/")
        .max_keys(100)
        .send(client).await?;
    for object in page.object_summaries {
        println!("{}\t{}", object.object_name, object.size);
    }
    for prefix in page.common_prefixes {
        println!("目录前缀：{prefix}");
    }
    if page.truncated {
        // 将服务端 next_marker 原样传回；不是 offset 或自行计算的 key。
        let _next = ListObjectsRequest::new(bucket)
            .prefix("reports/").delimiter("/").max_keys(100)
            .marker(page.next_marker).send(client).await?;
    }
    Ok(())
}
```

默认 `max_keys = 1000`；非正数在执行时拒绝。`ObjectListing` 包含对象数组、common_prefixes、truncated、next_marker 等分页信息。`ObjectSummary` 包含 `object_name`、`size`、`etag`、`owner`、可选 UTC `last_modified` 和整数 `upload_time`。

prefix 是字符串前缀，没有真实目录实体。要遍历前缀下全部对象通常省略 delimiter；设为 `/` 后，下一层分组出现在 common_prefixes，SDK 不递归进入这些分组。

### 自动遍历页或对象

```rust,no_run
use fds::{Client, ListObjectsRequest};
use futures_util::{pin_mut, StreamExt};

async fn sum_prefix(client: &Client, bucket: &str, prefix: &str) -> fds::Result<u64> {
    let objects = ListObjectsRequest::new(bucket).prefix(prefix).objects(client);
    pin_mut!(objects);
    let mut total = 0u64;
    while let Some(object) = objects.next().await {
        let object = object?;
        let size = u64::try_from(object.size)
            .map_err(|_| fds::FdsError::invalid("服务返回负数大小"))?;
        total = total.checked_add(size)
            .ok_or_else(|| fds::FdsError::invalid("累计大小溢出"))?;
    }
    Ok(total)
}

async fn count_pages(client: &Client, bucket: &str) -> fds::Result<usize> {
    let pages = ListObjectsRequest::new(bucket).pages(client);
    pin_mut!(pages);
    let mut count = 0;
    while let Some(page) = pages.next().await {
        let _page = page?;
        count += 1;
    }
    Ok(count)
}
```

`pages()`/`objects()` 是惰性流，创建时不请求、不预取下一页；错误后终止。truncated 但 next_marker 缺失或相邻重复时报协议错误；较长循环用常数空间检测，可能先输出部分重复页再报错。objects 会展开每页对象，保留一页响应和常数个 marker，不保证对象去重。列举不是事务快照，并发新增/删除时不要假定清单代表某个精确时刻的完整集合。

## 复制、重命名与误删恢复

```rust,no_run
use fds::{Client, CopyObjectRequest};

async fn copy_and_rename(client: &Client, src_bucket: &str, dst_bucket: &str) -> fds::Result<()> {
    let receipt = CopyObjectRequest::new(src_bucket, "incoming/a.bin", dst_bucket, "backup/a.bin")
        .send_with_response(client).await?;
    assert_eq!(receipt.bucket_name, dst_bucket);
    assert_eq!(receipt.object_name, "backup/a.bin");
    client.rename_object(dst_bucket, "backup/a.bin", "backup/final.bin").await?;
    Ok(())
}
async fn restore_deleted(client: &Client, bucket: &str, key: &str) -> fds::Result<()> {
    client.restore_object(bucket, key).await
}
```

复制是服务端操作，不会把源文件下载到调用进程。支持分别指定源/目标 Bucket 和 key；仍使用此 Client 的目标服务入口，不能据此假设能跨地区或跨独立 FDS 服务复制。没有条件防覆盖保证，也没有把复制和 rename 包装为事务。

`copy_object_with_response(&request)` / `send_with_response(client)` 返回 `CopyObjectResponse`。原 `copy_object` / `send` 仍返回 `Result<()>`，但同样执行完整回执校验：必须 HTTP 200、完整有效 JSON、必填 `bucketName` / `objectName` 与请求目标完全一致。回执最多 64 KiB（含额外字段和空白），空体、截断、缺字段、目标不符、超限及超时均失败；读取受 Client 的 `request_timeout` 和 `read_timeout` 约束。

Copy 不自动重试。`FdsError::CopyOutcomeUnknown` 表示未取得有效匹配回执，**目标可能已写入，不能据此认定复制未执行，也不能盲目重试**；可通过 `source()` 检查原因，HTTP 诊断仍可通过 `status()` / `response()` 读取。应由业务结合目标身份、版本或内容证据核对结果，仅检查目标存在不能证明它来自本次 Copy。参数校验失败仍返回原本的本地错误。

`rename_object` 在同一 Bucket 中改名。`restore_object` 是误删恢复（`?restore`），是否仍可恢复取决于服务保留策略；它不是归档解冻。

## 单对象、批量与前缀删除

单个对象调用 `client.delete_object(bucket, key).await?`。批量删除接受 `&[String]` 和 `put_to_trash: bool`，发送一个批次；它不自动把任意长数组切成符合服务限制的小批次。

```rust,no_run
use fds::{Client, DeleteObjectsError, DeleteObjectsOutput, ListObjectsRequest};

async fn delete_selected(client: &Client, bucket: &str, keys: &[String])
    -> Result<DeleteObjectsOutput, DeleteObjectsError>
{
    match client.delete_objects(bucket, keys, true).await {
        Ok(output) => Ok(output),
        Err(error) => {
            eprintln!("已确认删除 {}，明确失败 {}，结果未知 {}，未提交 {}",
                error.output.deleted.len(), error.output.failures.len(),
                error.unconfirmed.len(), error.not_submitted.len());
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
    let request = ListObjectsRequest::new(bucket).prefix(prefix).max_keys(100);
    Ok(client.delete_objects_with_prefix(&request, true).await?)
}
```

`put_to_trash = true` 发送 `enableTrash=true`，false 发送 false；不要把 true 理解成 SDK 提供可恢复保证。空 names 不发请求。

| 结果字段 | 如何使用 |
| --- | --- |
| `Ok(output).deleted` | 按合法服务失败数组推导的成功项，并非逐个 HEAD 再检查 |
| `Err(error).output.deleted` | 错误发生前已确认删除的项 |
| `output.failures` | 服务返回的 object_name、error_code、error_description；HTTP 200 也可能有失败 |
| `unconfirmed` | 已尝试传输但结果未确认的批次，可能实际已删除 |
| `not_submitted` | 本地校验拒绝而未发送的项 |
| `source` | 可选底层 FdsError；只有逐项失败时可以为 None |

前缀删除逐页列举、逐页删除，首次部分失败或错误即停止；后续尚未列举的对象不会出现在错误清单中。它会累计已确认结果，因此结果内存随删除数量增长；海量清理可由应用分页、分批持久记录进度。

## Bucket 与对象 ACL

ACL 的 `Add` 是默认增量添加，`Delete` 删除指定授权项；没有 Replace action。把从本地 Vec 删除 grant 后的列表发为 Add，不会撤销远端原授权。

```rust,no_run
use fds::{AccessControlList, AclAction, Client, GetObjectACLRequest, Grant,
    GrantKey, GrantPermission, GrantType, SetBucketAclRequest, SetObjectACLRequest};

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
    SetBucketAclRequest::new(bucket, read_grant(user_id)).send(client).await?;
    SetObjectACLRequest::new(bucket, key, read_grant(user_id)).send(client).await?;
    let _bucket_acl = client.get_bucket_acl(bucket).await?;
    let _object_acl = GetObjectACLRequest::new(bucket, key).send(client).await?;
    Ok(())
}

async fn revoke_object_read(client: &Client, bucket: &str, key: &str, user_id: &str) -> fds::Result<()> {
    SetObjectACLRequest::new(bucket, key, read_grant(user_id))
        .action(AclAction::Delete).send(client).await
}
```

`GrantType` 为 User/Group；`GrantPermission` 为 Read、Write、ReadObjects、SsoWrite、FullControl。不同资源、用户、服务权限可能限制可用组合。ACL 结果还包含 owner。对象 GET/SET ACL 请求都有 `.version_id(...)`；其余对象读写入口不能因此推断支持版本寻址。服务文档中的“不能删除自身 ACL”限制仍需遵守。

`client.set_object_public(bucket, key).await?` 添加 `ALL_USERS` Group 的 Read 授权，意味着公开可读。只有业务明确需要匿名访问时才使用。

创建桶的 canned ACL 使用独立枚举，可设置多个值：

```rust,no_run
use fds::{BucketCannedAcl, Client, CreateBucketRequest};

async fn create_public_read_bucket(client: &Client, bucket: &str) -> fds::Result<()> {
    CreateBucketRequest::new(bucket)
        .acl([BucketCannedAcl::PublicRead])
        .send(client).await
}
```

可选值：PublicRead、PublicWrite、PublicReadObjects、PublicSsoWrite、AuthenticatedRead、AuthenticatedWrite、AuthenticatedSsoWrite。空列表不发送 canned ACL 头。常规私有业务不要为了生成 URL 而开放全桶 ACL，优先按需签名访问。

## 手动分片上传

适合调用者自行管理分片来源、调度、回执和恢复流程。步骤是 init → 一个或多个 part → complete；放弃已知会话时显式 abort。普通文件上传优先考虑下一节托管接口。

```rust,no_run
use fds::{Client, FdsError, InitMultipartUploadRequest, InitMultipartUploadResponse,
    PutObjectResponse, UploadPartResponse};

// 返回会话给调用者保存；随后执行分片，不把会话藏在一个会丢失上下文的函数内。
async fn begin(client: &Client, bucket: &str, key: &str) -> fds::Result<InitMultipartUploadResponse> {
    let mut session = InitMultipartUploadRequest::new(bucket, key).send(client).await?;
    if session.upload_id.is_empty() {
        return Err(FdsError::invalid("未收到 upload ID，初始化结果需核对"));
    }
    // 手动入口保留服务响应字段；后续地址显式绑定调用者输入。
    session.bucket_name = bucket.into();
    session.object_name = key.into();
    Ok(session)
}

async fn send_part(client: &Client, session: &InitMultipartUploadResponse,
    number: i32, bytes: bytes::Bytes) -> fds::Result<UploadPartResponse>
{
    session.part(number, bytes).send(client).await
}

async fn finish(client: &Client, session: &InitMultipartUploadResponse,
    mut receipts: Vec<UploadPartResponse>) -> fds::Result<PutObjectResponse>
{
    receipts.sort_by_key(|part| part.part_number);
    session.complete(receipts)
        .expires(chrono::Utc::now() + chrono::Duration::minutes(10))
        .send(client).await
}

async fn abandon(client: &Client, session: &InitMultipartUploadResponse) -> fds::Result<()> {
    session.abort(client).await
}
```

调用顺序示例：保存 `begin()` 的返回值；按从 1 开始的编号调用 `send_part()`，每收到一份回执就保存；确认分片编号、长度、ETag 和总数满足预期后把回执交给 `finish()`。回执为 `UploadPartResponse { part_number, etag, part_size }`。手动 complete 按调用者提供的集合序列化，不替调用者检查完整性或自动排序。

init 与 complete 支持 `.metadata(...)`，complete 另支持 `.expires(...)`。part 支持和 PUT 一样的 UploadBody，可显式 `.content_md5(...)`；也可用 `UploadPartRequest::new(bucket, key, upload_id, number, body)` 从持久化信息重建请求。

分片编号必须为正数。服务端分片尺寸、数量和地区限制需单独满足；库内 `MIN_PART_SIZE`/`MAX_PART_SIZE` 常量不代表所有接口都自动检查，更不能当成各地区统一约束。

初始化超时可能已创建远端会话但没有收到 ID。part 失败可能已被接收。complete 失败可能已经完成对象：保留 upload ID、回执和目标身份，先核对远端结果，再决定下一步；不要在 complete 错误后自动 abort 或重发。SDK 没有 list-parts 或通用的跨进程续传协调接口。

## 托管文件分片上传

`MultipartUpload` 打开一个非空普通文件，限制并发和缓冲，收集回执并按编号 complete。它属于核心 SDK，不依赖 manager feature。

```rust,no_run
use fds::{CancellationToken, CleanupPolicy, Client, InitMultipartUploadRequest,
    MultipartOptions, MultipartUpload, MultipartUploadError, PutObjectResponse};

async fn prepare_large_file(client: &Client, bucket: &str, key: &str, path: &std::path::Path)
    -> fds::Result<MultipartUpload>
{
    let options = MultipartOptions::new(8 * 1024 * 1024, 4, CleanupPolicy::Preserve)?;
    let request = InitMultipartUploadRequest::new(bucket, key);
    MultipartUpload::start(client, request, path, options).await
}

// session 由上层持有；错误返回后仍可读取 progress，不会随本函数丢失。
async fn run_upload(session: &mut MultipartUpload, cancel: &CancellationToken)
    -> Result<PutObjectResponse, MultipartUploadError>
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

调用者先 await `prepare_large_file` 并保存会话，再创建 `CancellationToken::new()`，调用 `run_upload(&mut session, &cancel)`。控制任务可持有 token 的 clone；失败后从仍存活的 session 读取并持久化进度，随后决定恢复。

默认 8 MiB、4 并发、Preserve。`MultipartOptions::new` 的本地限制：每片 1 字节至 64 MiB，并发 1–64，二者乘积不超过 256 MiB，文件最多 10,000 片。非最后一片仍需满足服务要求；不要因本地允许 1 字节就按 1 字节部署。payload 缓冲约束为 `part_size × concurrency`，另有 HTTP、TLS、文件和响应模型开销，不是进程 RSS 上限。

| 状态 | 含义与处理 |
| --- | --- |
| Ready | 已初始化，尚未上传 |
| Uploading | 上传中；future 被丢弃可能留下此状态，需要显式核对 |
| Failed | 分片/文件/完成前取消等错误；Preserve 保留会话 |
| Aborted | 显式清理策略的 abort 得到成功响应 |
| CompletionUnknown | complete 可能已发送或返回无效结果；先核对，禁止盲目清理 |
| Completed | complete 响应有效且对象身份匹配 |

`progress()` 包含 `upload`（bucket/key/upload_id）、`parts`、`total_parts`、`file_size`、`state`。这是会话快照，不是可直接导入的自动恢复格式。`upload(&mut self, ...)` 只允许运行一次；失败后不能再次调用它来隐式续传。

`CleanupPolicy::AbortOnFailure` 仅在完成前错误/合作式取消时尝试一次 abort；主错误为 `MultipartUploadError.cause`，清理的第二个错误为 `cleanup_error`。`Preserve` 是默认策略。合作式取消要调用 token.cancel() 后继续 await；直接 timeout/drop future 不执行异步清理，整个 session Drop 也不发请求。源文件在全会话期间必须保持不变。

可运行基础示例见 [upload_multipart.rs](crates/fds/examples/upload_multipart.rs)，详细状态行为见 [重试与分片](docs/runtime.md#分片会话与恢复)。

## 并发下载与断点续传

需要 `manager`（默认开启）。下载器接受 Client 的廉价副本，配置和单次下载请求分开构造。

```rust,no_run
use fds::{Client, DownloadOptions, DownloadRequest, Downloader};

async fn download_safely(client: &Client, bucket: &str, key: &str, path: &std::path::Path)
    -> Result<(), fds::ManagerError>
{
    let options = DownloadOptions::builder()
        .part_size(4 * 1024 * 1024)
        .concurrency(4)
        .resume(true)
        .build()?;
    let downloader = Downloader::new(client, options);
    DownloadRequest::new(bucket, key, path).send(&downloader).await
}
```

默认每片 1 MiB、4 并发、resume=false、无限流。分片字节数和并发须大于零；最大分片数为 1,000,000，太多时应增大片尺寸。范围下载使用 `.range(ByteRange::from(...))`，只将选中范围写入目标文件，文件开头对应选中范围的起点。

执行时 HEAD 获取对象长度。具有强 ETag 时，逐片检查相同 ETag、206、Content-Range 和精确长度；没有强 ETag 时使用单个流式 Range 请求，不复用断点。HTTP 200 忽略 Range 会报错。

下载先写同目录暂存文件，全部成功并同步后替换目标，失败保留旧目标。父目录必须存在且可写。同目标使用 `.download.lock` 合作式排他；还会使用 `.tmp`、`.download.bp`、`.download.bp.new` 等路径，调用者不能同时使用这些附属路径。中断后对同一路径重新调用并开启 resume，下载器校验对象身份和本地分片摘要，损坏或不匹配分片重新下载。进程被杀死可能留下锁，确认无下载进程后才移除。

取消直接丢弃 download future；worker 结束前仍持有锁，立即重试可能暂时报 AlreadyExists。同步文件 IO、fsync 和恢复摘要可能占用运行时线程，建议多线程 Tokio。它不承诺跨程序目录防篡改或目录 fsync 级断电持久性；详见 [下载安全](docs/runtime.md#下载文件与断点)。

### 请求启动限流

```rust,no_run
use fds::{DownloadOptions, ManagerError};
use governor::{Quota, RateLimiter};
use std::{num::NonZeroU32, sync::Arc};

fn limited_download_options() -> Result<DownloadOptions, ManagerError> {
    let limiter = Arc::new(RateLimiter::direct(
        Quota::per_second(NonZeroU32::new(5).unwrap()),
    ));
    DownloadOptions::builder().concurrency(4).request_limiter(limiter).build()
}
```

每个分片请求启动消耗一个 permit，限的是请求频率，不是字节带宽。governor 的 quota 允许突发，并非严格每 200 ms 一次；多个 downloader 共用同一个 Arc 可共享此预算。

## 预签名与普通 URL

```rust,no_run
use fds::{Client, GeneratePresignedURLRequest, HttpMethod, ObjectMetadata};
use std::time::Duration;

fn download_link(client: &Client, bucket: &str, key: &str) -> fds::Result<String> {
    GeneratePresignedURLRequest::expires_in(bucket, key, HttpMethod::Get, Duration::from_secs(600))?
        .generate(client)
}

fn upload_link(client: &Client, bucket: &str, key: &str) -> fds::Result<String> {
    let mut metadata = ObjectMetadata::new();
    metadata.set_content_type("application/octet-stream");
    GeneratePresignedURLRequest::expires_in(bucket, key, HttpMethod::Put, Duration::from_secs(600))?
        .metadata(metadata).generate(client)
}

fn public_link(client: &Client, bucket: &str, key: &str) -> fds::Result<String> {
    client.generate_absolute_object_url(bucket, key)
}
```

预签名仅支持 access key Client，OAuth Client 返回错误。生成操作不联网、不验证对象存在或凭据是否有效。有效期从构造请求时计算，不是实际交付 URL 时；`expires_in` 至少 1 ms，绝对时间可用 `new(bucket, key, method, DateTime<Utc>)`。

接收方必须使用签名时的 HTTP method 与参与签名的头；例如上传示例使用相同 Content-Type。SDK 返回完整 String，保留它原样使用，不自行重排 query 或重复编码 Signature。该协议 Expires 是毫秒时间戳，HEAD 预签名还附带 metadata 子资源。URL 包含临时访问凭证，应经受控渠道交付，不放公开日志。

`.cdn(true)` 请求 CDN 地址，必须在构建 Client 时显式设置 `cdn_endpoint(origin)`；未配置会返回错误，不会从 FDS 主机自动推导。普通 SDK 数据请求始终使用 FDS endpoint。CDN 地址是否已开通由服务环境决定。

普通 URL 不含认证，也不会把对象设为公开；匿名访问仍需权限。空 object 可生成 Bucket URL，Bucket/object 都空可生成 endpoint 根 URL。

## 生命周期与访问日志

### 生命周期规则

```rust,no_run
use fds::{Client, GetLifecycleConfigRequest, LifecycleBaseItem, LifecycleConfig, LifecycleRule};
use fds::bucket::{LifecycleAction, ACTION_EXPIRATION};

async fn configure_lifecycle(client: &Client, bucket: &str) -> fds::Result<()> {
    let mut action = LifecycleAction::new();
    action.insert(ACTION_EXPIRATION.into(), LifecycleBaseItem { days: 30.0 });
    let rule = LifecycleRule {
        id: "expire-temporary-reports".into(), prefix: "temporary-reports/".into(),
        enabled: true, action,
    };
    // 配置指定规则。
    client.set_lifecycle_rule(bucket, &rule).await?;
    let _selected = GetLifecycleConfigRequest::new(bucket)
        .rule_id(&rule.id).send(client).await?;
    let _all = GetLifecycleConfigRequest::new(bucket).send(client).await?;

    // 整体提交 rules；仅在确实要管理完整配置时调用。
    let config = LifecycleConfig { rules: vec![rule] };
    client.set_lifecycle_config(bucket, &config).await?;
    Ok(())
}
```

`LifecycleRule.action` 是 action 名到 `{ days: f64 }` 的 map，序列化字段名为 `actions`。`LifecycleRule::from_json` 和 `LifecycleConfig::from_json` 接受 JSON bytes；完整配置形式为 `{"rules":[...]}`。

模块常量还有 `ACTION_NON_CURRENT_VERSION_EXPIRATION` 和 `ACTION_ABORT_INCOMPLETE_MULTIPART_UPLOAD`。后者保留了协议兼容字符串的尾随空格，不能自行 trim 后假定等价。SDK 不验证天数业务范围，也没有独立 delete-lifecycle-rule 方法；禁用可提交同 ID 的 `enabled: false` 规则，具体更新/删除语义以服务为准。expiration 会影响匹配对象的数据保留，请按业务策略选择前缀和天数。

### 访问日志

```rust,no_run
use fds::{AccessLog, Client};

async fn enable_logs(client: &Client, bucket: &str, log_bucket: &str) -> fds::Result<()> {
    let config = AccessLog {
        bucket_name: bucket.into(), enabled: true,
        log_bucket_name: log_bucket.into(), log_prefix: "fds-access/".into(),
    };
    client.set_access_log(bucket, &config).await?;
    let _effective = client.get_access_log(bucket).await?;
    Ok(())
}
async fn disable_logs(client: &Client, bucket: &str) -> fds::Result<()> {
    let mut config = client.get_access_log(bucket).await?;
    config.bucket_name = bucket.into();
    config.enabled = false;
    client.set_access_log(bucket, &config).await
}
```

保证路径参数 bucket 与配置的 bucket_name 一致。日志目标 Bucket、写入权限、交付延迟需在服务端配置和确认；返回成功只表示配置请求被接受。

## CDN 预取与刷新

```rust,no_run
use fds::Client;

async fn request_prefetch(client: &Client, bucket: &str, key: &str) -> fds::Result<()> {
    client.prefetch_object(bucket, key).await
}
async fn request_refresh(client: &Client, bucket: &str, key: &str) -> fds::Result<()> {
    client.refresh_object(bucket, key).await
}
```

预取用于请求缓存对象；刷新用于请求失效更新。使用对象所属 FDS endpoint，不向 CDN URL 调用这些 SDK 方法。两者都是 mutation，不自动重试，返回 `Result<()>`，HTTP 200 只表示请求确认，没有任务 ID 或完成等待接口。需要对象 owner、目标地区和 CDN 服务支持；若流程依赖 CDN 已生效，要另外读取 CDN 内容核验。协议依据见 [CDN 说明](docs/protocol.md#cdn-预取与刷新)。

## 服务发现与 OAuth

### 显式发现 endpoint

```rust,no_run
use fds::{Client, FdsError, FdsServiceClient};

async fn discover_client(discovery_origin: &str, region: &str, service_name: &str,
    allowed_hosts: &[String], id: &str, secret: &str) -> fds::Result<Client>
{
    let discovery = FdsServiceClient::new(discovery_origin)?;
    let host = discovery.get_endpoint(region, service_name).await?;
    if !allowed_hosts.contains(&host) {
        return Err(FdsError::invalid("发现主机不在应用允许列表"));
    }
    Client::builder().endpoint(format!("https://{host}"))
        .credentials(id, secret).build()
}
```

必须显式提供 discovery origin、非空 region 和 service；返回 JSON 主机名字符串，不是带 scheme 的 URL。SDK 只检查主机名格式，不验证其可信性、可达性或可用地区，不自动切换已有 Client。应用验证返回主机后再绑定凭据。

`FdsServiceClient::from_builder(Client::builder().endpoint(...).request_timeout(...))` 可复用超时/连接设置；builder 上的凭据会被丢弃，此客户端专用于发现和交换。

### 交换 storage token 并访问对象

```rust,no_run
use fds::{Client, FdsServiceClient, GetObjectRequest, OAuthProvider, StorageTokenRequest};

async fn oauth_read(exchange_origin: &str, object_origin: &str, app_id: &str,
    oauth_app_id: &str, provider_token: &str, bucket: &str, key: &str) -> fds::Result<u64>
{
    let service = FdsServiceClient::new(exchange_origin)?;
    let input = StorageTokenRequest::new(app_id, oauth_app_id, provider_token, OAuthProvider::QQ);
    let token = service.exchange_storage_token(&input).await?;
    let expires_at = token.expires_at();
    let client = Client::builder().endpoint(object_origin).storage_token(token).build()?;
    let _app_buckets = client.list_app_buckets().await?;
    let mut output = GetObjectRequest::new(bucket, key).send(&client).await?;
    let mut count = 0;
    while let Some(chunk) = output.chunk().await? {
        count += chunk.len() as u64;
    }
    println!("读取 {count} 字节，token 到期时间 {expires_at}");
    Ok(count)
}
```

provider 为 XiaoMi、QQ、Sina、RenRen。XiaoMi 还需追加 `.xiaomi_mac(open_id, mac_key, algorithm)`，三者非空；algorithm 原样交给服务，不在本地计算 MAC。Weixin 尚无可调用枚举。provider token 由应用通过相应授权流程获取，SDK 不提供用户登录流程。

`StorageAccessToken` 提供 `app_id()`、`expires_at()`、`is_expired()` 和敏感访问器 `token()`；过期时间是 UTC 绝对时间，接口不提供任意字符串手动构造 token 的公开入口。交换始终只尝试一次。token 绑定 appId，`list_app_buckets` 自动使用该 appId，返回 `AppBuckets { owner, buckets }`；AppBucket 含 name、org_id、creation_time、used_space、num_objects。

OAuth Client 普通对象 API 共用 token 认证；`credentials` 与 `storage_token` 最后一个 setter 生效。构建及每次发送前检查过期；不会自动续期。需要新的 provider 凭据时由应用取得，再显式 exchange、构建新 Client 并替换应用持有的句柄，旧 clone 不更新。

token 传输要求 HTTPS，仅 HTTP loopback IP 允许本地测试。OAuth 错误会丢弃服务端诊断字符串；不要记录 provider token、MAC key、完整认证 URL。应用开通、region 和 Bucket 权限前提见 [OAuth 说明](docs/protocol.md#服务发现与-oauth)。

## 错误处理、超时与重试

### 区分错误种类

多数 API 返回 `fds::Result<T>`；批量删除返回 `DeleteObjectsError`，下载管理器返回 `ManagerError`，托管上传执行返回 `MultipartUploadError`。

```rust,no_run
use fds::FdsError;

fn report_error(error: &FdsError) {
    if let Some(response) = error.response() {
        eprintln!("FDS 状态 {}，错误类别 {:?}，诊断截断 {}",
            response.status(), response.kind(), response.body_truncated());
        // service_code/request_id/diagnostic_body 是服务原文，按应用规则脱敏后再使用。
        return;
    }
    match error {
        FdsError::Http(source) if source.is_timeout() => eprintln!("传输超时"),
        FdsError::Http(_) => eprintln!("连接或传输失败"),
        FdsError::Io(_) => eprintln!("本地文件 IO 失败"),
        FdsError::Invalid(_) | FdsError::Endpoint => eprintln!("本地参数或 endpoint 无效"),
        FdsError::Cancelled => eprintln!("调用取消"),
        _ => eprintln!("其他 SDK 错误"),
    }
}
```

`FdsError` 为 non_exhaustive，match 必须留兜底分支。`status()`、`service_code()`、`request_id()` 是便利访问器；ResponseError 还含 `kind()`、最多 8 KiB 的 `diagnostic_body()`、`body_truncated()`。类别包括 UnexpectedStatus、EmptyJson、InvalidJson、BodyRead。

HTTP status 与服务 error code 是两套字段。JSON 操作即使 HTTP 200，也可能因空体或非法 JSON 失败；权限错误不能转换成“对象不存在”。SDK 禁止自动跟随重定向，未列入操作成功状态的 2xx 也会失败。更多状态规则见 [响应处理](docs/runtime.md#响应与错误)。

SDK 错误与响应 Debug 做了部分脱敏，但调用者自己的错误文本、metadata、ACL、key、原始响应和 PutObjectResponse 等并非统一脱敏对象。不要把任意 SDK 模型整体打印当成安全日志策略。

### 重试和整体期限

`RetryPolicy::new(max_retries, initial_delay, max_delay)` 中 max_retries 不含首次尝试，允许 0–100；初始等待大于零且不超过最大等待，最大等待不超过 60 秒。默认 max_retries=0。

启用后，仅明确安全的读取在响应交付前的连接/发送/超时错误和 429、502、503、504 可重试。采用有随机抖动的指数退避；Retry-After 作为等待下限，超过 max_delay 时放弃重试。JSON 解码错误、成功响应之后的 body 断流、上传流错误均不重放。

PUT、复制、rename、ACL 修改、删除、分片 init/part/complete/abort、token exchange 和 CDN 操作不自动重试。前缀删除仅列举步骤可以重试。不能仅凭 HTTP 动词判断重复业务操作安全。

客户端 request_timeout 是每次请求的期限；完整下载 body 消费和所有重试的外层期限可以这样设置：

```rust,no_run
use fds::{Client, GetObjectRequest};
use std::time::Duration;

async fn read_with_deadline(client: &Client, bucket: &str, key: &str)
    -> Result<bytes::Bytes, Box<dyn std::error::Error>>
{
    let bytes = tokio::time::timeout(Duration::from_secs(30), async {
        GetObjectRequest::new(bucket, key).send(client).await?.bytes().await
    }).await??;
    Ok(bytes)
}
```

这里仍是小对象读取。若将 timeout 用于写入，超时只意味着停止等待，不证明远端没有执行；托管上传需要清理时使用合作式 CancellationToken。

## CSLB 负载均衡

启用 `cslb` feature 后，构建 Client 就会安装 DNS resolver；可用 `.cslb_max_nodes(20)` 调整每个主机保留的节点上限。

它在 DNS 解析时轮换首选地址并附带备选 IP，以 600 秒 TTL 刷新。HTTP 连接池复用不会逐请求重新选节点；节点数不是 HTTP 并发数。集成 resolver 没有连接失败反馈回调，不能视为自动故障摘除系统；无节点时会回退系统 DNS。

普通 SDK 接入只需配置 Client。自定义节点来源或哈希选择策略等独立工具位于 `fds::cslb`，可通过 `cargo doc --all-features --no-deps --open` 查看类型文档。

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

环境变量：`FDS_OAUTH_ORIGIN`（交换入口）、`FDS_ENDPOINT`（对象入口）、`FDS_APP_ID`、`FDS_OAUTH_APP_ID`、`FDS_PROVIDER_TOKEN`。两处 origin 都由应用明确配置。程序不修改 Bucket 或对象，但 token 交换仍可能创建/续期凭据。没有自动刷新；长时间统计若 token 到期会返回错误，需要应用按策略重新交换并决定是否重新扫描。

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
