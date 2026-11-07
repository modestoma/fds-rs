//! SDK-only operations remain explicit wire contracts even without operation YAML.
use fds::bucket::{
    AccessLog, GetLifecycleConfigRequest, LifecycleAction, LifecycleConfig, LifecycleRule,
    MigrateBucketRequest,
};
use fds::Client;
use mockito::Matcher;

#[tokio::test]
async fn sdk_only_bucket_operations_preserve_dispatch_and_models() {
    let mut server = mockito::Server::new_async().await;
    let client = Client::builder()
        .endpoint(server.url())
        .credentials("id", "secret")
        .build()
        .unwrap();
    let get = server
        .mock("GET", "/b")
        .match_query("lifecycle=rule+%2B")
        .with_body(
            r#"{"rules":[{"id":"rule +","enabled":true,"actions":{"expiration":{"days":1.5}}}]}"#,
        )
        .create_async()
        .await;
    let config = client
        .get_lifecycle_config(&GetLifecycleConfigRequest {
            bucket_name: "b".into(),
            rule_id: "rule +".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        config.rules[0].action[&LifecycleAction::Expiration].days,
        1.5
    );
    get.assert_async().await;
    let put = server
        .mock("PUT", "/b")
        .match_query("lifecycle=")
        .match_body(Matcher::Json(serde_json::json!({"rules":[]})))
        .create_async()
        .await;
    client
        .set_lifecycle_config("b", &LifecycleConfig::default())
        .await
        .unwrap();
    put.assert_async().await;
    let rule = server
        .mock("PUT", "/b")
        .match_query("lifecycle=rule")
        .match_body(Matcher::Json(
            serde_json::json!({"id":"r", "prefix":"", "enabled":false,"actions":{}}),
        ))
        .create_async()
        .await;
    client
        .set_lifecycle_rule(
            "b",
            &LifecycleRule {
                id: "r".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    rule.assert_async().await;
    let migration = server
        .mock("PUT", "/b")
        .match_query("migrate=&orgId=org+%2B&teamId=")
        .match_body("")
        .create_async()
        .await;
    client
        .migrate_bucket(&MigrateBucketRequest {
            bucket_name: "b".into(),
            org_id: "org +".into(),
            team_id: String::new(),
        })
        .await
        .unwrap();
    migration.assert_async().await;
    let log = server
        .mock("GET", "/b")
        .match_query("accessLog=")
        .with_body(r#"{"bucketName":"logs","enabled":true}"#)
        .create_async()
        .await;
    let config: AccessLog = client.get_access_log("b").await.unwrap();
    assert!(config.enabled);
    assert_eq!(config.bucket_name, "logs");
    log.assert_async().await;
    let set_log = server
        .mock("PUT", "/b")
        .match_query("accessLog=")
        .match_body(Matcher::Json(serde_json::to_value(&config).unwrap()))
        .create_async()
        .await;
    client.set_access_log("b", &config).await.unwrap();
    set_log.assert_async().await;
}

#[tokio::test]
async fn sdk_only_metadata_forms_remain_distinct() {
    let mut server = mockito::Server::new_async().await;
    let client = Client::builder()
        .endpoint(server.url())
        .credentials("id", "secret")
        .build()
        .unwrap();
    let basic = server.mock("GET", "/b/a%2Fb").match_query("basicInfo=")
        .with_body(r#"{"size":5,"metadataBean":{"rawMeta":{"entry":[{"key":"x-xiaomi-meta-tag","value":"v"}]}}}"#)
        .create_async().await;
    let output = client
        .get_object_basic_info("b", "a/b")
        .send()
        .await
        .unwrap();
    assert_eq!(output.size, 5);
    assert_eq!(output.metadata.get("x-xiaomi-meta-tag"), Some("v"));
    basic.assert_async().await;
    let get = server
        .mock("GET", "/b/a%2Fb")
        .match_query("metadata=")
        .with_header("x-xiaomi-meta-tag", "header-value")
        .with_body("not JSON")
        .create_async()
        .await;
    let mut metadata = client.get_object_metadata("b", "a/b").send().await.unwrap();
    assert_eq!(metadata.get("X-XIAOMI-META-TAG"), Some("header-value"));
    get.assert_async().await;
    // Use a fresh metadata map so transport response headers are not sent back.
    metadata = fds::object::ObjectMetadata::new();
    metadata.set("x-xiaomi-meta-tag", "new-value").unwrap();
    let set = server
        .mock("PUT", "/b/a%2Fb")
        .match_query("setMetaData=")
        .match_body(Matcher::Json(
            serde_json::json!({"rawMeta":{"x-xiaomi-meta-tag":"new-value"}}),
        ))
        .create_async()
        .await;
    client
        .set_object_metadata("b", "a/b", metadata)
        .send()
        .await
        .unwrap();
    set.assert_async().await;
}
