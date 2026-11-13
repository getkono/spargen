//! RFC 3339 date-time and full-date newtypes over `chrono`, for `format: date-time` and
//! `format: date` under the `chrono` date-time backend.
//!
//! The wire form is spargen's own RFC 3339 codec (`rfc3339.rs`), not `chrono`'s parser or `serde`
//! support, so the strings read and written are the ones every backend reads and writes. A decoded
//! `date-time` keeps the offset it was written with, and is encoded with that offset again.
//!
//! Both types are transparent wrappers — they `Deref` to the `chrono` type and convert with `From`
//! in both directions — so anything `chrono` can do with the inner value is one `.0` or one deref
//! away.

use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::str::FromStr;

use chrono::{Datelike, TimeZone, Timelike};
use serde::de::{Deserialize, Deserializer};
use serde::ser::{Serialize, Serializer};

pub use crate::rfc3339::DateParseError;
use crate::rfc3339::{DateTimeFields, FullDate, ParseVisitor};

/// An RFC 3339 `date-time`, the wire form of OpenAPI's `format: date-time`, with its offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DateTime(pub chrono::DateTime<chrono::FixedOffset>);

/// An RFC 3339 `full-date` (`YYYY-MM-DD`), the wire form of OpenAPI's `format: date`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(pub chrono::NaiveDate);

/// What both newtypes implement alike over their wrapped `chrono` value: `into_inner`, `Deref` and
/// `DerefMut`, `From` in both directions, a `Display` and `Serialize` that write the RFC 3339 form,
/// and a `Deserialize` that parses the string through the type's `FromStr`.
macro_rules! transparent_newtype {
    ($name:ident, $inner:ty, $what:literal) => {
        impl $name {
            /// The wrapped `chrono` value.
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
            /// JSON body does. A value RFC 3339 cannot express (a year outside `0..=9999`, or an
            /// offset with seconds) falls back to `chrono`'s own rendering rather than failing,
            /// because a failing `Display` panics through `to_string()`; `Serialize` still refuses
            /// such a value.
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

transparent_newtype!(DateTime, chrono::DateTime<chrono::FixedOffset>, "date-time");
transparent_newtype!(Date, chrono::NaiveDate, "full-date");

/// `date`'s fields.
fn full_date(date: chrono::NaiveDate) -> Option<FullDate> {
    Some(FullDate {
        year: date.year(),
        month: u8::try_from(date.month()).ok()?,
        day: u8::try_from(date.day()).ok()?,
    })
}

/// The `chrono::NaiveDate` of `fields`, or `None` for a day its month does not have.
fn naive_date(fields: FullDate) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::from_ymd_opt(fields.year, u32::from(fields.month), u32::from(fields.day))
}

impl DateTime {
    /// The RFC 3339 text at the value's own offset, or `None` for a year outside `0..=9999` or an
    /// offset RFC 3339 cannot write.
    fn rfc3339(&self) -> Option<String> {
        let local = self.0.naive_local();
        let date = full_date(local.date())?;
        let offset_seconds = self.0.offset().local_minus_utc();
        // `chrono` holds a leap second as a nanosecond count past one second, at second 59 of any
        // minute. RFC 3339 writes it as second 60, which the codec (like `time`) reads only in the
        // minute that is 23:59 UTC on a month's last day; in any other minute it is written as the
        // last nanosecond of the second, which every backend reads.
        let (second, nanosecond) = match local.nanosecond() {
            leap @ 1_000_000_000.. => {
                if crate::rfc3339::is_leap_second_minute(
                    date,
                    local.hour() * 60 + local.minute(),
                    offset_seconds,
                ) {
                    (60, leap - 1_000_000_000)
                } else {
                    (local.second(), 999_999_999)
                }
            }
            nanosecond => (local.second(), nanosecond),
        };
        crate::rfc3339::format_date_time(DateTimeFields {
            date,
            hour: u8::try_from(local.hour()).ok()?,
            minute: u8::try_from(local.minute()).ok()?,
            second: u8::try_from(second).ok()?,
            nanosecond,
            offset_seconds,
        })
    }
}

impl FromStr for DateTime {
    type Err = DateParseError;

    /// Parses an RFC 3339 `date-time`, keeping its offset.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let value = || {
            let fields = crate::rfc3339::parse_date_time(text)?;
            let local = naive_date(fields.date)?.and_hms_nano_opt(
                u32::from(fields.hour),
                u32::from(fields.minute),
                u32::from(fields.second),
                fields.nanosecond,
            )?;
            chrono::FixedOffset::east_opt(fields.offset_seconds)?
                .from_local_datetime(&local)
                .single()
        };
        value().map(Self).ok_or_else(DateParseError::date_time)
    }
}

impl Date {
    /// The RFC 3339 `full-date`, or `None` for a year outside `0..=9999`.
    fn rfc3339(&self) -> Option<String> {
        crate::rfc3339::format_full_date(full_date(self.0)?)
    }
}

impl FromStr for Date {
    type Err = DateParseError;

    /// Parses exactly `YYYY-MM-DD`, rejecting a day its month does not have.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        crate::rfc3339::parse_full_date(text)
            .and_then(naive_date)
            .map(Self)
            .ok_or_else(DateParseError::full_date)
    }
}

#[cfg(test)]
mod tests;
