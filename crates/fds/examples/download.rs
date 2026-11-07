//! Concurrent, resumable download via the `manager` feature (default).
//!
//! ```bash
//! GO_FDS_TEST_ENDPOINT=https://cnbj1.fds.api.xiaomi.com \
//! GO_FDS_TEST_ACCESS_KEY_ID=... GO_FDS_TEST_ACCESS_KEY_SECRET=... \
//! cargo run --example download -- hellodf build.log /tmp/build.log
//! ```

use std::error::Error;

use fds::manager::{DownloadRequest, Downloader};
use fds::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let bucket = args.next().unwrap_or_else(|| "hellodf".into());
    let object = args.next().unwrap_or_else(|| "build.log".into());
    let file_path = args.next().unwrap_or_else(|| "/tmp/build.log".into());

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

    // 1 MiB parts, 10 concurrent workers, breakpoint/resume enabled.
    let downloader = Downloader::new(
        client.clone(),
        fds::manager::DownloadOptions::builder()
            .part_size(1024 * 1024)
            .concurrency(10)
            .resume(true)
            .build()?,
    );
    downloader
        .download(&DownloadRequest::new(bucket, object, &file_path))
        .await?;

    println!("downloaded to {file_path}");
    Ok(())
}
