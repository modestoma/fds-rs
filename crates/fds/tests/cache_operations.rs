use fds::{Client, RetryPolicy};
use mockito::{Matcher, Server};

#[tokio::test]
async fn cache_operations_use_query_flags_and_accept_empty_or_numeric_acknowledgments() {
    for operation in ["prefetch", "refresh"] {
        for body in ["", "12345"] {
            let mut server = Server::new_async().await;
            let client = Client::builder()
                .endpoint(server.url())
                .credentials("AKID", "sk")
                .build()
                .unwrap();
            let mock = server
                .mock("PUT", "/b/dir%2Fa%20%2B%25%E4%B8%AD")
                .match_query(Matcher::UrlEncoded(operation.into(), "".into()))
                .match_header("prefetch", Matcher::Missing)
                .match_header("refresh", Matcher::Missing)
                .match_header("content-type", "application/json; charset=UTF-8")
                .match_header("authorization", Matcher::Regex("^Galaxy-V2 AKID:".into()))
                .match_body("")
                .with_status(200)
                .with_body(body)
                .expect(1)
                .create_async()
                .await;
            if operation == "prefetch" {
                client.prefetch_object("b", "dir/a +%中").await.unwrap();
            } else {
                client.refresh_object("b", "dir/a +%中").await.unwrap();
            }
            mock.assert_async().await;
        }
    }
}

#[tokio::test]
async fn cache_failures_preserve_status_and_do_not_retry_mutations() {
    for operation in ["prefetch", "refresh"] {
        for status in [202, 204, 302, 403, 409, 503] {
            let mut server = Server::new_async().await;
            let client = Client::builder()
                .endpoint(server.url())
                .credentials("AKID", "sk")
                .retry_policy(
                    RetryPolicy::new(
                        2,
                        std::time::Duration::from_millis(1),
                        std::time::Duration::from_millis(2),
                    )
                    .unwrap(),
                )
                .build()
                .unwrap();
            let mock = server
                .mock("PUT", "/b/o")
                .match_query(Matcher::UrlEncoded(operation.into(), "".into()))
                .with_status(status)
                .with_header("x-xiaomi-request-id", "cache-request")
                .expect(1)
                .create_async()
                .await;
            let error = if operation == "prefetch" {
                client.prefetch_object("b", "o").await.unwrap_err()
            } else {
                client.refresh_object("b", "o").await.unwrap_err()
            };
            assert_eq!(error.status().map(|s| s.as_u16()), Some(status as u16));
            assert_eq!(error.request_id(), Some("cache-request"));
            mock.assert_async().await;
        }
    }
}
