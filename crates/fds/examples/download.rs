//! Concurrent, resumable download via the `manager` feature (default).
//!
//! ```bash
//! FDS_ENDPOINT=https://cnbj1.fds.api.xiaomi.com \
//! FDS_ACCESS_KEY_ID=... FDS_ACCESS_KEY_SECRET=... \
//! cargo run --example download -- hellodf build.log /tmp/build.log
//! ```

use std::error::Error;

use fds::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let bucket = args.next().unwrap_or_else(|| "hellodf".into());
    let object = args.next().unwrap_or_else(|| "build.log".into());
    let file_path = args.next().unwrap_or_else(|| "/tmp/build.log".into());

    let client = Client::builder()
        .endpoint(std::env::var("FDS_ENDPOINT")?)
        .credentials(
            std::env::var("FDS_ACCESS_KEY_ID")?,
            std::env::var("FDS_ACCESS_KEY_SECRET")?,
        )
        .build()?;

    // 1 MiB parts, 10 concurrent workers, breakpoint/resume enabled.
    client
        .download_file(bucket, object, &file_path)
        .part_size(1024 * 1024)
        .concurrency(10)
        .resume()
        .send()
        .await?;

    println!("downloaded to {file_path}");
    Ok(())
}
