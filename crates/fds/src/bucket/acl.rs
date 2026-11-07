//! Bucket ACL operations.

use super::validate_bucket;
use crate::acl::{AccessControlList, AclAction};
use crate::client::ClientRequest;
use crate::{Client, Error, Result};
use bytes::Bytes;
use http::Method;
use std::time::Duration;

bucket_builder! {
    /// Builder for [`Client::get_bucket_acl`].
    GetBucketAcl
}

impl GetBucketAcl {
    /// GET `/{bucket}?acl`.
    pub async fn send(self) -> Result<AccessControlList> {
        validate_bucket(&self.bucket)?;
        let mut cr = ClientRequest::new(&self.bucket, "", Method::GET)
            .read_only()
            .with_timeout(self.timeout)?;
        cr.query.add("acl", "");
        self.client.do_json(cr).await
    }
}

/// Builder for [`Client::set_bucket_acl`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct SetBucketAcl {
    client: Client,
    bucket: String,
    acl: AccessControlList,
    action: AclAction,
    timeout: Option<Duration>,
}

impl SetBucketAcl {
    timeout_option!();

    /// Add the grants (the default) or delete them.
    pub fn action(mut self, action: AclAction) -> Self {
        self.action = action;
        self
    }

    /// PUT `/{bucket}?acl[&action=delete]` with the ACL as JSON body.
    pub async fn send(self) -> Result<()> {
        validate_bucket(&self.bucket)?;
        let body = serde_json::to_vec(&self.acl).map_err(Error::json_encode)?;
        let mut cr =
            ClientRequest::new(&self.bucket, "", Method::PUT).with_timeout(self.timeout)?;
        cr.query.add("acl", "");
        self.action.apply(&mut cr.query);
        cr.body = Some(Bytes::from(body).into());
        self.client.do_discard(cr).await
    }
}

impl Client {
    /// Get a bucket's ACL.
    pub fn get_bucket_acl(&self, bucket: impl Into<String>) -> GetBucketAcl {
        GetBucketAcl::new(self, bucket)
    }

    /// Add exactly the supplied grants to a bucket's ACL, or delete them with
    /// [`SetBucketAcl::action`].
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client, acl: fds::acl::AccessControlList) -> fds::Result<()> {
    /// use fds::acl::AclAction;
    ///
    /// client
    ///     .set_bucket_acl("bucket", acl)
    ///     .action(AclAction::Delete)
    ///     .send()
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub fn set_bucket_acl(
        &self,
        bucket: impl Into<String>,
        acl: AccessControlList,
    ) -> SetBucketAcl {
        SetBucketAcl {
            client: self.clone(),
            bucket: bucket.into(),
            acl,
            action: AclAction::Add,
            timeout: None,
        }
    }
}
