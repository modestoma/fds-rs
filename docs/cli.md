# fds_cli 命令手册

按功能查找命令，每行可单独执行。`demo-bucket`、`target-bucket`、`demo/hello.txt`、ID 和本地文件路径均按实际环境替换；写入、删除和权限修改请使用测试资源。

## 构建与配置

安装：在仓库根目录执行 `cargo install --path crates/fds_cli --locked`。

普通命令需要配置环境变量 `FDS_ENDPOINT`、`FDS_ACCESS_KEY_ID`、`FDS_ACCESS_KEY_SECRET`；endpoint 使用完整 origin，例如 `https://cnbj1.fds.api.xiaomi.com`。也可通过参数指定：

```bash
fds_cli --version # 查看版本
fds_cli --help # 查看全部命令
fds_cli bucket --help # 查看 Bucket 命令
fds_cli object --help # 查看 Object 命令
fds_cli multipart --help # 查看分片命令
fds_cli download --help # 查看下载参数
fds_cli service --help # 查看服务发现与 OAuth 命令
fds_cli object put --help # 查看某个具体命令的参数
fds_cli --endpoint https://cnbj1.fds.api.xiaomi.com --access-key-id YOUR_ACCESS_KEY_ID --access-key-secret YOUR_ACCESS_KEY_SECRET bucket list # 指定连接和凭据；真实 secret 建议通过环境变量提供
```

成功退出码为 `0`，运行失败为 `1`，命令参数错误为 `2`。普通结果为 JSON，GET 正文和 URL 直接输出；错误写入 stderr。

## Bucket 基础操作

```bash
fds_cli bucket list # 列出自己的 Bucket
fds_cli bucket list-authorized # 列出有权访问的 Bucket
fds_cli bucket exists demo-bucket # 检查是否存在，返回 true 或 false
fds_cli bucket info demo-bucket # 查看名称、对象数、用量、存储类型等信息
fds_cli bucket create demo-bucket # 创建 Bucket
fds_cli bucket create demo-bucket --storage-class STANDARD # 指定标准存储
fds_cli bucket create demo-bucket --storage-class STANDARD_IA # 指定低频存储
fds_cli bucket create demo-bucket --storage-class Archive # 指定归档存储
fds_cli bucket create demo-bucket --org-id YOUR_ORG_ID # 指定组织
fds_cli bucket create demo-bucket --acl public-read # 创建时授予公开读取权限
fds_cli bucket create demo-bucket --acl public-read --acl authenticated-read # 创建时组合多个 ACL
fds_cli bucket get-acl demo-bucket # 回读创建时的 ACL
fds_cli bucket delete demo-bucket # 删除测试 Bucket，先清空对象
fds_cli bucket exists demo-bucket # 删除后应返回 false
```

上面的 create 是不同用法，分别选择新桶名执行。存储类型由服务决定；`--acl` 支持 `public-read`、`public-write`、`public-read-objects`、`public-sso-write`、`authenticated-read`、`authenticated-write`、`authenticated-sso-write`。

## Bucket ACL

```bash
fds_cli bucket get-acl demo-bucket # 查看当前授权
fds_cli bucket set-acl demo-bucket --file ./acl.json --action add # 添加文件中的授权
fds_cli bucket get-acl demo-bucket # 验证授权已出现
fds_cli bucket set-acl demo-bucket --file ./acl.json --action delete # 删除文件中的授权
fds_cli bucket get-acl demo-bucket # 验证对应授权已移除
```

`add/delete` 操作指定 grant，不是整体覆盖 ACL；仅删除本次验证添加的授权。`acl.json` 格式见文末。

## 生命周期

