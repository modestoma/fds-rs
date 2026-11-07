//! Client-only RSS probe; see scripts/benchmark_download.py.
use fds::manager::{DownloadRequest, Downloader};
use fds::Client;
#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let client = Client::builder()
        .endpoint(&args[1])
        .credentials("benchmark", "benchmark")
        .build()?;
    let downloader = Downloader::new(
        client,
        fds::manager::DownloadOptions::builder()
            .part_size(args[3].parse()?)
            .concurrency(args[4].parse()?)
            .resume(false)
            .build()?,
    );
    downloader
        .download(&DownloadRequest::new("b", "o", &args[2]))
        .await?;
    Ok(())
}
