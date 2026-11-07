//! Compile-checked uploads; this example does not contact a service by default.
use bytes::Bytes;
use fds::object::{PutObjectRequest, UploadBody};
use fds::Client;

#[allow(dead_code)]
async fn upload(client: &Client, path: &std::path::Path) -> fds::Result<()> {
    PutObjectRequest::from_file("bucket", "file.bin", path)
        .await?
        .content_type("application/octet-stream")
        .send(client)
        .await?;
    let chunks = futures_util::stream::iter([Ok(Bytes::from_static(b"hello"))]);
    PutObjectRequest::new("bucket", "stream.txt", UploadBody::from_stream(chunks, 5))
        .content_type("text/plain")
        .send(client)
        .await?;
    Ok(())
}
fn main() {}
