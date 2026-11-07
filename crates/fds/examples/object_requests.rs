//! Ranged GET with typed response headers, then a presigned GET URL.
//! Requires FDS_ENDPOINT, FDS_ACCESS_KEY_ID, FDS_ACCESS_KEY_SECRET and FDS_BUCKET.
use fds::object::ByteRange;
use fds::Client;
use http::Method;
use std::time::Duration;

#[tokio::main]
async fn main() -> fds::Result<()> {
    let client = Client::builder()
        .endpoint(std::env::var("FDS_ENDPOINT").expect("FDS_ENDPOINT"))
        .credentials(
            std::env::var("FDS_ACCESS_KEY_ID").expect("FDS_ACCESS_KEY_ID"),
            std::env::var("FDS_ACCESS_KEY_SECRET").expect("FDS_ACCESS_KEY_SECRET"),
        )
        .build()?;
    let bucket = std::env::var("FDS_BUCKET").expect("FDS_BUCKET");
    client
        .put_object(&bucket, "example.txt", b"hello".to_vec())
        .content_type("text/plain")
        .send()
        .await?;
    let mut output = client
        .get_object(&bucket, "example.txt")
        .range(ByteRange::inclusive(0, 4)?)
        .send()
        .await?;
    println!(
        "status={}, partial={}, length={:?}, content-type={:?}",
        output.status().as_u16(),
        output.is_partial(),
        output.content_length(),
        output.content_type()
    );
    while let Some(chunk) = output.chunk().await? {
        println!("received {} bytes", chunk.len());
    }
    // The URL is a credential: pass it to its intended consumer, do not log it.
    let _url = client
        .presign_object(&bucket, "example.txt")
        .method(Method::GET)
        .expires_in(Duration::from_secs(60))
        .build()?;
    Ok(())
}
