use super::*;

fn date_time() -> DateTime {
    DateTime(
        time::OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("representable timestamp"),
    )
}

/// The regression this module exists for: a JSON string, not `time`'s nine-integer sequence.
#[test]
fn date_time_serializes_as_an_rfc3339_string() {
    let json = serde_json::to_string(&date_time()).unwrap();
    assert_eq!(json, "\"2023-11-14T22:13:20Z\"");
    assert!(!json.starts_with('['), "must not be a sequence: {json}");
}

#[test]
fn date_time_round_trips_through_json() {
    let value = date_time();
    let json = serde_json::to_string(&value).unwrap();
    assert_eq!(serde_json::from_str::<DateTime>(&json).unwrap(), value);
}

/// An offset is preserved as `±HH:MM`, never the space-separated form `time`'s own
/// human-readable serializer would emit.
#[test]
fn date_time_keeps_a_non_utc_offset_in_rfc3339_form() {
    let offset = time::UtcOffset::from_hms(-5, 30, 0).unwrap();
    let value = DateTime(date_time().0.to_offset(offset));
    let json = serde_json::to_string(&value).unwrap();
    assert_eq!(json, "\"2023-11-14T16:43:20-05:30\"");
    assert_eq!(serde_json::from_str::<DateTime>(&json).unwrap(), value);
}

#[test]
fn date_time_display_matches_serialization() {
    let value = date_time();
    let json: String = serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
    assert_eq!(value.to_string(), json);
}

#[test]
fn date_time_rejects_a_non_rfc3339_string() {
    // Exactly `time`'s own human-readable rendering: a space where RFC 3339 requires `T`.
    let error = serde_json::from_str::<DateTime>("\"2023-11-14 22:13:20.0 +00:00:00\"");
    assert!(error.is_err(), "{error:?}");
    assert!(serde_json::from_str::<DateTime>("[2023,318,22,13,20,0,0,0,0]").is_err());
}

#[test]
fn date_serializes_as_a_full_date_string() {
    let value = Date(time::Date::from_calendar_date(2023, time::Month::November, 14).unwrap());
    assert_eq!(serde_json::to_string(&value).unwrap(), "\"2023-11-14\"");
    assert_eq!(value.to_string(), "2023-11-14");
    assert_eq!(
        serde_json::from_str::<Date>("\"2023-11-14\"").unwrap(),
        value
    );
}

#[test]
fn date_pads_single_digit_months_and_days() {
    let value = Date(time::Date::from_calendar_date(7, time::Month::January, 2).unwrap());
    assert_eq!(serde_json::to_string(&value).unwrap(), "\"0007-01-02\"");
}

#[test]
fn date_rejects_malformed_and_over_long_input() {
    // A full `date-time` must not be silently truncated to its date part.
    assert!(serde_json::from_str::<Date>("\"2023-11-14T22:13:20Z\"").is_err());
    assert!(serde_json::from_str::<Date>("\"2023-11-4\"").is_err());
    assert!(serde_json::from_str::<Date>("\"2023/11/14\"").is_err());
    assert!(serde_json::from_str::<Date>("\"2023-13-01\"").is_err());
    assert!(serde_json::from_str::<Date>("\"2023-02-30\"").is_err());
}

/// `Display` must never fail: generated code renders a `multipart/form-data` text part and a
/// `text/plain` body through `to_string()`, which panics on a `Display` error. A year outside
/// RFC 3339's four-digit range is constructible (`time` holds -9999..=9999), so this is
/// reachable from ordinary user input rather than only from a malformed response.
#[test]
fn display_never_panics_for_a_year_rfc3339_cannot_express() {
    let date = Date(time::Date::from_calendar_date(-1, time::Month::January, 2).unwrap());
    assert!(!date.to_string().is_empty());
    let stamp = DateTime(time::OffsetDateTime::new_utc(date.0, time::Time::MIDNIGHT));
    assert!(!stamp.to_string().is_empty());
    // Serialization still refuses: such a value must never reach a body claiming RFC 3339.
    assert!(serde_json::to_string(&date).is_err());
    assert!(serde_json::to_string(&stamp).is_err());
}

/// A parse failure names the RFC 3339 production that was expected, one message per type.
#[test]
fn a_parse_failure_names_the_rfc3339_form_it_expected() {
    let date_time = "2023-11-14".parse::<DateTime>().unwrap_err();
    assert_eq!(date_time.to_string(), "expected an RFC 3339 date-time");
    let date = "2023-11-14T22:13:20Z".parse::<Date>().unwrap_err();
    assert_eq!(
        date.to_string(),
        "expected an RFC 3339 full-date (YYYY-MM-DD)"
    );
    // Deserialization surfaces the same message through serde's custom error.
    let error = serde_json::from_str::<Date>("\"2023/11/14\"").unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("expected an RFC 3339 full-date (YYYY-MM-DD)"),
        "{error}"
    );
}