```bash
fds_cli bucket get-lifecycle demo-bucket # 查看全部生命周期规则
fds_cli bucket get-lifecycle demo-bucket > ./lifecycle-before.json # 保存原配置
fds_cli bucket set-lifecycle demo-bucket --file ./lifecycle.json # 设置完整生命周期配置
fds_cli bucket get-lifecycle demo-bucket # 回读并核对规则
fds_cli bucket set-lifecycle-rule demo-bucket --file ./lifecycle-rule.json # 设置单条规则
fds_cli bucket get-lifecycle demo-bucket --rule-id expire-demo # 查询指定规则
fds_cli bucket set-lifecycle demo-bucket --file ./lifecycle-before.json # 恢复原配置
fds_cli bucket get-lifecycle demo-bucket # 验证恢复结果
```

先确认原配置读取成功再修改；配置回读通过不代表过期删除已经执行。

## 访问日志

```bash
fds_cli bucket get-access-log demo-bucket # 查看日志配置
fds_cli bucket get-access-log demo-bucket > ./accesslog-before.json # 保存原配置
fds_cli bucket set-access-log demo-bucket --file ./accesslog-enable.json # 开启访问日志
fds_cli bucket get-access-log demo-bucket # 回读 enabled、logBucketName、logPrefix
fds_cli object put demo-bucket demo/log-probe.txt --text 'log probe' # 创建测试对象
fds_cli object get demo-bucket demo/log-probe.txt # 产生对象访问
fds_cli object list log-bucket --prefix access/ --all # 等待服务投递后检查日志对象
fds_cli bucket set-access-log demo-bucket --file ./accesslog-disable.json # 关闭日志
fds_cli bucket get-access-log demo-bucket # 验证 enabled=false
fds_cli bucket set-access-log demo-bucket --file ./accesslog-before.json # 恢复原配置
fds_cli object delete demo-bucket demo/log-probe.txt # 清理测试对象
```

日志目标桶需要允许投递；生成日志是异步行为，实际日志中出现本轮访问才算投递验证通过。

## Bucket 迁移

```bash
fds_cli bucket info demo-bucket # 查看迁移前的 Bucket
fds_cli bucket migrate demo-bucket --org-id TARGET_ORG_ID --team-id TARGET_TEAM_ID # 迁移到目标组织和团队
fds_cli bucket info demo-bucket # 检查迁移后的可访问性
fds_cli bucket list-authorized # 查看当前身份的可访问列表
fds_cli bucket migrate demo-bucket --org-id ORIGINAL_ORG_ID --team-id ORIGINAL_TEAM_ID # 有权限时迁回原组织和团队
```

迁移使用独立测试桶。`bucket info` 不返回 org/team，归属需要在管理平台核对；迁移后可能需要切换认证身份。

## 对象上传

```bash
fds_cli object put demo-bucket demo/hello.txt --text 'hello fds' # 上传文本，共 9 字节
fds_cli object put demo-bucket demo/empty.txt --text '' # 上传空对象
fds_cli object put demo-bucket demo/file.bin --file ./file.bin # 流式上传本地文件
fds_cli object put demo-bucket demo/stdin.txt --stdin --length 9 < ./hello.txt # 从 stdin 上传，hello.txt 应为 9 字节
fds_cli object put demo-bucket demo/hello.txt --text 'hello fds' --content-type text/plain # 设置内容类型
fds_cli object put demo-bucket demo/hello.txt --text 'hello fds' --cache-control no-cache # 设置缓存策略
fds_cli object put demo-bucket demo/file.txt.gz --file ./file.txt.gz --content-encoding gzip # 上传实际 gzip 文件
fds_cli object put demo-bucket demo/file.bin --file ./file.bin --content-disposition 'attachment; filename="file.bin"' # 设置下载文件名
fds_cli object put demo-bucket demo/file.bin --file ./file.bin --expect 100-continue # 设置 Expect 请求头
fds_cli object put demo-bucket demo/hello.txt --text 'hello fds' --meta x-xiaomi-meta-author=alice --meta x-xiaomi-meta-purpose=verify # 设置自定义元数据
fds_cli object put demo-bucket demo/hello.txt --text 'hello fds' --expires 2030-01-01T00:00:00Z # 指定上传响应签名到期时间，不是对象 TTL
fds_cli --md5 object put demo-bucket demo/md5.txt --text 'hello fds' # SDK 自动计算内存 body 的 MD5
fds_cli --md5 object put demo-bucket demo/file.bin --file ./file.bin --content-md5 YOUR_FILE_MD5_HEX # 流式文件使用预计算的十六进制 MD5
fds_cli --md5 object put demo-bucket demo/stdin.txt --stdin --length 9 --content-md5 YOUR_FILE_MD5_HEX < ./hello.txt # stdin 使用预计算 MD5
fds_cli object exists demo-bucket demo/hello.txt # 验证对象存在
fds_cli object basic-info demo-bucket demo/hello.txt # 验证 size=9
fds_cli object get demo-bucket demo/hello.txt # 回读应为 hello fds
fds_cli object get demo-bucket demo/file.bin --out ./downloaded.bin # 下载后与源文件逐字节比较
```

