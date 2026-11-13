use super::*;

fn date_time() -> DateTime {
    DateTime::from(jiff::Timestamp::from_second(1_700_000_000).expect("representable timestamp"))
}

fn offset(seconds: i32) -> jiff::tz::Offset {
    jiff::tz::Offset::from_seconds(seconds).expect("an offset in jiff's range")
}

/// A JSON string in RFC 3339 form, not `jiff`'s own rendering.
#[test]
fn date_time_serializes_as_an_rfc3339_string() {
    let json = serde_json::to_string(&date_time()).unwrap();
    assert_eq!(json, "\"2023-11-14T22:13:20Z\"");
    assert_eq!(date_time().to_string(), "2023-11-14T22:13:20Z");
}

#[test]
fn date_time_round_trips_through_json() {
    let value = DateTime::from(jiff::Timestamp::new(1_700_000_000, 120_000_000).unwrap());
    let json = serde_json::to_string(&value).unwrap();
    assert_eq!(json, "\"2023-11-14T22:13:20.12Z\"");
    assert_eq!(serde_json::from_str::<DateTime>(&json).unwrap(), value);
}

/// The offset a value was written with is kept, and written back as `±HH:MM`; equality is by the
/// instant, so it equals the same instant at UTC.
#[test]
fn date_time_keeps_a_non_utc_offset_in_rfc3339_form() {
    let json = "\"2023-11-14T16:43:20-05:30\"";
    let value: DateTime = serde_json::from_str(json).unwrap();
    assert_eq!(value.offset(), offset(-19_800));
    assert_eq!(
        value.datetime(),
        jiff::civil::datetime(2023, 11, 14, 16, 43, 20, 0)
    );
    assert_eq!(value, date_time());
    assert_eq!(serde_json::to_string(&value).unwrap(), json);
    assert_eq!(value.to_timestamp().unwrap().as_second(), 1_700_000_000);
}

/// Every four-digit year is a valid RFC 3339 `date-time` at any offset, including the instants past
/// `jiff::Timestamp::MAX` (`9999-12-30T22:00:00.999999999Z`): they decode and encode, and only the
/// conversion to a `Timestamp` fails.
#[test]
fn date_time_holds_the_whole_four_digit_year_range() {
    assert_eq!(
        jiff::Timestamp::MAX.to_string(),
        "9999-12-30T22:00:00.999999999Z"
    );
    for text in [
        "9999-12-31T23:59:59Z",
        "9999-12-31T23:59:59.999999999-23:59",
        "9999-12-30T22:00:01Z",
    ] {
        let value: DateTime = text.parse().unwrap();
        assert_eq!(value.to_string(), text);
        assert!(value.to_timestamp().is_err(), "{text}");
        assert!(jiff::Timestamp::try_from(value).is_err(), "{text}");
    }
    let earliest: DateTime = "0000-01-01T00:00:00+23:59".parse().unwrap();
    assert_eq!(earliest.to_string(), "0000-01-01T00:00:00+23:59");
    let max: DateTime = "9999-12-30T22:00:00.999999999Z".parse().unwrap();
    assert_eq!(
        jiff::Timestamp::try_from(max).unwrap(),
        jiff::Timestamp::MAX
    );
}

/// Equality, ordering and hashing go by the instant, not the parts.
#[test]
fn date_time_compares_and_hashes_by_instant() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn hash(value: &DateTime) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    let utc: DateTime = "2024-03-05T11:00:00Z".parse().unwrap();
    let east: DateTime = "2024-03-05T12:00:00+01:00".parse().unwrap();
    let later: DateTime = "2024-03-05T10:30:00-01:00".parse().unwrap();
    assert_eq!(utc, east);
    assert_eq!(hash(&utc), hash(&east));
    assert_ne!(utc.offset(), east.offset());
    assert!(later > utc && later.datetime() < utc.datetime());
    let top: DateTime = "9999-12-31T23:59:59-23:59".parse().unwrap();
    let bottom: DateTime = "0000-01-01T00:00:00+23:59".parse().unwrap();
    assert!(bottom < utc && utc < top);
}

#[test]
fn date_time_rejects_a_non_rfc3339_string() {
    for json in [
        "\"2023-11-14 22:13:20Z\"",
        "\"2023-11-14T22:13:20Z[UTC]\"",
        "\"2023-02-30T00:00:00Z\"",
        "\"2023-11-14\"",
        "1700000000",
    ] {
        assert!(serde_json::from_str::<DateTime>(json).is_err(), "{json}");
    }
    let error = "2023-11-14".parse::<DateTime>().unwrap_err();
    assert_eq!(error.to_string(), "expected an RFC 3339 date-time");
}

