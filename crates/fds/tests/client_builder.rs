//! Local transport checks; no live FDS credentials or service are used.
use fds::Client;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn builder() -> fds::ClientBuilder {
    Client::builder()
        .endpoint("http://127.0.0.1:1")
        .credentials("AKID", "secret-value")
}

#[test]
fn origins_and_credentials_are_validated_without_networking() {
    for origin in [
        "",
        "example.com",
        "ftp://example.com",
        "https://",
        "https://user:secret@example.com",
        "https://@example.com",
        "https://example.com?token=secret",
        "https://example.com#fragment",
        "https://example.com/prefix",
        "https://example.com/a/..",
        "https://example.com/./",
        " https://example.com",
        "https://example.com\\path",
        "http://localhost:0",
        "http://localhost:99999",
    ] {
        assert!(builder().endpoint(origin).build().is_err(), "{origin}");
        assert!(
            builder().cdn_endpoint(origin).build().is_err(),
            "CDN {origin}"
        );
    }
    for origin in [
        "http://localhost:8080",
        "http://[::1]:8080/",
        "https://example.com/",
    ] {
        assert!(builder().endpoint(origin).build().is_ok(), "{origin}");
    }
    assert!(Client::builder().credentials("ak", "sk").build().is_err());
    assert!(Client::builder()
        .endpoint("https://example.com")
        .build()
        .is_err());
    for (id, secret) in [
        ("", "sk"),
        ("ak", ""),
        ("a:b", "sk"),
        ("a\nb", "sk"),
        ("a b", "sk"),
    ] {
        assert!(builder().credentials(id, secret).build().is_err());
    }
    let debug = format!(
        "{:?}",
        builder().endpoint("http://user:secret-value@example.com")
    );
    assert!(!debug.contains("secret-value"));
    let client = builder().build().unwrap();
    assert!(!format!("{client:?}").contains("secret-value"));
    assert!(!format!("{client:?}").contains("AKID"));
    fn send_sync_clone<T: Send + Sync + Clone>() {}
    send_sync_clone::<Client>();
}

#[test]
fn limits_and_effective_configuration() {
    let defaults = builder().build().unwrap();
    let conf = defaults.configuration();
    assert_eq!(conf.connect_timeout(), Duration::from_secs(50));
    assert_eq!(conf.request_timeout(), None);
    assert_eq!(conf.read_timeout(), Duration::from_secs(50));
    assert_eq!(conf.pool_idle_timeout(), Duration::from_secs(90));
    assert_eq!(conf.tcp_keepalive(), Duration::from_secs(30));
    assert_eq!(conf.pool_max_idle_per_host(), 100);
    assert_eq!(
        conf.user_agent(),
        concat!("fds-rs/", env!("CARGO_PKG_VERSION"))
    );
    assert!(!conf.enable_md5_calculate());
    for suffix in ["", "  ", "app\n", "app\u{7f}", "应用/1"] {
        let error = builder().user_agent_suffix(suffix).build().unwrap_err();
        assert_eq!(error.kind(), fds::ErrorKind::ConfigInvalid);
    }
    for duration in [Duration::ZERO, Duration::MAX] {
        for b in [
            builder().connect_timeout(duration),
            builder().request_timeout(duration),
            builder().read_timeout(duration),
            builder().pool_idle_timeout(duration),
            builder().tcp_keepalive(duration),
        ] {
            assert!(b.build().is_err());
        }
    }
    for d in [
        Duration::from_millis(1),
        Duration::from_millis(1500),
        Duration::from_secs(32768),
    ] {
        assert!(builder().tcp_keepalive(d).build().is_err());
    }
    let d = Duration::from_secs(7);
    let c = builder()
        .connect_timeout(d)
        .request_timeout(d)
        .read_timeout(d)
        .pool_idle_timeout(d)
        .tcp_keepalive(d)
        .pool_max_idle_per_host(0)
        .user_agent_suffix("my-app/1.2 (ops)")
        .enable_md5_calculate(true)
        .build()
        .unwrap();
    let conf = c.configuration();
    assert_eq!(conf.pool_max_idle_per_host(), 0);
    assert_eq!(
        conf.user_agent(),
        concat!("fds-rs/", env!("CARGO_PKG_VERSION"), " my-app/1.2 (ops)")
    );
    assert_eq!(conf.connect_timeout(), d);
    assert_eq!(conf.request_timeout(), Some(d));
    assert_eq!(conf.read_timeout(), d);
    assert_eq!(conf.pool_idle_timeout(), d);
    assert_eq!(conf.tcp_keepalive(), d);
    assert!(conf.enable_md5_calculate());
    assert!(conf.cdn_endpoint().is_none());
    let cloned = c.clone();
    assert!(std::ptr::eq(c.configuration(), cloned.configuration()));
    #[cfg(feature = "cslb")]
    {
        assert!(builder().cslb_max_nodes(0).build().is_err());
        assert_eq!(
            builder()
                .cslb_max_nodes(3)
                .build()
                .unwrap()
                .configuration()
                .cslb_max_nodes(),
            3
        );
    }
}

