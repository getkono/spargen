use super::*;

fn offset(seconds: i32) -> chrono::FixedOffset {
    chrono::FixedOffset::east_opt(seconds).expect("an offset under a day")
}

fn date_time() -> DateTime {
    DateTime(
        chrono::DateTime::from_timestamp(1_700_000_000, 0)
            .expect("representable timestamp")
            .fixed_offset(),
    )
}

/// A JSON string in RFC 3339 form, not `chrono`'s own rendering.
#[test]
fn date_time_serializes_as_an_rfc3339_string() {
    let json = serde_json::to_string(&date_time()).unwrap();
    assert_eq!(json, "\"2023-11-14T22:13:20Z\"");
    assert_eq!(date_time().to_string(), "2023-11-14T22:13:20Z");
}

#[test]
fn date_time_round_trips_through_json() {
    let value = DateTime(
        chrono::DateTime::from_timestamp(1_700_000_000, 120_000_000)
            .unwrap()
            .fixed_offset(),
    );
    let json = serde_json::to_string(&value).unwrap();
    assert_eq!(json, "\"2023-11-14T22:13:20.12Z\"");
    assert_eq!(serde_json::from_str::<DateTime>(&json).unwrap(), value);
}

/// The offset a value was written with is kept, and written back as `±HH:MM`.
#[test]
fn date_time_keeps_a_non_utc_offset_in_rfc3339_form() {
    let json = "\"2023-11-14T16:43:20-05:30\"";
    let value: DateTime = serde_json::from_str(json).unwrap();
    assert_eq!(value.offset().local_minus_utc(), -19_800);
    assert_eq!(value.timestamp(), 1_700_000_000);
    assert_eq!(serde_json::to_string(&value).unwrap(), json);
}

/// `chrono` holds a leap second past the first second; it is written as second 60, and the
/// codec reads `:60` back as the last nanosecond of the minute.
#[test]
fn date_time_writes_a_leap_second_as_second_sixty() {
    let leap = chrono::NaiveDate::from_ymd_opt(2016, 12, 31)
        .unwrap()
        .and_hms_nano_opt(23, 59, 59, 1_500_000_000)
        .unwrap();
    let value = DateTime(offset(0).from_utc_datetime(&leap));
    assert_eq!(value.to_string(), "2016-12-31T23:59:60.5Z");
    let read: DateTime = "2016-12-31T23:59:60.5Z".parse().unwrap();
    assert_eq!(read.to_string(), "2016-12-31T23:59:59.999999999Z");
}

/// `chrono` holds a leap nanosecond in any minute, but the codec reads `:60` only in the minute
/// that is 23:59 UTC on a month's last day: elsewhere the value is written as the last nanosecond
/// of its second, which reads back, rather than as a `:60` nothing reads.
#[test]
fn date_time_writes_a_leap_second_outside_a_leap_minute_as_readable_text() {
    let leap = chrono::NaiveDate::from_ymd_opt(2024, 3, 5)
        .unwrap()
        .and_hms_nano_opt(12, 0, 59, 1_500_000_000)
        .unwrap();
    let value = DateTime(offset(0).from_utc_datetime(&leap));
    assert_eq!(value.to_string(), "2024-03-05T12:00:59.999999999Z");
    let read: DateTime = value.to_string().parse().unwrap();
    assert_eq!(read.to_string(), value.to_string());

    // The leap minute itself, written at an offset, keeps its `:60`.
    let at_offset = DateTime(
        offset(9 * 3600).from_utc_datetime(
            &chrono::NaiveDate::from_ymd_opt(2016, 12, 31)
                .unwrap()
                .and_hms_nano_opt(23, 59, 59, 1_000_000_000)
                .unwrap(),
        ),
    );
    assert_eq!(at_offset.to_string(), "2017-01-01T08:59:60+09:00");
    assert!(at_offset.to_string().parse::<DateTime>().is_ok());
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

/// An offset with seconds, or a year outside `0..=9999`, has no RFC 3339 text: `Serialize` refuses
/// it and `Display` falls back to `chrono`'s rendering rather than panicking through `to_string()`.
#[test]
fn date_time_rfc3339_cannot_express_does_not_serialize() {
    let seconds_offset = DateTime(date_time().0.with_timezone(&offset(30)));
    let early = DateTime(
        offset(0).from_utc_datetime(
            &chrono::NaiveDate::from_ymd_opt(-1, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        ),
    );
    for value in [seconds_offset, early] {
        assert!(serde_json::to_string(&value).is_err(), "{value:?}");
        assert_eq!(value.to_string(), value.0.to_string());
    }
}

#[test]
fn date_serializes_as_a_full_date_string() {
    let value = Date(chrono::NaiveDate::from_ymd_opt(2023, 11, 14).unwrap());
    assert_eq!(serde_json::to_string(&value).unwrap(), "\"2023-11-14\"");
    assert_eq!(value.to_string(), "2023-11-14");
    assert_eq!(
        serde_json::from_str::<Date>("\"2023-11-14\"").unwrap(),
        value
    );
    let early = Date(chrono::NaiveDate::from_ymd_opt(7, 1, 2).unwrap());
    assert_eq!(serde_json::to_string(&early).unwrap(), "\"0007-01-02\"");
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
fn date_outside_four_digit_years_does_not_serialize() {
    let inner = chrono::NaiveDate::from_ymd_opt(10_000, 1, 1).unwrap();
    let value = Date(inner);
    assert!(serde_json::to_string(&value).is_err());
    assert_eq!(value.to_string(), inner.to_string());
}

/// The newtypes are transparent: `Deref`, `DerefMut`, `into_inner` and `From` both ways reach the
/// wrapped value unchanged.
#[test]
fn newtypes_are_transparent_wrappers() {
    let epoch = chrono::DateTime::UNIX_EPOCH.fixed_offset();
    let mut value = date_time();
    assert_eq!(value.timestamp(), 1_700_000_000);
    *value = epoch;
    assert_eq!(value.into_inner(), epoch);
    assert_eq!(chrono::DateTime::from(value), epoch);
    assert_eq!(DateTime::from(epoch), value);

    let next = chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
    let mut date = Date(chrono::NaiveDate::from_ymd_opt(2023, 11, 14).unwrap());
    assert_eq!(date.year(), 2023);
    *date = next;
    assert_eq!(date.into_inner(), next);
    assert_eq!(chrono::NaiveDate::from(date), next);
    assert_eq!(Date::from(next), date);
}
