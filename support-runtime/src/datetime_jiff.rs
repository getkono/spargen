//! RFC 3339 date-time and full-date newtypes over `jiff`, for `format: date-time` and
//! `format: date` under the `jiff` date-time backend.
//!
//! The wire form is spargen's own RFC 3339 codec (`rfc3339.rs`), not `jiff`'s parser or `serde`
//! support, so the strings read and written are the ones every backend reads and writes.
//!
//! A `date-time` is held as the civil date-time the sender wrote and the offset it wrote it at, and
//! is written back at that offset. RFC 3339's `date-fullyear` is four digits, so every year from
//! `0000` to `9999` at any offset is a valid `date-time`; `jiff::Timestamp` (and `jiff::Zoned`,
//! which is built on it) ends at `9999-12-30T22:00:00.999999999Z`, so neither holds all of them.
//! `jiff::civil::DateTime` spans years `-9999` to `9999`, so the pair holds every value RFC 3339
//! can write.
//!
//! `Date` is a transparent wrapper: it `Deref`s to `jiff::civil::Date` and converts with `From` in
//! both directions. `DateTime` holds two `jiff` values, so it has accessors for each instead, and
//! converts from a `jiff::Timestamp` (at UTC) and fallibly back to one.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::str::FromStr;

use serde::de::{Deserialize, Deserializer};
use serde::ser::{Serialize, Serializer};

pub use crate::rfc3339::DateParseError;
use crate::rfc3339::{DateTimeFields, FullDate, ParseVisitor};

/// An RFC 3339 `date-time`, the wire form of OpenAPI's `format: date-time`: a civil date-time and
/// the offset from UTC it is at.
///
/// Equality, ordering and hashing are by the instant the value names, as for `time` and `chrono`'s
/// date-times: `12:00+01:00` equals `11:00Z`. The parts are what [`DateTime::datetime`] and
/// [`DateTime::offset`] read back.
#[derive(Debug, Clone, Copy)]
pub struct DateTime {
    /// The wall-clock date and time at `offset`.
    datetime: jiff::civil::DateTime,
    /// The offset from UTC `datetime` is at.
    offset: jiff::tz::Offset,
}

/// An RFC 3339 `full-date` (`YYYY-MM-DD`), the wire form of OpenAPI's `format: date`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(pub jiff::civil::Date);

