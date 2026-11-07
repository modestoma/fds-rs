# 运行语义与数据安全

本文集中说明调用后的状态、资源和失败边界。构造器、默认配置和示例统一见 [SDK 手册](../README.md)；协议依据见 [协议说明](protocol.md)，兼容性变化见 [迁移指南](migration.md)。

## 客户端与请求生命周期

Client 的 clone 共享内部 Arc、凭据、配置、连接池和可选 CSLB resolver。构建时不联网；配置和凭据不提供可变入口，重新配置后旧 clone 不更新。

endpoint 仅接受 HTTP(S) origin，允许端口、IPv6 与根路径 `/`；拒绝端口 0、userinfo、query、fragment、路径前缀、空白及会被解析器消除的 dot 路径。凭据非空且 access ID 必须能安全用于鉴权头。Duration 必须非零且能表示为当前平台期限；TCP keepalive 为 1–32767 整秒；CSLB 节点数必须大于零。

请求构造器本身不发网络请求，执行时由 Client 统一校验 bucket/key 等参数。文件 body 构造会打开文件，但还不发送。读取请求可借用，上传请求拥有且消耗 body。请求链之间没有事务或自动回滚。

连接池每主机最大空闲连接数默认为 100，可用 `pool_max_idle_per_host` 调整，不代表总连接或并发上限。每个请求带 `User-Agent: fds-rs/<版本>`，可用 `user_agent_suffix` 追加应用标识；User-Agent 由 HTTP 层在签名之后添加，不参与签名。CDN origin 只用于显式 CDN 预签名，未配置时报错；普通数据请求始终使用 FDS endpoint。不跟随同源或跨源重定向。

CSLB 在 DNS 解析时轮换首选 IP 并附加备选地址，按 TTL 刷新；连接复用不重新轮换。resolver 没有 reqwest 失败反馈回调，不能保证自动故障摘除，也不统计节点指标。`cslb` feature 只影响 Client 的 DNS 解析，不提供公开的负载均衡类型。

## 响应与错误

| 操作 | 接受状态 | 响应行为 |
| --- | --- | --- |
| JSON 查询、PUT 上传、分片 init/part/complete | 200 | 必须有可解码 JSON；空体、非法模型或读取失败报错 |
| 普通对象 GET | 200 | 返回流式 body，允许零字节 |
| 带 Range 的普通对象 GET | 200、206 | 200 可能是忽略 Range，调用者检查 Content-Range 和长度 |
| Bucket/Object 存在性 | 200、404 | true/false；其他状态和网络错误继续返回错误 |
| `head_object` | 200、404 | Some(metadata)/None；裸 HEAD，无 metadata query |
| `get_object_metadata` | 200 | GET `?metadata`，按响应头白名单解析 |
| 创建、删除、ACL 设置、CDN 等无返回值操作 | 200 | 忽略 body，不要求 body 必须为空 |
| 下载管理器数据请求 | 206 | 严格校验范围、长度和适用的 ETag |

未明确接受的 2xx、全部 3xx 和错误状态不会自动当成功。批删即使 HTTP 200 仍需解析逐项失败；归档 API 尚未提供，不能将任意 202/409 解释为成功。

所有操作只返回一种错误 `fds::Error`。被拒绝的状态按 `ErrorKind` 映射：404 NotFound、401 Unauthenticated、403 PermissionDenied、409 Conflict、412 PreconditionFailed、429 RateLimited、5xx ServerError，其余 4xx 及未列入成功的 2xx/3xx 为 Unexpected（保留 status）。空 JSON、非法 JSON 和超限响应是 ProtocolViolation，此时 HTTP status 可以是 200；读取响应体失败是 Timeout 或 Transport。收到响应后的错误 `dispatch()` 均为 `Dispatch::Sent`；构造、签名和连接阶段的失败为 `NotSent`；请求发送中途或等待响应时中断为 `Unknown`。non_exhaustive 类型的 match 需保留兜底分支。

| 诊断入口 | 约束 |
| --- | --- |
| `status()` | HTTP 状态（`http::StatusCode`），独立于服务 code |
| `service_code()` | 尽力解析 JSON code/errorCode 的字符串或数字，最多 256 字符 |
| `request_id()` | 优先 x-xiaomi-request-id/x-request-id，再尝试 JSON requestId，最多 256 字符；不是必返字段 |
| `diagnostic_body()` | 最多 8192 字节 UTF-8 文本；截断或读取不完整反映在 body_truncated |
| `source()` | 传输错误去掉 URL；JSON 解析源仅保留类别、行、列 |

