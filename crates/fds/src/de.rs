//! serde helpers for lenient FDS response models.
//!
//! FDS frequently returns `null` for absent scalars/slices (e.g. `marker`,
//! `nextMarker`, `delimiter`, `commonPrefixes`), which serde would otherwise
//! reject with `invalid type: null, expected a string`. Lenient models treat a
//! JSON `null` exactly like a missing key: the field takes its default value.

use serde::{Deserialize, Deserializer};

/// Deserialize `T`, mapping an explicit JSON `null` to `T::default()`. Pair it
/// with `#[serde(default)]` so a *missing* key is also defaulted.
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

/// Declare a lenient response model: every field is deserialized with
/// [`null_default`], so `null` reads as the field's default.
///
/// The struct keeps its own attributes. Lenient models put
/// `#[serde(rename_all = "camelCase", default)]` on the container, so missing
/// keys take the (derived) `Default` value, and only fields whose wire name is
/// not the camelCase of the field name carry an explicit `rename`. Unknown
/// keys are ignored, as for every model in this crate.
///
/// ```ignore
/// crate::de::lenient! {
///     #[derive(Debug, Clone, Default, Deserialize)]
///     #[serde(rename_all = "camelCase", default)]
///     pub struct Listing {
///         #[serde(rename = "name")]
///         pub bucket_name: String,
///         pub next_marker: String,
///     }
/// }
/// ```
macro_rules! lenient {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                $field_vis:vis $field:ident : $ty:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $name {
            $(
                $(#[$field_meta])*
                #[serde(deserialize_with = "crate::de::null_default")]
                $field_vis $field: $ty,
            )*
        }
    };
}
pub(crate) use lenient;
