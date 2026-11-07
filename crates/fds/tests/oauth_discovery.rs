use chrono::{Duration, Utc};
use fds::{Client, FdsServiceClient, OAuthProvider, StorageTokenRequest};
use mockito::{Matcher, Server};

fn request(provider: OAuthProvider) -> StorageTokenRequest {
    StorageTokenRequest::new("app +&中", "oauth-app", "oauth-secret +&%中", provider)
}
fn response() -> String {
    serde_json::json!({"token":"storage-secret +&%中", "expireTime":(Utc::now()+Duration::days(7)).timestamp_millis()}).to_string()
}
async fn token(server: &mut Server) -> fds::StorageAccessToken {
    let mock = server
        .mock("GET", "/")
        .match_query(Matcher::UrlEncoded("storageAccessToken".into(), "".into()))
        .with_body(response())
        .create_async()
        .await;
    let token = FdsServiceClient::new(server.url())
        .unwrap()
        .exchange_storage_token(&request(OAuthProvider::QQ))
        .await
        .unwrap();
    mock.assert_async().await;
    token
}
#[tokio::test]
async fn discovery_uses_documented_string_and_never_follows_or_authenticates() {
    let mut server = Server::new_async().await;
    let service = FdsServiceClient::from_builder(
        Client::builder()
            .endpoint(server.url())
            .credentials("id", "secret"),
    )
    .unwrap();
    for (body, valid) in [
        ("\"awssgp0.fds.api.xiaomi.com\"", true),
        ("{\"EndpointResult\":\"host\"}", false),
        ("\"https://host/path\"", false),
        ("\"\"", false),
    ] {
        let mock = server
            .mock("GET", "/")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("endpoint".into(), "".into()),
                Matcher::UrlEncoded("region".into(), "sg +&中".into()),
                Matcher::UrlEncoded("service".into(), "aws".into()),
            ]))
            .match_header("authorization", Matcher::Missing)
            .with_body(body)
            .create_async()
            .await;
        assert_eq!(service.get_endpoint("sg +&中", "aws").await.is_ok(), valid);
        mock.assert_async().await;
    }
    assert!(service.get_endpoint("", "aws").await.is_err());
    assert!(service.get_endpoint("sg", " ").await.is_err());
}
#[tokio::test]
async fn exchange_encodes_every_provider_field_and_utc_milliseconds() {
    for provider in [
        OAuthProvider::XiaoMi,
        OAuthProvider::QQ,
        OAuthProvider::Sina,
        OAuthProvider::RenRen,
    ] {
        let mut server = Server::new_async().await;
        let mut pairs = vec![
            ("storageAccessToken", ""),
            ("appId", "app +&中"),
            ("oauthAppId", "oauth-app"),
            ("oauthAccessToken", "oauth-secret +&%中"),
        ];
        let mut input = request(provider);
        let wire = format!("{provider:?}");
        pairs.push(("oauthProvider", &wire));
        if provider == OAuthProvider::XiaoMi {
            pairs.extend([
                ("oauthOpenId", "open+id"),
                ("oauthMacKey", "mac+secret"),
                ("oauthMacAlgorithm", "HmacSHA1"),
            ]);
            input = input.xiaomi_mac("open+id", "mac+secret", "HmacSHA1");
        }
        let expected: std::collections::BTreeMap<_, _> = pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let mock = server
            .mock("GET", "/")
            .match_query(Matcher::Any)
            .match_request(move |r| {
                let url =
                    url::Url::parse(&format!("http://localhost{}", r.path_and_query())).unwrap();
                let actual: std::collections::BTreeMap<_, _> = url
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                actual == expected
            })
            .match_header("authorization", "OAuth")
            .with_body(response())
            .create_async()
            .await;
        let token = FdsServiceClient::new(server.url())
            .unwrap()
            .exchange_storage_token(&input)
            .await
            .unwrap();
        assert!((token.expires_at() - Utc::now()).num_days() >= 6);
        assert!(!token.is_expired());
        for debug in [format!("{input:?}"), format!("{token:?}")] {
            assert!(!debug.contains("secret"));
        }
        mock.assert_async().await;
    }
}
#[tokio::test]
async fn provider_validation_and_https_fail_before_network() {
    let service = FdsServiceClient::new("https://invalid.example").unwrap();
    for input in [
        request(OAuthProvider::XiaoMi),
        request(OAuthProvider::XiaoMi).xiaomi_mac("", "key", "alg"),
        request(OAuthProvider::XiaoMi).xiaomi_mac("id", "", "alg"),
        request(OAuthProvider::XiaoMi).xiaomi_mac("id", "key", ""),
        request(OAuthProvider::QQ).xiaomi_mac("id", "key", "alg"),
        StorageTokenRequest::new("", "id", "secret", OAuthProvider::QQ),
        StorageTokenRequest::new("id", "", "secret", OAuthProvider::QQ),
        StorageTokenRequest::new("id", "id", " ", OAuthProvider::QQ),
    ] {
        assert!(matches!(
            service.exchange_storage_token(&input).await,
            Err(fds::FdsError::Invalid(_))
        ));
    }
    assert!(FdsServiceClient::new("http://invalid.example")
        .unwrap()
        .exchange_storage_token(&request(OAuthProvider::QQ))
        .await
        .is_err());
}
#[tokio::test]
async fn oauth_operations_use_bound_app_token_and_reject_presigning() {
    let mut server = Server::new_async().await;
    let credential = token(&mut server).await;
    let builder = Client::builder()
        .endpoint(server.url())
        .storage_token(credential.clone());
    assert!(!format!("{builder:?}").contains("storage-secret"));
    let client = builder.build().unwrap();
    assert!(!format!("{client:?}").contains("storage-secret"));
    assert!(Client::builder()
        .endpoint("http://example.com")
        .storage_token(credential)
        .build()
        .is_err());
    for (path, body) in [("/", "{\"owner\":{\"id\":\"user\"},\"buckets\":[{\"name\":\"b\",\"orgId\":\"org\",\"creationTime\":123,\"usedSpace\":4,\"numObjects\":1}]}"), ("/b/dir%2Fa%20%2B%25%E4%B8%AD", "content")] {
        let mock = server.mock("GET", path).match_header("authorization", "OAuth").match_query(Matcher::AllOf(vec![Matcher::UrlEncoded("appId".into(),"app +&中".into()),Matcher::UrlEncoded("storageAccessToken".into(),"storage-secret +&%中".into())])).with_body(body).create_async().await;
        if path == "/" { let output = client.list_app_buckets().await.unwrap(); assert_eq!(output.buckets[0].org_id,"org"); }
        else { let output = fds::GetObjectRequest::new("b","dir/a +%中").send(&client).await.unwrap(); assert!(!format!("{output:?}").contains("secret")); let raw = output.into_raw(); assert!(!format!("{raw:?}").contains("secret")); assert!(raw.url().query().is_none()); assert_eq!(raw.bytes().await.unwrap(),"content"); }
        mock.assert_async().await;
    }
    let req = fds::GeneratePresignedURLRequest {
        bucket_name: "b".into(),
        object_name: "o".into(),
        method: fds::HttpMethod::Get,
        expiration: Utc::now() + Duration::minutes(1),
        cdn: false,
        metadata: None,
    };
    assert!(client.generate_presigned_url(&req).is_err());
}
#[tokio::test]
async fn token_errors_never_retain_response_secrets_and_exchange_is_not_retried() {
    for (status, body) in [
        (503, "oauth-secret"),
        (302, "oauth-secret"),
        (
            200,
            "{\"token\":\"oauth-secret\",\"expireTime\":\"oauth-secret\"}",
        ),
        (200, "{\"token\":\"oauth-secret\",\"expireTime\":0}"),
        (200, "{\"token\":\"\",\"expireTime\":9999999999999}"),
        (200, "{}"),
        (200, ""),
    ] {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/")
            .match_query(Matcher::Any)
            .with_status(status)
            .with_body(body)
            .with_header("x-xiaomi-request-id", "oauth-secret")
            .with_header("location", "https://example.com/oauth-secret")
            .expect(1)
            .create_async()
            .await;
        let service = FdsServiceClient::from_builder(
            Client::builder().endpoint(server.url()).retry_policy(
                fds::RetryPolicy::new(
                    2,
                    std::time::Duration::from_millis(1),
                    std::time::Duration::from_millis(2),
                )
                .unwrap(),
            ),
        )
        .unwrap();
        let error = service
            .exchange_storage_token(&request(OAuthProvider::QQ))
            .await
            .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("oauth-secret"));
        assert!(!error
            .response()
            .map(fds::ResponseError::diagnostic_body)
            .unwrap_or("")
            .contains("oauth-secret"));
        assert!(error.request_id().is_none());
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn expired_clones_fail_locally_and_refresh_is_explicit() {
    let mut server = Server::new_async().await;
    let mock = server.mock("GET", "/").match_query(Matcher::Any)
        .with_body(serde_json::json!({"token":"short-lived", "expireTime":(Utc::now()+Duration::seconds(1)).timestamp_millis()}).to_string())
        .expect(1).create_async().await;
    let service = FdsServiceClient::new(server.url()).unwrap();
    let token = service
        .exchange_storage_token(&request(OAuthProvider::QQ))
        .await
        .unwrap();
    let expiry = token.expires_at();
    let client = Client::builder()
        .endpoint(server.url())
        .storage_token(token)
        .build()
        .unwrap();
    let clone = client.clone();
    tokio::time::sleep(
        (expiry - Utc::now()).to_std().unwrap_or_default() + std::time::Duration::from_millis(20),
    )
    .await;
    for client in [&client, &clone] {
        assert!(matches!(
            client.list_app_buckets().await,
            Err(fds::FdsError::Invalid(_))
        ));
    }
    mock.assert_async().await;
    // No hidden exchange occurred; only an explicit call obtains a replacement.
    mock.remove_async().await;
    let new_token = token_for_refresh(&mut server).await;
    assert!(!new_token.is_expired());
    assert!(client.list_app_buckets().await.is_err());
}
async fn token_for_refresh(server: &mut Server) -> fds::StorageAccessToken {
    token(server).await
}

#[tokio::test]
async fn oauth_operation_errors_discard_echoed_tokens() {
    let mut server = Server::new_async().await;
    let credential = token(&mut server).await;
    let client = Client::builder()
        .endpoint(server.url())
        .storage_token(credential)
        .build()
        .unwrap();
    let mock = server
        .mock("GET", "/")
        .match_query(Matcher::Any)
        .with_status(403)
        .with_header("x-xiaomi-request-id", "storage-secret")
        .with_body("{\"code\":\"storage-secret\",\"message\":\"storage-secret\"}")
        .create_async()
        .await;
    let error = client.list_app_buckets().await.unwrap_err();
    assert_eq!(error.status(), Some(403));
    assert_eq!(
        error.response().map(fds::ResponseError::diagnostic_body),
        Some("")
    );
    assert!(error.request_id().is_none());
    assert!(!format!("{error:?}").contains("storage-secret"));
    mock.assert_async().await;
}