`--text`、`--file`、`--stdin` 三选一；stdin 必须指定准确长度。`--md5` 对文件/stdin 不会自动计算校验和。各上传行是独立参数示例，重复使用同一 key 会覆盖对象。

## 对象读取

```bash
fds_cli object exists demo-bucket demo/hello.txt # 查看对象是否存在
fds_cli object head demo-bucket demo/hello.txt # 查看 HEAD 元数据，不存在时输出 null
fds_cli object basic-info demo-bucket demo/hello.txt # 查看大小、存储类型和基本信息
fds_cli object get demo-bucket demo/hello.txt # 将内容输出到终端
fds_cli object get demo-bucket demo/hello.txt --out ./hello-downloaded.txt # 下载到文件
fds_cli object get demo-bucket demo/hello.txt --show-headers # 同时向 stderr 输出 HTTP 状态和响应头
fds_cli object get demo-bucket demo/hello.txt --range bytes=0-4 # 读取前 5 字节，应为 hello
fds_cli object get demo-bucket demo/hello.txt --range bytes=6- # 从第 7 字节读到结尾，应为 fds
fds_cli object get demo-bucket demo/hello.txt --range bytes=-3 # 读取末尾 3 字节，应为 fds
```

`get --out` 读取成功后才替换目标文件；正文写向 stdout 时，失败前已经输出的字节无法撤回。

## 对象元数据

```bash
fds_cli object get-metadata demo-bucket demo/hello.txt # 查看当前元数据
fds_cli object set-metadata demo-bucket demo/hello.txt --content-type text/plain --cache-control no-cache --meta x-xiaomi-meta-reviewed=yes # 设置元数据
fds_cli object get-metadata demo-bucket demo/hello.txt # 回读并核对类型、缓存策略及 reviewed 字段
fds_cli object get demo-bucket demo/hello.txt # 验证正文仍为 hello fds
```

设置时提供需要保留的可写元数据，不假设服务执行 merge。共享参数包括 `--meta`、`--content-type`、`--cache-control`、`--content-encoding`、`--content-md5`；同一键不能同时通过专用参数和 `--meta` 提供。`--meta` 的 key 忽略大小写，只接受这四个系统头和 `x-xiaomi-meta-*`，其他 key 报 `Invalid metadata`；去掉前缀后为空（`x-xiaomi-meta-=v`）或仍带前缀（`x-xiaomi-meta-x-xiaomi-meta-a=v`）的 key 由 SDK 报错，请求不发出。`object head`、`object get-metadata` 输出响应头中的协议元数据头与全部 `x-xiaomi-meta-*`，`object basic-info` 的 `metadata` 输出全部原始条目，均按 key 排序。

## 对象列表与分页

