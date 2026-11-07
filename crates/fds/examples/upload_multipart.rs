//! Run with FDS_ENDPOINT/FDS_ACCESS_KEY/FDS_SECRET_KEY and bucket, key, file args.
use fds::multipart::CleanupPolicy;
use fds::Client;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: upload_multipart BUCKET KEY FILE".into());
    }
    let client = Client::builder()
        .endpoint(std::env::var("FDS_ENDPOINT")?)
        .credentials(
            std::env::var("FDS_ACCESS_KEY")?,
            std::env::var("FDS_SECRET_KEY")?,
        )
        .build()?;
    let mut session = client
        .upload_file(&args[1], &args[2], &args[3])
        .part_size(8 * 1024 * 1024)
        .concurrency(4)
        .cleanup(CleanupPolicy::Preserve)
        .start()
        .await?;
    // Retain session outside the future. Another task can call token.cancel(),
    // but keep awaiting upload to receive its failure/cleanup result.
    let token = CancellationToken::new();
    match session.upload(token.cancelled()).await {
        Ok(_) => println!("upload completed"),
        Err(error) => {
            // In an application, securely persist this snapshot before returning.
            // It contains object identity and the upload ID; do not blindly log it.
            let progress = session.progress().clone();
            eprintln!(
                "upload failed in {:?}; {} confirmed parts; explicit recovery required",
                progress.state,
                progress.parts.len()
            );
            return Err(error.into());
        }
    }
    Ok(())
}
