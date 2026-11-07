//! Client-only RSS probe; see scripts/benchmark_download.py.
use fds::Client;
#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let client = Client::builder()
        .endpoint(&args[1])
        .credentials("benchmark", "benchmark")
        .build()?;
    client
        .download_file("b", "o", &args[2])
        .part_size(args[3].parse()?)
        .concurrency(args[4].parse()?)
        .send()
        .await?;
    Ok(())
}
