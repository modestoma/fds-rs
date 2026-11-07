//! Credentials providers: per-attempt credentials, storage-token refresh,
//! single flight, exchange failures, presigning and redaction.
use chrono::{Duration, Utc};
use fds::credentials::{
    Credentials, CredentialsFuture, CredentialsProvider, StaticCredentials, StorageTokenProvider,
};
use fds::oauth::{ExchangeStorageToken, FdsServiceClient, OAuthProvider, StorageAccessToken};
use fds::{Client, Dispatch, ErrorKind, RetryPolicy};
use mockito::{Matcher, Mock, Server, ServerGuard};
use std::error::Error as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn exchange(server: &ServerGuard) -> ExchangeStorageToken {
    FdsServiceClient::new(server.url())
        .unwrap()
        .exchange_storage_token("app", "oauth-app", "oauth-secret", OAuthProvider::QQ)
}

fn token_body(token: &str, lifetime: Duration) -> String {
    serde_json::json!({"token": token, "expireTime": (Utc::now() + lifetime).timestamp_millis()})
        .to_string()
}

/// The exchange request: GET `/` with an empty `storageAccessToken`.
fn exchange_mock(server: &mut ServerGuard) -> Mock {
    server
        .mock("GET", "/")
        .match_query(Matcher::UrlEncoded("storageAccessToken".into(), "".into()))
}

/// A business GET authenticated with `token`.
async fn object_mock(server: &mut ServerGuard, token: &str, hits: usize) -> Mock {
    server
        .mock("GET", "/b/o")
        .match_header("authorization", "OAuth")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("appId".into(), "app".into()),
            Matcher::UrlEncoded("storageAccessToken".into(), token.into()),
        ]))
        .with_body("content")
        .expect(hits)
        .create_async()
        .await
}

/// Exchange one token with `lifetime` through a temporary mock.
async fn exchanged(
    server: &mut ServerGuard,
    token: &str,
    lifetime: Duration,
) -> StorageAccessToken {
    let mock = exchange_mock(server)
        .with_body(token_body(token, lifetime))
        .expect(1)
        .create_async()
        .await;
    let token = exchange(server).send().await.unwrap();
    mock.assert_async().await;
    mock.remove_async().await;
    token
}

fn client(server: &ServerGuard, provider: impl CredentialsProvider + 'static) -> Client {
    Client::builder()
        .endpoint(server.url())
        .credentials_provider(provider)
        .build()
        .unwrap()
}

async fn get(client: &Client) -> fds::Result<bytes::Bytes> {
    client.get_object("b", "o").send().await?.bytes().await
}

#[tokio::test]
async fn token_close_to_expiry_is_exchanged_before_the_next_request() {
    let mut server = Server::new_async().await;
    let old = exchanged(&mut server, "old-token", Duration::minutes(2)).await;
    let refresh = exchange_mock(&mut server)
        .with_body(token_body("new-token", Duration::days(7)))
        .expect(1)
        .create_async()
        .await;
    let old_requests = object_mock(&mut server, "old-token", 0).await;
    let new_requests = object_mock(&mut server, "new-token", 2).await;
    // Two minutes left is within the default five-minute margin.
    let client = client(
        &server,
        StorageTokenProvider::new(exchange(&server)).with_token(old),
    );
    let clone = client.clone();
    assert_eq!(get(&client).await.unwrap(), "content");
    // The refreshed token is shared by clones and reused without exchanging.
    assert_eq!(get(&clone).await.unwrap(), "content");
    refresh.assert_async().await;
    new_requests.assert_async().await;
    old_requests.assert_async().await;
}

#[tokio::test]
async fn token_outside_the_margin_is_reused_and_margin_is_configurable() {
    let mut server = Server::new_async().await;
    let token = exchanged(&mut server, "token", Duration::minutes(30)).await;
    let no_exchange = exchange_mock(&mut server).expect(0).create_async().await;
    let requests = object_mock(&mut server, "token", 2).await;
    let provider = StorageTokenProvider::new(exchange(&server)).with_token(token.clone());
    let client = client(&server, provider);
    get(&client).await.unwrap();
    get(&client).await.unwrap();
    requests.assert_async().await;
    no_exchange.assert_async().await;
    no_exchange.remove_async().await;

    // The same token is due under a one-hour margin.
    let refresh = exchange_mock(&mut server)
        .with_body(token_body("long-token", Duration::days(7)))
        .expect(1)
        .create_async()
        .await;
    let refreshed = object_mock(&mut server, "long-token", 1).await;
    let provider = StorageTokenProvider::new(exchange(&server))
        .with_token(token)
        .refresh_margin(std::time::Duration::from_secs(3600));
    get(&self::client(&server, provider)).await.unwrap();
    refresh.assert_async().await;
    refreshed.assert_async().await;
}

