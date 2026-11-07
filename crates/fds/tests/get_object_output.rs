//! `GetObjectOutput`: typed header accessors, `into_stream` and
//! `into_async_read`.

use chrono::{DateTime, Utc};
use fds::object::{ByteRange, ContentRange, GetObjectOutput};
use fds::{Client, Dispatch, ErrorKind};
use futures_util::{Stream, TryStreamExt};
use http::StatusCode;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

fn client(endpoint: impl Into<String>) -> Client {
    Client::builder()
        .endpoint(endpoint.into())
        .credentials("AKID", "sk")
        .build()
        .unwrap()
}

fn assert_stream<S: Stream<Item = fds::Result<bytes::Bytes>> + Send + Unpin + 'static>(s: S) -> S {
    s
}

fn assert_reader<R: AsyncRead + Send + Unpin + 'static>(r: R) -> R {
    r
}

/// A GET answered by mockito with `status`, `headers` and `body`.
async fn output(
    server: &mut mockito::ServerGuard,
    status: usize,
    headers: &[(&str, &str)],
    body: &'static str,
) -> GetObjectOutput {
    let mut mock = server
        .mock("GET", "/b/o")
        .with_status(status)
        .with_body(body);
    for (name, value) in headers {
        mock = mock.with_header(*name, value);
    }
    let _mock = mock.create_async().await;
    let mut request = client(server.url()).get_object("b", "o");
    if status == 206 {
        request = request.range(ByteRange::inclusive(2, 5).unwrap());
    }
    request.send().await.unwrap()
}

/// A response that promises 10 bytes, sends `hello`, then waits for `stall`
/// before closing the connection.
async fn truncated(stall: Duration, client_read_timeout: Option<Duration>) -> GetObjectOutput {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut builder = Client::builder()
        .endpoint(format!("http://{}", listener.local_addr().unwrap()))
        .credentials("AKID", "sk");
    if let Some(timeout) = client_read_timeout {
        builder = builder.read_timeout(timeout);
    }
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nhello")
            .await
            .unwrap();
        tokio::time::sleep(stall).await;
        let _ = socket.shutdown().await;
    });
    builder
        .build()
        .unwrap()
        .get_object("b", "o")
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn typed_accessors_read_partial_response_headers() {
    let mut server = mockito::Server::new_async().await;
    let output = output(
        &mut server,
        206,
        &[
            ("content-type", "text/plain; charset=utf-8"),
            ("etag", "\"v1\""),
            ("last-modified", "Sun, 06 Nov 1994 08:49:37 GMT"),
            ("content-range", "bytes 2-5/10"),
        ],
        "llo ",
    )
    .await;
    assert_eq!(output.status(), StatusCode::PARTIAL_CONTENT);
    assert!(output.is_partial());
    assert_eq!(output.content_length(), Some(4));
    assert_eq!(output.content_type(), Some("text/plain; charset=utf-8"));
    assert_eq!(output.etag(), Some("\"v1\""));
    assert_eq!(
        output.last_modified(),
        Some("1994-11-06T08:49:37Z".parse::<DateTime<Utc>>().unwrap())
    );
    let range = output.content_range().unwrap();
    assert_eq!(
        (range.start(), range.end(), range.complete_length()),
        (2, 5, Some(10))
    );
    assert_eq!(format!("{output:?}"), "GetObjectOutput { status: 206, .. }");
    assert_eq!(output.bytes().await.unwrap(), "llo ");
}

