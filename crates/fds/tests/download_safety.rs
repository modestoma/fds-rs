#![cfg(feature = "manager")]
use fds::{Client, DownloadRequest, Downloader};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Output(PathBuf);
impl Output {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "fds-safety-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn path(&self) -> PathBuf {
        self.0.join("out")
    }
    fn request(&self) -> DownloadRequest {
        DownloadRequest {
            bucket_name: "b".into(),
            object_name: "o".into(),
            file_path: self.path().to_str().unwrap().into(),
            range: None,
        }
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn downloader(
    server: &mockito::ServerGuard,
    part: u64,
    concurrency: usize,
    resume: bool,
) -> Downloader {
    Downloader::new(
        Client::builder()
            .endpoint(server.url())
            .credentials("ak", "sk")
            .build()
            .unwrap(),
        fds::DownloadOptions::builder()
            .part_size(part)
            .concurrency(concurrency)
            .resume(resume)
            .build()
            .unwrap(),
    )
}
async fn head(server: &mut mockito::ServerGuard, size: usize, etag: &str) -> mockito::Mock {
    server
        .mock("HEAD", "/b/o")
        .with_status(200)
        .with_header("content-length", &size.to_string())
        .with_header("etag", etag)
        .create_async()
        .await
}
async fn part(
    server: &mut mockito::ServerGuard,
    start: usize,
    end: usize,
    size: usize,
    body: &str,
) -> mockito::Mock {
    server
        .mock("GET", "/b/o")
        .match_header("range", format!("bytes={start}-{end}").as_str())
        .with_status(206)
        .with_header("etag", "\"v1\"")
        .with_header("content-range", &format!("bytes {start}-{end}/{size}"))
        .with_body(body)
        .create_async()
        .await
}
#[tokio::test]
async fn rejects_invalid_wire_responses_without_replacing_destination() {
    for (status, range, etag, body) in [
        (200, "bytes 0-3/4", "\"v1\"", "abcd"),
        (206, "bytes 1-4/4", "\"v1\"", "abcd"),
        (206, "bytes 0-3/5", "\"v1\"", "abcd"),
        (206, "", "\"v1\"", "abcd"),
        (206, "bytes 0-3/4", "\"v2\"", "abcd"),
        (206, "bytes 0-3/4", "\"v1\"", "ab"),
        (206, "bytes 0-3/4", "\"v1\"", "abcdef"),
    ] {
        let mut server = mockito::Server::new_async().await;
        let _h = head(&mut server, 4, "\"v1\"").await;
        let _p = server
            .mock("GET", "/b/o")
            .with_status(status)
            .with_header("content-range", range)
            .with_header("etag", etag)
            .with_body(body)
            .create_async()
            .await;
        let out = Output::new();
        std::fs::write(out.path(), "original").unwrap();
        assert!(downloader(&server, 4, 2, true)
            .download(&out.request())
            .await
            .is_err());
        assert_eq!(std::fs::read(out.path()).unwrap(), b"original");
        assert!(!out.0.join("out.download.lock").exists());
    }
}
#[tokio::test]
async fn stale_temp_zero_and_range_downloads_have_exact_length() {
    for (size, range, expected) in [
        (4, None, "abcd"),
        (0, None, ""),
        (4, Some("bytes=1-2"), "bc"),
    ] {
        let mut server = mockito::Server::new_async().await;
        let _h = head(&mut server, size, "\"v1\"").await;
        let _p = if size == 0 {
            None
        } else if range.is_some() {
            Some(part(&mut server, 1, 2, 4, "bc").await)
        } else {
            Some(part(&mut server, 0, 3, 4, "abcd").await)
        };
        let out = Output::new();
        std::fs::write(out.0.join("out.tmp"), [b'x'; 100]).unwrap();
        let mut req = out.request();
        req.range = range.map(str::parse).transpose().unwrap();
        downloader(&server, 4, 2, true)
            .download(&req)
            .await
            .unwrap();
        assert_eq!(std::fs::read(out.path()).unwrap(), expected.as_bytes());
    }
}
#[tokio::test]
async fn resume_validates_source_geometry_and_local_bytes() {
    for scenario in ["resume", "corrupt", "source", "geometry", "missing"] {
        let mut server = mockito::Server::new_async().await;
        let mut h = head(&mut server, 8, "\"v1\"").await;
        let first = part(&mut server, 0, 3, 8, "abcd").await;
        let failure = server
            .mock("GET", "/b/o")
            .match_header("range", "bytes=4-7")
            .with_status(500)
            .create_async()
            .await;
        let out = Output::new();
        let dl = downloader(&server, 4, 1, true);
        assert!(dl.download(&out.request()).await.is_err());
        first.assert_async().await;
        failure.remove_async().await;
        let mut cp: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.0.join("out.download.bp")).unwrap()).unwrap();
        match scenario {
            "corrupt" => std::fs::write(out.0.join("out.tmp"), "XXXXXXXX").unwrap(),
            "source" => {
                h.remove_async().await;
                h = head(&mut server, 8, "\"v2\"").await;
            }
            "geometry" => {
                cp["identity"]["part_size"] = 2.into();
                std::fs::write(
                    out.0.join("out.download.bp"),
                    serde_json::to_vec(&cp).unwrap(),
                )
                .unwrap();
            }
            "missing" => std::fs::remove_file(out.0.join("out.tmp")).unwrap(),
            _ => {}
        }
        let tag = if scenario == "source" {
            "\"v2\""
        } else {
            "\"v1\""
        };
        let a = server
            .mock("GET", "/b/o")
            .match_header("range", "bytes=0-3")
            .with_status(206)
            .with_header("content-range", "bytes 0-3/8")
            .with_header("etag", tag)
            .with_body("abcd")
            .expect(if scenario == "resume" { 0 } else { 1 })
            .create_async()
            .await;
        let b = server
            .mock("GET", "/b/o")
            .match_header("range", "bytes=4-7")
            .with_status(206)
            .with_header("content-range", "bytes 4-7/8")
            .with_header("etag", tag)
            .with_body("efgh")
            .create_async()
            .await;
        dl.download(&out.request()).await.unwrap();
        a.assert_async().await;
        b.assert_async().await;
        assert_eq!(std::fs::read(out.path()).unwrap(), b"abcdefgh");
        drop(h);
    }
}
#[tokio::test]
async fn no_strong_validator_restarts_in_one_request() {
    for etag in ["", "W/\"v1\""] {
        let mut server = mockito::Server::new_async().await;
        let _h = head(&mut server, 8, etag).await;
        let p = part(&mut server, 0, 7, 8, "abcdefgh").await;
        let out = Output::new();
        std::fs::write(out.0.join("out.tmp"), "bad").unwrap();
        downloader(&server, 1, 8, true)
            .download(&out.request())
            .await
            .unwrap();
        p.assert_async().await;
        assert_eq!(std::fs::read(out.path()).unwrap(), b"abcdefgh");
    }
}
#[test]
fn invalid_options_fail_at_construction() {
    assert!(fds::DownloadOptions::builder()
        .part_size(0)
        .build()
        .is_err());
    assert!(fds::DownloadOptions::builder()
        .concurrency(0)
        .build()
        .is_err());
    assert!(fds::DownloadOptions::builder()
        .part_size(u64::MAX)
        .build()
        .is_err());
}

