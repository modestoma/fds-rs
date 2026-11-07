//! Compile-checked uploads; this example does not contact a service by default.
use bytes::Bytes;
use fds::object::UploadBody;
use fds::Client;

#[allow(dead_code)]
async fn upload(client: &Client, path: &std::path::Path) -> fds::Result<()> {
    client
        .put_object("bucket", "file.bin", UploadBody::from_file(path).await?)
        .content_type("application/octet-stream")
        .send()
        .await?;
    let chunks = futures_util::stream::iter([Ok(Bytes::from_static(b"hello"))]);
    client
        .put_object("bucket", "stream.txt", UploadBody::from_stream(chunks, 5))
        .content_type("text/plain")
        .send()
        .await?;
    Ok(())
}
fn main() {}
