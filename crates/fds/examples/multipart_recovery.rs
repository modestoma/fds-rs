//! Compile-only tour: object round trip, listing pages, ACL, and a manual
//! multipart upload that keeps its recovery context (upload ID, error kind,
//! dispatch, status) when a part or the completion fails. main performs no I/O.
use fds::acl::{AccessControlList, AclAction};
use fds::object::ByteRange;
use fds::Client;
use futures_util::{pin_mut, StreamExt};

#[allow(dead_code)]
async fn round_trip_and_multipart(client: &Client, bucket: &str, key: &str) -> fds::Result<()> {
    client
        .put_object(bucket, key, b"hello".to_vec())
        .content_type("text/plain")
        .send()
        .await?;
    let mut output = client
        .get_object(bucket, key)
        .range(ByteRange::inclusive(0, 4)?)
        .send()
        .await?;
    while let Some(chunk) = output.chunk().await? {
        let _ = chunk;
    }
    let _metadata = client.head_object(bucket, key).send().await?;
    let pages = client.list_objects(bucket).prefix(key).into_pages();
    pin_mut!(pages);
    while let Some(page) = pages.next().await {
        let _ = page?;
    }
    client
        .set_object_acl(bucket, key, AccessControlList::default())
        .action(AclAction::Add)
        .send()
        .await?;
    let session = client.create_multipart_upload(bucket, key).send().await?;
    let first = std::num::NonZeroU16::MIN;
    let part = match session.upload_part(first, b"part".to_vec()).send().await {
        Ok(part) => part,
        Err(primary) => {
            let cleanup = session.abort().send().await.err();
            record_failure(session.upload_id(), &primary, cleanup.as_ref());
            return Err(primary);
        }
    };
    let parts = vec![part];
    if let Err(primary) = session.complete(parts).send().await {
        // Outcome may be unknown: preserve recovery context, never auto-abort/retry.
        record_failure(session.upload_id(), &primary, None);
        return Err(primary);
    }
    Ok(())
}
fn record_failure(id: &str, primary: &fds::Error, cleanup: Option<&fds::Error>) {
    // A real application persists this context securely, without logging secrets.
    let _recovery = (
        id,
        primary.kind(),
        primary.dispatch(),
        primary.status(),
        primary.service_code(),
        primary.request_id(),
        cleanup.map(fds::Error::status),
    );
}
fn main() {
    // Checked by cargo check --all-targets; deliberately no service execution.
    let _client = Client::builder()
        .endpoint("https://example.invalid")
        .credentials("id", "secret")
        .build();
    let error = fds::Error::new(fds::ErrorKind::InvalidInput, "example");
    let _ = (
        error.kind(),
        error.is_retryable(),
        error.status(),
        error.service_code(),
        error.request_id(),
    );
}
