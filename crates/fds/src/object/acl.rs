//! Object ACL requests and operations.

use super::validate_object;
use crate::client::ClientRequest;
use crate::{
    AccessControlList, AclAction, Client, Error, Grant, GrantKey, GrantPermission, GrantType,
    HttpMethod, Result,
};
use bytes::Bytes;

/// Input of `get_object_acl` (Go `GetObjectACLRequest`).
#[derive(Debug, Clone, Default)]
pub struct GetObjectACLRequest {
    pub bucket_name: String,
    pub object_name: String,
    /// `versionId` — omitempty.
    pub version_id: Option<String>,
}

/// Input of `set_object_acl` (Go `SetObjectACLRequest`).
#[derive(Debug, Clone, Default)]
pub struct SetObjectACLRequest {
    pub bucket_name: String,
    pub object_name: String,
    pub version_id: Option<String>,
    pub acl: AccessControlList,
    pub action: AclAction,
}

impl GetObjectACLRequest {
    pub fn new(bucket: impl Into<String>, object: impl Into<String>) -> Self {
        Self {
            bucket_name: bucket.into(),
            object_name: object.into(),
            version_id: None,
        }
    }
    pub async fn send(self, client: &Client) -> Result<AccessControlList> {
        client.get_object_acl(&self).await
    }

    pub fn version_id(mut self, value: impl Into<String>) -> Self {
        self.version_id = Some(value.into());
        self
    }
}

impl SetObjectACLRequest {
    /// Add grants by default; use `action` to request deletion.
    pub fn new(
        bucket: impl Into<String>,
        object: impl Into<String>,
        acl: AccessControlList,
    ) -> Self {
        Self {
            bucket_name: bucket.into(),
            object_name: object.into(),
            acl,
            action: AclAction::Add,
            version_id: None,
        }
    }
    pub async fn send(self, client: &Client) -> Result<()> {
        client.set_object_acl(&self).await
    }

    pub fn action(mut self, action: AclAction) -> Self {
        self.action = action;
        self
    }

    pub fn version_id(mut self, value: impl Into<String>) -> Self {
        self.version_id = Some(value.into());
        self
    }
}

impl Client {
    /// Get object ACL (GET `/{b}/{o}?acl[&versionId]`).
    pub async fn get_object_acl(&self, req: &GetObjectACLRequest) -> Result<AccessControlList> {
        validate_object(&req.bucket_name, &req.object_name)?;
        let mut cr =
            ClientRequest::new(&req.bucket_name, &req.object_name, HttpMethod::Get).read_only();
        cr.query.add("acl", "");
        if let Some(v) = &req.version_id {
            if !v.is_empty() {
                cr.query.add("versionId", v);
            }
        }
        self.do_json(cr).await
    }

    /// Set object ACL (PUT `/{b}/{o}?acl[&versionId]`, body = JSON ACL).
    pub async fn set_object_acl(&self, req: &SetObjectACLRequest) -> Result<()> {
        validate_object(&req.bucket_name, &req.object_name)?;
        let body = serde_json::to_vec(&req.acl).map_err(Error::json_encode)?;
        let mut cr = ClientRequest::new(&req.bucket_name, &req.object_name, HttpMethod::Put);
        cr.query.add("acl", "");
        req.action.apply(&mut cr.query);
        if let Some(v) = &req.version_id {
            if !v.is_empty() {
                cr.query.add("versionId", v);
            }
        }
        cr.body = Some(Bytes::from(body).into());
        self.do_discard(cr).await
    }

    /// Make an object public-readable (shortcut around `set_object_acl`).
    pub async fn set_object_public(&self, bucket: &str, object: &str) -> Result<()> {
        let mut acl = AccessControlList::default();
        acl.add_grant(Grant {
            grantee: GrantKey {
                id: "ALL_USERS".to_string(),
                display_name: String::new(),
            },
            permission: GrantPermission::Read,
            grant_type: GrantType::Group,
        });
        self.set_object_acl(&SetObjectACLRequest::new(bucket, object, acl))
            .await
    }
}