#[tokio::test]
async fn first_request_exchanges_and_short_tokens_refresh_at_half_lifetime() {
    let mut server = Server::new_async().await;
    let count = Arc::new(AtomicUsize::new(0));
    let counter = count.clone();
    let exchange_requests = exchange_mock(&mut server)
        .with_body_from_request(move |_| {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            token_body(&format!("token-{n}"), Duration::seconds(2)).into_bytes()
        })
        .expect(2)
        .create_async()
        .await;
    let first = object_mock(&mut server, "token-1", 2).await;
    let second = object_mock(&mut server, "token-2", 1).await;
    let client = client(&server, StorageTokenProvider::new(exchange(&server)));
    // A two-second token is not due for a second, although the default
    // margin is five minutes: the margin is capped at half the lifetime.
    get(&client).await.unwrap();
    get(&client).await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    get(&client).await.unwrap();
    exchange_requests.assert_async().await;
    first.assert_async().await;
    second.assert_async().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_share_one_exchange() {
    const N: usize = 16;
    let mut server = Server::new_async().await;
    let old = exchanged(&mut server, "old-token", Duration::seconds(30)).await;
    let body = token_body("new-token", Duration::days(7));
    let refresh = exchange_mock(&mut server)
        .with_chunked_body(move |w| {
            // Keep the exchange in flight while every request arrives.
            std::thread::sleep(std::time::Duration::from_millis(300));
            w.write_all(body.as_bytes())
        })
        .expect(1)
        .create_async()
        .await;
    let old_requests = object_mock(&mut server, "old-token", 0).await;
    let new_requests = object_mock(&mut server, "new-token", N).await;
    let provider = StorageTokenProvider::new(exchange(&server)).with_token(old);
    // Two clients built from clones of one provider share its cache.
    let clients = [client(&server, provider.clone()), client(&server, provider)];
    let tasks: Vec<_> = (0..N)
        .map(|i| {
            let client = clients[i % 2].clone();
            tokio::spawn(async move { get(&client).await })
        })
        .collect();
    for task in tasks {
        assert_eq!(task.await.unwrap().unwrap(), "content");
    }
    refresh.assert_async().await;
    new_requests.assert_async().await;
    old_requests.assert_async().await;
}

#[tokio::test]
async fn failed_first_exchange_is_unauthenticated_and_nothing_is_sent() {
    for status in [401, 403, 503] {
        let mut server = Server::new_async().await;
        let failure = exchange_mock(&mut server)
            .with_status(status)
            .with_body("{\"code\":\"oauth-secret\"}")
            .expect(2)
            .create_async()
            .await;
        let requests = server
            .mock("GET", "/b/o")
            .match_query(Matcher::Any)
            .expect(0)
            .create_async()
            .await;
        let client = client(&server, StorageTokenProvider::new(exchange(&server)));
        for _ in 0..2 {
            // Failures are not cached: each request tries to exchange again.
            let error = get(&client).await.unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Unauthenticated, "{error:?}");
            assert_eq!(error.dispatch(), Dispatch::NotSent);
            let source = error.source().expect("the exchange error is the source");
            assert!(source.to_string().contains("FDS rejected the request"));
            assert!(!format!("{error} {error:?}").contains("oauth-secret"));
        }
        failure.assert_async().await;
        requests.assert_async().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_refresh_of_an_expired_token_never_sends_it() {
    let mut server = Server::new_async().await;
    let first = exchange_mock(&mut server)
        .with_body(token_body("short-token", Duration::seconds(1)))
        .expect(1)
        .create_async()
        .await;
    // The only business request the service may see.
    let requests = object_mock(&mut server, "short-token", 1).await;
    let client = client(&server, StorageTokenProvider::new(exchange(&server)));
    get(&client).await.unwrap();
    first.assert_async().await;
    first.remove_async().await;

    let failure = exchange_mock(&mut server)
        .with_status(503)
        .with_chunked_body(|w| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            w.write_all(b"unavailable")
        })
        .expect(1)
        .create_async()
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let tasks: Vec<_> = (0..4)
        .map(|_| {
            let client = client.clone();
            tokio::spawn(async move { get(&client).await })
        })
        .collect();
    for task in tasks {
        let error = task.await.unwrap().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::CredentialsExpired, "{error:?}");
        assert_eq!(error.dispatch(), Dispatch::NotSent);
        assert!(!format!("{error} {error:?}").contains("short-token"));
    }
    // One shared, failed exchange; the expired token was never sent.
    failure.assert_async().await;
    requests.assert_async().await;
}

