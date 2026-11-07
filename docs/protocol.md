# 协议说明与能力限制

本文集中说明原始 API 资料与当前 SDK 的差异、采用依据和未提供能力。文件及 SDK 映射统一见 [原始 API 索引](open_api/README.md)，调用代码见 [SDK 手册](../README.md)。这里保留既有资料中的来源链接，不把文档重组视为对线上服务的重新验证。

## 原始资料的冲突

| 项目 | 核对结论 |
| --- | --- |
| 批量删除 | `delete-objects.yaml` 与 `delete-multiple-objects.yaml` 解析后的完整内容相同；保留两份来源，业务能力只计一次。HTTP 200 仍可能包含逐对象删除失败，必须检查返回数组。 |
| 对象 HEAD | `head-object` 与 `get-object-metadata` 都是 `HEAD /{bucketname}/{objectname}`，都返回元数据；前者权限说明允许 Bucket 或 Object READ，并列出 404，后者只写 Object READ。属于同一请求的不同描述，不擅自抹平权限差异。 |
| Bucket 列举 | `Bucket.yaml` 不是公共模型文件，它定义 `GET /`，要求 `appId`；`list-bucket.yaml` 同样是 `GET /`，要求认证头和 Date，并支持 `authorizedBuckets`。两者模型字段也不同，属于重叠/变体定义，不能直接认定等价。 |
| Object ACL 删除 | `put-object-acl` 已允许 `action=delete`；`delete-object-acl` 单独展开同一操作，并补充不能删除自身 ACL 的约束。新增与删除能力均保留。 |
| Bucket 元数据与列举对象 | 都定义 `GET /{bucketname}`；元数据文件没有列出区分 query，列举对象要求 `prefix`。SDK 分别发送裸 GET 与显式 prefix/delimiter/maxKeys，不自行补写 `meta`；调整分派前需真实路由证据。 |
| Endpoint 必填标记 | `get-endpoint` 的 `service.required` 为字符串 `query`，不是布尔值；该标记本身不能判定必填性；当前 SDK 按 REST 说明要求 region/service 非空，返回 JSON 主机名。400 的 schema 为空。 |
| 预取与刷新 | `prefetch`、`refresh` 在 YAML 中被定义为 header，原件保留；SDK 按下文 REST 请求记录与 Java SDK 交叉证据使用 query。 |
| 归档恢复 | `restore-archive-object` 的摘要误写成“恢复被删除文件、保留7天”，与 `restoreArchive` 参数及 202/409 解冻响应冲突；能力名称根据参数和响应归类为归档还原，误删保留期不套用到归档恢复。 |
| 复制对象响应 | `copy-object` 的 409 描述是“已经存在同名的Bucket”，疑似复制粘贴错误；不能据此断言 Copy Object 支持条件创建或禁止覆盖。 |
| POST 上传 | 摘要声称服务端生成唯一名字、不覆盖，但参数又允许 `key` 指定名字。指定 key 时的冲突/覆盖语义未明确，不能沿用无 key 情形的保证。`form-data`、`form` 也不是标准 Swagger 2.0 的 `formData`。 |
| Range 文案 | `get-object` 将 `bytes=0-` 解释成跳过第一个字节，描述有误；它实际从偏移 0 开始，跳过第一个字节应为 `bytes=1-`。 |
| 分片复制模式 | `upload-part-copy` 要求以 `MULTI_BLOB` 初始化，但 `init-multipart-upload` 文件没有给出选择该模式的参数，不能仅依此目录完成该模式的调用契约。归档源需先解冻，源对象名需 URL 编码。 |
| 分页和时间 | `list-objects` 需按 `truncated`/`nextMarker` 翻页，单次最多 min(maxKeys, 1000)；上传响应签名的 expires 是毫秒时间戳，文档默认30天，不是对象 TTL。 |
| Schema 质量 | 源模型存在 `strin`、`long`、`string (int32)` 等非标准或不匹配类型，以及空 produces/schema；原始文件保留来源，YAML 能解析不代表 Swagger 校验通过，也不宜直接作为代码生成输入。 |

## 服务发现与 OAuth

