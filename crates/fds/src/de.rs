//! serde helpers for deserializing FDS responses the way Go's `encoding/json`
//! does — i.e. a JSON `null` for a non-pointer field is treated as the zero
//! value instead of an error.
//!
//! FDS frequently returns `null` for absent scalars/slices (e.g. `marker`,
//! `nextMarker`, `delimiter`, `commonPrefixes`), which serde would otherwise
//! reject with `invalid type: null, expected a string`.

use serde::{Deserialize, Deserializer};

/// Deserialize `T`, mapping an explicit JSON `null` to `T::default()`. Combine
/// with `#[serde(default)]` so a *missing* key is also defaulted:
///
/// ```ignore
/// #[serde(rename = "marker", default, deserialize_with = "crate::de::null_default")]
/// pub marker: String,
/// ```
///
/// Also correct for `Option<T>` fields: a `null` yields `None`, a value yields
/// `Some(..)`, a missing key yields `None`.
pub(crate) fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}
