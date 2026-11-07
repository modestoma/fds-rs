//! Run only against an intended bucket: FDS_ENDPOINT, FDS_ACCESS_KEY_ID,
//! FDS_ACCESS_KEY_SECRET and FDS_BUCKET must be configured.
use fds::object::ListObjectsRequest;
use fds::Client;
use futures_util::{pin_mut, StreamExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder()
        .endpoint(std::env::var("FDS_ENDPOINT")?)
        .credentials(
            std::env::var("FDS_ACCESS_KEY_ID")?,
            std::env::var("FDS_ACCESS_KEY_SECRET")?,
        )
        .build()?;
    let objects = ListObjectsRequest::new(std::env::var("FDS_BUCKET")?)
        .prefix("logs/")
        .max_keys(100)
        .objects(&client);
    pin_mut!(objects);
    while let Some(object) = objects.next().await {
        println!("{}", object?.object_name);
    }
    Ok(())
}