/// Every date RFC 3339's four-digit `full-date` can express: years `0000` through `9999`.
fn expressible_date() -> impl proptest::strategy::Strategy<Value = time::Date> {
    use proptest::strategy::Strategy;
    let first = time::Date::from_calendar_date(0, time::Month::January, 1).unwrap();
    let last = time::Date::from_calendar_date(9999, time::Month::December, 31).unwrap();
    (first.to_julian_day()..=last.to_julian_day())
        .prop_map(|day| time::Date::from_julian_day(day).expect("a day inside the range"))
}

/// An expressible date-time: any instant of an expressible date, to the nanosecond, at any
/// whole-minute offset RFC 3339 can write (`±HH:MM`, under a day).
fn expressible_date_time() -> impl proptest::strategy::Strategy<Value = time::OffsetDateTime> {
    use proptest::strategy::Strategy;
    (
        expressible_date(),
        0u8..24,
        0u8..60,
        0u8..60,
        0u32..1_000_000_000,
        -(23 * 60 + 59)..=(23i32 * 60 + 59),
    )
        .prop_map(|(date, hour, minute, second, nano, offset_minutes)| {
            let time = time::Time::from_hms_nano(hour, minute, second, nano).unwrap();
            let offset = time::UtcOffset::from_whole_seconds(offset_minutes * 60).unwrap();
            time::PrimitiveDateTime::new(date, time).assume_offset(offset)
        })
}

/// Whether `text` is exactly `YYYY-MM-DD`: ten bytes, ASCII digits around two dashes.
fn is_full_date_shape(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

/// Strings of digits and dashes whose first byte is a digit, biased toward the `YYYY-MM-DD`
/// shape so a good share of cases reach acceptance: well-formed calendar fields, then the
/// ten-byte shape with either separator possibly a digit, then free strings of 8 to 12 bytes.
///
/// The first byte stays a digit because the parser still accepts a leading `-` on the year
/// (`-023-11-14`, issue #690), which is not canonical; widen the alphabet once that lands.
fn digit_dash_string() -> impl proptest::strategy::Strategy<Value = String> {
    proptest::prop_oneof![
        2 => "[0-9]{4}-(0[0-9]|1[0-3])-([0-2][0-9]|3[0-2])",
        1 => "[0-9]{4}[0-9-][0-9]{2}[0-9-][0-9]{2}",
        1 => "[0-9][0-9-]{7,11}",
    ]
}

proptest::proptest! {
    /// `Display` and `Serialize` write the same `YYYY-MM-DD` bytes for every expressible
    /// date, and those bytes read back as the date they came from.
    #[test]
    fn a_date_displays_as_it_serializes_in_full_date_form(date in expressible_date()) {
        let value = Date(date);
        let text = value.to_string();
        proptest::prop_assert!(is_full_date_shape(&text), "{}", text);
        proptest::prop_assert_eq!(serde_json::to_string(&value).unwrap(), format!("\"{text}\""));
        proptest::prop_assert_eq!(text.parse::<Date>().unwrap(), value);
    }

    /// `Display` and `Serialize` write the same RFC 3339 bytes for every expressible
    /// date-time, and those bytes read back as the same instant at the same offset.
    #[test]
    fn a_date_time_displays_as_it_serializes(stamp in expressible_date_time()) {
        let value = DateTime(stamp);
        let text = value.to_string();
        proptest::prop_assert_eq!(serde_json::to_string(&value).unwrap(), format!("\"{text}\""));
        let read: DateTime = text.parse().unwrap();
        proptest::prop_assert_eq!(read, value);
        proptest::prop_assert_eq!(read.offset(), value.offset());
    }

    /// The date parser accepts nothing but the exact ten bytes: a full date with anything
    /// before or after it is refused rather than truncated to its date part.
    #[test]
    fn a_full_date_with_anything_around_it_is_refused(
        date in expressible_date(),
        prefix in "[0-9T:Z +-]{0,4}",
        suffix in "[0-9T:Z +-]{0,10}",
    ) {
        proptest::prop_assume!(!prefix.is_empty() || !suffix.is_empty());
        let text = format!("{prefix}{}{suffix}", Date(date));
        proptest::prop_assert!(text.parse::<Date>().is_err(), "{}", text);
    }

    /// Over strings of digits and dashes that open with a digit, whatever the parser accepts
    /// is exactly the canonical `YYYY-MM-DD` rendering of what it read.
    #[test]
    fn an_accepted_digit_string_is_the_canonical_full_date(text in digit_dash_string()) {
        if let Ok(date) = text.parse::<Date>() {
            proptest::prop_assert!(is_full_date_shape(&text), "{}", text);
            proptest::prop_assert_eq!(date.to_string(), text);
        }
    }
}

/// The newtypes are transparent: `time`'s API is one deref away.
#[test]
fn newtypes_deref_to_the_time_types() {
    assert_eq!(date_time().year(), 2023);
    let date = Date(time::Date::from_calendar_date(2023, time::Month::November, 14).unwrap());
    assert_eq!(date.month(), time::Month::November);
    assert_eq!(time::Date::from(date), date.0);
}
