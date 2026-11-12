//! RFC 3339 date-time and full-date newtypes over `jiff`, for `format: date-time` and
//! `format: date` under the `jiff` date-time backend.
//!
//! The wire form is spargen's own RFC 3339 codec (`rfc3339.rs`), not `jiff`'s parser or `serde`
//! support, so the strings read and written are the ones every backend reads and writes.
//!
//! `jiff::Timestamp` is an instant: it has no offset. A decoded `date-time` is therefore normalized
//! to UTC, and an encoded one is always written with `Z`. The instant is exact; only the offset the
//! sender wrote is not kept. `jiff::Zoned` would need a time zone, which RFC 3339 does not carry.
//!
//! Both types are transparent wrappers — they `Deref` to the `jiff` type and convert with `From` in
//! both directions — so anything `jiff` can do with the inner value is one `.0` or one deref away.

use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::str::FromStr;

use serde::de::{Deserialize, Deserializer};
use serde::ser::{Serialize, Serializer};

pub use crate::rfc3339::DateParseError;
use crate::rfc3339::{DateTimeFields, FullDate, ParseVisitor};

/// An RFC 3339 `date-time`, the wire form of OpenAPI's `format: date-time`, held as a UTC instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DateTime(pub jiff::Timestamp);

/// An RFC 3339 `full-date` (`YYYY-MM-DD`), the wire form of OpenAPI's `format: date`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(pub jiff::civil::Date);

/// What both newtypes implement alike over their wrapped `jiff` value: `into_inner`, `Deref` and
/// `DerefMut`, `From` in both directions, a `Display` and `Serialize` that write the RFC 3339 form,
/// and a `Deserialize` that parses the string through the type's `FromStr`.
macro_rules! transparent_newtype {
    ($name:ident, $inner:ty, $what:literal) => {
        impl $name {
            /// The wrapped `jiff` value.
            pub fn into_inner(self) -> $inner {
                self.0
            }
        }

        impl Deref for $name {
            type Target = $inner;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl DerefMut for $name {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }

        impl From<$inner> for $name {
            fn from(value: $inner) -> Self {
                Self(value)
            }
        }

        impl From<$name> for $inner {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            /// The RFC 3339 rendering, the same bytes `Serialize` writes, so a `multipart/form-data`
            /// text part or `text/plain` body (rendered through `Display`) carries the encoding a
            /// JSON body does. A year RFC 3339 cannot express (before year 0) falls back to `jiff`'s
            /// own rendering rather than failing, because a failing `Display` panics through
            /// `to_string()`; `Serialize` still refuses such a value.
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self.rfc3339() {
                    Some(text) => formatter.write_str(&text),
                    None => write!(formatter, "{}", self.0),
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let text = self.rfc3339().ok_or_else(|| {
                    serde::ser::Error::custom(format!(
                        "{} has no RFC 3339 {} representation",
                        self.0, $what
                    ))
                })?;
                serializer.serialize_str(&text)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                deserializer.deserialize_str(ParseVisitor(PhantomData))
            }
        }
    };
}

transparent_newtype!(DateTime, jiff::Timestamp, "date-time");
transparent_newtype!(Date, jiff::civil::Date, "full-date");

/// `date`'s fields, which every `jiff::civil::Date` has in range for the codec's types.
fn full_date(date: jiff::civil::Date) -> Option<FullDate> {
    Some(FullDate {
        year: i32::from(date.year()),
        month: u8::try_from(date.month()).ok()?,
        day: u8::try_from(date.day()).ok()?,
    })
}

/// The `jiff::civil::Date` of `fields`, or `None` for a day its month does not have.
fn civil_date(fields: FullDate) -> Option<jiff::civil::Date> {
    jiff::civil::Date::new(
        i16::try_from(fields.year).ok()?,
        i8::try_from(fields.month).ok()?,
        i8::try_from(fields.day).ok()?,
    )
    .ok()
}

impl DateTime {
    /// The RFC 3339 text, in UTC, or `None` for an instant before year 0.
    fn rfc3339(&self) -> Option<String> {
        let utc = jiff::tz::Offset::UTC.to_datetime(self.0);
        crate::rfc3339::format_date_time(DateTimeFields {
            date: full_date(utc.date())?,
            hour: u8::try_from(utc.hour()).ok()?,
            minute: u8::try_from(utc.minute()).ok()?,
            second: u8::try_from(utc.second()).ok()?,
            nanosecond: u32::try_from(utc.subsec_nanosecond()).ok()?,
            offset_seconds: 0,
        })
    }
}

impl FromStr for DateTime {
    type Err = DateParseError;

    /// Parses an RFC 3339 `date-time` and normalizes it to the instant it names.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let instant = || {
            let fields = crate::rfc3339::parse_date_time(text)?;
            let time = jiff::civil::Time::new(
                i8::try_from(fields.hour).ok()?,
                i8::try_from(fields.minute).ok()?,
                i8::try_from(fields.second).ok()?,
                i32::try_from(fields.nanosecond).ok()?,
            )
            .ok()?;
            let local = civil_date(fields.date)?.to_datetime(time);
            jiff::tz::Offset::from_seconds(fields.offset_seconds)
                .ok()?
                .to_timestamp(local)
                .ok()
        };
        instant().map(Self).ok_or_else(DateParseError::date_time)
    }
}

impl Date {
    /// The RFC 3339 `full-date`, or `None` for a year before 0.
    fn rfc3339(&self) -> Option<String> {
        crate::rfc3339::format_full_date(full_date(self.0)?)
    }
}

impl FromStr for Date {
    type Err = DateParseError;

    /// Parses exactly `YYYY-MM-DD`, rejecting a day its month does not have.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        crate::rfc3339::parse_full_date(text)
            .and_then(civil_date)
            .map(Self)
            .ok_or_else(DateParseError::full_date)
    }
}

#[cfg(test)]
mod tests;
