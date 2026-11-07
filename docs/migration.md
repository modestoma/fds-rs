# 0.2 迁移指南

从旧接口迁移时查阅本文；新项目直接阅读 [SDK 手册](../README.md)。0.2 不保留旧构造器、重复请求类型或旧模块路径适配层。

## API 变化

| 原调用/假设 | 当前真实 API / 行为 |
| --- | --- |
| 直接写配置和凭据字段 | `Client::builder().endpoint(url).credentials(id, secret).build()?`，构建后不可变、可 Clone |
| 下载配置 | `DownloadOptions::builder()` 的具名选项，构造时校验；`Downloader::new(client, options)` |
| 下载请求 | `DownloadRequest::new(bucket, key, path)`，`PathBuf` 路径和 `ByteRange`；成功才原子替换目标 |
| Metadata | 读取结果 `ObjectMetadata` 分为类型化的 `attributes` 与 `user_metadata`；写入使用专用 setter 与 `UserMetadata` |
| multipart 完成 | `session.complete(vec![part1, part2])` 或传入拥有 receipts 的迭代器；progress.parts 直接是 Vec |
| 上传 | 唯一 `PutObjectRequest` / `UploadPartRequest` 拥有 UploadBody，`send` 与 Client 上传方法均消耗请求 |
| 对象读取 | `GetObjectRequest::new(bucket, key).send(&client)` 返回 `GetObjectOutput`，用 `chunk()` 流式读取；`ByteRange` 表达单段范围 |
| 手动翻页 | `ListObjectsRequest::new(bucket).prefix(prefix).pages(&client)` / `objects(&client)`，无预取；marker 保持不透明 |
| 同名 metadata 即 HEAD | `head_object` 返回 `Option<ObjectMetadata>`；GET `?metadata` 仍由 `get_object_metadata` 提供 |
| HTTP 200 批删即成功 | 检查 `DeleteObjectsOutput.failures`；prefix 删除失败保留已完成进度 |
| set ACL 即替换 | ACL 请求的 `.action(...)` 明确 Add/Delete；默认 Add，不承诺 replace |
| 重试任何失败 | `RetryPolicy` 默认关闭，开启后默认仅明确安全的读取重试；幂等写（Bytes body）需 `retry_idempotent_writes()` 或单请求 `.retry(RetryMode::Enabled)` 显式开启；其他 mutation 与文件/流 body 不自动重放 |
| 托管 multipart 自动完成恢复 | `MultipartUpload` 保存 session；失败检查 progress/state，完成结果未知时不盲重试、不自动 abort |
| OAuth token 当 access secret | `FdsServiceClient` 显式 exchange，再用 `storage_token` 构建绑定 appId 的客户端；需要自动续期时用 `credentials_provider(StorageTokenProvider::new(..))` |
| 任何 2xx 都成功、错误字符串稳定 | 按操作收紧状态，使用类型化错误访问器；服务诊断不作为稳定解析协议 |

批量删除的返回类型为 `Result<DeleteObjectsOutput, DeleteObjectsError>`，不再是 `fds::Result<()>`。调用者需接收专用错误并保留部分结果，不能只将 HTTP 200 当作成功。HTTP/协议错误改用 `FdsError::Response`；移除旧 Server、code/message 访问器，使用 `status()`、`service_code()`、`request_id()` 和 `response()`。

对象模型和 builder 位于 `crates/fds/src/object/`；`ByteRange`、`GetObjectOutput` 位于 `fds::download`，常用类型由 crate 根重导出。[迁移样例](../crates/fds/examples/migration.rs)编译实际 SDK 类型。

## 配置字段迁移

| 移除的旧配置 | 迁移方式 |
| --- | --- |
| endpoint、enable_https | `endpoint(full_url)`，不再允许构造后变更 |
| http_timeout.connect_timeout / idle_conn_timeout / keep_alive_timeout | connect_timeout / pool_idle_timeout / tcp_keepalive |
| timeout、read_write_timeout、header_timeout、long_timeout | 旧项未接入请求；现在明确选择 request_timeout 和 read_timeout，不宣称等价 |
| tls_handshake_timeout | 旧项未生效；TLS 受 connect_timeout 约束 |
| enable_cdn_for_upload / enable_cdn_for_download | 旧项无效；CDN 只用于显式预签名 URL |
| max_connection | cslb_max_nodes（仅在 cslb 下曾生效）；没有连接并发限流含义 |
| batch_delete_size、retry_count、retry_interval、part_size | 旧客户端字段无效；读取重试改用显式 RetryPolicy，托管上传使用 MultipartOptions；Downloader 分片/并发由 DownloadOptions 的具名 builder 提供 |
| max_dns_record、download_bandwidth、upload_bandwidth、http_keep_alive_timeout_ms | 旧项无效；下载限速继续用 DownloadOptionsBuilder::request_limiter；不承诺上传限速 |
| region_name | 通用 origin 不必对应 FDS 区域，删除派生区域访问器 |

`manager` 默认启用；`cslb` 独立启用，只影响 Client 的 DNS 解析。原 `fds::cslb` 模块及其独立负载均衡类型（LoadBalancer、Service、Strategy、Group、Metrics、CslbError 等）已删除，没有替代 API。CLI 已拆为 workspace 的 `fds_cli` 包，SDK 不再提供 `cli` feature；使用 `cargo run -p fds_cli -- --help`。客户端和 CLI 均只接受完整 origin。Client、配置和凭据构建后不可变；轮换配置需构建新实例。

## 下载断点兼容性

旧断点格式不能复用。当前下载器使用包含 endpoint、bucket/key、强 ETag、范围与分片摘要的检查点，恢复时重新核验本地分片；旧数据或不匹配部分重新下载。详细文件与取消边界见 [运行语义](runtime.md#下载文件与断点)。

## Rust 与依赖

当前 `rust-version` 为 1.86，锁定依赖中的 ICU 2.2/idna_adapter 不支持旧 1.75 声明。已验证平台为 macOS aarch64；其他目标需自行构建验证。lockfile 中仅用于其他 target 的 wasip2 可能要求更高版本，不代表当前主机使用了它。

下游重新解析依赖可能提高最低版本，请在自己的目标平台锁定并验证依赖。可重复检查入口见 [验证指南](validation.md#本地检查)。