/// Before year 0, or at an offset with seconds, there is no RFC 3339 text: `Serialize` refuses the
/// value and `Display` falls back to `jiff`'s rendering rather than panicking through
/// `to_string()`.
#[test]
fn date_time_rfc3339_cannot_express_does_not_serialize() {
    let early = DateTime::from(jiff::Timestamp::MIN);
    let seconds_offset = DateTime::new(jiff::civil::datetime(2024, 3, 5, 12, 0, 0, 0), offset(30));
    for (value, fallback) in [
        (early, "-009999-01-02T01:59:59+00"),
        (seconds_offset, "2024-03-05T12:00:00+00:00:30"),
    ] {
        assert!(serde_json::to_string(&value).is_err(), "{value:?}");
        assert_eq!(value.to_string(), fallback);
    }
}

#[test]
fn date_serializes_as_a_full_date_string() {
    let value = Date(jiff::civil::date(2023, 11, 14));
    assert_eq!(serde_json::to_string(&value).unwrap(), "\"2023-11-14\"");
    assert_eq!(value.to_string(), "2023-11-14");
    assert_eq!(
        serde_json::from_str::<Date>("\"2023-11-14\"").unwrap(),
        value
    );
    assert_eq!(
        serde_json::to_string(&Date(jiff::civil::date(7, 1, 2))).unwrap(),
        "\"0007-01-02\""
    );
}

#[test]
fn date_rejects_malformed_and_impossible_input() {
    for json in [
        "\"2023-11-14T22:13:20Z\"",
        "\"2023-02-30\"",
        "\"2023-13-01\"",
        "20231114",
    ] {
        assert!(serde_json::from_str::<Date>(json).is_err(), "{json}");
    }
    let error = "2023-02-30".parse::<Date>().unwrap_err();
    assert_eq!(
        error.to_string(),
        "expected an RFC 3339 full-date (YYYY-MM-DD)"
    );
}

#[test]
fn date_before_year_zero_does_not_serialize() {
    let value = Date(jiff::civil::date(-1, 1, 1));
    assert!(serde_json::to_string(&value).is_err());
    assert_eq!(value.to_string(), jiff::civil::date(-1, 1, 1).to_string());
}

/// `DateTime`'s parts read back what `new` was given, and a `Timestamp` converts in at UTC and back
/// out unchanged. `Date` is transparent: `Deref`, `DerefMut`, `into_inner` and `From` both ways
/// reach the wrapped value unchanged, and the deserializer reads UTF-8 bytes as it reads a string.
#[test]
fn newtypes_convert_to_and_from_their_jiff_values() {
    let civil = jiff::civil::datetime(2024, 3, 5, 12, 0, 0, 500);
    let value = DateTime::new(civil, offset(3600));
    assert_eq!((value.datetime(), value.offset()), (civil, offset(3600)));
    let epoch = DateTime::from(jiff::Timestamp::UNIX_EPOCH);
    assert_eq!(epoch.offset(), jiff::tz::Offset::UTC);
    assert_eq!(
        epoch.datetime(),
        jiff::civil::datetime(1970, 1, 1, 0, 0, 0, 0)
    );
    assert_eq!(
        jiff::Timestamp::try_from(epoch).unwrap(),
        jiff::Timestamp::UNIX_EPOCH
    );

    let mut date = Date(jiff::civil::date(2023, 11, 14));
    assert_eq!(date.year(), 2023);
    *date = jiff::civil::date(2024, 1, 1);
    assert_eq!(date.into_inner(), jiff::civil::date(2024, 1, 1));
    assert_eq!(jiff::civil::Date::from(date), jiff::civil::date(2024, 1, 1));
    assert_eq!(Date::from(jiff::civil::date(2024, 1, 1)), date);

    let visitor = ParseVisitor::<Date>(PhantomData);
    let parsed: Result<Date, serde_json::Error> =
        serde::de::Visitor::visit_bytes(visitor, b"2024-01-01");
    assert_eq!(parsed.unwrap(), date);
    let visitor = ParseVisitor::<Date>(PhantomData);
    let invalid: Result<Date, serde_json::Error> =
        serde::de::Visitor::visit_bytes(visitor, b"\xff");
    assert!(invalid.is_err());
}
