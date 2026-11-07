//! Compiled against the real SDK; requires explicit endpoint and credentials to run.
use fds::object::ByteRange;
use fds::Client;
use http::Method;
use std::time::Duration;

#[tokio::main]
async fn main() -> fds::Result<()> {
    let client = Client::builder()
        .endpoint(std::env::var("FDS_ENDPOINT").expect("FDS_ENDPOINT"))
        .credentials(
            std::env::var("FDS_ACCESS_KEY").expect("FDS_ACCESS_KEY"),
            std::env::var("FDS_SECRET_KEY").expect("FDS_SECRET_KEY"),
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
        "status={}, content-type={:?}",
        output.status(),
        output.metadata().get("Content-Type")
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