```bash
fds_cli object list demo-bucket # 列出第一页对象
fds_cli object list demo-bucket --prefix demo/ # 按前缀过滤
fds_cli object list demo-bucket --prefix demo/ --delimiter / # 按目录层级列出，查看 commonPrefixes
fds_cli object list demo-bucket --prefix demo/ --max-keys 2 # 每页最多 2 项，查看 truncated 和 nextMarker
fds_cli object list demo-bucket --prefix demo/ --max-keys 2 --marker YOUR_NEXT_MARKER # 使用上一页的 nextMarker 继续读取
fds_cli object list demo-bucket --prefix demo/ --max-keys 2 --all # 自动读取全部分页
```

分页验证至少准备 3 个对象。`--all` 输出连续的分页 JSON；手动翻页使用返回的原始 `nextMarker`。

## 对象复制与重命名

```bash
fds_cli object copy demo-bucket demo/hello.txt demo-bucket demo/copy.txt # 同桶复制
fds_cli object get demo-bucket demo/copy.txt # 验证复制后的内容
fds_cli object exists demo-bucket demo/hello.txt # 原对象应仍存在
fds_cli object copy demo-bucket demo/hello.txt target-bucket demo/copy.txt # 跨桶复制
fds_cli object get target-bucket demo/copy.txt # 验证目标桶内容
fds_cli object rename demo-bucket demo/copy.txt demo/renamed.txt # 重命名
fds_cli object exists demo-bucket demo/copy.txt # 旧 key 应不存在
fds_cli object get demo-bucket demo/renamed.txt # 新 key 内容应一致
```

## 对象 ACL

```bash
fds_cli object get-acl demo-bucket demo/hello.txt # 查看对象权限
fds_cli object set-acl demo-bucket demo/hello.txt --file ./acl.json --action add # 增加指定授权
fds_cli object get-acl demo-bucket demo/hello.txt # 验证 grant 已出现
fds_cli object set-acl demo-bucket demo/hello.txt --file ./acl.json --action delete # 移除本轮新增授权
fds_cli object get-acl demo-bucket demo/hello.txt # 验证 grant 已移除
fds_cli object set-public demo-bucket demo/hello.txt # 设为公开可读
fds_cli object get-acl demo-bucket demo/hello.txt # 应出现 ALL_USERS 的 GROUP/READ 授权
fds_cli object get-acl demo-bucket demo/versioned.txt --version-id YOUR_VERSION_ID # 查看指定版本 ACL
fds_cli object set-acl demo-bucket demo/versioned.txt --version-id YOUR_VERSION_ID --file ./acl.json --action add # 增加指定版本授权
fds_cli object get-acl demo-bucket demo/versioned.txt --version-id YOUR_VERSION_ID # 回读指定版本授权
fds_cli object set-acl demo-bucket demo/versioned.txt --version-id YOUR_VERSION_ID --file ./acl.json --action delete # 移除本轮新增的版本授权
```

版本操作需要服务支持及真实 version ID。grant 回读后，还需用被授权身份访问，才能验证实际权限；公开操作仅用于可公开的测试内容。

## URL 与预签名

```bash
fds_cli object url demo-bucket demo/hello.txt # 生成不带签名的对象 URL
fds_cli object presign demo-bucket demo/hello.txt # 生成默认有效期 3600 秒的 GET URL
fds_cli object presign demo-bucket demo/hello.txt --method GET --expires 600 # 生成有效期 600 秒的下载 URL
fds_cli object presign demo-bucket demo/hello.txt --method HEAD --expires 600 # 生成 HEAD URL
fds_cli object presign demo-bucket demo/upload.txt --method PUT --expires 600 # 生成上传 URL
fds_cli object presign demo-bucket demo/upload.txt --method PUT --expires 600 --content-type text/plain --meta x-xiaomi-meta-author=alice # 将指定请求头纳入签名
fds_cli object presign demo-bucket demo/hello.txt --method DELETE --expires 600 # 生成删除 URL
fds_cli object presign demo-bucket demo/post.txt --method POST --expires 600 # 仅验证 POST 方法的签名生成
fds_cli --cdn-endpoint https://your-cdn.example.com object presign demo-bucket demo/hello.txt --cdn --expires 600 # 使用 CDN origin 生成 URL
```

