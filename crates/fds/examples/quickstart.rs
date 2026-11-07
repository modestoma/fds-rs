//! Minimal bucket/object flow: put → get → list → presign.
//!
//! Run (requires a real FDS bucket + credentials):
//! ```bash
//! GO_FDS_TEST_ENDPOINT=https://cnbj1.fds.api.xiaomi.com \
//! GO_FDS_TEST_ACCESS_KEY_ID=... GO_FDS_TEST_ACCESS_KEY_SECRET=... \
//! cargo run --example quickstart -- my-bucket
//! ```

use std::error::Error;

use fds::object::ListObjectsRequest;
use fds::Client;
use http::Method;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let bucket = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "my-bucket".into());

    let client = Client::builder()
        .endpoint(
            std::env::var("GO_FDS_TEST_ENDPOINT")
                .unwrap_or_else(|_| "https://cnbj1.fds.api.xiaomi.com".into()),
        )
        .credentials(
            std::env::var("GO_FDS_TEST_ACCESS_KEY_ID")?,
            std::env::var("GO_FDS_TEST_ACCESS_KEY_SECRET")?,
        )
        .build()?;

    let object = "quickstart/hello.txt";

    client
        .put_object(&bucket, object, b"hello fds from rust".to_vec())
        .content_type("text/plain")
        .send()
        .await?;
    println!("put {bucket}/{object}");

    let bytes = client
        .get_object(&bucket, object)
        .send()
        .await?
        .bytes()
        .await?;
    println!(
        "got {} bytes: {:?}",
        bytes.len(),
        String::from_utf8_lossy(&bytes)
    );

    let listing = ListObjectsRequest::new(&bucket)
        .prefix("quickstart/")
        .send(&client)
        .await?;
    for o in &listing.object_summaries {
        println!("  - {} ({} bytes)", o.object_name, o.size);
    }

    let url = client
        .presign_object(&bucket, object)
        .method(Method::GET)
        .expires_in(std::time::Duration::from_secs(3600))
        .build()?;
    println!("presigned GET url:\n{url}");

    Ok(())
}
