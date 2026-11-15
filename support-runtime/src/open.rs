//! The catch-alls of a generated open string enum and an open discriminated union: the value or
//! payload the description does not list, held where only decoding (or, for an enum, `FromStr`)
//! builds one.
//!
//! An open string enum declares one unit variant per listed value and an `Unknown(UnknownValue<Self>)`
//! variant. Its `FromStr` and `Deserialize` both read through [`parse_open`], which asks the enum's
//! own [`OpenEnum::listed`] first, so `Unknown` never holds a value a unit variant names. An
//! [`UnknownValue`] is typed by the enum it belongs to, and the enum's [`OpenEnum`] impl is the
//! one the generator emits (a second impl for the same type does not compile), so a value built
//! for one enum cannot be moved into another.
//!
//! An open discriminated union's `Unknown(UnknownPayload<Self>)` is built the same way, by
//! [`UnknownPayload::from_payload`], which reads the tag from the payload and refuses one the
//! union's [`OpenUnion::lists_tag`] names.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

/// A generated open string enum: its listed values, and its catch-all.
pub trait OpenEnum: Sized {
    /// The unit variant that lists `value`, matched byte for byte, or `None`.
    fn listed(value: &str) -> Option<Self>;

    /// The catch-all variant holding `value`.
    fn unknown(value: UnknownValue<Self>) -> Self;
}

/// A string an open enum `E` does not list, as decoded or parsed. Built only by [`parse_open`]
/// and [`deserialize_open`], each of which first gives every listed value its own variant.
pub struct UnknownValue<E> {
    value: String,
    enumeration: PhantomData<fn() -> E>,
}

impl<E> UnknownValue<E> {
    /// The value, exactly as it arrived.
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// The value, exactly as it arrived, owned.
    pub fn into_string(self) -> String {
        self.value
    }
}

impl<E> Clone for UnknownValue<E> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            enumeration: PhantomData,
        }
    }
}

impl<E> PartialEq for UnknownValue<E> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<E> Eq for UnknownValue<E> {}

impl<E> Hash for UnknownValue<E> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<E> fmt::Debug for UnknownValue<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("UnknownValue").field(&self.value).finish()
    }
}

impl<E> fmt::Display for UnknownValue<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.value)
    }
}

/// `value` as the open enum `E`: the variant that lists it, else `E`'s catch-all holding it.
pub fn parse_open<E: OpenEnum>(value: &str) -> E {
    E::listed(value).unwrap_or_else(|| {
        E::unknown(UnknownValue {
            value: value.to_owned(),
            enumeration: PhantomData,
        })
    })
}

/// Decode a string as the open enum `E`, as [`parse_open`] reads it. Anything but a string fails.
pub fn deserialize_open<'de, D, E>(deserializer: D) -> Result<E, D::Error>
where
    D: serde::Deserializer<'de>,
    E: OpenEnum,
{
    let value = <String as serde::Deserialize>::deserialize(deserializer)?;
    Ok(E::listed(&value).unwrap_or_else(|| {
        E::unknown(UnknownValue {
            value,
            enumeration: PhantomData,
        })
    }))
}

/// A generated open discriminated union: the property its tag rides in, and the tags it lists.
pub trait OpenUnion: Sized {
    /// The name of the property carrying the tag.
    const TAG_FIELD: &'static str;

    /// Whether `tag` selects one of the union's listed variants.
    fn lists_tag(tag: &str) -> bool;
}

/// A payload of the open union `U` whose tag `U` does not list, as it arrived. Built only by
/// [`UnknownPayload::from_payload`].
pub struct UnknownPayload<U> {
    tag: String,
    value: serde_json::Value,
    union: PhantomData<fn() -> U>,
}

impl<U: OpenUnion> UnknownPayload<U> {
    /// `value` as `U`'s catch-all: `None` where it is not an object, its tag property is missing
    /// or not a string, or `U` lists the tag.
    pub fn from_payload(value: serde_json::Value) -> Option<Self> {
        let tag = value.get(U::TAG_FIELD)?.as_str()?.to_owned();
        if U::lists_tag(&tag) {
            return None;
        }
        Some(Self {
            tag,
            value,
            union: PhantomData,
        })
    }
}