| 证据 | 已确认内容与决策 |
| --- | --- |
| [官方 get-endpoint REST 1.0.0](https://docs.api.xiaomi.com/en/fds/api-specification/get-endpoint.html) | 示例是未签名 GET，region=sg、service=aws；200 响应为 JSON 主机名字符串，不是 EndpointResult 对象。region/service 必填。显式提供发现入口；不内置过时地区枚举，不自动切换已携带凭据的客户端。 |
| [官方 OAuth REST 1.0.0](https://docs.api.xiaomi.com/en/fds/api-specification/OAuth.html) | 支持 XiaoMi/QQ/Sina/RenRen；XiaoMi 字段说明要求 openId/macKey/macAlgorithm，旧示例省略 openId，本实现按较严格的文字契约要求三者。Weixin 的交换参数未确认，不提供枚举值。 |
| [官方 Android OAuthCredential](https://github.com/XiaoMi/galaxy-fds-sdk-android/blob/c38a867971786ce3c5807b4c1db218b7387e1064/src/main/java/com/xiaomi/infra/galaxy/fds/android/auth/OAuthCredential.java) | 交换与后续请求使用 Authorization: OAuth；后续查询携带 appId 和 storageAccessToken。不是 Galaxy-V2 secret。SDK 统一编码参数。 |
| [官方 StorageAccessToken 模型](https://github.com/XiaoMi/galaxy-fds-sdk-android/blob/c38a867971786ce3c5807b4c1db218b7387e1064/src/main/java/com/xiaomi/infra/galaxy/fds/android/model/StorageAccessToken.java) | 模型标注毫秒；结合 REST 示例 Date 与 expireTime=1486806381536 相隔约一周，推断为 Unix epoch 毫秒，使用 DateTime<Utc>，不作为相对 TTL。 |
| [官方 OAuth 认证说明](https://docs.api.xiaomi.com/en/fds/request-authentication/0Auth.html) 与 [Bucket 定义](open_api/Bucket.yaml) | appId 与 token 共同认证。list_app_buckets 自动采用交换绑定的 appId，不提供任意 appId 覆盖；保留 orgId 等六个 Bucket 字段。服务端仍决定当前用户和应用可见范围。 |

发现主机只做格式校验，不代表其可信或可达；调用方确认后才能绑定凭据。交换与使用 token 必须采用 HTTPS（本地 loopback IP 测试除外）。appId 固定为交换输入，过期后显式重新交换并构建新 Client，既有 clone 不更新；不提供 Weixin 参数或自动切换入口。实际 appId、region、provider 和 Bucket 权限需由服务方开通。

## CDN 预取与刷新

[Prefetch REST](https://docs.api.xiaomi.net/fds/api-specification/prefetch-object.html) 与 [Refresh REST](https://docs.api.xiaomi.net/fds/api-specification/refresh-object.html) 的 Version 1.0.0 参数表列为 header，但同页 2017-03-06 请求记录实际使用 `?prefetch=` / `?refresh=`，均返回 200、Content-Length: 0。以具体请求记录为实现依据，保留仓库原 YAML，不把错误参数表改造成既成事实。

官网链接的 [Java SDK 固定版本](https://github.com/XiaoMi/galaxy-fds-sdk-java/blob/4d7bfe5fc26c4e478e9016a139beaedca7f17089/src/main/java/com/xiaomi/infra/galaxy/fds/client/GalaxyFDSClient.java#L1346) 的 `prefetchUri` / `refreshUri` 同样发送 query，不过解析 Long 响应。SDK 保留 HTTP 200 的确认语义、忽略响应体，兼容官方空体记录与数字响应；不声称数字含义，也不返回未经确认的任务 ID。

[预取说明](https://docs.api.xiaomi.net/fds/object-api/prefetch-object.html) 和[刷新说明](https://docs.api.xiaomi.net/fds/object-api/refresh-object.html) 明确异步及 region/CDN 限制，操作需对象 owner；网页列出的时间只是估计，部分海外服务可能不执行操作。客户端不内置地区白名单或完成时间保证。

## 归档恢复

[官方 REST](https://docs.api.xiaomi.net/fds/api-specification/restore-archive-object.html) 的 200/202/409 描述与本地 YAML 一致，新增示例证明 200 可为空体。官方 Java SDK 使用 `restoreArchive=true`，未提供独立的 409 成功识别逻辑。仍缺 202 响应形态和能区分其他冲突的 409 专用错误码/响应形态，不能把任意 409 解释为解冻中。因此不提供归档恢复 API。

现有 `Client::restore_object` 使用 PUT `?restore`，对应[误删恢复](open_api/restore-object.yaml)。与 `?restoreArchive` 分离。

## 分片复制

[官方复制分片 REST](https://docs.api.xiaomi.net/fds/api-specification/upload-part-copy.html) 的 2021-10-21 请求/响应补充了 source/range headers 和 UploadPartResult JSON 证据。源 key 需要 URL 编码；range 为可选闭区间，省略则复制整个对象；归档源需先解冻。

[官方分片说明](https://docs.api.xiaomi.net/fds/multipart-upload.html) 确认复制需要 MULTI_BLOB，要求 SDK 的 `initMultiPartUploadCopy` 初始化，并列出因地区而异的 part 数量和大小限制。但[初始化 REST](https://docs.api.xiaomi.net/fds/api-specification/init-multipart-upload.html) 仍没有模式头；已查官网链接的公开 Java、Go、C++ SDK 未找到该初始化方法实现。Go 常量、现有 Rust metadata 和签名 fixture 只能证明客户端能发送 `x-xiaomi-multipart-upload-mode: MULTI_BLOB`，不能证明复制初始化的服务契约。

尚缺指定模式的初始化请求证据，以及适用版本 init→copy→complete 链路。保持 typed MultiBlob 和复制入口未启用。普通 multipart、metadata 传头能力保留，不把某个地区限制硬编码成全局上限。

## POST 创建对象

[post-object.yaml](open_api/post-object.yaml) 同时描述自动唯一名字和指定 key，且混用 body、form、form-data，无法确定指定 key 的覆盖/冲突行为及 body 编码。SDK 不提供 POST 创建入口；需取得带/不带 key 的 Content-Type、body、成功与冲突响应契约后才能实现。普通 PUT 支持同名覆盖，不能当作该能力的替代保证。

## 签名、编码与时间

Galaxy-V2 使用 HMAC-SHA1/Base64。签名包含 Date/Expires 行、排序的 x-xiaomi headers 和解码后的签名资源；签名子资源为 `acl/quota/uploads/partNumber/uploadId/storageAccessToken/metadata`。不能因为新增 query 就自动将其加入签名集合。

对象名斜杠编码为 `%2F`、空格为 `%20`、字面加号为 `%2B`。Content-MD5 沿用十六进制；预签名结果的 Signature 后缀原样追加，调用方不要重复编码。小写 `expires` 控制上传响应签名的到期，大写 `Expires` 用于预签名认证；均以毫秒表达，不是对象 TTL。OAuth expireTime 按上述模型与响应时间证据解释为 UTC epoch 毫秒。

[签名实现](../crates/fds/src/auth.rs)、[查询编码](../crates/fds/src/encoding/query.rs)和[保存的签名向量](../crates/fds/tests/fixtures/golden.json)可用于回归，但不构成对未覆盖服务行为的证明。`encoding::parse_range` 保留旧解析语义，不支持开放结束且不按标准后缀范围解释空起点；下载请求应使用 `ByteRange`。

## 补充服务证据

协议资料有缺口时，记录适用 endpoint/服务版本、日期、官方出处或维护者确认，以及脱敏 method/path/query、相关 headers、status/body。归档需区分首次接受、解冻中、已解冻延长和其他 409；分片复制需覆盖模式初始化、copy、complete、源 key 编码和范围边界。不得记录 Authorization、密钥、token 或完整签名 URL。

本地 mock 不能解除真实协议疑点，验证操作见 [验证指南](validation.md)。

## Copy 回执

官方 [galaxy-fds-core 3.0.44 源码包](https://repo.maven.apache.org/maven2/com/xiaomi/infra/galaxy/galaxy-fds-core/3.0.44/galaxy-fds-core-3.0.44-sources.jar) 的 `CopyObjectResult` 继承 `PutObjectResult`，目标字段为 `bucketName` / `objectName`。SDK 要求这两个字段为字符串且匹配请求，允许其他字段以兼容服务演进，但不返回其中的签名或凭据字段。这是源码契约依据，不代表已完成目标环境的真实 Copy 验证。