/// With `cslb`, a hostname endpoint is resolved by the internal CSLB resolver
/// (IP literals bypass DNS). The server listens on 127.0.0.1 only, so this also
/// covers falling back from a refused `::1` to the next resolved node.
#[cfg(feature = "cslb")]
#[tokio::test]
async fn cslb_client_resolves_hostname_endpoints() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("HEAD", "/b/o")
        .with_header("content-length", "5")
        .expect(3)
        .create_async()
        .await;
    let port = server.socket_address().port();
    let c = builder()
        .endpoint(format!("http://localhost:{port}"))
        .build()
        .unwrap();
    for _ in 0..3 {
        let md = c.head_object("b", "o").send().await.unwrap().unwrap();
        assert_eq!(md.get("content-length"), Some("5"));
    }
    m.assert_async().await;
}

#[tokio::test]
async fn explicit_cdn_and_md5_keep_wire_behavior() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("PUT", "/b/o")
        .match_header("content-md5", "5d41402abc4b2a76b9719d911017c592")
        .with_body(r#"{"bucketName":"b","objectName":"o"}"#)
        .create_async()
        .await;
    let c = builder()
        .endpoint(server.url())
        .cdn_endpoint("https://cdn.example.com")
        .enable_md5_calculate(true)
        .build()
        .unwrap();
    c.put_object("b", "o", b"hello".to_vec())
        .send()
        .await
        .unwrap();
    m.assert_async().await;
    let presign = |client: &Client| {
        client
            .presign_object("b", "a/b +")
            .expires_at(chrono::Utc::now())
            .cdn()
            .build()
    };
    assert!(presign(&c)
        .unwrap()
        .starts_with("https://cdn.example.com/b/a%2Fb%20%2B?"));
    assert!(presign(&builder().build().unwrap()).is_err());
}

async fn stalled_server(tls: bool, headers: bool) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "{}://{}",
        if tls { "https" } else { "http" },
        listener.local_addr().unwrap()
    );
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0; 8192];
        assert!(
            socket.read(&mut buf).await.unwrap() > 0,
            "peer closed before sending request bytes"
        );
        if headers {
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx")
                .await
                .unwrap();
        }
        std::future::pending::<()>().await;
    });
    (endpoint, task)
}

#[tokio::test]
async fn connect_timeout_enforces_tls_handshake_deadline() {
    let (endpoint, server) = stalled_server(true, false).await;
    let c = builder()
        .endpoint(endpoint)
        .connect_timeout(Duration::from_millis(100))
        .request_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let e = tokio::time::timeout(Duration::from_secs(3), c.does_bucket_exist("b"))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(e.kind(), fds::ErrorKind::Timeout, "{e:?}");
    assert_eq!(e.dispatch(), fds::Dispatch::NotSent);
    server.abort();
}