impl<U> UnknownPayload<U> {
    /// The tag the payload carries.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// The whole payload, as it arrived.
    pub fn value(&self) -> &serde_json::Value {
        &self.value
    }

    /// The whole payload, as it arrived, owned.
    pub fn into_value(self) -> serde_json::Value {
        self.value
    }
}

impl<U> Clone for UnknownPayload<U> {
    fn clone(&self) -> Self {
        Self {
            tag: self.tag.clone(),
            value: self.value.clone(),
            union: PhantomData,
        }
    }
}

impl<U> PartialEq for UnknownPayload<U> {
    fn eq(&self, other: &Self) -> bool {
        self.tag == other.tag && self.value == other.value
    }
}

impl<U> fmt::Debug for UnknownPayload<U> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnknownPayload")
            .field("tag", &self.tag)
            .field("value", &self.value)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{deserialize_open, parse_open, OpenEnum, OpenUnion, UnknownPayload, UnknownValue};

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    enum Priority {
        Low,
        Unknown(UnknownValue<Priority>),
    }

    impl OpenEnum for Priority {
        fn listed(value: &str) -> Option<Self> {
            (value == "low").then_some(Priority::Low)
        }

        fn unknown(value: UnknownValue<Self>) -> Self {
            Priority::Unknown(value)
        }
    }

    impl<'de> serde::Deserialize<'de> for Priority {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserialize_open(deserializer)
        }
    }

    #[test]
    fn a_listed_value_is_its_variant_and_never_unknown() {
        assert_eq!(parse_open::<Priority>("low"), Priority::Low);
        assert_eq!(
            serde_json::from_str::<Priority>(r#""low""#).unwrap(),
            Priority::Low
        );
    }

    #[test]
    fn an_unlisted_value_is_held_exactly() {
        let Priority::Unknown(value) = parse_open::<Priority>("Low ") else {
            panic!("an unlisted value decodes as Unknown");
        };
        assert_eq!(value.as_str(), "Low ");
        assert_eq!(value.to_string(), "Low ");
        assert_eq!(format!("{value:?}"), r#"UnknownValue("Low ")"#);
        assert_eq!(
            serde_json::from_str::<Priority>(r#""urgent""#).unwrap(),
            parse_open::<Priority>("urgent")
        );
        assert_eq!(value.into_string(), "Low ");
    }

    #[test]
    fn unknown_values_hash_and_compare_by_their_text() {
        let set: HashSet<Priority> = ["low", "urgent", "urgent", "later"]
            .into_iter()
            .map(parse_open::<Priority>)
            .collect();
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn a_non_string_fails_to_decode() {
        assert!(serde_json::from_str::<Priority>("3").is_err());
    }

    struct Pet;

    impl OpenUnion for Pet {
        const TAG_FIELD: &'static str = "kind";

        fn lists_tag(tag: &str) -> bool {
            tag == "dog"
        }
    }

    #[test]
    fn a_payload_is_unknown_only_under_an_unlisted_string_tag() {
        let payload = serde_json::json!({"kind": "cat", "lives": 9});
        let unknown = UnknownPayload::<Pet>::from_payload(payload.clone()).unwrap();
        assert_eq!(unknown.tag(), "cat");
        assert_eq!(unknown.value(), &payload);
        assert_eq!(unknown.clone(), unknown);
        assert!(format!("{unknown:?}").contains("cat"));
        assert_eq!(unknown.into_value(), payload);
        for refused in [
            serde_json::json!({"kind": "dog"}),
            serde_json::json!({"kind": 3}),
            serde_json::json!({"name": "cat"}),
            serde_json::json!("cat"),
        ] {
            assert!(UnknownPayload::<Pet>::from_payload(refused).is_none());
        }
    }
}
