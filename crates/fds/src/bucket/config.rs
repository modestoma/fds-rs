//! Lifecycle and access-log configuration operations.

use super::{validate_bucket, AccessLog, LifecycleConfig, LifecycleRule};
use crate::client::ClientRequest;
use crate::{Client, Error, Result, RetryMode};
use bytes::Bytes;
use http::Method;
use serde::Serialize;
use std::time::Duration;

/// PUT `/{bucket}?{key}={value}` with `body` as JSON; `retry` is `Some` only
/// for an idempotent write.
async fn put_json(
    client: &Client,
    bucket: &str,
    (key, value): (&str, &str),
    body: &impl Serialize,
    timeout: Option<Duration>,
    retry: Option<RetryMode>,
) -> Result<()> {
    validate_bucket(bucket)?;
    let body = serde_json::to_vec(body).map_err(Error::json_encode)?;
    let mut cr = ClientRequest::new(bucket, "", Method::PUT);
    if let Some(mode) = retry {
        cr = cr.idempotent(mode);
    }
    let mut cr = cr.with_timeout(timeout)?;
    cr.query.add(key, value);
    cr.body = Some(Bytes::from(body).into());
    client.do_discard(cr).await
}

/// Builder for [`Client::get_lifecycle_config`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct GetLifecycleConfig {
    client: Client,
    bucket: String,
    rule_id: Option<String>,
    timeout: Option<Duration>,
}

impl GetLifecycleConfig {
    timeout_option!();

    /// Read only the rule with this ID. Without it (or with an empty value)
    /// the whole configuration is returned.
    pub fn rule_id(mut self, rule_id: impl Into<String>) -> Self {
        self.rule_id = Some(rule_id.into());
        self
    }

    /// GET `/{bucket}?lifecycle={rule_id}`; the `lifecycle` key is always
    /// sent, with an empty value when no rule ID is set.
    pub async fn send(self) -> Result<LifecycleConfig> {
        validate_bucket(&self.bucket)?;
        let mut cr = ClientRequest::new(&self.bucket, "", Method::GET)
            .read_only()
            .with_timeout(self.timeout)?;
        cr.query
            .add("lifecycle", self.rule_id.as_deref().unwrap_or_default());
        self.client.do_json(cr).await
    }
}

/// Builder for [`Client::set_lifecycle_config`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct SetLifecycleConfig {
    client: Client,
    bucket: String,
    config: LifecycleConfig,
    timeout: Option<Duration>,
    retry: RetryMode,
}

impl SetLifecycleConfig {
    timeout_option!();

    retry_option!();

    /// PUT `/{bucket}?lifecycle` with the configuration as JSON body.
    ///
    /// An idempotent write (the configuration is replaced as a whole),
    /// retried when the client's [`RetryPolicy`](crate::RetryPolicy) enables
    /// idempotent writes or [`retry`](Self::retry) says so.
    pub async fn send(self) -> Result<()> {
        put_json(
            &self.client,
            &self.bucket,
            ("lifecycle", ""),
            &self.config,
            self.timeout,
            Some(self.retry),
        )
        .await
    }
}

/// Builder for [`Client::set_lifecycle_rule`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct SetLifecycleRule {
    client: Client,
    bucket: String,
    rule: LifecycleRule,
    timeout: Option<Duration>,
}

impl SetLifecycleRule {
    timeout_option!();

    /// PUT `/{bucket}?lifecycle=rule` with the rule as JSON body. Never
    /// retried: whether the service replaces a rule with the same ID or adds
    /// another one is not documented.
    pub async fn send(self) -> Result<()> {
        put_json(
            &self.client,
            &self.bucket,
            ("lifecycle", "rule"),
            &self.rule,
            self.timeout,
            None,
        )
        .await
    }
}

bucket_builder! {
    /// Builder for [`Client::get_access_log`].
    GetAccessLog
}

impl GetAccessLog {
    /// GET `/{bucket}?accessLog`.
    pub async fn send(self) -> Result<AccessLog> {
        validate_bucket(&self.bucket)?;
        let mut cr = ClientRequest::new(&self.bucket, "", Method::GET)
            .read_only()
            .with_timeout(self.timeout)?;
        cr.query.add("accessLog", "");
        self.client.do_json(cr).await
    }
}

/// Builder for [`Client::set_access_log`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct SetAccessLog {
    client: Client,
    bucket: String,
    access_log: AccessLog,
    timeout: Option<Duration>,
    retry: RetryMode,
}

impl SetAccessLog {
    timeout_option!();

    retry_option!();

    /// PUT `/{bucket}?accessLog` with the configuration as JSON body.
    ///
    /// An idempotent write (the configuration is replaced as a whole),
    /// retried when the client's [`RetryPolicy`](crate::RetryPolicy) enables
    /// idempotent writes or [`retry`](Self::retry) says so.
    pub async fn send(self) -> Result<()> {
        put_json(
            &self.client,
            &self.bucket,
            ("accessLog", ""),
            &self.access_log,
            self.timeout,
            Some(self.retry),
        )
        .await
    }
}

impl Client {
    /// Get a bucket's lifecycle configuration, or one rule of it with
    /// [`GetLifecycleConfig::rule_id`].
    pub fn get_lifecycle_config(&self, bucket: impl Into<String>) -> GetLifecycleConfig {
        GetLifecycleConfig {
            client: self.clone(),
            bucket: bucket.into(),
            rule_id: None,
            timeout: None,
        }
    }

    /// Replace a bucket's lifecycle configuration.
    pub fn set_lifecycle_config(
        &self,
        bucket: impl Into<String>,
        config: LifecycleConfig,
    ) -> SetLifecycleConfig {
        SetLifecycleConfig {
            client: self.clone(),
            bucket: bucket.into(),
            config,
            timeout: None,
            retry: RetryMode::Policy,
        }
    }

    /// Set a single lifecycle rule of a bucket.
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client) -> fds::Result<()> {
    /// use fds::bucket::LifecycleRule;
    ///
    /// let json = br#"{"id":"r1","prefix":"logs/","enabled":true,"actions":{"expiration":{"days":30}}}"#;
    /// let rule: LifecycleRule = serde_json::from_slice(json).expect("valid rule JSON");
    /// client.set_lifecycle_rule("bucket", rule).send().await?;
    /// # Ok(()) }
    /// ```
    pub fn set_lifecycle_rule(
        &self,
        bucket: impl Into<String>,
        rule: LifecycleRule,
    ) -> SetLifecycleRule {
        SetLifecycleRule {
            client: self.clone(),
            bucket: bucket.into(),
            rule,
            timeout: None,
        }
    }

    /// Get a bucket's access-log configuration.
    pub fn get_access_log(&self, bucket: impl Into<String>) -> GetAccessLog {
        GetAccessLog::new(self, bucket)
    }

    /// Set a bucket's access-log configuration; `enabled: false` disables it.
    pub fn set_access_log(&self, bucket: impl Into<String>, access_log: AccessLog) -> SetAccessLog {
        SetAccessLog {
            client: self.clone(),
            bucket: bucket.into(),
            access_log,
            timeout: None,
            retry: RetryMode::Policy,
        }
    }
}
