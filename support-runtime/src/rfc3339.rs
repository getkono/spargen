//! The RFC 3339 codec the `jiff` and `chrono` date-time backends share.
//!
//! `format: date-time` is an RFC 3339 `date-time` and `format: date` an RFC 3339 `full-date`. The
//! `time` backend (`datetime.rs`) delegates to `time`'s own RFC 3339 support; the other backends
//! read and write the text here instead, so their wire form is one grammar whichever crate holds
//! the value, rather than each crate's own notion of what a timestamp string may look like. A
//! backend converts between these fields and its own types, and the calendar (the number of days
//! in a month) is checked by that conversion.
//!
//! The grammar accepted is RFC 3339's `date-time`: `YYYY-MM-DD`, `T` (or `t`), `HH:MM:SS`, an
//! optional `.` and one or more fraction digits, then `Z` (or `z`) or `±HH:MM`. Fraction digits past
//! the ninth (below a nanosecond) are dropped. A leap second (`:60`) is accepted only where `time`
//! accepts it, in the minute that is 23:59 UTC on a month's last day, and reads as the last
//! nanosecond of that minute (`59.999999999`), as `time` reads it, since neither `jiff` nor
//! `chrono`'s fixed offset types carry it otherwise. Writing gives `T`, the fraction trimmed of trailing zeros (and
//! no fraction at all for a whole second), and `Z` for a zero offset.

use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;

use serde::de::{Unexpected, Visitor};

/// A value could not be read as its RFC 3339 form: the `FromStr` error of `Date` and `DateTime`.
///
/// A generated client re-exports it at its root beside those two types, where a bare `ParseError`
/// would read as the error of any parse and would claim an operation named `parse`'s error type.
#[derive(Debug)]
pub struct DateParseError {
    /// What was expected, for the message.
    expected: &'static str,
}

impl DateParseError {
    /// The error of a string that is not an RFC 3339 `date-time`.
    pub(crate) fn date_time() -> Self {
        Self {
            expected: "date-time",
        }
    }

    /// The error of a string that is not an RFC 3339 `full-date`.
    pub(crate) fn full_date() -> Self {
        Self {
            expected: "full-date (YYYY-MM-DD)",
        }
    }
}

impl fmt::Display for DateParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "expected an RFC 3339 {}", self.expected)
    }
}

impl std::error::Error for DateParseError {}

/// Parses a string through `T`'s `FromStr`, reading it in place: a borrowed or transient string is
/// never copied into an owned one first. It accepts what `String` does (a string, or UTF-8 bytes)
/// and names the same expectation, so a value of the wrong type is refused with the same message.
pub(crate) struct ParseVisitor<T>(pub(crate) PhantomData<T>);

impl<T: FromStr<Err = DateParseError>> Visitor<'_> for ParseVisitor<T> {
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string")
    }

    fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<T, E> {
        text.parse().map_err(E::custom)
    }

    fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<T, E> {
        match std::str::from_utf8(bytes) {
            Ok(text) => self.visit_str(text),
            Err(_) => Err(E::invalid_value(Unexpected::Bytes(bytes), &self)),
        }
    }
}

/// A `full-date`'s three fields. Parsing bounds the month to `1..=12` and the day to `1..=31`; the
/// backend's conversion rejects a day its month does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FullDate {
    pub(crate) year: i32,
    pub(crate) month: u8,
    pub(crate) day: u8,
}

/// A `date-time`'s fields: the date and wall-clock time at `offset_seconds` east of UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DateTimeFields {
    pub(crate) date: FullDate,
    pub(crate) hour: u8,
    pub(crate) minute: u8,
    pub(crate) second: u8,
    pub(crate) nanosecond: u32,
    pub(crate) offset_seconds: i32,
}

/// The value of `digits`, when every byte of it is an ASCII digit. Integer `FromStr` would also
/// accept a leading sign (`+1`), which no RFC 3339 field admits.
fn number(digits: &[u8]) -> Option<u32> {
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    digits.iter().try_fold(0_u32, |value, digit| {
        value.checked_mul(10)?.checked_add(u32::from(digit - b'0'))
    })
}

/// `bytes` read as `NN:NN` (with `separator` between), the two numbers it holds.
fn pair(bytes: &[u8], separator: u8) -> Option<(u32, u32)> {
    match bytes {
        [a, b, sep, c, d] if *sep == separator => Some((number(&[*a, *b])?, number(&[*c, *d])?)),
        _ => None,
    }
}

