//! Explicit, isolated real-service acceptance. Never part of the default suite.
use fds::{ByteRange, Client, GetObjectRequest, ListObjectsRequest, PutObjectRequest};
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
#[ignore = "requires explicit FDS_LIVE=1 and a dedicated scratch bucket"]
async fn live_object_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(std::env::var("FDS_LIVE").as_deref(), Ok("1"));
    let endpoint = std::env::var("FDS_ENDPOINT")?;
    assert!(
        endpoint.starts_with("https://"),
        "live acceptance requires HTTPS"
    );
    let client = Client::builder()
        .endpoint(endpoint)
        .credentials(
            std::env::var("FDS_ACCESS_KEY_ID")?,
            std::env::var("FDS_ACCESS_KEY_SECRET")?,
        )
        .build()?;
    let bucket = std::env::var("FDS_LIVE_BUCKET")?;
    let run = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let key = format!(
        "fds-sdk-acceptance/{}/{run}/空 格+%.txt",
        std::process::id()
    );
    // This identifier is safe to retain for cleanup if the process is killed.
    eprintln!("scratch object: {key}");
    let result: fds::Result<()> = async {
        PutObjectRequest::new(&bucket, &key, b"hello fds".to_vec())
            .content_type("text/plain")
            .send(&client)
            .await?;
        if client.head_object(&bucket, &key).await?.is_none() {
            return Err(fds::FdsError::invalid("uploaded object missing from HEAD"));
        }
        let full = GetObjectRequest::new(&bucket, &key)
            .send(&client)
            .await?
            .bytes()
            .await?;
        if full.as_ref() != b"hello fds" {
            return Err(fds::FdsError::invalid("roundtrip bytes differ"));
        }
        let ranged = GetObjectRequest::new(&bucket, &key)
            .range(ByteRange::inclusive(1, 4)?)
            .send(&client)
            .await?;
        if ranged.status() != 206 || ranged.bytes().await?.as_ref() != b"ello" {
            return Err(fds::FdsError::invalid("range response differs"));
        }
        // Distinguish bare Bucket GET from explicit empty/nonempty prefix requests.
        let _ = client.get_bucket_info(&bucket).await?;
        let _ = ListObjectsRequest::new(&bucket)
            .max_keys(1)
            .send(&client)
            .await?;
        let listing = ListObjectsRequest::new(&bucket)
            .prefix(&key)
            .send(&client)
            .await?;
        if !listing
            .object_summaries
            .iter()
            .any(|o| o.object_name == key)
        {
            return Err(fds::FdsError::invalid(
                "listing did not return uploaded object",
            ));
        }
        Ok(())
    }
    .await;
    // Delete only this run's exact key, even after ambiguous upload failure.
    // Keep both outcomes visible; no prefix deletion or Bucket mutation.
    let cleanup = client.delete_object(&bucket, &key).await;
    if cleanup.is_err() {
        eprintln!("scratch cleanup failed; delete the recorded exact object manually");
    }
    result?;
    cleanup?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires FDS_LIVE=1, provisioned OAuth app/provider and readable fixture"]
async fn live_oauth_discovery_and_read() -> Result<(), Box<dyn std::error::Error>> {
    use fds::{FdsServiceClient, OAuthProvider, StorageTokenRequest};
    assert_eq!(std::env::var("FDS_LIVE").as_deref(), Ok("1"));
    let service = FdsServiceClient::new(std::env::var("FDS_DISCOVERY_ORIGIN")?)?;
    let discovered = service
        .get_endpoint(
            &std::env::var("FDS_REGION")?,
            &std::env::var("FDS_SERVICE")?,
        )
        .await?;
    assert_eq!(
        discovered,
        std::env::var("FDS_EXPECTED_DISCOVERED_ENDPOINT")?
    );
    // Do not automatically send a credential to the discovered host.
    let provider = match std::env::var("FDS_OAUTH_PROVIDER")?.as_str() {
        "XiaoMi" => OAuthProvider::XiaoMi,
        "QQ" => OAuthProvider::QQ,
        "Sina" => OAuthProvider::Sina,
        "RenRen" => OAuthProvider::RenRen,
        _ => return Err("unsupported FDS_OAUTH_PROVIDER".into()),
    };
    let mut input = StorageTokenRequest::new(
        std::env::var("FDS_APP_ID")?,
        std::env::var("FDS_OAUTH_APP_ID")?,
        std::env::var("FDS_OAUTH_ACCESS_TOKEN")?,
        provider,
    );
    if matches!(provider, OAuthProvider::XiaoMi) {
        input = input.xiaomi_mac(
            std::env::var("FDS_OAUTH_OPEN_ID")?,
            std::env::var("FDS_OAUTH_MAC_KEY")?,
            std::env::var("FDS_OAUTH_MAC_ALGORITHM")?,
        );
    }
    let token = service.exchange_storage_token(&input).await?;
    assert!(!token.is_expired(), "exchanged token already expired");
    let endpoint = std::env::var("FDS_ENDPOINT")?;
    assert!(endpoint.starts_with("https://"));
    let client = Client::builder()
        .endpoint(endpoint)
        .storage_token(token)
        .build()?;
    let _ = client.list_app_buckets().await?;
    let actual = GetObjectRequest::new(
        std::env::var("FDS_LIVE_BUCKET")?,
        std::env::var("FDS_OAUTH_FIXTURE_KEY")?,
    )
    .send(&client)
    .await?
    .bytes()
    .await?;
    assert!(
        actual.as_ref() == std::env::var("FDS_OAUTH_FIXTURE_TEXT")?.as_bytes(),
        "OAuth fixture content differs"
    );
    Ok(())
}