// Raw HTTP peer holds one body open to observe cancellation of the actual socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_and_checkpoint_failure_stop_workers_and_preserve_destination() {
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Notify;
    for checkpoint_failure in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let started = Arc::new(Notify::new());
        let finish_first = Arc::new(Notify::new());
        let disconnected = Arc::new(Notify::new());
        let (s, f, d) = (started.clone(), finish_first.clone(), disconnected.clone());
        let server = tokio::spawn(async move {
            let mut handlers = tokio::task::JoinSet::new();
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (s, f, d) = (s.clone(), f.clone(), d.clone());
                handlers.spawn(async move {
                    let mut request=Vec::new();let mut byte=[0u8;1];
                    while !request.ends_with(b"\r\n\r\n") { if socket.read(&mut byte).await.unwrap_or(0)==0 {return;} request.push(byte[0]); }
                    let request=String::from_utf8(request).unwrap();
                    if request.starts_with("HEAD ") {
                        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n").await.unwrap();
                    } else if request.to_ascii_lowercase().contains("range: bytes=0-3") {
                        f.notified().await;
                        let _=socket.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nContent-Range: bytes 0-3/8\r\nETag: \"v1\"\r\nConnection: close\r\n\r\nabcd").await;
                    } else {
                        socket.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nContent-Range: bytes 4-7/8\r\nETag: \"v1\"\r\nConnection: close\r\n\r\ne").await.unwrap();
                        s.notify_one();
                        if socket.read(&mut byte).await.unwrap_or(0)==0 {d.notify_one();}
                    }
                });
            }
        });
        let client = Client::builder()
            .endpoint(format!("http://{address}"))
            .credentials("ak", "sk")
            .build()
            .unwrap();
        let dl = Arc::new(Downloader::new(
            client,
            fds::DownloadOptions::builder()
                .part_size(4)
                .concurrency(2)
                .resume(true)
                .build()
                .unwrap(),
        ));
        let out = Output::new();
        std::fs::write(out.path(), "original").unwrap();
        let req = out.request();
        let worker_dl = dl.clone();
        let task = tokio::spawn(async move { worker_dl.download(&req).await });
        tokio::time::timeout(Duration::from_secs(3), started.notified())
            .await
            .unwrap();
        // The same destination (including a normalized parent alias) is rejected.
        let mut alias = out.request();
        alias.file_path = out.0.join(".").join("out").to_str().unwrap().into();
        assert!(dl.download(&alias).await.is_err());
        if checkpoint_failure {
            std::fs::create_dir(out.0.join("out.download.bp.new")).unwrap();
            finish_first.notify_one();
            assert!(tokio::time::timeout(Duration::from_secs(3), task)
                .await
                .unwrap()
                .unwrap()
                .is_err());
        } else {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        }
        tokio::time::timeout(Duration::from_secs(3), disconnected.notified())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while out.0.join("out.download.lock").exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let before = std::fs::read(out.0.join("out.tmp")).unwrap();
        finish_first.notify_one();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(std::fs::read(out.0.join("out.tmp")).unwrap(), before);
        assert_eq!(std::fs::read(out.path()).unwrap(), b"original");
        server.abort();
        let _ = server.await;
    }
}