错误状态只读取受限诊断前缀，正常 JSON 响应仍全量读取，没有通用大小硬上限。SDK 错误 Display/Debug 只打印 kind、SDK 消息、status 和形如标识符的 service code（Debug 另含 dispatch、上下文和按 Display 输出的 source 链），不打印 request ID 和响应体，tracing 仅记录 method，Authorization 标记 sensitive。显式诊断访问器仍返回可能含敏感内容的服务原文；调用者构造的错误、模型、raw response、其他依赖日志不受这些格式化规则保护。

OAuth 的服务错误丢弃 body、request ID 和 service code，仅保留状态/kind；raw response 的 URL query 被移除。显式读取 token、body、header 后仍须按应用规则使用。

## 流式上传与读取

| 数据形态 | 内存与所有权 |
| --- | --- |
| Vec/Bytes 上传 | Vec 移入 Bytes，不复制整个 buffer；Bytes 可共享克隆 |
| 文件上传 | 先打开普通文件并记录长度，64 KiB 分块读取；不推断 MIME、不计算整文件 MD5、不自动分片 |
| 异步流上传 | 必须声明精确长度，按 HTTP 背压消费；上游块大小和队列由调用者约束 |
| `GetObjectOutput::chunk()` | 消费下一块，不累计完整 body |
| `bytes()` | 全量收集剩余 body；先消费过的 chunk 不会重现 |
| `copy_to()` | 流式写入调用者的 writer；不 flush/shutdown，不回滚目标 |

流提前 EOF 或单块超过剩余长度时报错，达到声明长度后停止轮询，因此不验证尾随数据。未知长度没有隐式内存收集或临时落盘入口。Content-Length 由 body 产生，metadata 不能覆盖 Content-Length/Transfer-Encoding。

自动 MD5 开启时，内存 body 计算十六进制 Content-MD5；显式头优先。文件/流必须事先提供校验和，否则发送前报错。显式值不意味着 SDK 已验证实际流内容。

UploadBody::try_clone 仅为内存 Bytes 返回共享副本；文件/流不可重放，每次 HTTP body 都不可克隆。丢弃发送 future 释放 SDK body，但应用自己的生产任务或队列需另行取消。远端可能已收到或写入数据；普通 copy_to 失败或取消也可能留下部分输出。

ByteRange 支持单个闭区间、开放结束、尾部长度，拒绝多范围、非法值和溢出；普通 GET 不自行截取返回内容。需要失败保留旧文件与范围完整性时使用下载管理器。

## 元数据与请求选项

ObjectMetadata 的 get 忽略大小写，None 与 Some(空字符串) 不同；iter/as_map 借用数据。set 仅接受预定义 key 或 x-xiaomi-meta 前缀。entry-list 反序列化保留未知 key，大小写重复项按输入最后一个生效。

SET 序列化是 rawMeta map，basicInfo 接收是 rawMeta.entry 数组，HTTP 读取是响应头白名单，三者不对称。`get_content_length` 对应 x-xiaomi-meta-content-length，不是 HTTP Content-Length。GET 的完整 ETag 从 headers 读取；HEAD 返回的 ObjectMetadata 不保留 ETag。

ACL Add 省略 action，Delete 发送 action=delete，并携带要增删的 grant 集合；没有 replace。创建桶 canned ACL 多值以逗号连接，空列表不发送。对象 ACL 的 version_id 不代表其他数据接口支持版本寻址。

小写 expires 和预签名大写 Expires 都表示签名到期，不是对象生命周期；expires_in 在构造时计算一次时间，拒绝不足 1 ms 或溢出，绝对时间构造允许历史时间。元数据与专用 setter 不应重复设置同一个 HTTP 头。

## 分页与批量删除

`list_objects(..).into_pages()` / `into_objects()` 没有预取或后台任务，保留一个解码页和常数个 marker。bucket/prefix/delimiter/max_keys 始终来自调用者，不能被响应回显替换。空但 truncated 的页继续，非 truncated 结束；错误只输出一次后终止。objects 展开对象但忽略 common_prefixes，不递归进入目录分组。

marker 为不透明字符串。缺失和相邻重复立即拒绝，更长循环采用常数空间 Brent 检测，可能先输出部分重复页再报错；普通分页不是快照或去重保证。有副作用的消费应自行防重复。

`delete_objects(bucket).keys(..).send()` 返回 `fds::Result<DeleteObjectsOutput>`：服务给出合法失败数组时一律是 `Ok`，逐项失败放在 `failures` 里；批次未发出时返回原错误（`NotSent`，不附带结果）；可能已发出但结果无法确认时返回 `OutcomeUnknown`，`partial_delete_output()` 的 `unconfirmed` 列出整批名字。空批次不请求，即使 bucket 为空也返回空成功。非空批次先校验所有名字，一次请求一个 JSON 数组，不自动按服务限制切批。

