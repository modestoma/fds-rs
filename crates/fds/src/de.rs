//! serde helpers for lenient FDS response models.
//!
//! FDS frequently returns `null` for absent scalars/slices (e.g. `marker`,
//! `nextMarker`, `delimiter`, `commonPrefixes`), which serde would otherwise
//! reject with `invalid type: null, expected a string`. Lenient models treat a
//! JSON `null` exactly like a missing key: the field takes its default value.

use chrono::{DateTime, Utc};
use serde::de::{self, Unexpected};
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

/// `Option<DateTime<Utc>>` from Unix epoch milliseconds: `null` reads as
/// `None` (pair it with a container `default` for missing keys). A negative or
/// unrepresentable timestamp, or a value that is not an integer, is an error,
/// which a response decoder reports as a protocol violation.
pub(crate) fn opt_millis<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<i64>::deserialize(deserializer)?
        .map(from_millis)
        .transpose()
}

/// A required `DateTime<Utc>` from Unix epoch milliseconds; `null` is an
/// error, as are the values [`opt_millis`] rejects.
pub(crate) fn millis<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    from_millis(i64::deserialize(deserializer)?)
}

fn from_millis<E: de::Error>(millis: i64) -> Result<DateTime<Utc>, E> {
    const EXPECTED: &str = "a non-negative Unix timestamp in milliseconds";
    if millis < 0 {
        return Err(E::invalid_value(Unexpected::Signed(millis), &EXPECTED));
    }
    DateTime::from_timestamp_millis(millis)
        .ok_or_else(|| E::invalid_value(Unexpected::Signed(millis), &EXPECTED))
}

/// Declare a lenient response model: every field is deserialized with
/// [`null_default`], so `null` reads as the field's default, unless the field
/// names its own null-tolerant deserializer after `=>` (for example
/// [`opt_millis`]).
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
///         pub upload_time: Option<DateTime<Utc>> => "crate::de::opt_millis",
///     }
/// }
/// ```
macro_rules! lenient {
    // Fields are rewritten one at a time into `[$($done)*]`.
    (@fields $head:tt [$($done:tt)*]
        $(#[$field_meta:meta])*
        $field_vis:vis $field:ident : $ty:ty => $with:literal
        $(, $($rest:tt)*)?
    ) => {
        crate::de::lenient!(@fields $head [$($done)*
            $(#[$field_meta])*
            #[serde(deserialize_with = $with)]
            $field_vis $field: $ty,
        ] $($($rest)*)?);
    };
    (@fields $head:tt [$($done:tt)*]
        $(#[$field_meta:meta])*
        $field_vis:vis $field:ident : $ty:ty
        $(, $($rest:tt)*)?
    ) => {
        crate::de::lenient!(@fields $head [$($done)*
            $(#[$field_meta])*
            #[serde(deserialize_with = "crate::de::null_default")]
            $field_vis $field: $ty,
        ] $($($rest)*)?);
    };
    (@fields [$($head:tt)*] [$($done:tt)*]) => {
        $($head)* { $($done)* }
    };
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident { $($fields:tt)* }
    ) => {
        crate::de::lenient!(@fields [$(#[$meta])* $vis struct $name] [] $($fields)*);
    };
}
pub(crate) use lenient;