/// Parses exactly `YYYY-MM-DD`. The whole string must be the date, so a longer one (an accidental
/// full `date-time`, say) is rejected rather than silently truncated to its date part.
pub(crate) fn parse_full_date(text: &str) -> Option<FullDate> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = i32::try_from(number(&bytes[0..4])?).ok()?;
    let month = u8::try_from(number(&bytes[5..7])?).ok()?;
    let day = u8::try_from(number(&bytes[8..10])?).ok()?;
    ((1..=12).contains(&month) && (1..=31).contains(&day)).then_some(FullDate { year, month, day })
}

/// `date` as `YYYY-MM-DD`, or `None` for a year outside `0..=9999`, which the format's four-digit
/// year cannot express.
pub(crate) fn format_full_date(date: FullDate) -> Option<String> {
    (0..=9999)
        .contains(&date.year)
        .then(|| format!("{:04}-{:02}-{:02}", date.year, date.month, date.day))
}

/// Parses an RFC 3339 `date-time` into its fields, with the hour, minute, second and offset
/// bounded as the grammar bounds them.
pub(crate) fn parse_date_time(text: &str) -> Option<DateTimeFields> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || !matches!(bytes[10], b'T' | b't') {
        return None;
    }
    let date = parse_full_date(text.get(0..10)?)?;
    let (hour, minute) = pair(&bytes[11..16], b':')?;
    if bytes[16] != b':' {
        return None;
    }
    let mut second = number(&bytes[17..19])?;
    let mut rest = &bytes[19..];
    let mut nanosecond = 0_u32;
    if let [b'.', fraction @ ..] = rest {
        let digits = fraction
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits == 0 {
            return None;
        }
        // The first nine digits are the nanoseconds; any past them are below a nanosecond.
        let kept = &fraction[..digits.min(9)];
        nanosecond = number(kept)? * 10_u32.pow(9 - kept.len() as u32);
        rest = &fraction[digits..];
    }
    let offset_seconds = match rest {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), offset @ ..] => {
            let (hours, minutes) = pair(offset, b':')?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let seconds = i32::try_from(hours * 3600 + minutes * 60).ok()?;
            if *sign == b'-' {
                -seconds
            } else {
                seconds
            }
        }
        _ => return None,
    };
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    if second == 60 {
        if !is_leap_second_minute(date, hour * 60 + minute, offset_seconds) {
            return None;
        }
        second = 59;
        nanosecond = 999_999_999;
    }
    Some(DateTimeFields {
        date,
        hour: u8::try_from(hour).ok()?,
        minute: u8::try_from(minute).ok()?,
        second: u8::try_from(second).ok()?,
        nanosecond,
        offset_seconds,
    })
}

/// The number of days in `month` of `year`, under the proleptic Gregorian calendar.
fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Whether the wall-clock minute `local_minute` (minutes past midnight) of `date`, at
/// `offset_seconds` east of UTC, is 23:59 UTC on the last day of a month: the only minute a leap
/// second can end, and the only one `time` accepts `:60` in. An offset is under a day, so 23:59 UTC
/// falls on the local date or the day before it, never the day after.
fn is_leap_second_minute(date: FullDate, local_minute: u32, offset_seconds: i32) -> bool {
    let Ok(local_minute) = i32::try_from(local_minute) else {
        return false;
    };
    let utc_minute = local_minute - offset_seconds / 60;
    if utc_minute.rem_euclid(24 * 60) != 23 * 60 + 59 {
        return false;
    }
    match utc_minute.div_euclid(24 * 60) {
        // The UTC date is the day before: a month's last day exactly when the local date is a 1st.
        -1 => date.day == 1,
        0 => date.day == days_in_month(date.year, date.month),
        _ => false,
    }
}

/// `fields` as an RFC 3339 `date-time`, or `None` where RFC 3339 cannot express it: a year outside
/// `0..=9999`, or an offset that is not a whole number of minutes under a day.
pub(crate) fn format_date_time(fields: DateTimeFields) -> Option<String> {
    let mut text = format_full_date(fields.date)?;
    text.push_str(&format!(
        "T{:02}:{:02}:{:02}",
        fields.hour, fields.minute, fields.second
    ));
    if fields.nanosecond != 0 {
        let fraction = format!("{:09}", fields.nanosecond);
        text.push('.');
        text.push_str(fraction.trim_end_matches('0'));
    }
    let offset = fields.offset_seconds;
    if offset == 0 {
        text.push('Z');
    } else {
        let magnitude = offset.unsigned_abs();
        if !magnitude.is_multiple_of(60) || magnitude >= 24 * 3600 {
            return None;
        }
        let sign = if offset < 0 { '-' } else { '+' };
        text.push_str(&format!(
            "{sign}{:02}:{:02}",
            magnitude / 3600,
            magnitude % 3600 / 60
        ));
    }
    Some(text)
}

#[cfg(test)]
mod tests;