/// What both newtypes implement alike: a `Display` and `Serialize` that write the RFC 3339 form,
/// and a `Deserialize` that parses the string through the type's `FromStr`.
macro_rules! rfc3339_text {
    ($name:ident, $what:literal) => {
        impl fmt::Display for $name {
            /// The RFC 3339 rendering, the same bytes `Serialize` writes, so a `multipart/form-data`
            /// text part or `text/plain` body (rendered through `Display`) carries the encoding a
            /// JSON body does. A value RFC 3339 cannot express (a year before 0, or an offset with
            /// seconds or of a day or more) falls back to `jiff`'s own rendering rather than
            /// failing, because a failing `Display` panics through `to_string()`; `Serialize` still
            /// refuses such a value.
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self.rfc3339() {
                    Some(text) => formatter.write_str(&text),
                    None => self.fallback(formatter),
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let text = self.rfc3339().ok_or_else(|| {
                    serde::ser::Error::custom(format!(
                        "{self} has no RFC 3339 {} representation",
                        $what
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

rfc3339_text!(DateTime, "date-time");
rfc3339_text!(Date, "full-date");

impl DateTime {
    /// `datetime` at `offset` from UTC.
    pub fn new(datetime: jiff::civil::DateTime, offset: jiff::tz::Offset) -> Self {
        Self { datetime, offset }
    }

    /// The civil (wall-clock) date and time, at [`DateTime::offset`].
    pub fn datetime(&self) -> jiff::civil::DateTime {
        self.datetime
    }

    /// The offset from UTC the civil date-time is at.
    pub fn offset(&self) -> jiff::tz::Offset {
        self.offset
    }

    /// The instant this value names, or an error where it is outside `jiff::Timestamp`'s range
    /// (for one, `9999-12-31T23:59:59Z`).
    pub fn to_timestamp(&self) -> Result<jiff::Timestamp, jiff::Error> {
        self.offset.to_timestamp(self.datetime)
    }

    /// The instant as a duration from `0000-01-01T00:00:00Z`, which every value has: the civil
    /// date-time is at most ten thousand years from that origin, and the offset under a day.
    fn instant(&self) -> jiff::SignedDuration {
        self.datetime.duration_since(jiff::civil::DateTime::ZERO)
            - jiff::SignedDuration::from_secs(i64::from(self.offset.seconds()))
    }

    /// `jiff`'s rendering of the two parts, for a value RFC 3339 cannot express.
    fn fallback(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}{}", self.datetime, self.offset)
    }
}

impl PartialEq for DateTime {
    fn eq(&self, other: &Self) -> bool {
        self.instant() == other.instant()
    }
}

impl Eq for DateTime {}

impl PartialOrd for DateTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DateTime {
    fn cmp(&self, other: &Self) -> Ordering {
        self.instant().cmp(&other.instant())
    }
}

impl Hash for DateTime {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.instant().hash(state);
    }
}

impl From<jiff::Timestamp> for DateTime {
    /// The instant, at UTC.
    fn from(timestamp: jiff::Timestamp) -> Self {
        Self::new(
            jiff::tz::Offset::UTC.to_datetime(timestamp),
            jiff::tz::Offset::UTC,
        )
    }
}

impl TryFrom<DateTime> for jiff::Timestamp {
    type Error = jiff::Error;

    /// The instant, as [`DateTime::to_timestamp`] gives it.
    fn try_from(value: DateTime) -> Result<Self, Self::Error> {
        value.to_timestamp()
    }
}

impl Date {
    /// The wrapped `jiff` value.
    pub fn into_inner(self) -> jiff::civil::Date {
        self.0
    }

    /// `jiff`'s rendering, for a year RFC 3339 cannot express.
    fn fallback(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl Deref for Date {
    type Target = jiff::civil::Date;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Date {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<jiff::civil::Date> for Date {
    fn from(value: jiff::civil::Date) -> Self {
        Self(value)
    }
}

impl From<Date> for jiff::civil::Date {
    fn from(value: Date) -> Self {
        value.0
    }
}

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
    /// The RFC 3339 text at the value's own offset, or `None` for a year before 0 or an offset
    /// RFC 3339 cannot write.
    fn rfc3339(&self) -> Option<String> {
        let local = self.datetime;
        crate::rfc3339::format_date_time(DateTimeFields {
            date: full_date(local.date())?,
            hour: u8::try_from(local.hour()).ok()?,
            minute: u8::try_from(local.minute()).ok()?,
            second: u8::try_from(local.second()).ok()?,
            nanosecond: u32::try_from(local.subsec_nanosecond()).ok()?,
            offset_seconds: self.offset.seconds(),
        })
    }
}

impl FromStr for DateTime {
    type Err = DateParseError;

    /// Parses an RFC 3339 `date-time`, keeping its offset.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let value = || {
            let fields = crate::rfc3339::parse_date_time(text)?;
            let time = jiff::civil::Time::new(
                i8::try_from(fields.hour).ok()?,
                i8::try_from(fields.minute).ok()?,
                i8::try_from(fields.second).ok()?,
                i32::try_from(fields.nanosecond).ok()?,
            )
            .ok()?;
            let offset = jiff::tz::Offset::from_seconds(fields.offset_seconds).ok()?;
            Some(Self::new(
                civil_date(fields.date)?.to_datetime(time),
                offset,
            ))
        };
        value().ok_or_else(DateParseError::date_time)
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
