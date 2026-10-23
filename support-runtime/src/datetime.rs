//! RFC 3339 date-time and full-date newtypes for `format: date-time` and `format: date`.
//!
//! JSON Schema 2020-12 — the dialect OpenAPI 3.1 and 3.2 use — defines `date-time` as an RFC 3339
//! `date-time` and `date` as an RFC 3339 `full-date`. Neither of those is what `time`'s own `serde`
//! implementations produce:
//!
//! - Without `time`'s `serde-human-readable` feature, `OffsetDateTime` serializes as a **nine-element
//!   integer sequence** and `Date` as a two-element one — not a string at all.
//! - *With* that feature, the human-readable form is `[year]-[month]-[day] [hour]:[minute]:[second]
//!   .[subsecond] [offset]`, which uses a space separator and a seconds-bearing offset. RFC 3339
//!   requires `T` (or a lowercase `t`) and `±HH:MM`.
//!
//! Either way the bytes on the wire would not be what the specification describes, so these types
//! carry their own hand-written `Serialize`/`Deserialize` that are always RFC 3339. They do not
//! consult `is_human_readable`: a generated client speaks JSON, and the specification fixes the
//! representation regardless of the format's self-description.
//!
//! Both types are transparent wrappers — they `Deref` to the `time` type and convert with `From` in
//! both directions — so anything `time` can do with the inner value is one `.0` or one deref away.

use std::fmt;
use std::ops::{Deref, DerefMut};
use std::str::FromStr;

use serde::de::{Deserialize, Deserializer, Error as _};
use serde::ser::{Serialize, Serializer};
use time::format_description::well_known::Rfc3339;

/// An RFC 3339 `date-time`, the wire form of OpenAPI's `format: date-time`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DateTime(pub time::OffsetDateTime);

/// An RFC 3339 `full-date` (`YYYY-MM-DD`), the wire form of OpenAPI's `format: date`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(pub time::Date);

/// A value could not be read as its RFC 3339 form: the `FromStr` error of [`Date`] and
/// [`DateTime`].
///
/// A generated client re-exports it at its root beside those two types, where a bare `ParseError`
/// would read as the error of any parse and would claim an operation named `parse`'s error type.
#[derive(Debug)]
pub struct DateParseError {
    /// What was expected, for the message.
    expected: &'static str,
}

impl fmt::Display for DateParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "expected an RFC 3339 {}", self.expected)
    }
}

impl std::error::Error for DateParseError {}

// --- DateTime ----------------------------------------------------------------------------------

impl DateTime {
    /// The wrapped `time` value.
    pub fn into_inner(self) -> time::OffsetDateTime {
        self.0
    }
}

impl Deref for DateTime {
    type Target = time::OffsetDateTime;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for DateTime {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<time::OffsetDateTime> for DateTime {
    fn from(value: time::OffsetDateTime) -> Self {
        Self(value)
    }
}

impl From<DateTime> for time::OffsetDateTime {
    fn from(value: DateTime) -> Self {
        value.0
    }
}

impl fmt::Display for DateTime {
    /// The RFC 3339 rendering — the same bytes `Serialize` writes.
    ///
    /// This matters beyond convenience: a `multipart/form-data` text part and a `text/plain` body
    /// are rendered through `Display`, so a `Display` that disagreed with `Serialize` would put two
    /// different encodings of the same value on the wire depending on the body's media type.
    ///
    /// RFC 3339 fixes the year at four digits, so a year outside `0..=9999` — which `time` can hold
    /// but RFC 3339 cannot express — has no rendering here. It falls back to `time`'s own form
    /// rather than failing, because `Display` failure would panic through `to_string()` at exactly
    /// those two call sites. `Serialize` still refuses such a value, so it can never reach a JSON
    /// body claiming to be RFC 3339.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.format(&Rfc3339) {
            Ok(text) => formatter.write_str(&text),
            Err(_) => write!(formatter, "{}", self.0),
        }
    }
}

impl FromStr for DateTime {
    type Err = DateParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        time::OffsetDateTime::parse(text, &Rfc3339)
            .map(Self)
            .map_err(|_| DateParseError {
                expected: "date-time",
            })
    }
}

impl Serialize for DateTime {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let text = self.0.format(&Rfc3339).map_err(|error| {
            serde::ser::Error::custom(format!("date-time is not RFC 3339: {error}"))
        })?;
        serializer.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for DateTime {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

// --- Date --------------------------------------------------------------------------------------

impl Date {
    /// The wrapped `time` value.
    pub fn into_inner(self) -> time::Date {
        self.0
    }
}

impl Deref for Date {
    type Target = time::Date;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Date {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<time::Date> for Date {
    fn from(value: time::Date) -> Self {
        Self(value)
    }
}

impl From<Date> for time::Date {
    fn from(value: Date) -> Self {
        value.0
    }
}

impl fmt::Display for Date {
    /// `YYYY-MM-DD`, matching `Serialize` for the reason given on [`DateTime`]'s `Display`, with the
    /// same fallback for a year RFC 3339's four-digit `full-date` cannot express.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match full_date(self.0) {
            Some(text) => formatter.write_str(&text),
            None => write!(formatter, "{}", self.0),
        }
    }
}

/// `date` as an RFC 3339 `full-date` (`YYYY-MM-DD`), or `None` for a year outside `0..=9999`,
/// which the format's four-digit year cannot express.
fn full_date(date: time::Date) -> Option<String> {
    let year = date.year();
    (0..=9999)
        .contains(&year)
        .then(|| format!("{year:04}-{:02}-{:02}", u8::from(date.month()), date.day()))
}

impl FromStr for Date {
    type Err = DateParseError;

    /// Parses exactly `YYYY-MM-DD`. The length and separator positions are checked first so that a
    /// longer string (an accidental full `date-time`, say) is rejected rather than silently
    /// truncated to its date part. Every other byte must be an ASCII digit, because integer
    /// `FromStr` would otherwise accept a sign (`2023-+1-+4`, or year -1 from `-001-01-02`).
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || DateParseError {
            expected: "full-date (YYYY-MM-DD)",
        };
        let bytes = text.as_bytes();
        if bytes.len() != 10
            || !bytes.iter().enumerate().all(|(index, byte)| match index {
                4 | 7 => *byte == b'-',
                _ => byte.is_ascii_digit(),
            })
        {
            return Err(invalid());
        }
        let year: i32 = text[0..4].parse().map_err(|_| invalid())?;
        let month: u8 = text[5..7].parse().map_err(|_| invalid())?;
        let day: u8 = text[8..10].parse().map_err(|_| invalid())?;
        let month = time::Month::try_from(month).map_err(|_| invalid())?;
        time::Date::from_calendar_date(year, month, day)
            .map(Self)
            .map_err(|_| invalid())
    }
}

impl Serialize for Date {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let text = full_date(self.0).ok_or_else(|| {
            serde::ser::Error::custom(format!(
                "date year {} has no RFC 3339 full-date representation",
                self.0.year()
            ))
        })?;
        serializer.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for Date {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests;
