use super::*;

fn date(year: i32, month: u8, day: u8) -> FullDate {
    FullDate { year, month, day }
}

fn fields(
    (year, month, day): (i32, u8, u8),
    (hour, minute, second): (u8, u8, u8),
    nanosecond: u32,
    offset_seconds: i32,
) -> DateTimeFields {
    DateTimeFields {
        date: date(year, month, day),
        hour,
        minute,
        second,
        nanosecond,
        offset_seconds,
    }
}

/// Each accepted spelling reads as its fields: `t`/`z` in either case, a fraction of any length
/// (digits past the ninth dropped), a signed offset, `-00:00` as UTC, and a leap second as the
/// last nanosecond of its minute.
#[test]
fn date_time_reads_every_rfc3339_spelling() {
    let cases = [
        (
            "2023-11-14T22:13:20Z",
            fields((2023, 11, 14), (22, 13, 20), 0, 0),
        ),
        (
            "2023-11-14t22:13:20z",
            fields((2023, 11, 14), (22, 13, 20), 0, 0),
        ),
        (
            "2023-11-14T22:13:20.5Z",
            fields((2023, 11, 14), (22, 13, 20), 500_000_000, 0),
        ),
        (
            "2023-11-14T22:13:20.123456789Z",
            fields((2023, 11, 14), (22, 13, 20), 123_456_789, 0),
        ),
        (
            "2023-11-14T22:13:20.1234567891Z",
            fields((2023, 11, 14), (22, 13, 20), 123_456_789, 0),
        ),
        (
            "2023-11-14T16:43:20-05:30",
            fields((2023, 11, 14), (16, 43, 20), 0, -19_800),
        ),
        (
            "2023-11-14T23:59:59+23:59",
            fields((2023, 11, 14), (23, 59, 59), 0, 86_340),
        ),
        (
            "2023-11-14T22:13:20-00:00",
            fields((2023, 11, 14), (22, 13, 20), 0, 0),
        ),
        (
            "2016-12-31T23:59:60Z",
            fields((2016, 12, 31), (23, 59, 59), 999_999_999, 0),
        ),
        (
            "2016-02-29T23:59:60Z",
            fields((2016, 2, 29), (23, 59, 59), 999_999_999, 0),
        ),
        (
            "2017-01-01T05:29:60+05:30",
            fields((2017, 1, 1), (5, 29, 59), 999_999_999, 19_800),
        ),
        (
            "2015-06-30T18:59:60-05:00",
            fields((2015, 6, 30), (18, 59, 59), 999_999_999, -18_000),
        ),
        (
            "2015-06-30T00:00:60-23:59",
            fields((2015, 6, 30), (0, 0, 59), 999_999_999, -86_340),
        ),
        ("0000-01-01T00:00:00Z", fields((0, 1, 1), (0, 0, 0), 0, 0)),
    ];
    for (text, expected) in cases {
        assert_eq!(parse_date_time(text), Some(expected), "{text}");
    }
}

/// Anything outside the grammar is refused rather than read loosely: other separators, a missing
/// or malformed offset or fraction, out-of-range fields, signs inside fields, and trailing text.
#[test]
fn date_time_refuses_what_rfc3339_does_not_admit() {
    for text in [
        "",
        "2023-11-14",
        "2023-11-14 22:13:20Z",
        "2023-11-14T22:13:20",
        "2023-11-14T22:13:20+0000",
        "2023-11-14T22:13:20+00",
        "2023-11-14T22:13:20.Z",
        "2023-11-14T22:13:20,5Z",
        "2023-11-14T24:00:00Z",
        "2023-11-14T22:60:00Z",
        "2023-11-14T22:13:61Z",
        // A leap second outside 23:59 UTC on a month's last day, which `time` refuses too.
        "2023-11-14T12:00:60Z",
        "2016-12-30T23:59:60Z",
        "2016-02-28T23:59:60Z",
        "2016-12-31T23:59:60+01:00",
        "2017-01-02T05:29:60+05:30",
        "2023-11-14T22:13:20+24:00",
        "2023-11-14T22:13:20+05:60",
        "2023-11-14T22:13:20ZZ",
        "2023-11-14T22:13:20Z[UTC]",
        "2023-11-14T+2:13:20Z",
        "2023-13-14T22:13:20Z",
        "2023-11-00T22:13:20Z",
        "+023-11-14T22:13:20Z",
        "2023-11-14T22:13:20.5é",
        "2023-11-14T22:1é:20Z",
    ] {
        assert_eq!(parse_date_time(text), None, "{text:?}");
    }
}

/// Writing gives `T`, the fraction trimmed of trailing zeros, `Z` for UTC, and `±HH:MM` otherwise.
#[test]
fn date_time_writes_the_canonical_form() {
    let cases = [
        (
            fields((2023, 11, 14), (22, 13, 20), 0, 0),
            "2023-11-14T22:13:20Z",
        ),
        (
            fields((2023, 11, 14), (22, 13, 20), 500_000_000, 0),
            "2023-11-14T22:13:20.5Z",
        ),
        (
            fields((2023, 11, 14), (22, 13, 20), 1, 0),
            "2023-11-14T22:13:20.000000001Z",
        ),
        (
            fields((2023, 11, 14), (16, 43, 20), 0, -19_800),
            "2023-11-14T16:43:20-05:30",
        ),
        (
            fields((7, 1, 2), (3, 4, 5), 0, 3_600),
            "0007-01-02T03:04:05+01:00",
        ),
    ];
    for (value, expected) in cases {
        assert_eq!(format_date_time(value).as_deref(), Some(expected));
        assert_eq!(parse_date_time(expected), Some(value), "{expected}");
    }
}

/// RFC 3339 cannot write a year outside `0..=9999` or an offset that is not whole minutes under a
/// day, so those have no text.
#[test]
fn date_time_has_no_text_for_what_rfc3339_cannot_express() {
    for value in [
        fields((-1, 1, 1), (0, 0, 0), 0, 0),
        fields((10_000, 1, 1), (0, 0, 0), 0, 0),
        fields((2023, 1, 1), (0, 0, 0), 0, 30),
        fields((2023, 1, 1), (0, 0, 0), 0, -86_400),
    ] {
        assert_eq!(format_date_time(value), None, "{value:?}");
    }
}

#[test]
fn full_date_reads_and_writes_exactly_yyyy_mm_dd() {
    assert_eq!(parse_full_date("2023-11-14"), Some(date(2023, 11, 14)));
    assert_eq!(
        format_full_date(date(2023, 11, 14)).as_deref(),
        Some("2023-11-14")
    );
    assert_eq!(
        format_full_date(date(7, 1, 2)).as_deref(),
        Some("0007-01-02")
    );
    assert_eq!(format_full_date(date(-1, 1, 2)), None);
    assert_eq!(format_full_date(date(10_000, 1, 2)), None);
    for text in [
        "2023-11-14T22:13:20Z",
        "2023-11-4",
        "2023/11/14",
        "2023-13-01",
        "2023-00-01",
        "2023-01-32",
        "-001-01-02",
        "2023-+1-+4",
        "2023-1é-14",
    ] {
        assert_eq!(parse_full_date(text), None, "{text:?}");
    }
}

#[test]
fn parse_errors_name_what_was_expected() {
    assert_eq!(
        DateParseError::date_time().to_string(),
        "expected an RFC 3339 date-time"
    );
    assert_eq!(
        DateParseError::full_date().to_string(),
        "expected an RFC 3339 full-date (YYYY-MM-DD)"
    );
}
