//! Compile-only usage: explicitly provision the app and supply real credentials
//! before using the corresponding operations. This example makes no requests.
use fds::oauth::{FdsServiceClient, OAuthProvider};
use fds::{Client, Result};

#[allow(dead_code)]
async fn provisioned_application(
    origin: &str,
    app: &str,
    oauth_app: &str,
    access_token: &str,
) -> Result<()> {
    let service = FdsServiceClient::new(origin)?;
    let token = service
        .exchange_storage_token(app, oauth_app, access_token, OAuthProvider::QQ)
        .send()
        .await?;
    // Retain expires_at for scheduling an explicit exchange using fresh provider
    // credentials. Existing client clones retain their original token.
    let _expires_at = token.expires_at();
    let client = Client::builder()
        .endpoint(origin)
        .storage_token(token)
        .build()?;
    let _buckets = client.list_app_buckets().send().await?;
    Ok(())
}
fn main() {}