#[tokio::test]
async fn chunked_bodies_are_counted_through_eof() {
    for body in ["ab", "abcdef", "abcd"] {
        let mut server = mockito::Server::new_async().await;
        let _h = head(&mut server, 4, "\"v1\"").await;
        let _p = server
            .mock("GET", "/b/o")
            .with_status(206)
            .with_header("etag", "\"v1\"")
            .with_header("content-range", "bytes 0-3/4")
            .with_chunked_body(move |w| w.write_all(body.as_bytes()))
            .create_async()
            .await;
        let out = Output::new();
        std::fs::write(out.path(), "original").unwrap();
        let result = downloader(&server, 4, 1, true)
            .download(&out.request())
            .await;
        assert_eq!(result.is_ok(), body == "abcd");
        assert_eq!(
            std::fs::read(out.path()).unwrap(),
            if body == "abcd" {
                b"abcd".as_slice()
            } else {
                b"original".as_slice()
            }
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn staging_aliases_cannot_modify_an_existing_file() {
    for hard_link in [false, true] {
        let mut server = mockito::Server::new_async().await;
        let _h = head(&mut server, 4, "\"v1\"").await;
        let out = Output::new();
        std::fs::write(out.path(), "original").unwrap();
        if hard_link {
            std::fs::hard_link(out.path(), out.0.join("out.tmp")).unwrap();
        } else {
            std::os::unix::fs::symlink(out.path(), out.0.join("out.tmp")).unwrap();
        }
        assert!(downloader(&server, 4, 1, false)
            .download(&out.request())
            .await
            .is_err());
        assert_eq!(std::fs::read(out.path()).unwrap(), b"original");
    }
}

#[tokio::test]
async fn limiter_controls_part_request_admission() {
    use std::{
        num::NonZeroU32,
        sync::Arc,
        time::{Duration, Instant},
    };
    let mut server = mockito::Server::new_async().await;
    let _h = head(&mut server, 8, "\"v1\"").await;
    let a = part(&mut server, 0, 3, 8, "abcd").await;
    let b = part(&mut server, 4, 7, 8, "efgh").await;
    let out = Output::new();
    let client = Client::builder()
        .endpoint(server.url())
        .credentials("ak", "sk")
        .build()
        .unwrap();
    let options = fds::DownloadOptions::builder()
        .part_size(4)
        .concurrency(2)
        .request_limiter(Arc::new(governor::RateLimiter::direct(
            governor::Quota::per_second(NonZeroU32::new(2).unwrap())
                .allow_burst(NonZeroU32::new(1).unwrap()),
        )));
    let dl = Downloader::new(client, options.build().unwrap());
    let start = Instant::now();
    dl.download(&out.request()).await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(450));
    a.assert_async().await;
    b.assert_async().await;
}

#[tokio::test]
async fn invalid_download_target_precedes_lock_and_network() {
    let mut s = mockito::Server::new_async().await;
    let no_head = s
        .mock("HEAD", mockito::Matcher::Any)
        .expect(0)
        .create_async()
        .await;
    let out = Output::new();
    std::fs::write(out.path(), b"original").unwrap();
    let dl = downloader(&s, 4, 2, true);
    for (bucket, object) in [("", "o"), ("b", "")] {
        let error = DownloadRequest::new(bucket, object, out.path())
            .send(&dl)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            fds::ManagerError::Fds(fds::FdsError::Invalid(_))
        ));
        assert_eq!(std::fs::read(out.path()).unwrap(), b"original");
        assert!(!out.0.join("out.download.lock").exists());
        assert!(!out.0.join("out.tmp").exists());
    }
    no_head.assert_async().await;
}

