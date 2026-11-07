//! Bucket creation, existence checks, information, listing, migration and
//! deletion.

use super::{validate_bucket, BucketInfo, BucketList};
use crate::acl::BucketCannedAcl;
use crate::client::ClientRequest;
use crate::object::StorageClass;
use crate::{Client, Result};
use bytes::Bytes;
use http::Method;
use std::time::Duration;

/// Builder for [`Client::create_bucket`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct CreateBucket {
    client: Client,
    bucket: String,
    acl: Vec<BucketCannedAcl>,
    storage_class: Option<StorageClass>,
    org_id: Option<String>,
    timeout: Option<Duration>,
}

impl CreateBucket {
    timeout_option!();

    /// Canned grants for the new bucket, sent as the `x-xiaomi-meta-acl`
    /// header. Replaces grants set earlier; no grants (the default) keep the
    /// service default.
    pub fn acl(mut self, grants: impl IntoIterator<Item = BucketCannedAcl>) -> Self {
        self.acl = grants.into_iter().collect();
        self
    }

    /// The `storageClass` of the new bucket, passed verbatim. Not sent unless
    /// set.
    pub fn storage_class(mut self, storage_class: StorageClass) -> Self {
        self.storage_class = Some(storage_class);
        self
    }

    /// Create the bucket in this organization (`orgId`). Not sent unless set;
    /// an empty value is not sent either.
    pub fn org_id(mut self, org_id: impl Into<String>) -> Self {
        self.org_id = Some(org_id.into());
        self
    }

    /// PUT `/{bucket}[?orgId][&storageClass]` with an empty body.
    pub async fn send(self) -> Result<()> {
        validate_bucket(&self.bucket)?;
        let mut cr =
            ClientRequest::new(&self.bucket, "", Method::PUT).with_timeout(self.timeout)?;
        if let Some(storage_class) = &self.storage_class {
            cr.query.add("storageClass", storage_class.as_str());
        }
        if let Some(org_id) = self.org_id.as_deref().filter(|v| !v.is_empty()) {
            cr.query.add("orgId", org_id);
        }
        if !self.acl.is_empty() {
            let value = self
                .acl
                .iter()
                .map(|grant| grant.as_str())
                .collect::<Vec<_>>()
                .join(",");
            cr.headers
                .insert("x-xiaomi-meta-acl", crate::client::header_value(&value)?);
        }
        cr.body = Some(Bytes::new().into()); // empty PUT body (Content-Length: 0)
        self.client.do_discard(cr).await
    }
}

bucket_builder! {
    /// Builder for [`Client::bucket_exists`].
    BucketExists
}

impl BucketExists {
    /// HEAD `/{bucket}`: 200 is `true`, 404 is `false`; every other status
    /// (403 included) and transport failure is an error.
    pub async fn send(self) -> Result<bool> {
        validate_bucket(&self.bucket)?;
        let mut cr = ClientRequest::new(&self.bucket, "", Method::HEAD)
            .read_only()
            .with_timeout(self.timeout)?;
        cr.accepted_statuses = &[200, 404];
        let resp = self.client.do_request(cr).await?;
        Ok(resp.status().as_u16() == 200)
    }
}

bucket_builder! {
    /// Builder for [`Client::delete_bucket`].
    DeleteBucket
}

impl DeleteBucket {
    /// DELETE `/{bucket}`. Objects are not removed first; the service decides
    /// whether a non-empty bucket can be deleted.
    pub async fn send(self) -> Result<()> {
        validate_bucket(&self.bucket)?;
        let cr = ClientRequest::new(&self.bucket, "", Method::DELETE).with_timeout(self.timeout)?;
        self.client.do_discard(cr).await
    }
}

bucket_builder! {
    /// Builder for [`Client::get_bucket_info`].
    GetBucketInfo
}

impl GetBucketInfo {
    /// GET `/{bucket}`.
    pub async fn send(self) -> Result<BucketInfo> {
        validate_bucket(&self.bucket)?;
        let cr = ClientRequest::new(&self.bucket, "", Method::GET)
            .read_only()
            .with_timeout(self.timeout)?;
        self.client.do_json(cr).await
    }
}