#[tokio::test]
async fn read_timeout_enforces_header_and_stream_body_stalls() {
    for headers in [false, true] {
        let (endpoint, server) = stalled_server(false, headers).await;
        // Default configuration: no total deadline, only the read idle timeout.
        let c = builder()
            .endpoint(endpoint)
            .read_timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        assert_eq!(c.configuration().request_timeout(), None);
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            let resp = c.get_object("b", "o").send().await?;
            resp.bytes().await
        })
        .await
        .unwrap();
        assert!(result.unwrap_err().kind() == fds::ErrorKind::Timeout);
        server.abort();
    }
}

/// Shortened stand-in for a total deadline. Every slow body below takes well
/// over this long while making steady progress, far under the read timeout.
const THRESHOLD: Duration = Duration::from_millis(200);
const SLOW_BODY_LEN: usize = 12;

/// Serve one request with a body of `SLOW_BODY_LEN` bytes sent one byte every
/// 40ms (about 480ms in total, i.e. more than twice `THRESHOLD`).
async fn slow_body_server() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        socket.set_nodelay(true).unwrap();
        let mut buf = [0; 8192];
        assert!(
            socket.read(&mut buf).await.unwrap() > 0,
            "peer closed before sending request bytes"
        );
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {SLOW_BODY_LEN}\r\n\r\n");
        if socket.write_all(head.as_bytes()).await.is_err() {
            return;
        }
        for _ in 0..SLOW_BODY_LEN {
            tokio::time::sleep(Duration::from_millis(40)).await;
            if socket.write_all(b"x").await.is_err() {
                return;
            }
        }
    });
    (endpoint, task)
}

/// GET from a fresh slow-body server and read the whole body, guarded by an
/// outer deadline so a regression cannot hang the test suite.
async fn get_slow_body(
    client_builder: fds::ClientBuilder,
    timeout: Option<Duration>,
) -> (fds::Result<bytes::Bytes>, Duration) {
    let (endpoint, server) = slow_body_server().await;
    let c = client_builder
        .endpoint(endpoint)
        .read_timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut request = c.get_object("b", "o");
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        request.send().await?.bytes().await
    })
    .await
    .expect("outer guard elapsed");
    let elapsed = started.elapsed();
    server.abort();
    (result, elapsed)
}

fn assert_timeout(result: fds::Result<bytes::Bytes>) {
    let e = result.unwrap_err();
    assert!(
        e.kind() == fds::ErrorKind::Timeout,
        "expected a timeout, got {e:?}"
    );
}

#[tokio::test]
async fn default_client_reads_slow_body_beyond_total_threshold() {
    let (result, elapsed) = get_slow_body(builder(), None).await;
    assert_eq!(result.unwrap().len(), SLOW_BODY_LEN);
    assert!(elapsed > THRESHOLD, "body finished too fast: {elapsed:?}");
}

#[tokio::test]
async fn total_timeout_expires_despite_continuing_body_progress() {
    let (result, _) = get_slow_body(builder().request_timeout(THRESHOLD), None).await;
    assert_timeout(result);
}

#[tokio::test]
async fn per_request_timeout_overrides_client_configuration() {
    // Applies on a default client, which has no total deadline of its own.
    let (result, _) = get_slow_body(builder(), Some(THRESHOLD)).await;
    assert_timeout(result);
    // A shorter per-request deadline wins over a longer client deadline.
    let (result, _) = get_slow_body(
        builder().request_timeout(Duration::from_secs(30)),
        Some(THRESHOLD),
    )
    .await;
    assert_timeout(result);
    // A longer per-request deadline also wins over a shorter client deadline.
    let (result, elapsed) = get_slow_body(
        builder().request_timeout(THRESHOLD),
        Some(Duration::from_secs(30)),
    )
    .await;
    assert_eq!(result.unwrap().len(), SLOW_BODY_LEN);
    assert!(elapsed > THRESHOLD, "body finished too fast: {elapsed:?}");
}