#[cfg(unix)]
#[tokio::test]
async fn path_destination_supports_typed_range() {
    let mut s = mockito::Server::new_async().await;
    let h = head(&mut s, 4, "\"v1\"").await;
    let p = part(&mut s, 1, 2, 4, "bc").await;
    let out = Output::new();
    let path = out.0.join("下载 output");
    DownloadRequest::new("b", "o", &path)
        .range(fds::ByteRange::inclusive(1, 2).unwrap())
        .send(&downloader(&s, 4, 2, true))
        .await
        .unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"bc");
    h.assert_async().await;
    p.assert_async().await;
}

#[cfg(unix)]
#[test]
fn request_preserves_non_utf8_path_without_lossy_conversion() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let path = std::ffi::OsString::from_vec(vec![b'o', 0xff]);
    let request = DownloadRequest::new("b", "o", PathBuf::from(path));
    assert_eq!(request.file_path.as_os_str().as_bytes(), &[b'o', 0xff]);
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

/// Serves `parts` contiguous parts of `body`; `fail` makes that part return 500.
async fn serve_parts(
    server: &mut mockito::ServerGuard,
    body: &[u8],
    part_size: usize,
    fail: Option<usize>,
) -> Vec<mockito::Mock> {
    let mut mocks = Vec::new();
    for (index, chunk) in body.chunks(part_size).enumerate() {
        let start = index * part_size;
        let end = start + chunk.len() - 1;
        let mock = server
            .mock("GET", "/b/o")
            .match_header("range", format!("bytes={start}-{end}").as_str());
        let mock = if fail == Some(index) {
            mock.with_status(500)
        } else {
            mock.with_status(206)
                .with_header("etag", "\"v1\"")
                .with_header(
                    "content-range",
                    &format!("bytes {start}-{end}/{}", body.len()),
                )
                .with_body(chunk)
        };
        mocks.push(mock.create_async().await);
    }
    mocks
}

/// Runs `future` while a sibling task on the same current_thread runtime ticks
/// every 2 ms; returns the future's output and the largest observed tick gap.
async fn with_ticker<F: std::future::Future>(future: F) -> (F::Output, std::time::Duration) {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    let done = Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    let ticker = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(2));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last = Instant::now();
        let mut max = Duration::ZERO;
        while !flag.load(Ordering::Relaxed) {
            interval.tick().await;
            let now = Instant::now();
            max = max.max(now - last);
            last = now;
        }
        max
    });
    // Let the ticker start before the measured work begins.
    tokio::task::yield_now().await;
    let output = future.await;
    done.store(true, Ordering::Relaxed);
    (output, ticker.await.unwrap())
}

