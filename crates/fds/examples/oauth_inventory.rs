//! QQ provider token 交换、应用 Bucket 校验与惰性对象统计。
//! 环境变量：FDS_OAUTH_ORIGIN / FDS_ENDPOINT / FDS_APP_ID /
//! FDS_OAUTH_APP_ID / FDS_PROVIDER_TOKEN。
//! 运行：cargo run --example oauth_inventory -- BUCKET PREFIX
//! 只读取存储数据，但 token 交换可能创建或续期凭据。
use fds::oauth::{FdsServiceClient, OAuthProvider};
use fds::object::ListObjectsRequest;
use fds::Client;
use futures_util::{pin_mut, StreamExt};
use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: oauth_inventory BUCKET PREFIX".into());
    }
    let service = FdsServiceClient::new(std::env::var("FDS_OAUTH_ORIGIN")?)?;
    let token = service
        .exchange_storage_token(
            std::env::var("FDS_APP_ID")?,
            std::env::var("FDS_OAUTH_APP_ID")?,
            std::env::var("FDS_PROVIDER_TOKEN")?,
            OAuthProvider::QQ,
        )
        .send()
        .await?;
    let expires_at = token.expires_at();
    let client = Client::builder()
        .endpoint(std::env::var("FDS_ENDPOINT")?)
        .storage_token(token)
        .build()?;
    let app_buckets = client.list_app_buckets().send().await?;
    if !app_buckets.buckets.iter().any(|b| b.name == args[1]) {
        return Err("requested bucket is absent from the application bucket listing".into());
    }
    // Bucket 列表检查不替代服务的逐请求授权；token 到期时停止并交给应用处理。
    let objects = ListObjectsRequest::new(&args[1])
        .prefix(&args[2])
        .objects(&client);
    pin_mut!(objects);
    let mut count = 0u64;
    let mut total = 0u64;
    while let Some(object) = objects.next().await {
        let object = object?;
        if !object.object_name.starts_with(&args[2]) {
            return Err("listing returned an object outside the requested prefix".into());
        }
        let size = u64::try_from(object.size)?;
        count = count.checked_add(1).ok_or("object count overflow")?;
        total = total.checked_add(size).ok_or("total size overflow")?;
    }
    println!("objects={count} bytes={total}");
    eprintln!("storage token 到期时间：{expires_at}");
    Ok(())
}