#[tokio::test]
async fn typed_accessors_return_none_for_missing_or_malformed_headers() {
    let mut server = mockito::Server::new_async().await;
    let missing = output(&mut server, 200, &[], "ok").await;
    assert_eq!(missing.status(), StatusCode::OK);
    assert!(!missing.is_partial());
    assert_eq!(missing.content_length(), Some(2));
    assert_eq!(
        (
            missing.content_type(),
            missing.etag(),
            missing.last_modified(),
            missing.content_range()
        ),
        (None, None, None, None)
    );
    // (header, malformed values); every accessor tolerates them as None.
    let malformed: [(&str, &[&str]); 2] = [
        (
            "last-modified",
            &[
                "yesterday",
                "2024-01-02T03:04:05Z",
                "Sun, 32 Nov 1994 08:49:37 GMT",
            ],
        ),
        (
            "content-range",
            &[
                "bytes 5-2/10",
                "bytes 0-10/10",
                "bytes */10",
                "bytes 0-1",
                "items 0-1/2",
                "bytes -1-2/3",
                "bytes 0-1/x",
                "bytes 0-18446744073709551616/*",
            ],
        ),
    ];
    for (name, values) in malformed {
        for value in values {
            let output = output(&mut server, 200, &[(name, value)], "ok").await;
            assert_eq!(output.headers()[name], *value);
            assert_eq!(output.last_modified(), None, "{name}: {value}");
            assert_eq!(output.content_range(), None, "{name}: {value}");
            assert_eq!(output.bytes().await.unwrap(), "ok");
        }
    }
    // An unknown complete length is `*`.
    let output = output(&mut server, 200, &[("content-range", "bytes 0-1/*")], "ok").await;
    assert_eq!(output.headers()["content-range"], "bytes 0-1/*");
    let range: ContentRange = output.content_range().unwrap();
    assert_eq!(
        (range.start(), range.end(), range.complete_length()),
        (0, 1, None)
    );
}

#[tokio::test]
async fn into_stream_yields_the_remaining_body() {
    let mut server = mockito::Server::new_async().await;
    let mut output = output(&mut server, 200, &[], "hello world").await;
    let mut collected = Vec::new();
    // Chunks already read are not repeated.
    if let Some(first) = output.chunk().await.unwrap() {
        collected.extend_from_slice(&first);
    }
    let mut stream = assert_stream(output.into_stream());
    while let Some(chunk) = stream.try_next().await.unwrap() {
        collected.extend_from_slice(&chunk);
    }
    assert_eq!(collected, b"hello world");
    assert!(stream.try_next().await.unwrap().is_none());
    let empty = output_empty(&mut server).await;
    assert!(empty.into_stream().try_next().await.unwrap().is_none());
}

async fn output_empty(server: &mut mockito::ServerGuard) -> GetObjectOutput {
    output(server, 200, &[], "").await
}

#[tokio::test]
async fn into_stream_reports_a_truncated_body() {
    let mut stream = truncated(Duration::ZERO, None).await.into_stream();
    let error = loop {
        match stream.try_next().await {
            Ok(Some(_)) => continue,
            Ok(None) => panic!("truncated body must fail"),
            Err(error) => break error,
        }
    };
    assert_eq!(error.kind(), ErrorKind::Transport, "{error:?}");
    assert_eq!(error.dispatch(), Dispatch::Sent);
}

#[tokio::test]
async fn into_async_read_reads_the_body() {
    let mut server = mockito::Server::new_async().await;
    let output = output(&mut server, 200, &[], "hello world").await;
    let mut reader = assert_reader(output.into_async_read());
    let mut body = String::new();
    reader.read_to_string(&mut body).await.unwrap();
    assert_eq!(body, "hello world");
    assert_eq!(reader.read(&mut [0; 4]).await.unwrap(), 0);
    let mut copied = Vec::new();
    let output = output_empty(&mut server).await;
    let written = tokio::io::copy(&mut output.into_async_read(), &mut copied)
        .await
        .unwrap();
    assert_eq!((written, copied.len()), (0, 0));
}

#[tokio::test]
async fn into_async_read_wraps_sdk_errors_in_io_errors() {
    // (stall before closing, client read timeout, io kind, SDK kind)
    let cases = [
        (
            Duration::ZERO,
            None,
            std::io::ErrorKind::Other,
            ErrorKind::Transport,
        ),
        (
            Duration::from_secs(5),
            Some(Duration::from_millis(100)),
            std::io::ErrorKind::TimedOut,
            ErrorKind::Timeout,
        ),
    ];
    for (stall, read_timeout, io_kind, kind) in cases {
        let mut reader = truncated(stall, read_timeout).await.into_async_read();
        let mut body = Vec::new();
        let error = reader.read_to_end(&mut body).await.unwrap_err();
        assert_eq!(body, b"hello");
        assert_eq!(error.kind(), io_kind, "{error:?}");
        let inner = error
            .into_inner()
            .unwrap()
            .downcast::<fds::Error>()
            .unwrap();
        assert_eq!(inner.kind(), kind, "{inner:?}");
        assert_eq!(inner.dispatch(), Dispatch::Sent);
    }
}
