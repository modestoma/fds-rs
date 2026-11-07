//! End-to-end download-manager tests against a stub server. Notably verifies
//! that `concurrency = 1` completes (Go's BUG-2 would deadlock there).

#![cfg(feature = "manager")]

use std::sync::Arc;

use fds::{Client, DownloadRequest, Downloader};

fn client_for(server: &mockito::ServerGuard) -> Client {
    Client::builder()
        .endpoint(server.url())
        .credentials("AKID", "sk")
        .build()
        .unwrap()
}

#[tokio::test]
async fn download_single_part_concurrency_one() {
    let content = "hello world FDS";
    let mut server = mockito::Server::new_async().await;

    // True HEAD supplies the size and source identity.
    let meta = server
        .mock("HEAD", "/b/o")
        .with_status(200)
        .with_header("content-length", &content.len().to_string())
        .with_header("etag", "\"source-v1\"")
        .with_header("last-modified", "Mon, 13 Jul 2026 10:00:00 GMT")
        .create_async()
        .await;

    // Data request: one part, GET /b/o with Range bytes=0-14.
    let data = server
        .mock("GET", "/b/o")
        .match_header("range", format!("bytes=0-{}", content.len() - 1).as_str())
        .with_status(206)
        .with_header("etag", "\"source-v1\"")
        .with_header(
            "content-range",
            &format!("bytes 0-{}/{}", content.len() - 1, content.len()),
        )
        .with_body(content)
        .create_async()
        .await;

    let out = std::env::temp_dir().join("fds_dl_single_part.bin");
    let _ = std::fs::remove_file(&out);

    let client = Arc::new(client_for(&server));
    // concurrency = 1 must NOT deadlock (Go BUG-2 is fixed).
    let downloader = Downloader::new(
        client,
        fds::DownloadOptions::builder()
            .part_size(1024 * 1024)
            .concurrency(1)
            .resume(false)
            .build()
            .unwrap(),
    );
    downloader
        .download(&DownloadRequest {
            bucket_name: "b".into(),
            object_name: "o".into(),
            range: None,
            file_path: out.clone(),
        })
        .await
        .unwrap();

    let got = std::fs::read_to_string(&out).unwrap();
    assert_eq!(got, content);

    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_file(out.with_extension("bin.tmp"));
    meta.assert_async().await;
    data.assert_async().await;
}

#[tokio::test]
async fn download_multi_part_concurrent() {
    // 10 bytes, part size 4 → 3 parts: [0-3],[4-7],[8-9]; concurrency 3.
    let content = b"0123456789";
    let mut server = mockito::Server::new_async().await;

    let meta = server
        .mock("HEAD", "/b/big")
        .with_status(200)
        .with_header("content-length", "10")
        .with_header("etag", "\"source-v1\"")
        .with_header("last-modified", "Mon, 13 Jul 2026 10:00:00 GMT")
        .create_async()
        .await;

    for (start, end) in [(0usize, 3usize), (4, 7), (8, 9)] {
        server
            .mock("GET", "/b/big")
            .match_header("range", format!("bytes={start}-{end}").as_str())
            .with_status(206)
            .with_header("etag", "\"source-v1\"")
            .with_header("content-range", &format!("bytes {start}-{end}/10"))
            .with_body(&content[start..=end])
            .expect(1)
            .create_async()
            .await;
    }

    let out = std::env::temp_dir().join("fds_dl_multi_part.bin");
    let _ = std::fs::remove_file(&out);

    let client = Arc::new(client_for(&server));
    let downloader = Downloader::new(
        client,
        fds::DownloadOptions::builder()
            .part_size(4)
            .concurrency(3)
            .resume(false)
            .build()
            .unwrap(),
    );
    downloader
        .download(&DownloadRequest {
            bucket_name: "b".into(),
            object_name: "big".into(),
            range: None,
            file_path: out.clone(),
        })
        .await
        .unwrap();

    let got = std::fs::read(&out).unwrap();
    assert_eq!(got, content);

    let _ = std::fs::remove_file(&out);
    meta.assert_async().await;
}