/// Returns `credentials[n]` on the n-th call (the last one afterwards).
struct Sequence {
    credentials: Vec<Credentials>,
    calls: AtomicUsize,
}
impl Sequence {
    fn new(credentials: Vec<Credentials>) -> Arc<Self> {
        Arc::new(Self {
            credentials,
            calls: AtomicUsize::new(0),
        })
    }
}
impl CredentialsProvider for Sequence {
    fn provide_credentials(&self) -> CredentialsFuture<'_> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let credentials = self.credentials[n.min(self.credentials.len() - 1)].clone();
        Box::pin(async move { Ok(credentials) })
    }
}

fn retrying(server: &ServerGuard, provider: impl CredentialsProvider + 'static) -> Client {
    Client::builder()
        .endpoint(server.url())
        .credentials_provider(provider)
        .retry_policy(
            RetryPolicy::new(
                1,
                std::time::Duration::from_millis(1),
                std::time::Duration::from_millis(2),
            )
            .unwrap(),
        )
        .build()
        .unwrap()
}

#[tokio::test]
async fn every_attempt_signs_with_the_credentials_current_at_that_attempt() {
    let mut server = Server::new_async().await;
    let first = server
        .mock("GET", "/b/o")
        .match_header("authorization", Matcher::Regex("^Galaxy-V2 ak-1:".into()))
        .match_header("date", Matcher::Any)
        .with_status(503)
        .expect(1)
        .create_async()
        .await;
    let second = server
        .mock("GET", "/b/o")
        .match_header("authorization", Matcher::Regex("^Galaxy-V2 ak-2:".into()))
        .with_body("content")
        .expect(1)
        .create_async()
        .await;
    let provider = Sequence::new(vec![
        Credentials::access_key("ak-1", "sk-1"),
        Credentials::access_key("ak-2", "sk-2"),
    ]);
    assert_eq!(
        get(&retrying(&server, provider.clone())).await.unwrap(),
        "content"
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    first.assert_async().await;
    second.assert_async().await;

    // A token is in the URL, which is rebuilt for the retry.
    let mut server = Server::new_async().await;
    let tokens = [
        exchanged(&mut server, "token-1", Duration::days(1)).await,
        exchanged(&mut server, "token-2", Duration::days(1)).await,
    ];
    let first = server
        .mock("GET", "/b/o")
        .match_query(Matcher::UrlEncoded(
            "storageAccessToken".into(),
            "token-1".into(),
        ))
        .with_status(503)
        .expect(1)
        .create_async()
        .await;
    let second = object_mock(&mut server, "token-2", 1).await;
    let provider = Sequence::new(tokens.map(Credentials::storage_token).to_vec());
    assert_eq!(get(&retrying(&server, provider)).await.unwrap(), "content");
    first.assert_async().await;
    second.assert_async().await;
}

#[tokio::test]
async fn redaction_follows_the_credentials_each_request_used() {
    let mut server = Server::new_async().await;
    let token = exchanged(&mut server, "storage-secret", Duration::days(1)).await;
    let rejected = server
        .mock("GET", "/b/o")
        .match_query(Matcher::Any)
        .with_status(403)
        .with_header("x-xiaomi-request-id", "request-diagnostic")
        .with_body("{\"code\":\"Denied\",\"message\":\"server-diagnostic\"}")
        .expect(2)
        .create_async()
        .await;
    let client = client(
        &server,
        Sequence::new(vec![
            Credentials::access_key("ak", "sk"),
            Credentials::storage_token(token.clone()),
        ]),
    );
    let kept = get(&client).await.unwrap_err();
    assert_eq!(kept.request_id(), Some("request-diagnostic"));
    assert!(kept
        .response()
        .unwrap()
        .diagnostic_body()
        .contains("server-diagnostic"));
    let redacted = get(&client).await.unwrap_err();
    assert_eq!(redacted.kind(), ErrorKind::PermissionDenied);
    assert_eq!(redacted.request_id(), None);
    assert_eq!(redacted.service_code(), None);
    assert_eq!(redacted.response().unwrap().diagnostic_body(), "");
    rejected.assert_async().await;

    // Response-body errors of a token request are redacted as well.
    let listing = server
        .mock("GET", "/")
        .match_query(Matcher::UrlEncoded(
            "storageAccessToken".into(),
            "storage-secret".into(),
        ))
        .with_body("storage-secret is not JSON")
        .create_async()
        .await;
    let client = self::client(
        &server,
        Sequence::new(vec![Credentials::storage_token(token)]),
    );
    let error = client.list_app_buckets().send().await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ProtocolViolation);
    assert_eq!(error.response().unwrap().diagnostic_body(), "");
    assert!(!format!("{error} {error:?}").contains("storage-secret"));
    listing.assert_async().await;
}