/// Builder for [`Client::list_buckets`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct ListBuckets {
    client: Client,
    timeout: Option<Duration>,
}

impl ListBuckets {
    timeout_option!();

    /// GET `/`.
    pub async fn send(self) -> Result<BucketList> {
        let cr = ClientRequest::new("", "", Method::GET)
            .read_only()
            .with_timeout(self.timeout)?;
        self.client.do_json(cr).await
    }
}

/// Builder for [`Client::list_authorized_buckets`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct ListAuthorizedBuckets {
    client: Client,
    timeout: Option<Duration>,
}

impl ListAuthorizedBuckets {
    timeout_option!();

    /// GET `/?authorizedBuckets`.
    pub async fn send(self) -> Result<BucketList> {
        let mut cr = ClientRequest::new("", "", Method::GET)
            .read_only()
            .with_timeout(self.timeout)?;
        cr.query.add("authorizedBuckets", "");
        self.client.do_json(cr).await
    }
}

/// Builder for [`Client::migrate_bucket`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct MigrateBucket {
    client: Client,
    bucket: String,
    org_id: String,
    team_id: String,
    timeout: Option<Duration>,
}

impl MigrateBucket {
    timeout_option!();

    /// PUT `/{bucket}?migrate&orgId&teamId` with an empty body. `orgId` and
    /// `teamId` are always sent, as given.
    pub async fn send(self) -> Result<()> {
        validate_bucket(&self.bucket)?;
        let mut cr =
            ClientRequest::new(&self.bucket, "", Method::PUT).with_timeout(self.timeout)?;
        cr.query.add("migrate", "");
        cr.query.add("orgId", &self.org_id);
        cr.query.add("teamId", &self.team_id);
        cr.body = Some(Bytes::new().into());
        self.client.do_discard(cr).await
    }
}

impl Client {
    /// Create a bucket.
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client) -> fds::Result<()> {
    /// use fds::acl::BucketCannedAcl;
    ///
    /// client
    ///     .create_bucket("bucket")
    ///     .acl([BucketCannedAcl::PublicRead])
    ///     .org_id("org")
    ///     .send()
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub fn create_bucket(&self, bucket: impl Into<String>) -> CreateBucket {
        CreateBucket {
            client: self.clone(),
            bucket: bucket.into(),
            acl: Vec::new(),
            storage_class: None,
            org_id: None,
            timeout: None,
        }
    }

    /// Whether a bucket exists.
    pub fn bucket_exists(&self, bucket: impl Into<String>) -> BucketExists {
        BucketExists::new(self, bucket)
    }

    /// Delete a bucket.
    pub fn delete_bucket(&self, bucket: impl Into<String>) -> DeleteBucket {
        DeleteBucket::new(self, bucket)
    }

    /// Get a bucket's information.
    pub fn get_bucket_info(&self, bucket: impl Into<String>) -> GetBucketInfo {
        GetBucketInfo::new(self, bucket)
    }

    /// List the buckets owned by the caller.
    pub fn list_buckets(&self) -> ListBuckets {
        ListBuckets {
            client: self.clone(),
            timeout: None,
        }
    }

    /// List the buckets the caller is authorized to access.
    pub fn list_authorized_buckets(&self) -> ListAuthorizedBuckets {
        ListAuthorizedBuckets {
            client: self.clone(),
            timeout: None,
        }
    }

    /// Move a bucket to another organization and team. This changes
    /// ownership only; no data is copied.
    pub fn migrate_bucket(
        &self,
        bucket: impl Into<String>,
        org_id: impl Into<String>,
        team_id: impl Into<String>,
    ) -> MigrateBucket {
        MigrateBucket {
            client: self.clone(),
            bucket: bucket.into(),
            org_id: org_id.into(),
            team_id: team_id.into(),
            timeout: None,
        }
    }
}