用生成的 URL 发起对应 HTTP 请求并核对结果，才算远端验证通过；签入 metadata 的请求必须携带相同请求头。URL 生成需要 AK/SK Client，不能使用 OAuth 预签名。POST 签名不代表 SDK 已实现 POST 上传。

## CDN 预取与刷新

```bash
fds_cli object put demo-bucket demo/cdn.txt --text 'cdn v1' # 上传初始内容
fds_cli object prefetch demo-bucket demo/cdn.txt # 提交 CDN 预取请求
fds_cli --cdn-endpoint https://your-cdn.example.com object presign demo-bucket demo/cdn.txt --cdn --expires 600 # 生成 CDN 读取 URL
fds_cli object put demo-bucket demo/cdn.txt --text 'cdn v2' # 更新源内容
fds_cli object refresh demo-bucket demo/cdn.txt # 提交 CDN 刷新请求
fds_cli object get demo-bucket demo/cdn.txt # 验证源站为 cdn v2
```

预取/刷新成功只代表请求受理。等待目标 CDN 的传播时间后，访问 CDN URL 核对内容和缓存指标；源站回读不能代替 CDN 验证。

## 手工分片上传

```bash
fds_cli multipart init demo-bucket demo/multipart.bin --content-type application/octet-stream --meta x-xiaomi-meta-purpose=verify # 初始化，记录返回的 uploadId
fds_cli multipart part demo-bucket demo/multipart.bin --upload-id YOUR_UPLOAD_ID --number 1 --file ./part1.bin > ./part1.json # 上传第一片并保存回执
fds_cli multipart part demo-bucket demo/multipart.bin --upload-id YOUR_UPLOAD_ID --number 2 --file ./part2.bin > ./part2.json # 上传第二片并保存回执
fds_cli --md5 multipart part demo-bucket demo/multipart.bin --upload-id YOUR_UPLOAD_ID --number 1 --file ./part1.bin --content-md5 YOUR_PART_MD5_HEX # 带 MD5 的分片上传用法
fds_cli multipart part demo-bucket demo/multipart.bin --upload-id YOUR_UPLOAD_ID --number 1 --stdin --length 8388608 < ./part1.bin # stdin 分片用法，示例文件应为 8 MiB
fds_cli multipart part demo-bucket demo/multipart.bin --upload-id YOUR_UPLOAD_ID --number 1 --text 'hello fds' # 内存分片用法，小片仅用于服务允许的单片或末片场景
fds_cli multipart complete demo-bucket demo/multipart.bin --upload-id YOUR_UPLOAD_ID --parts ./parts.json # 使用真实分片回执完成上传
fds_cli multipart complete demo-bucket demo/multipart.bin --upload-id YOUR_UPLOAD_ID --parts ./parts.json --content-type application/octet-stream --expires 2030-01-01T00:00:00Z # 完成时指定元数据和响应签名到期时间的用法
fds_cli object basic-info demo-bucket demo/multipart.bin # 核对合并后的总大小
fds_cli object get demo-bucket demo/multipart.bin --out ./multipart-downloaded.bin # 下载并与完整源文件比较
fds_cli multipart abort demo-bucket demo/unfinished.bin --upload-id YOUR_UNFINISHED_UPLOAD_ID # 取消另一笔确认未完成且要放弃的会话
```

各 part/complete 参数示例按需选择，不对同一会话重复执行整组。`parts.json` 使用真实返回的回执按片号组成数组，格式见文末。分片大小遵守目标服务限制；结果不明时不自动重发 complete 或 abort。

## 托管分片上传

