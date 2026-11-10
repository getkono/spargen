use super::*;

fn date_time() -> DateTime {
    DateTime(jiff::Timestamp::from_second(1_700_000_000).expect("representable timestamp"))
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
    let value = DateTime(jiff::Timestamp::new(1_700_000_000, 120_000_000).unwrap());
    let json = serde_json::to_string(&value).unwrap();
    assert_eq!(json, "\"2023-11-14T22:13:20.12Z\"");
    assert_eq!(serde_json::from_str::<DateTime>(&json).unwrap(), value);
}

/// A `Timestamp` holds no offset, so a non-UTC value decodes to the same instant and encodes in
/// UTC.
#[test]
fn date_time_normalizes_a_non_utc_offset_to_utc() {
    let value: DateTime = serde_json::from_str("\"2023-11-14T16:43:20-05:30\"").unwrap();
    assert_eq!(value, date_time());
    assert_eq!(
        serde_json::to_string(&value).unwrap(),
        "\"2023-11-14T22:13:20Z\""
    );
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

/// Before year 0 there is no RFC 3339 text: `Serialize` refuses the value and `Display` falls back
/// to `jiff`'s rendering rather than panicking through `to_string()`.
#[test]
fn date_time_before_year_zero_does_not_serialize() {
    let value = DateTime(jiff::Timestamp::MIN);
    assert!(serde_json::to_string(&value).is_err());
    assert_eq!(value.to_string(), jiff::Timestamp::MIN.to_string());
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

/// The newtypes are transparent: `Deref`, `DerefMut`, `into_inner` and `From` both ways reach the
/// wrapped value unchanged, and the deserializer reads UTF-8 bytes as it reads a string.
#[test]
fn newtypes_are_transparent_wrappers() {
    let mut value = date_time();
    assert_eq!(value.as_second(), 1_700_000_000);
    *value = jiff::Timestamp::UNIX_EPOCH;
    assert_eq!(value.into_inner(), jiff::Timestamp::UNIX_EPOCH);
    assert_eq!(jiff::Timestamp::from(value), jiff::Timestamp::UNIX_EPOCH);
    assert_eq!(DateTime::from(jiff::Timestamp::UNIX_EPOCH), value);

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