// AC-2: the Downloader must not stall a current_thread runtime with file IO,
// fsync or resume hashing. 32 x 1 MiB parts: the first run fails on the last
// part (fresh writes + checkpoints), the second resumes (hashes ~31 MiB of
// local data, writes the rest, fsyncs and renames).
#[tokio::test(flavor = "current_thread")]
async fn current_thread_runtime_keeps_ticking_during_download_and_resume() {
    use std::time::Duration;
    const PART: usize = 1024 * 1024;
    const PARTS: usize = 32;
    let body = pattern(PART * PARTS);
    let mut server = mockito::Server::new_async().await;
    let _h = head(&mut server, body.len(), "\"v1\"").await;
    let failing = serve_parts(&mut server, &body, PART, Some(PARTS - 1)).await;
    let out = Output::new();
    let dl = downloader(&server, PART as u64, 4, true);
    let (first, first_gap) = with_ticker(dl.download(&out.request())).await;
    assert!(first.is_err());
    assert!(out.0.join("out.download.bp").exists());
    for mock in failing {
        mock.remove_async().await;
    }
    let _ok = serve_parts(&mut server, &body, PART, None).await;
    let (second, second_gap) = with_ticker(dl.download(&out.request())).await;
    second.unwrap();
    assert!(std::fs::read(out.path()).unwrap() == body);
    // Threshold: with all file IO in spawn_blocking the ticker is delayed only
    // by HTTP parsing and buffer copies; measured max gap is ~3 ms (tick period
    // 2 ms) in debug builds. The original implementation measured 58 ms (first
    // run, fsync per part) and 335 ms (resume, synchronous MD5 of ~31 MiB) on
    // the same machine. 100 ms is >30x the expected gap, so scheduler noise on
    // a loaded host does not trip it, while the resume stall still exceeds it
    // by >3x.
    let limit = Duration::from_millis(100);
    eprintln!("max tick gap: first run {first_gap:?}, resume {second_gap:?}");
    assert!(
        first_gap < limit,
        "first run stalled ticks for {first_gap:?}"
    );
    assert!(
        second_gap < limit,
        "resume stalled ticks for {second_gap:?}"
    );
}

// Checkpoints are throttled, but the final write on failure records every
// completed part, so a resumed download fetches exactly the missing ones.
#[tokio::test]
async fn throttled_checkpoint_still_resumes_exactly_the_missing_parts() {
    const PARTS: usize = 12;
    const FAIL: usize = 8;
    let body = pattern(4 * PARTS);
    let mut server = mockito::Server::new_async().await;
    let _h = head(&mut server, body.len(), "\"v1\"").await;
    let failing = serve_parts(&mut server, &body, 4, Some(FAIL)).await;
    let out = Output::new();
    // concurrency 1 makes the set of completed parts deterministic: 0..FAIL.
    let dl = downloader(&server, 4, 1, true);
    assert!(dl.download(&out.request()).await.is_err());
    assert!(!out.0.join("out.download.lock").exists());
    let cp: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.0.join("out.download.bp")).unwrap()).unwrap();
    let hashes = cp["hashes"].as_array().unwrap();
    assert_eq!(hashes.len(), PARTS);
    for (index, hash) in hashes.iter().enumerate() {
        assert_eq!(hash.is_string(), index < FAIL, "part {index}: {hash}");
    }
    for mock in failing {
        mock.remove_async().await;
    }
    let mut resumed = Vec::new();
    for (index, chunk) in body.chunks(4).enumerate() {
        let (start, end) = (index * 4, index * 4 + 3);
        resumed.push(
            server
                .mock("GET", "/b/o")
                .match_header("range", format!("bytes={start}-{end}").as_str())
                .with_status(206)
                .with_header("etag", "\"v1\"")
                .with_header(
                    "content-range",
                    &format!("bytes {start}-{end}/{}", body.len()),
                )
                .with_body(chunk)
                .expect(usize::from(index >= FAIL))
                .create_async()
                .await,
        );
    }
    dl.download(&out.request()).await.unwrap();
    for mock in &resumed {
        mock.assert_async().await;
    }
    assert_eq!(std::fs::read(out.path()).unwrap(), body);
    assert!(!out.0.join("out.download.bp").exists());
    assert!(!out.0.join("out.download.lock").exists());
}