```bash
fds_cli multipart upload demo-bucket demo/managed.bin --file ./large.bin --recovery ./upload-1.jsonl # 使用默认 8 MiB 分片、4 并发上传
fds_cli multipart upload demo-bucket demo/managed.bin --file ./large.bin --part-size 8388608 --concurrency 2 --recovery ./upload-2.jsonl # 自定义分片和并发
fds_cli multipart upload demo-bucket demo/managed.bin --file ./large.bin --abort-on-failure --recovery ./upload-3.jsonl # 分片失败或合作取消时尝试 abort
fds_cli multipart upload demo-bucket demo/managed.bin --file ./large.bin --content-type application/octet-stream --meta x-xiaomi-meta-purpose=verify --recovery ./upload-4.jsonl # 指定元数据
fds_cli object basic-info demo-bucket demo/managed.bin # 核对最终大小
fds_cli object get demo-bucket demo/managed.bin --out ./managed-downloaded.bin # 下载并与源文件比较
fds_cli multipart abort demo-bucket demo/managed.bin --upload-id YOUR_FAILED_UPLOAD_ID # 仅清理确认未完成、采用保留策略且要放弃的会话
```

每个 `--recovery` 必须是新文件。文件需非空；JSONL 保存会话 ID、确认分片和最终状态，不支持自动续传上传。分片传输期间按 Ctrl-C 可验证合作取消：启用 `--abort-on-failure` 且取消成功时为 `Aborted`。`CompletionUnknown` 需先核对服务端结果，不能直接取消或重传。

## 并发下载与断点

```bash
fds_cli download demo-bucket demo/large.bin ./downloaded.bin # 并发下载，默认 1 MiB 分片、4 并发
fds_cli download demo-bucket demo/large.bin ./downloaded.bin --part-size 1048576 --concurrency 8 # 指定分片大小和并发
fds_cli download demo-bucket demo/large.bin ./range.bin --range bytes=0-1048575 # 下载前 1 MiB
fds_cli download demo-bucket demo/large.bin ./tail.bin --range bytes=1048576- # 从第 1 MiB 后下载到末尾
fds_cli download demo-bucket demo/large.bin ./suffix.bin --range bytes=-1024 # 下载末尾 1024 字节
fds_cli download demo-bucket demo/large.bin ./limited.bin --requests-per-second 2 # 限制分片请求启动速率，不是字节带宽
fds_cli download demo-bucket demo/large.bin ./resume.bin --resume --part-size 1048576 --concurrency 1 # 开启断点记录
fds_cli download demo-bucket demo/large.bin ./resume.bin --resume --part-size 1048576 --concurrency 1 # 中断后以相同对象、路径、范围和片大小重新执行
```

下载后与源文件逐字节比较。续传需要强 ETag 和有效 checkpoint；成功下载后再次执行不能证明复用了断点。强制退出可能留下 `.download.lock`，确认旧进程已结束后再处理残留锁，并保留 `.tmp` 和 `.download.bp`。

## 删除与恢复

```bash
fds_cli object delete demo-bucket demo/hello.txt # 删除单个对象
fds_cli object exists demo-bucket demo/hello.txt # 删除后应为 false
fds_cli object delete-many demo-bucket demo/a.txt demo/b.txt # 批量删除，查看 deleted 和 failures
fds_cli object delete-many demo-bucket demo/a.txt demo/b.txt --trash # 批量移入回收站
fds_cli object list demo-bucket --prefix demo/temp/ --all # 删除前查看范围
fds_cli object delete-prefix demo-bucket demo/temp/ # 删除前缀下所有枚举到的对象
fds_cli object delete-prefix demo-bucket demo/temp/ --max-keys 100 # 按指定分页大小枚举并删除
fds_cli object delete-prefix demo-bucket demo/temp/ --marker YOUR_MARKER # 从指定 marker 后开始删除，会跳过之前的对象
fds_cli object delete-prefix demo-bucket demo/temp/ --delimiter / # 仅删除枚举到的当前层对象，不递归 commonPrefixes
fds_cli object delete-prefix demo-bucket demo/temp/ --trash # 按前缀移入回收站
fds_cli object restore demo-bucket demo/a.txt # 恢复满足服务保留条件的已删除对象
fds_cli object get demo-bucket demo/a.txt # 验证恢复后的内容
fds_cli object list demo-bucket --prefix demo/temp/ --all # 检查前缀清理结果
```