| 字段 / 判定 | 含义 |
| --- | --- |
| `deleted` | 根据合法服务失败数组推导的确认成功项，不是逐个 HEAD 检查 |
| `failures` | 服务失败数组，保留对象名、错误码、描述、顺序与重复次数；单批删除时在 `Ok` 里 |
| `unconfirmed` | 已尝试传输但无法确定结果的批次（kind 为 OutcomeUnknown），不能认定未删除；只出现在错误附带的结果里 |
| `dispatch() == NotSent` | 失败批次未提交（本地校验或连接失败），不附带 partial output |
| `source()` | OutcomeUnknown 的底层错误 |

重复输入名字保留在线路和成功输出中；某名字失败则其全部出现均不计成功。响应必须为合法 JSON 失败数组，空 body/null/非法模型不视为全成功；出现未请求名字时保留失败数组并将整个批次标记未确认。

`delete_prefix(bucket, prefix)` 只按 bucket 和 prefix 选择对象（不带 delimiter，也不接受起始 marker），逐页执行，首个逐项失败（kind 为 PartialFailure）或请求错误即停，错误的 `partial_delete_output()` 保留先前结果。它使用记住本次全部 marker 的严格已见集合，在删除当前页前拒绝循环和前缀外 key；与普通分页的常数空间循环检测不同。后续未列举项不在结果中，并发新增不保证被清空。累计结果与 marker 集合随处理量增长，海量任务可由应用分批持久化。

## 重试与超时

默认关闭 SDK 重试。启用后，只有显式分类为读取且无 body/可克隆 Bytes body 的请求可重试。包括对象 GET/HEAD（含 `download_file` 的 HEAD）、列举、basicInfo、metadata/ACL 和 Bucket 信息/lifecycle/accessLog 读取；前缀删除仅其列举步骤适用。

连接、发送、超时错误和 429/502/503/504 可在成功响应交付前重试；body producer、成功后的读取/JSON 解码错误不重试。写入、复制、rename、删除、ACL 更新、分片全链路、CDN 和 OAuth exchange 不自动重发；reqwest 自带重试关闭。

max_retries 不含首次，范围 0–100；initial_delay 必须大于零且不大于 max_delay，max_delay 不超过 60 秒。等待采用指数退避和当前上限 50%–100% 的随机抖动，Retry-After 支持秒数和 HTTP 日期；作为等待下限，超过最大等待时放弃重试，过去日期按零，无效值忽略。

每次尝试重新准备头、Date 和签名。耗尽返回最后错误；TLS 配置错误也可能耗尽重试预算，不保证每个 connect 错误都是瞬时故障。request_timeout 约束单次请求及 body 消费，不覆盖所有重试的总时间；整体期限由调用者包裹 tokio::time::timeout。取消 future 停止后续尝试，不撤销远端写入。

## 分片会话与恢复

手动接口（`create_multipart_upload` 返回的 `MultipartUploadSession`，或直接接收 upload ID 的 `upload_part`/`complete_multipart_upload`/`abort_multipart_upload`）由调用者保存 upload ID、确认回执及目标身份，检查分片完整性、排序后 complete；SDK 不提供 list-parts 或可自动导入的恢复格式。会话绑定调用者传入的 bucket/key；响应缺少 upload ID 时为 OutcomeUnknown。初始化响应丢失可能留下没有本地 ID 的远端会话；缺少 part 回执不代表上传未发生。分片编号为 `NonZeroU16`，发送前检查不超过 10,000。

托管 MultipartUpload 属于核心 SDK，不依赖 manager，由 `client.upload_file(bucket, key, path)` 配置。`start()` 先校验名字和选项（失败为 InvalidInput，不打开文件、不发请求），再打开非空普通文件并初始化一次；后续请求绑定调用者输入 bucket/key。upload 只允许运行一次，失败或丢弃 future 后不能通过再次调用隐式恢复。

默认 8 MiB × 4 并发、Preserve；本地允许每片 1 字节至 64 MiB、并发 1–64，乘积不超过 256 MiB，总片数不超过 10,000。服务尺寸限制仍需满足。内存 body 上界为 part_size × concurrency，另有文件、网络与响应解析开销；每份回执 ETag 最多 1024 字节，整体 JSON 无通用上限。

回执编号、大小必须匹配请求，ETag 非空。全部成功后按编号 complete，返回 objectName 必须匹配，非空 bucketName 也必须匹配。源文件必须保持不变；长度/修改时间检查不能检测保持时间戳的原地改写。

