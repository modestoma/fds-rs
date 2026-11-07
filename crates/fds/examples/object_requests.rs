//! Compiled against the real SDK; requires explicit endpoint and credentials to run.
use fds::{
    ByteRange, Client, GeneratePresignedURLRequest, GetObjectRequest, HttpMethod, PutObjectRequest,
};
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
    PutObjectRequest::new(&bucket, "example.txt", b"hello".to_vec())
        .content_type("text/plain")
        .send(&client)
        .await?;
    let mut output = GetObjectRequest::new(&bucket, "example.txt")
        .range(ByteRange::inclusive(0, 4)?)
        .send(&client)
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
    let _url = GeneratePresignedURLRequest::expires_in(
        &bucket,
        "example.txt",
        HttpMethod::Get,
        Duration::from_secs(60),
    )?
    .generate(&client)?;
    Ok(())
}