批删失败时仍会输出 `output`、`unconfirmed`、`not_submitted`，先核对每个 key 再处理。`delete-prefix` 直接调用 SDK 的 `delete_prefix`，`--marker`、`--delimiter` 分别对应 `.marker()`、`.delimiter()`；不给 `--marker` 时仍发送空的 `marker=`。起始 marker 也计入循环检查：后续页的 nextMarker 回到它时立即停止，不删除该页；名字越出 prefix 或 nextMarker 缺失、重复时，都以 `ProtocolViolation: invalid prefix deletion page or pagination marker` 退出 1，已删除的部分仍在 `output` 中。`restore` 是删除恢复，不是归档解冻；普通对象删除不清理未完成的分片会话。

## 服务发现与 OAuth

服务命令不需要 AK/SK。普通 OAuth 命令使用前清除 `FDS_ACCESS_KEY_ID`、`FDS_ACCESS_KEY_SECRET`，避免与 `--oauth` 冲突；输入文件格式见文末。

```bash
fds_cli --endpoint https://your-service.example.com service endpoint YOUR_REGION YOUR_SERVICE # 发现 endpoint，返回 hostname
fds_cli --endpoint https://your-auth.example.com service exchange --file ./oauth.json # 交换并检查 token，仅输出应用 ID 和过期信息
fds_cli --endpoint https://your-auth.example.com service exchange --file ./oauth-xiaomi.json # 使用小米 provider 交换 token
fds_cli --oauth ./oauth.json bucket list-app # 交换 token 后列出该 App 的 Bucket
fds_cli --oauth ./oauth.json --auth-endpoint https://your-auth.example.com bucket list-app # 指定独立的交换 origin
fds_cli --oauth ./oauth.json object list demo-bucket --all # 使用 OAuth 列举对象
fds_cli --oauth ./oauth.json object get demo-bucket demo/hello.txt --out ./oauth-downloaded.txt # 使用 OAuth 下载并比对已知内容
```

普通存储请求仍使用 `FDS_ENDPOINT`；未指定 `--auth-endpoint` 时也用它交换 token。每次 `--oauth` 调用在操作开始前交换一次 token，交换失败时立即报错退出（码 1），不执行操作；之后由 SDK 的 `StorageTokenProvider` 在 token 剩余有效期不超过默认提前量（5 分钟）时自动重新交换，长时间运行的命令（大文件传输、分页列举、批量删除等）不会因 token 过期中断。提前量不可通过命令行修改。续期失败时，当前请求不发送，命令以 `Unauthenticated`（token 未过期）或 `CredentialsExpired`（token 已过期）报错并以 1 退出，已完成的操作不回滚；过期 token 不会被发出。发现的 hostname 不会被自动采用。provider 支持 `XiaoMi`、`QQ`、`Sina`、`RenRen`，需分别提供对应平台的真实凭据。

## 超时、重试与诊断

```bash
fds_cli --connect-timeout-ms 5000 object head demo-bucket demo/hello.txt # 设置连接超时
fds_cli --request-timeout-ms 30000 object get demo-bucket demo/hello.txt # 设置整个请求的截止时间
fds_cli --read-timeout-ms 10000 object get demo-bucket demo/hello.txt # 设置 I/O 空闲超时（上传、等待响应、读取）
fds_cli --pool-idle-timeout-ms 60000 object list demo-bucket --all # 设置连接池空闲时间
fds_cli --tcp-keepalive-secs 30 object list demo-bucket --all # 设置 TCP keepalive
fds_cli --retries 2 --retry-initial-ms 100 --retry-max-ms 1000 object head demo-bucket demo/hello.txt # 设置只读请求重试
fds_cli --diagnostics object get demo-bucket demo/missing.txt # 失败时输出详细响应诊断
```

