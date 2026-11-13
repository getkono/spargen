//! The tri-state value of an optional, nullable member: absent, an explicit `null`, or a value.
//!
//! A schema member that is both optional and nullable has three wire states — the key missing,
//! the key holding `null`, and the key holding a value — and a request that clears a member (a
//! JSON Merge Patch's `{"nickname": null}`) needs the second as much as a decoder needs to tell
//! it from the first. `Option<T>` holds two of them, so a generated model declares such a member
//! as a `Nullable<T>` instead, and a generated `…Params` bundle does the same for a parameter that
//! sends an explicit `null`.
//!
//! On the wire it is an `Option<T>`: `Value` writes the value, `Null` writes `null`, and decoding
//! reads `null` as `Null` and anything else as `Value` of `T`'s own decode. `Absent` is the field's
//! business: a generated field skips it when serializing
//! (`skip_serializing_if = "Nullable::is_absent"`) and decodes a missing key as it
//! (`#[serde(default)]`). Serialized anywhere else, `Absent` writes `null`, as `Null` does.

use serde::de::{Deserialize, Deserializer, Visitor};
use serde::ser::{Serialize, Serializer};
use std::fmt;
use std::marker::PhantomData;

/// An optional, nullable member: absent, an explicit `null`, or a value.
///
/// `Default` is `Absent`, so a member nobody sets is left off the wire. `From<T>` gives `Value`,
/// and `From<Option<T>>` maps `None` to `Null`, so a generated `with_<field>` setter taking
/// `impl Into<Nullable<T>>` accepts `value`, `Some(value)` and `None` alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Nullable<T> {
    /// The member is left off the wire, or was missing from what was decoded.
    #[default]
    Absent,
    /// The member is an explicit `null`.
    Null,
    /// The member holds a value.
    Value(T),
}

impl<T> Nullable<T> {
    /// Whether the member is absent: the predicate a generated field's `skip_serializing_if`
    /// names.
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }

    /// Whether the member is an explicit `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Whether the member holds a value.
    pub fn is_value(&self) -> bool {
        matches!(self, Self::Value(_))
    }

    /// The value, borrowed: `Some` for `Value`, and `None` for both `Null` and `Absent`.
    pub fn as_option(&self) -> Option<&T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Null | Self::Absent => None,
        }
    }

    /// The value: `Some` for `Value`, and `None` for both `Null` and `Absent`.
    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Null | Self::Absent => None,
        }
    }

    /// The same state, with any value borrowed.
    pub fn as_ref(&self) -> Nullable<&T> {
        match self {
            Self::Absent => Nullable::Absent,
            Self::Null => Nullable::Null,
            Self::Value(value) => Nullable::Value(value),
        }
    }
}

impl<T> From<T> for Nullable<T> {
    fn from(value: T) -> Self {
        Self::Value(value)
    }
}

impl<T> From<Option<T>> for Nullable<T> {
    /// `Some(value)` is `Value`, and `None` is an explicit `Null`, never `Absent`.
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::Value(value),
            None => Self::Null,
        }
    }
}

impl<T: Serialize> Serialize for Nullable<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Value(value) => serializer.serialize_some(value),
            Self::Null | Self::Absent => serializer.serialize_none(),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Nullable<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_option(NullableVisitor(PhantomData))
    }
}

/// Reads what `Option<T>` reads: `null` (or a unit) as `Null`, anything else as `Value`.
struct NullableVisitor<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for NullableVisitor<T> {
    type Value = Nullable<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("null or a value")
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Nullable::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Nullable::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Nullable::Value)
    }
}

#[cfg(test)]
mod tests {
    use super::Nullable;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Patch {
        #[serde(default, skip_serializing_if = "Nullable::is_absent")]
        nickname: Nullable<String>,
        #[serde(default, skip_serializing_if = "Nullable::is_absent")]
        age: Nullable<u32>,
    }

    #[test]
    fn each_state_round_trips_through_a_skipping_field() {
        let cases = [
            (
                Patch {
                    nickname: Nullable::Absent,
                    age: Nullable::Absent,
                },
                "{}",
            ),
            (
                Patch {
                    nickname: Nullable::Null,
                    age: Nullable::Absent,
                },
                r#"{"nickname":null}"#,
            ),
            (
                Patch {
                    nickname: Nullable::Value("Rex".to_owned()),
                    age: Nullable::Value(3),
                },
                r#"{"nickname":"Rex","age":3}"#,
            ),
        ];
        for (patch, json) in cases {
            assert_eq!(serde_json::to_string(&patch).unwrap(), json);
            assert_eq!(serde_json::from_str::<Patch>(json).unwrap(), patch);
        }
    }

    #[test]
    fn a_present_value_is_decoded_as_the_inner_type_would_be() {
        let error = serde_json::from_str::<Patch>(r#"{"age":"three"}"#).unwrap_err();
        assert!(error.to_string().contains("invalid type"), "{error}");
        assert_eq!(
            serde_json::from_str::<Nullable<u32>>("null").unwrap(),
            Nullable::Null
        );
        assert_eq!(
            serde_json::from_str::<Nullable<u32>>("7").unwrap(),
            Nullable::Value(7)
        );
    }

    #[test]
    fn absent_writes_null_where_no_field_skips_it() {
        assert_eq!(
            serde_json::to_string(&Nullable::<u32>::Absent).unwrap(),
            "null"
        );
        assert_eq!(
            serde_json::to_string(&Nullable::<u32>::Null).unwrap(),
            "null"
        );
    }

    #[test]
    fn conversions_and_accessors_agree_on_each_state() {
        assert_eq!(Nullable::<u8>::default(), Nullable::Absent);
        assert_eq!(Nullable::from(4_u8), Nullable::Value(4));
        assert_eq!(Nullable::from(Some(4_u8)), Nullable::Value(4));
        assert_eq!(Nullable::<u8>::from(None), Nullable::Null);

        let absent = Nullable::<u8>::Absent;
        let null = Nullable::<u8>::Null;
        let value = Nullable::Value(4_u8);
        assert_eq!(
            [absent.is_absent(), null.is_absent(), value.is_absent()],
            [true, false, false]
        );
        assert_eq!(
            [absent.is_null(), null.is_null(), value.is_null()],
            [false, true, false]
        );
        assert_eq!(
            [absent.is_value(), null.is_value(), value.is_value()],
            [false, false, true]
        );
        assert_eq!(
            [absent.as_option(), null.as_option(), value.as_option()],
            [None, None, Some(&4)]
        );
        assert_eq!(
            [
                absent.into_option(),
                null.into_option(),
                value.into_option()
            ],
            [None, None, Some(4)]
        );
        assert_eq!(
            [absent.as_ref(), null.as_ref(), value.as_ref()],
            [Nullable::Absent, Nullable::Null, Nullable::Value(&4)]
        );
    }
}