#[tokio::test]
async fn app_listing_requires_a_token_from_the_current_credentials() {
    let mut server = Server::new_async().await;
    let none = server
        .mock("GET", "/")
        .match_query(Matcher::Any)
        .expect(0)
        .create_async()
        .await;
    let access_key = Sequence::new(vec![Credentials::access_key("ak", "sk")]);
    let error = client(&server, access_key.clone())
        .list_app_buckets()
        .send()
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ConfigInvalid);
    assert_eq!(error.dispatch(), Dispatch::NotSent);
    assert_eq!(access_key.calls.load(Ordering::SeqCst), 1);
    none.assert_async().await;
    none.remove_async().await;

    let token = exchanged(&mut server, "token", Duration::days(1)).await;
    let listing = server
        .mock("GET", "/")
        .match_query(Matcher::UrlEncoded(
            "storageAccessToken".into(),
            "token".into(),
        ))
        .with_body("{\"owner\":{\"id\":\"user\"},\"buckets\":[]}")
        .expect(1)
        .create_async()
        .await;
    let provider = StorageTokenProvider::new(exchange(&server)).with_token(token);
    let output = client(&server, provider)
        .list_app_buckets()
        .send()
        .await
        .unwrap();
    assert!(output.buckets.is_empty());
    listing.assert_async().await;
}

#[tokio::test]
async fn presigning_needs_static_access_keys() {
    let mut server = Server::new_async().await;
    let presign = |client: &Client| {
        client
            .presign_object("b", "o")
            .expires_at(chrono::DateTime::from_timestamp_millis(4_102_444_800_000).unwrap())
            .build()
    };
    let shortcut = Client::builder()
        .endpoint(server.url())
        .credentials("ak", "sk")
        .build()
        .unwrap();
    let provider = client(
        &server,
        StaticCredentials::new(Credentials::access_key("ak", "sk")),
    );
    assert_eq!(presign(&shortcut).unwrap(), presign(&provider).unwrap());

    let token = exchanged(&mut server, "token", Duration::days(1)).await;
    for client in [
        client(
            &server,
            Sequence::new(vec![Credentials::access_key("ak", "sk")]),
        ),
        client(&server, StorageTokenProvider::new(exchange(&server))),
        client(
            &server,
            StaticCredentials::from(Credentials::storage_token(token)),
        ),
    ] {
        let error = presign(&client).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ConfigInvalid, "{error:?}");
    }
}

#[tokio::test]
async fn static_providers_validate_at_build_and_dynamic_ones_per_attempt() {
    let mut server = Server::new_async().await;
    for (id, secret) in [("", "sk"), ("ak", ""), ("a:k", "sk")] {
        let error = Client::builder()
            .endpoint(server.url())
            .credentials_provider(StaticCredentials::new(Credentials::access_key(id, secret)))
            .build()
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ConfigInvalid);
    }
    let token = exchanged(&mut server, "token", Duration::days(1)).await;
    // A token needs HTTPS (or a loopback IP), statically or from a provider.
    let error = Client::builder()
        .endpoint("http://example.com")
        .credentials_provider(StaticCredentials::new(Credentials::storage_token(
            token.clone(),
        )))
        .build()
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ConfigInvalid);
    let insecure = Client::builder()
        .endpoint("http://example.com")
        .credentials_provider(Sequence::new(vec![Credentials::storage_token(token)]))
        .build()
        .unwrap();
    let error = get(&insecure).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ConfigInvalid);
    assert_eq!(error.dispatch(), Dispatch::NotSent);

    let none = server.mock("GET", "/b/o").expect(0).create_async().await;
    let invalid = client(
        &server,
        Sequence::new(vec![Credentials::access_key("", "sk")]),
    );
    let error = get(&invalid).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ConfigInvalid);
    assert_eq!(error.dispatch(), Dispatch::NotSent);
    none.assert_async().await;
}

#[tokio::test]
async fn debug_output_never_contains_credentials() {
    let mut server = Server::new_async().await;
    let token = exchanged(&mut server, "storage-secret", Duration::days(1)).await;
    let provider = StorageTokenProvider::new(exchange(&server)).with_token(token.clone());
    let builder = Client::builder()
        .endpoint(server.url())
        .credentials_provider(provider.clone());
    let outputs = [
        format!("{:?}", Credentials::access_key("private-ak", "private-sk")),
        format!("{:?}", Credentials::storage_token(token.clone())),
        format!(
            "{:?}",
            StaticCredentials::new(Credentials::storage_token(token))
        ),
        format!("{provider:?}"),
        format!("{builder:?}"),
        format!("{:?}", builder.build().unwrap()),
    ];
    for output in outputs {
        for secret in ["private", "storage-secret", "oauth-secret", "app"] {
            assert!(!output.contains(secret), "{output}");
        }
    }
}