#[tokio::test]
async fn put_object_request_timeout_applies_while_awaiting_response() {
    let (endpoint, server) = stalled_server(false, false).await;
    let c = builder()
        .endpoint(endpoint)
        .read_timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        c.put_object("b", "o", b"hello".to_vec())
            .timeout(THRESHOLD)
            .send(),
    )
    .await
    .expect("outer guard elapsed");
    let e = result.unwrap_err();
    assert!(
        e.kind() == fds::ErrorKind::Timeout,
        "expected a timeout, got {e:?}"
    );
    server.abort();
}

/// How the upload server below treats the request body.
#[derive(Clone, Copy)]
enum Upload {
    /// Read `UPLOAD_STEP` bytes every `UPLOAD_TICK`, then answer after
    /// `UPLOAD_ANSWER_DELAY`.
    Steady,
    /// Read `STALL_AFTER` bytes, then stop reading without answering.
    Stall,
    /// Read the whole body as fast as possible, then never answer.
    NoAnswer,
}

/// Upload sizes: large enough that kernel socket buffers plus the transport's
/// own buffer cannot absorb a meaningful part of the body.
const UPLOAD_LEN: usize = 16 << 20;
const UPLOAD_STEP: usize = 1 << 20;
const UPLOAD_TICK: Duration = Duration::from_millis(50);
const UPLOAD_ANSWER_DELAY: Duration = Duration::from_millis(100);
const STALL_AFTER: usize = 256 << 10;

/// Serve one upload. The listener's small receive buffer is inherited by the
/// accepted socket, so the client feels the server's reading pace quickly.
async fn upload_server(mode: Upload) -> (String, tokio::task::JoinHandle<()>) {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(16 << 10).unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = socket.listen(1).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        let mut byte = [0; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                return;
            }
            head.push(byte[0]);
        }
        let head = String::from_utf8(head).unwrap().to_ascii_lowercase();
        let length: usize = head
            .lines()
            .find_map(|line| line.strip_prefix("content-length: "))
            .expect("upload carries a content length")
            .trim()
            .parse()
            .unwrap();
        let mut buf = vec![0; 64 << 10];
        let mut read = 0;
        match mode {
            Upload::Steady => {
                let mut target = 0;
                while target < length {
                    tokio::time::sleep(UPLOAD_TICK).await;
                    target = (target + UPLOAD_STEP).min(length);
                    if !read_up_to(&mut socket, &mut buf, &mut read, target).await {
                        return;
                    }
                }
            }
            Upload::Stall => {
                read_up_to(&mut socket, &mut buf, &mut read, STALL_AFTER.min(length)).await;
                std::future::pending::<()>().await;
            }
            Upload::NoAnswer => {
                read_up_to(&mut socket, &mut buf, &mut read, length).await;
                std::future::pending::<()>().await;
            }
        }
        tokio::time::sleep(UPLOAD_ANSWER_DELAY).await;
        let receipt = r#"{"bucketName":"b","objectName":"o"}"#;
        let answer = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{receipt}",
            receipt.len()
        );
        let _ = socket.write_all(answer.as_bytes()).await;
    });
    (endpoint, task)
}

/// Read from `socket` until `read` reaches `limit`; false once the peer is gone.
async fn read_up_to(
    socket: &mut tokio::net::TcpStream,
    buf: &mut [u8],
    read: &mut usize,
    limit: usize,
) -> bool {
    while *read < limit {
        let want = buf.len().min(limit - *read);
        match socket.read(&mut buf[..want]).await {
            Ok(0) | Err(_) => return false,
            Ok(n) => *read += n,
        }
    }
    true
}

