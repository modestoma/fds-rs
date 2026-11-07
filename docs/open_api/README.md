# FDS 原始 API 索引

本目录保存 35 份原始 YAML：33 份操作定义、2 份错误片段。它们来自 [FDS API 目录](https://docs.api.xiaomi.net/fds/api-specification/)，是协议资料快照，不是可直接生成客户端的统一 OpenAPI 文件。

日常 Rust 调用见 [SDK 手册](../../README.md)，目录导航见 [文档首页](../README.md)。源文件保持原样；冲突字段、采用的协议依据和未开放能力见 [协议说明](../protocol.md)。

## 阅读方式

表中方法、路径和参数来自原始 YAML；SDK 入口列说明实际采用的行为。`?` 表示 query，`H:` 表示 header，`B:` 表示 body。参数栏省略通用 Authorization/Date，完整参数、模型和响应请打开对应 YAML。

- `implemented`：SDK 有对应入口，共 27 项；仅表示代码覆盖，不代表真实服务验收。
- `duplicate`：与另一份定义重叠，共 3 项；保留独立来源和差异。
- `missing`：SDK 尚未提供，共 3 项：POST 创建、归档恢复、分片复制。

特别注意：预取/刷新原文将标志写在 header，SDK 根据交叉证据使用 query；`get-object-metadata.yaml` 定义 HEAD，但 Rust 同名方法是独立的 GET `?metadata`。`Bucket.yaml` 是应用 Bucket 的操作定义，不是共享模型文件。

## 文件索引

### 服务发现与认证

| 文件 | 功能 | 原始方法、路径与参数 | SDK 状态 | SDK 入口与差异 |
| --- | --- | --- | --- | --- |
| [get-endpoint.yaml](get-endpoint.yaml) | 根据地区码和后端服务获取endpoint | `GET /`；`?endpoint`、`?region`、`?service` | implemented | `FdsServiceClient::get_endpoint(..).send()`；显式入口、JSON 主机名，见 [协议证据](../protocol.md#服务发现与-oauth) |
| [OAuth.yaml](OAuth.yaml) | 获取OAuth存储令牌 | `GET /`；`?storageAccessToken`、`?appId`、`?oauthAccessToken`、`?oauthOpenId`、`?oauthAppId`、`?oauthProvider`、`?oauthMacKey`、`?oauthMacAlgorithm` | implemented | 显式交换、绑定 appId 的 OAuth Client、UTC 到期与脱敏，见 [协议证据](../protocol.md#服务发现与-oauth) |

### Bucket 管理

| 文件 | 功能 | 原始方法、路径与参数 | SDK 状态 | SDK 入口与差异 |
| --- | --- | --- | --- | --- |
| [list-bucket.yaml](list-bucket.yaml) | 获取创建的Bucket列表 | `GET /`；`?authorizedBuckets` | implemented | `list_buckets`、`list_authorized_buckets` |
| [Bucket.yaml](Bucket.yaml) | 按 appId 列举应用 Bucket | `GET /`；`?appId` | implemented | `list_app_buckets` 使用绑定 token/appId，保留 orgId；服务权限待真实验收 |
| [put-bucket.yaml](put-bucket.yaml) | 新建Bucket | `PUT /{bucketname}`；`H:x-xiaomi-meta-acl`、`?orgId` | implemented | `create_bucket(..).acl(..)` 支持创建 ACL，`.org_id(..)` / `.storage_class(..)` 可选 |
| [head-bucket.yaml](head-bucket.yaml) | 查看Bucket是否存在 | `HEAD /{bucketname}`；— | implemented | `bucket_exists`，200/404；其他状态报错 |
| [get-bucket-meta.yaml](get-bucket-meta.yaml) | 获取Bucket的meta | `GET /{bucketname}`；— | implemented | `get_bucket_info`，裸 GET Bucket |
| [delete-bucket.yaml](delete-bucket.yaml) | 删除Bucket | `DELETE /{bucketname}`；— | implemented | `delete_bucket` |

### Bucket ACL

| 文件 | 功能 | 原始方法、路径与参数 | SDK 状态 | SDK 入口与差异 |
| --- | --- | --- | --- | --- |
| [get-bucket-acl.yaml](get-bucket-acl.yaml) | 获取Bucket的ACL列表 | `GET /{bucketname}`；`?acl` | implemented | `get_bucket_acl` |
| [put-bucket-acl.yaml](put-bucket-acl.yaml) | 修改Bucket的ACL | `PUT /{bucketname}`；`?acl`、`?action`、`B:body` | implemented | `set_bucket_acl(..).action(..)` 支持类型化 add/delete |

### Object 读写与管理

| 文件 | 功能 | 原始方法、路径与参数 | SDK 状态 | SDK 入口与差异 |
| --- | --- | --- | --- | --- |
| [list-objects.yaml](list-objects.yaml) | 获取某个Bucket下的Object列表 | `GET /{bucketname}`；`?prefix`、`?delimiter`、`?marker`、`?maxKeys` | implemented | 同一 ListObjectsRequest 支持 marker、单页与惰性 pages / objects |
| [put-object.yaml](put-object.yaml) | 上传object，如果有同名Object，将会覆盖 | `PUT /{bucketname}/{objectname}`；`H:content-type`、`H:x-xiaomi-meta-`、`?expires`、`B:body` | implemented | `put_object(..)` builder 与 `UploadBody` 支持消耗式 bytes/file/stream 与 expires；`send` 消耗 builder |
| [post-object.yaml](post-object.yaml) | POST 创建对象；指定 key 与覆盖语义未明确 | `POST /{bucketname}/`；`?expires`、`B:body`、`form-data:contentType`、`form-data:cannedAcl`、`form:key` | missing | 无 POST 创建对象 |
| [get-object.yaml](get-object.yaml) | 获取object | `GET /{bucketname}/{objectname}`；`H:Range` | implemented | `get_object(..).send()` 返回 `GetObjectOutput`，支持 `ByteRange` |
| [head-object.yaml](head-object.yaml) | 获取object的meta信息 | `HEAD /{bucketname}/{objectname}`；— | implemented | `head_object` 返回 `Option<ObjectMetadata>`；`object_exists` 是同一请求的 bool 形式 |
| [get-object-metadata.yaml](get-object-metadata.yaml) | 获取object的meta信息 | `HEAD /{bucketname}/{objectname}`；— | duplicate | 与 head-object 同为 HEAD；权限文案不同。映射 `head_object`；同名 `get_object_metadata` 仍是独立 GET `?metadata` |
| [copy-object.yaml](copy-object.yaml) | 将一个src_objectname从src_bucketname复制到dst_bucketname，并重命名为dst_objectname | `PUT /{dst_bucketname}/{dst_objectname}`；`?cp`、`B:body` | implemented | `copy_object` 返回校验后的回执；409 文案不能作为防覆盖保证 |
| [rename-object.yaml](rename-object.yaml) | 重命名文件 | `PUT /{bucketname}/{objectname}`；`?renameTo` | implemented | `rename_object` |
| [delete-object.yaml](delete-object.yaml) | 删除指定object | `DELETE /{bucketname}/{objectname}`；— | implemented | `delete_object` |
| [delete-multiple-objects.yaml](delete-multiple-objects.yaml) | 删除指定Bucket下多个object | `PUT /{bucketname}`；`?deleteObjects`、`B:body` | implemented | `delete_objects` 返回逐项失败；prefix helper 保留已确认进度与失败 |
| [delete-objects.yaml](delete-objects.yaml) | 删除指定Bucket下多个object | `PUT /{bucketname}`；`?deleteObjects`、`B:body` | duplicate | 与 delete-multiple-objects 内容相同，复用同一方法 |

### Object ACL

| 文件 | 功能 | 原始方法、路径与参数 | SDK 状态 | SDK 入口与差异 |
| --- | --- | --- | --- | --- |
| [get-object-acl.yaml](get-object-acl.yaml) | 读取对象 ACL | `GET /{bucketname}/{objectname}`；`?acl` | implemented | `get_object_acl`，额外支持 versionId |
| [put-object-acl.yaml](put-object-acl.yaml) | 修改Object的ACL | `PUT /{bucketname}/{objectname}`；`?acl`、`?action`、`B:body` | implemented | `set_object_acl(..).action(..)` 支持 add/delete |
| [delete-object-acl.yaml](delete-object-acl.yaml) | 删除object的ACL信息 | `PUT /{bucketname}/{objectname}`；`?acl`、`?action`、`B:body` | duplicate | 与 put-object-acl 的 action=delete 重叠，已支持类型化 Delete；保留不能删除自身 ACL 的前提 |

### 分片上传与复制

| 文件 | 功能 | 原始方法、路径与参数 | SDK 状态 | SDK 入口与差异 |
| --- | --- | --- | --- | --- |
| [init-multipart-upload.yaml](init-multipart-upload.yaml) | 初始化分片上传 | `PUT /{bucketname}/{objectname}`；`?uploads` | implemented | `init_multipart_upload`；metadata 可发送模式头，typed MULTI_BLOB 未启用 |
| [upload-part.yaml](upload-part.yaml) | 上传一个分片 | `PUT /{bucketname}/{objectname}`；`?uploadId`、`?partNumber`、`B:body` | implemented | `UploadPartRequest` 支持消耗式内存/file/stream；Client 和 send 均消耗请求 |
| [upload-part-copy.yaml](upload-part-copy.yaml) | 复制一个分片 | `PUT /{bucketname}/{objectname}`；`?uploadId`、`?partNumber`、`H:x-xiaomi-copy-source`、`H:x-xiaomi-copy-source-range` | missing | 无复制分片 |
| [complete-multipart-upload.yaml](complete-multipart-upload.yaml) | 完成分片上传 | `PUT /{bucketname}/{objectname}`；`?uploadId`、`?expires`、`H:content-type`、`B:body` | implemented | `CompleteMultipartUploadRequest::expires` 支持 expires 和 metadata |
| [abort-multipart-upload.yaml](abort-multipart-upload.yaml) | 取消分片上传 | `DELETE /{bucketname}/{objectname}`；`?uploadId` | implemented | `abort_multipart_upload` |

### 恢复与缓存操作

| 文件 | 功能 | 原始方法、路径与参数 | SDK 状态 | SDK 入口与差异 |
| --- | --- | --- | --- | --- |
| [restore-object.yaml](restore-object.yaml) | 恢复误删对象；保留期限以服务为准 | `PUT /{bucketname}/{objectname}`；`?restore` | implemented | `restore_object` 发 `?restore`，是误删恢复；与归档解冻独立 |
| [restore-archive-object.yaml](restore-archive-object.yaml) | 暂时还原归档 Object（原摘要有误，见核对说明） | `PUT /{bucketname}/{objectname}`；`?restoreArchive` | missing | 无 `?restoreArchive` |
| [prefetch-object.yaml](prefetch-object.yaml) | 预取Object | `PUT /{bucketname}/{objectname}`；`H:prefetch` | implemented | `prefetch_object`；官方请求记录确认 query |
| [refresh-object.yaml](refresh-object.yaml) | 刷新Object | `PUT /{bucketname}/{objectname}`；`H:refresh` | implemented | `refresh_object`；官方请求记录确认 query |

### 错误描述片段

| 文件 | 内容 | 类型 |
| --- | --- | --- |
| [ClientError.yaml](ClientError.yaml) | 400 客户端错误、429 请求被拒绝的描述 | 无 paths，不计入操作数 |
| [ServerError.yaml](ServerError.yaml) | 500 Internal Server Error，integer/int32 描述 | 无 paths，不计入操作数 |

## SDK 补充能力

以下能力没有独立 YAML，不代表 SDK 不支持；调用示例在 [SDK 手册](../../README.md)。

| 能力 | 公开方法 / 当前 wire |
| --- | --- |
| Lifecycle | `get_lifecycle_config` GET `?lifecycle=rule_id`；`set_lifecycle_config` PUT `?lifecycle`；`set_lifecycle_rule` PUT `?lifecycle=rule` |
| Migration | `migrate_bucket` PUT `?migrate&orgId&teamId` |
| Access log | `get_access_log` / `set_access_log` GET/PUT `?accessLog` |
| basicInfo | `get_object_basic_info` GET `?basicInfo` |
| Metadata GET/SET | `get_object_metadata` GET `?metadata`；`set_object_metadata` PUT `?setMetaData` |
| Metadata codec | `ObjectMetadata::serialize` 输出 rawMeta map；Deserialize 输入 rawMeta.entry 数组；`from_headers` 解析头 |
| Presign / URL | `presign_object`、`absolute_object_url`，HEAD 签名附 metadata，expires 毫秒，Signature 原样追加 |
| Prefix delete | `delete_objects_with_prefix` 列举后逐页批删 |
| Download manager | `Downloader` / `DownloadRequest`，默认 manager feature |
| CSLB | `cslb` feature 下 Client 使用内部 DNS resolver（`cslb_max_nodes`），不提供公开类型 |

## 快照校验

SHA-256 标识当前保存的源文件内容。更新来源时同步复核索引、协议差异和 SDK 行为，再更新摘要；不能仅修改摘要来绕过内容审阅。

在仓库根目录执行 `python3 scripts/check_coverage.py`，检查 35 份文件索引、33 项操作映射和全部摘要。

| 文件 | SHA-256 |
| --- | --- |
| `Bucket.yaml` | `cbce622b607cf51d7c4cef7c729d3f6445e4409da08d9a3ba5c23b596171295e` |
| `ClientError.yaml` | `17c6f1a969e43ca0b25bcfdda6b37cb25c398f25b746f062d15191bfe5edfb1e` |
| `OAuth.yaml` | `d4d89cb4851eab203b4ef3e15f938b6826132e74e7580081f41ad23e5dd58963` |
| `ServerError.yaml` | `4003113ec183d35e31ffeee3989f03f6593fe1fa4f3972eadb571e95978bf87f` |
| `abort-multipart-upload.yaml` | `6f6c13e4edc3f259de7b882a99b5cb65179c8fb35f813c05f82d635ef87fa17e` |
| `complete-multipart-upload.yaml` | `0929697019bf0605183a3add16f0776a0809fc0ac04ef88d0f5bcdf98410e203` |
| `copy-object.yaml` | `c881724903103b28f7f5ae1803d993342af0b5c0a4a45d044b1a323c0761c24e` |
| `delete-bucket.yaml` | `a662f326e1a4b628ece7379774a43e2d7d783413075091875bc5a30f8f3cfd67` |
| `delete-multiple-objects.yaml` | `479089b7b22b68d149365651b748276c79fc5ab0856f4756621ec267b6bf8afa` |
| `delete-object-acl.yaml` | `c2ac8cab85c89c9e253e2a68ad0bd4ca676f6b75f1b8d436d7680407825d7801` |
| `delete-object.yaml` | `e4c1d8aa09b3b8ee24ceca6e275d815d29c9a65547bd99db10760763679fed3c` |
| `delete-objects.yaml` | `479089b7b22b68d149365651b748276c79fc5ab0856f4756621ec267b6bf8afa` |
| `get-bucket-acl.yaml` | `ff2a74db484a25451337d43dba4b2067b2dd548b6d1469483b0f09bcb7536198` |
| `get-bucket-meta.yaml` | `c37092f4d1afd163b228c7e3bd347af4b38af95842c7f879e48a5990dba38113` |
| `get-endpoint.yaml` | `f4cfab49776481cbc4ef67c43b84513fd4886e51ecc71d219b7277082738c9a0` |
| `get-object-acl.yaml` | `c9827e6d8ddd0f33fc38613820053096fea8f0d1cdc97154b29f08e23703d06f` |
| `get-object-metadata.yaml` | `57b88564238a38138d4f692f134369732fa720f568b7f874385201d0028ff955` |
| `get-object.yaml` | `b861d116f139dc1a759cbcfe0b2c3d4c59b7a0593910b21af5e157ad361df0cc` |
| `head-bucket.yaml` | `3a935f9ba95cc6ea4e0cd958b1430f62f1c411297987d20ea3e66a7491b9330d` |
| `head-object.yaml` | `dc09fa29a090f3fb3b3bb691385f8c94ab3ce6b18e43a5d3dc4be5c4ec3c1a1f` |
| `init-multipart-upload.yaml` | `efe1daa6277929b530157190503b9cd2c4aeafd16cb222e5c30a3082ea64761d` |
| `list-bucket.yaml` | `c2bd95950c20ee7339303784e203733fb5c55b042b62a297baaa93bb04e67eb8` |
| `list-objects.yaml` | `8bdcb71e5849b418916c481bb9e0c99a63a84b943a61c1ed97ccff05b55b9116` |
| `post-object.yaml` | `02065338b3b58f19e89d4ca4d90bf85bd5883fd9e2d17412882fffd53de2d085` |
| `prefetch-object.yaml` | `4b84e8421724ce56aab94016ef7a9bc9fb5d4a9f5cc7b259fb985fccace2e4ef` |
| `put-bucket-acl.yaml` | `79bcb81f11366d32d886b280a264a5bb390c7d19bc1921e47eec088053920716` |
| `put-bucket.yaml` | `bdf97b7da3b030f19b4739a140a3a100d145238630a94b769e9445d2e9eb1c9e` |
| `put-object-acl.yaml` | `f8e4c15eaef746acd1590a937f0bdae30010cb90b564e6658fe597ff507a6062` |
| `put-object.yaml` | `98542a7b7b6df6d397240293da46175e0f397687284ff49988fe0bb789876ae3` |
| `refresh-object.yaml` | `9c8932306d69a358227efce2f2b57801d5360c22eebfbf57e89edcf9866927e0` |
| `rename-object.yaml` | `d99754a7fc9f05cf45693a71db59fe248cb54eac308c04e203f8c8a3a127d6eb` |
| `restore-archive-object.yaml` | `17eb36e59de374273e758f102a298872603423bb76438c01acc9dfb7b422b3e7` |
| `restore-object.yaml` | `f2297d045c1ddbba2007812c4e57fe3e79db4e27de7531882a9c4d2aae55c8ac` |
| `upload-part-copy.yaml` | `52e4beabc72b94b03b31abfd48a938cd936f19697f692edb366b7e010ac46c03` |
| `upload-part.yaml` | `2450192d595952e2e7e525cb5bfb96079c504a9936aa9cc0c64c0d74daf51899` |
