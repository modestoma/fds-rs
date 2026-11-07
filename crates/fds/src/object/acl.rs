//! Object ACL operations.

use super::validate_object;
use crate::acl::{AccessControlList, AclAction, Grant, GrantKey, GrantPermission, GrantType};
use crate::client::ClientRequest;
use crate::{Client, Error, Result};
use bytes::Bytes;
use http::Method;
use std::time::Duration;

/// Builder for [`Client::get_object_acl`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct GetObjectAcl {
    client: Client,
    bucket: String,
    object: String,
    version_id: Option<String>,
    timeout: Option<Duration>,
}

impl GetObjectAcl {
    timeout_option!();

    /// Read the ACL of this object version. An empty value is not sent.
    pub fn version_id(mut self, value: impl Into<String>) -> Self {
        self.version_id = Some(value.into());
        self
    }

    /// GET `/{bucket}/{object}?acl[&versionId]`.
    pub async fn send(self) -> Result<AccessControlList> {
        validate_object(&self.bucket, &self.object)?;
        let mut cr = ClientRequest::new(&self.bucket, &self.object, Method::GET)
            .read_only()
            .with_timeout(self.timeout)?;
        cr.query.add("acl", "");
        add_version_id(&mut cr, self.version_id.as_deref());
        self.client.do_json(cr).await
    }
}

/// Builder for [`Client::set_object_acl`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct SetObjectAcl {
    client: Client,
    bucket: String,
    object: String,
    acl: AccessControlList,
    action: AclAction,
    version_id: Option<String>,
    timeout: Option<Duration>,
}

impl SetObjectAcl {
    timeout_option!();

    /// Add the grants (the default) or delete them.
    pub fn action(mut self, action: AclAction) -> Self {
        self.action = action;
        self
    }

    /// Change the ACL of this object version. An empty value is not sent.
    pub fn version_id(mut self, value: impl Into<String>) -> Self {
        self.version_id = Some(value.into());
        self
    }

    /// PUT `/{bucket}/{object}?acl[&action=delete][&versionId]` with the ACL
    /// as JSON body.
    pub async fn send(self) -> Result<()> {
        validate_object(&self.bucket, &self.object)?;
        let body = serde_json::to_vec(&self.acl).map_err(Error::json_encode)?;
        let mut cr = ClientRequest::new(&self.bucket, &self.object, Method::PUT)
            .with_timeout(self.timeout)?;
        cr.query.add("acl", "");
        self.action.apply(&mut cr.query);
        add_version_id(&mut cr, self.version_id.as_deref());
        cr.body = Some(Bytes::from(body).into());
        self.client.do_discard(cr).await
    }
}

fn add_version_id(cr: &mut ClientRequest<'_>, version_id: Option<&str>) {
    if let Some(version_id) = version_id.filter(|v| !v.is_empty()) {
        cr.query.add("versionId", version_id);
    }
}

object_builder! {
    /// Builder for [`Client::set_object_public`].
    SetObjectPublic
}

impl SetObjectPublic {
    /// The [`SetObjectAcl`] request adding a `READ` grant for the
    /// `ALL_USERS` group.
    pub async fn send(self) -> Result<()> {
        let mut acl = AccessControlList::default();
        acl.add_grant(Grant {
            grantee: GrantKey {
                id: "ALL_USERS".to_string(),
                display_name: String::new(),
            },
            permission: GrantPermission::Read,
            grant_type: GrantType::Group,
        });
        let mut request = self.client.set_object_acl(self.bucket, self.object, acl);
        request.timeout = self.timeout;
        request.send().await
    }
}

impl Client {
    /// Get an object's ACL.
    pub fn get_object_acl(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
    ) -> GetObjectAcl {
        GetObjectAcl {
            client: self.clone(),
            bucket: bucket.into(),
            object: object.into(),
            version_id: None,
            timeout: None,
        }
    }

    /// Add grants to an object's ACL, or delete them with
    /// [`SetObjectAcl::action`].
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client, acl: fds::acl::AccessControlList) -> fds::Result<()> {
    /// use fds::acl::AclAction;
    ///
    /// client
    ///     .set_object_acl("bucket", "key", acl)
    ///     .action(AclAction::Delete)
    ///     .send()
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub fn set_object_acl(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
        acl: AccessControlList,
    ) -> SetObjectAcl {
        SetObjectAcl {
            client: self.clone(),
            bucket: bucket.into(),
            object: object.into(),
            acl,
            action: AclAction::Add,
            version_id: None,
            timeout: None,
        }
    }

    /// Make an object readable by everyone: a shortcut for
    /// [`set_object_acl`](Self::set_object_acl) with a public `READ` grant.
    pub fn set_object_public(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
    ) -> SetObjectPublic {
        SetObjectPublic::new(self, bucket, object)
    }
}