| 状态 | 恢复含义 |
| --- | --- |
| Ready | 初始化成功，尚未开始 |
| Uploading | 上传中；future 丢弃后可能留在此状态 |
| Failed | 分片、文件或完成前取消失败；根据策略保留/清理 |
| Aborted | abort 得到成功响应，不代表服务端已物理回收 |
| CompletionUnknown | complete 可能已发送；错误、取消或无效响应均需核对远端 |
| Completed | complete 有效且对象身份匹配 |

Preserve 默认不清理。AbortOnFailure 仅在完成前的错误/合作式取消尝试一次 abort；先丢弃分片 future，返回原始错误；abort 也失败时其文本附加在错误上下文 `cleanup_error` 中。complete 可能已发出后的失败统一为 OutcomeUnknown。调用 token.cancel 后继续 await 才能获得清理结果；timeout/drop future 或整个 session Drop 不执行异步清理。

进度包含会话、确认回执、文件大小、总片数和状态，应用应按业务需求持久保存。进度中的 `session` 即 `MultipartUploadSession`：确认放弃后可显式 `abort()`；确认需要补片时用 `upload_part(n, body)`（第 n 片对应文件偏移 `(n - 1) × part_size`）；确认全部回执后可手动 `complete(parts)`。CompletionUnknown 不能自动 abort/retry，也不能仅用 HEAD 存在证明本次完成，同名对象可能来自旧版本或并发写入。

## 下载文件与断点

`client.download_file(bucket, key, path)` 返回 builder，`send()` 执行一次下载。路径使用 PathBuf，父目录须存在；路径层不要求 UTF-8，实际支持由文件系统决定。每片字节数与并发须大于零，分片大小不超过 i64::MAX，这些在 `send()` 中、加锁和请求前校验（InvalidInput）；运行时再校验范围、文件资源和对象。

- 使用真实 HEAD Content-Length。HEAD 与 `head_object` 是同一个只读请求，配置重试后按策略重试；对象不存在为 NotFound（`is_not_found()`）。数据响应必须是 206、精确 Content-Range 及精确长度；200 忽略 Range 会失败。
- 有强 ETag 时每片需同一 ETag；无强 ETag（含弱值）时改为单个流式 Range 请求，不复用断点，不以大小/Last-Modified 推断跨请求一致性。
- 范围结束超过对象时截断；不可满足范围报错；零字节完整对象生成空文件。
- 分片在同目录暂存，全部成功并同步后由主 future rename 替换目标；没有后台 rename。失败保留旧目标。
- lock 使用规范化父目录和文件名；同目标并发报 IO AlreadyExists。worker 退出前持有锁，取消后立即重试可能仍遇锁。进程被杀死留锁时，确认无运行下载后才移除。
- `.tmp`、`.download.bp`、`.download.bp.new`、`.download.lock` 都由下载器保留。Unix 拒绝暂存符号链接和多硬链接；合作锁不防其他程序修改目录。

检查点保存版本、endpoint、bucket/key、强 ETag、对象大小、范围、片大小和已完成分片 MD5。恢复先校验本地数据，缺失、长度或摘要错误及对象身份变化会触发重新下载；MD5 用于意外损坏检测，不是抗恶意篡改认证。旧格式不可复用。

暂存文件先设精确长度，各分片通过同一个已校验的文件句柄按偏移写入；整个下载只在发布前做一次 sync_all。开启 resume 时，检查点在第一个分片完成时写入，之后每完成 64 片或距上次写入满 2 秒再写一次，下载失败时再写最后一次；检查点经同目录新文件、sync_all、rename 更新，写入失败终止下载。检查点不保证对应分片已落盘，恢复时会重新校验本地分片的 MD5，不一致的分片重新下载。成功后检查点清除为 best effort，不承诺目录 fsync 级断电持久性。

最多 concurrency 个 worker，无全量任务队列；每个 worker 把网络数据攒成最多 256 KiB 一批交给阻塞线程写入，不累计整个分片。最多 1,000,000 片，检查点读取上限 64 MiB。文件 IO、fsync、恢复摘要、加锁与发布都在 `spawn_blocking` 中执行，不占用 async 运行时线程，current_thread 运行时也可以使用；取消后，已提交的阻塞写入会先执行完，锁才会释放。

`request_limiter(RequestLimiter)` 每个分片请求启动消耗一个 permit，不是字节带宽限制；`RequestLimiter::per_second(n)` 的突发上限为 n，克隆共享预算。性能测量方法统一见 [验证指南](validation.md#本地下载基准)。
