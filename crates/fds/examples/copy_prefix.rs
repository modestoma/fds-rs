//! 分页列举源前缀，逐对象服务端复制，再核对目标 HEAD 长度。
//! 环境变量：FDS_ENDPOINT / FDS_ACCESS_KEY_ID / FDS_ACCESS_KEY_SECRET。
//! 运行：cargo run --example copy_prefix -- SRC_BUCKET SRC_PREFIX DST_BUCKET DST_PREFIX
//! 目标同名对象可能被覆盖；失败保留已复制对象，源对象不删除。
use fds::Client;
use futures_util::{pin_mut, StreamExt};
use std::error::Error;
use std::num::NonZeroU32;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 {
        return Err("usage: copy_prefix SRC_BUCKET SRC_PREFIX DST_BUCKET DST_PREFIX".into());
    }
    let (source_bucket, source_prefix, target_bucket, target_prefix) =
        (&args[1], &args[2], &args[3], &args[4]);
    if source_prefix.is_empty() || target_prefix.is_empty() {
        return Err("source and target prefixes must be explicit and nonempty".into());
    }
    if source_bucket == target_bucket
        && (source_prefix.starts_with(target_prefix) || target_prefix.starts_with(source_prefix))
    {
        return Err("same-bucket source and target prefixes must not overlap".into());
    }
    let client = Client::builder()
        .endpoint(std::env::var("FDS_ENDPOINT")?)
        .credentials(
            std::env::var("FDS_ACCESS_KEY_ID")?,
            std::env::var("FDS_ACCESS_KEY_SECRET")?,
        )
        .build()?;
    // 不设 delimiter，遍历整个字符串前缀；服务清单不是事务快照。
    let objects = client
        .list_objects(source_bucket)
        .prefix(source_prefix)
        .max_keys(NonZeroU32::new(100).unwrap())
        .into_objects();
    pin_mut!(objects);
    let mut copied = 0u64;
    while let Some(object) = objects.next().await {
        let object = object?;
        let suffix = object
            .object_name
            .strip_prefix(source_prefix.as_str())
            .ok_or("listing returned an object outside the requested prefix")?;
        let target_key = format!("{target_prefix}{suffix}");
        let expected = u64::try_from(object.size)?;
        client
            .copy_object(
                (source_bucket, &object.object_name),
                (target_bucket, &target_key),
            )
            .send()
            .await?;
        let head = client
            .head_object(target_bucket, &target_key)
            .send()
            .await?
            .ok_or("copied object was not found by HEAD")?;
        let actual: u64 = head
            .get("content-length")
            .ok_or("target HEAD response has no Content-Length")?
            .parse()?;
        if actual != expected {
            return Err("target size differs from source listing; reconcile copied objects".into());
        }
        // 大小核对不证明内容相同；严格迁移应冻结源写入并另存业务清单。
        copied += 1;
        eprintln!("已完成复制和大小核对：{copied} 个对象");
    }
    println!("copied_objects={copied}");
    Ok(())
}