成功请求只验证配置入口；要验证超时和重试触发，使用可控延迟/503 服务。SDK 不重试写操作，也不重新读取已经开始消费的成功响应 body。诊断可能包含敏感服务端文本。

## CSLB

先用 `cargo install --path crates/fds_cli --locked --features cslb --force` 安装启用 CSLB 的版本。

```bash
fds_cli --help # 确认存在 --cslb-max-nodes 参数
fds_cli --cslb-max-nodes 4 object head demo-bucket demo/hello.txt # 设置每个主机最多保留 4 个 DNS 节点
```

需要多地址 DNS 和网络观测才能验证节点选择；此参数不是 HTTP 并发控制。

## smoke

```bash
fds_cli smoke demo-bucket # 执行常用对象操作、下载与手工分片验证，默认清理本轮对象
fds_cli smoke demo-bucket --skip-multipart # 跳过手工分片检查
fds_cli smoke demo-bucket --skip-download # 跳过下载管理器检查
fds_cli smoke demo-bucket --skip-multipart --skip-download # 执行基础检查
fds_cli smoke demo-bucket --keep # 保留本轮对象供人工回读
```

smoke 会创建独立前缀并设置测试对象公开可读，不包含所有 Bucket 配置、OAuth、CDN 或托管取消验证。独立功能用上面的命令验证；跳过项不能记为通过。

## JSON 输入文件

### acl.json

使用有效的 owner、grantee ID；示例向指定用户授予 READ。

```json
{"owner":{"id":"YOUR_OWNER_ID","displayName":""},"accessControlList":[{"grantee":{"id":"YOUR_USER_ID","displayName":""},"permission":"READ","type":"USER"}]}
```

### lifecycle.json

```json
{"rules":[{"id":"expire-demo","prefix":"demo/temp/","enabled":true,"actions":{"expiration":{"days":7}}}]}
```

### lifecycle-rule.json

```json
{"id":"expire-demo","prefix":"demo/temp/","enabled":true,"actions":{"expiration":{"days":14}}}
```

### accesslog-enable.json

```json
{"enabled":true,"logBucketName":"log-bucket","logPrefix":"access/"}
```

### accesslog-disable.json

```json
{"enabled":false,"logBucketName":"log-bucket","logPrefix":"access/"}
```

### parts.json

将真实 part 回执按 `partNumber` 排序组成数组，不能直接使用示例 ETag。

```json
[{"partNumber":1,"etag":"REAL_PART_1_ETAG","partSize":8388608},{"partNumber":2,"etag":"REAL_PART_2_ETAG","partSize":10485760}]
```

### oauth.json

QQ、Sina、RenRen 使用同样的字段结构，替换 provider 和对应平台凭据。

```json
{"app_id":"YOUR_FDS_APP_ID","oauth_app_id":"YOUR_PROVIDER_APP_ID","access_token":"YOUR_PROVIDER_TOKEN","provider":"QQ"}
```

### oauth-xiaomi.json

XiaoMi 必须提供三个 MAC 字段；其他 provider 不能携带 MAC 字段。

```json
{"app_id":"YOUR_FDS_APP_ID","oauth_app_id":"YOUR_XIAOMI_APP_ID","access_token":"YOUR_XIAOMI_TOKEN","provider":"XiaoMi","xiaomi_mac":{"open_id":"YOUR_OPEN_ID","key":"YOUR_MAC_KEY","algorithm":"YOUR_PROVIDER_MAC_ALGORITHM"}}
```

更多 SDK 约束见 [SDK 手册](../README.md)，本地与真实环境测试见 [验证指南](validation.md)。
