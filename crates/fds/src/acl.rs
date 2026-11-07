//! Access control list types, shared by bucket and object APIs.
//! Ports `fds/acl.go` plus `Owner` (which lives in `bucket.go` in Go).

use serde::{Deserialize, Serialize};

/// Resource owner (Go `Owner`, defined in bucket.go:124-128).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    #[serde(rename = "id", default, deserialize_with = "crate::de::null_default")]
    pub id: String,
    #[serde(
        rename = "displayName",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub display_name: String,
}

/// Grantee key (Go `GrantKey`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantKey {
    #[serde(rename = "id", default, deserialize_with = "crate::de::null_default")]
    pub id: String,
    #[serde(
        rename = "displayName",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub display_name: String,
}

/// Grantee type (Go `GrantType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GrantType {
    #[serde(rename = "USER")]
    User,
    #[serde(rename = "GROUP")]
    Group,
}

/// Grant permission (Go `GrantPermission`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GrantPermission {
    #[serde(rename = "READ")]
    Read,
    #[serde(rename = "WRITE")]
    Write,
    #[serde(rename = "READ_OBJECTS")]
    ReadObjects,
    #[serde(rename = "SSO_WRITE")]
    SsoWrite,
    #[serde(rename = "FULL_CONTROL")]
    FullControl,
}

/// A single grant (Go `Grant`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    #[serde(
        rename = "grantee",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub grantee: GrantKey,
    #[serde(rename = "permission")]
    pub permission: GrantPermission,
    #[serde(rename = "type")]
    pub grant_type: GrantType,
}

/// Access control list (Go `AccessControlList`).
///
/// NOTE: the `grants` field's JSON key is `accessControlList` (Go's field is
/// named `Grants` but tagged `json:"accessControlList"`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccessControlList {
    #[serde(
        rename = "accessControlList",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub grants: Vec<Grant>,
    #[serde(
        rename = "owner",
        default,
        deserialize_with = "crate::de::null_default"
    )]
    pub owner: Owner,
}

impl AccessControlList {
    /// Append a grant (Go `AddGrant`).
    pub fn add_grant(&mut self, grant: Grant) {
        self.grants.push(grant);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_acl_serialization_semantics() {
        // Mirrors SetObjectPublic's ACL.
        let mut acl = AccessControlList::default();
        acl.add_grant(Grant {
            grantee: GrantKey {
                id: "ALL_USERS".into(),
                display_name: String::new(),
            },
            permission: GrantPermission::Read,
            grant_type: GrantType::Group,
        });
        let v: serde_json::Value = serde_json::to_value(&acl).unwrap();
        assert_eq!(v["accessControlList"][0]["grantee"]["id"], "ALL_USERS");
        assert_eq!(v["accessControlList"][0]["grantee"]["displayName"], "");
        assert_eq!(v["accessControlList"][0]["permission"], "READ");
        assert_eq!(v["accessControlList"][0]["type"], "GROUP");
        assert_eq!(v["owner"]["id"], "");
        assert_eq!(v["owner"]["displayName"], "");
    }
}

/// ACL modification. Addition uses the documented omitted-action default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AclAction {
    #[default]
    Add,
    Delete,
}
impl AclAction {
    pub(crate) fn apply(self, query: &mut crate::encoding::QueryValues) {
        if self == Self::Delete {
            query.add("action", "delete");
        }
    }
}

/// Documented canned grants for bucket creation. Multiple grants may be combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketCannedAcl {
    PublicRead,
    PublicWrite,
    PublicReadObjects,
    PublicSsoWrite,
    AuthenticatedRead,
    AuthenticatedWrite,
    AuthenticatedSsoWrite,
}
impl BucketCannedAcl {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PublicRead => "PUBLIC_READ",
            Self::PublicWrite => "PUBLIC_WRITE",
            Self::PublicReadObjects => "PUBLIC_READ_OBJECTS",
            Self::PublicSsoWrite => "PUBLIC_SSO_WRITE",
            Self::AuthenticatedRead => "AUTHENTICATED_READ",
            Self::AuthenticatedWrite => "AUTHENTICATED_WRITE",
            Self::AuthenticatedSsoWrite => "AUTHENTICATED_SSO_WRITE",
        }
    }
}