/// PUT `body` to a fresh upload server with `THRESHOLD` as the idle timeout,
/// guarded by an outer deadline so a regression cannot hang the suite.
async fn put_with_idle_threshold(
    mode: Upload,
    body: fds::object::UploadBody,
) -> (fds::Result<fds::object::PutObjectResponse>, Duration) {
    let (endpoint, server) = upload_server(mode).await;
    let c = builder()
        .endpoint(endpoint)
        .read_timeout(THRESHOLD)
        .build()
        .unwrap();
    assert_eq!(c.configuration().request_timeout(), None);
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(10), c.put_object("b", "o", body).send())
        .await
        .expect("outer guard elapsed");
    let elapsed = started.elapsed();
    server.abort();
    (result, elapsed)
}

#[tokio::test]
async fn progressing_upload_outlasts_idle_threshold() {
    // One in-memory chunk: progress must still be visible while it drains.
    let (result, elapsed) =
        put_with_idle_threshold(Upload::Steady, vec![7; UPLOAD_LEN].into()).await;
    let receipt = result.expect("a progressing upload must not time out");
    assert_eq!(receipt.object_name, "o");
    // The upload plus the answer delay took several idle periods in total.
    assert!(
        elapsed > THRESHOLD * 2,
        "upload finished too fast to prove anything: {elapsed:?}"
    );
}

#[tokio::test]
async fn stalled_upload_times_out_with_unknown_dispatch() {
    let chunks = (0..UPLOAD_LEN / UPLOAD_STEP)
        .map(|_| Ok::<_, std::io::Error>(bytes::Bytes::from(vec![7; UPLOAD_STEP])));
    let body =
        fds::object::UploadBody::from_stream(futures_util::stream::iter(chunks), UPLOAD_LEN as u64);
    let (result, elapsed) = put_with_idle_threshold(Upload::Stall, body).await;
    let e = result.unwrap_err();
    assert_eq!(e.kind(), fds::ErrorKind::Timeout, "{e:?}");
    assert_eq!(e.dispatch(), fds::Dispatch::Unknown, "{e:?}");
    assert!(elapsed >= THRESHOLD, "timed out too early: {elapsed:?}");
}

#[tokio::test]
async fn missing_answer_after_upload_times_out() {
    let (result, elapsed) =
        put_with_idle_threshold(Upload::NoAnswer, vec![7; UPLOAD_STEP].into()).await;
    let e = result.unwrap_err();
    assert_eq!(e.kind(), fds::ErrorKind::Timeout, "{e:?}");
    assert_eq!(e.dispatch(), fds::Dispatch::Unknown, "{e:?}");
    assert!(elapsed >= THRESHOLD, "timed out too early: {elapsed:?}");
}

#[tokio::test]
async fn per_request_timeout_is_validated() {
    let c = builder().build().unwrap();
    for duration in [Duration::ZERO, Duration::MAX] {
        let e = c
            .get_object("b", "o")
            .timeout(duration)
            .send()
            .await
            .unwrap_err();
        assert_eq!(e.kind(), fds::ErrorKind::InvalidInput, "{e:?}");
        let e = c
            .put_object("b", "o", b"x".to_vec())
            .timeout(duration)
            .send()
            .await
            .unwrap_err();
        assert_eq!(e.kind(), fds::ErrorKind::InvalidInput, "{e:?}");
    }
}

#[tokio::test]
async fn cloned_clients_reuse_pool_and_expired_idle_connections_are_replaced() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let accepted = count.clone();
    let server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            accepted.fetch_add(1, Ordering::SeqCst);
            connections.spawn(async move {
                let mut buf = [0; 1];
                loop {
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        if socket.read(&mut buf).await.unwrap_or(0) == 0 {
                            return;
                        }
                        request.push(buf[0]);
                    }
                    if socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    let client = builder()
        .endpoint(endpoint)
        .pool_idle_timeout(Duration::from_millis(300))
        .build()
        .unwrap();
    assert!(client.does_bucket_exist("b").await.unwrap());
    assert!(client.clone().does_bucket_exist("b").await.unwrap());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    tokio::time::sleep(Duration::from_millis(650)).await;
    assert!(client.clone().does_bucket_exist("b").await.unwrap());
    assert_eq!(count.load(Ordering::SeqCst), 2);
    server.abort();
}
