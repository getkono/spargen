//! The media-type predicates as tables: one row per rule each documents, and the edge on each
//! side of it.

use indexmap::IndexMap;

use super::{
    classify_binary_family, classify_media, classify_media_range, first_list_element, is_cbor,
    is_field_name, is_field_value, media_essence_is_suffix_range, media_object_is_opaque,
    media_type_is_well_formed, media_type_with_parameters, request_media_candidates,
    ParameterFault,
};
use crate::diag::{Diagnostics, FileId, JsonPointer};
use crate::ir::MediaType;

/// A Media Type Object is opaque only when it carries no schema that constrains anything and none
/// of the fields that describe a body, a `$ref` in either position included.
#[test]
fn media_object_is_opaque_follows_the_table() {
    for (yaml, opaque) in [
        ("{}", true),
        ("schema: {}", true),
        ("schema: {type: string}", false),
        ("schema: {$ref: '#/components/schemas/A'}", false),
        ("$ref: '#/components/mediaTypes/A'", false),
        ("itemSchema: {}", false),
        ("encoding: {a: {contentType: text/plain}}", false),
        ("prefixEncoding: [{contentType: text/plain}]", false),
        ("itemEncoding: {contentType: text/plain}", false),
    ] {
        let mut diags = Diagnostics::default();
        let value = crate::source::parse_yaml(FileId(0), yaml, &mut diags)
            .unwrap_or_else(|_| panic!("probe does not parse: {yaml}"));
        let object =
            crate::oas31::deserialize::parse_media_type(&value, &JsonPointer::root(), &mut diags);
        assert_eq!(media_object_is_opaque(&object), opaque, "{yaml}");
    }
}

/// One `/` between two RFC 6838 `restricted-name`s, `*` only as the whole key, the whole subtype,
/// or in front of a suffix.
#[test]
fn media_type_is_well_formed_follows_the_table() {
    let long = format!("{}/json", "a".repeat(128));
    let longest = format!("{}/json", "a".repeat(127));
    for (essence, well_formed) in [
        ("application/json", true),
        ("Application/vnd.a-b_c.d+json", true),
        ("a!#$&-^_.+/b", true),
        ("*/*", true),
        ("text/*", true),
        ("application/*+json", true),
        (longest.as_str(), true),
        (long.as_str(), false),
        ("*/json", false),
        ("application", false),
        ("text/plain/extra", false),
        ("/json", false),
        ("application/", false),
        ("-ab/json", false),
        ("a b/json", false),
        ("a@b/json", false),
        ("application/*+", false),
        ("application/a*", false),
    ] {
        assert_eq!(media_type_is_well_formed(essence), well_formed, "{essence}");
    }
}

#[test]
fn only_a_star_suffix_subtype_is_a_suffix_range() {
    for (essence, suffix_range) in [
        ("application/*+json", true),
        ("application/vnd.a+json", false),
        ("application/*", false),
        ("*/*", false),
        ("application", false),
    ] {
        assert_eq!(
            media_essence_is_suffix_range(essence),
            suffix_range,
            "{essence}"
        );
    }
}

/// A suffix range is withheld exactly while a sibling is sendable: it classifies, is no range of
/// either kind, and is not streaming media.
#[test]
fn request_media_candidates_withhold_a_suffix_range_only_beside_a_sendable_key() {
    for (keys, candidates, withheld) in [
        (
            &["application/*+json", "application/json"][..],
            &["application/json"][..],
            &["application/*+json"][..],
        ),
        (
            &["application/*+json-seq", "application/json"],
            &["application/json"],
            &["application/*+json-seq"],
        ),
        (&["application/*+json"], &["application/*+json"], &[]),
        (
            &["application/*+json", "text/*"],
            &["application/*+json", "text/*"],
            &[],
        ),
        (
            &["application/*+json", "text/event-stream"],
            &["application/*+json", "text/event-stream"],
            &[],
        ),
        (
            &["application/*+json", "application/pdf"],
            &["application/*+json", "application/pdf"],
            &[],
        ),
    ] {
        let content = keys
            .iter()
            .map(|key| ((*key).to_owned(), ()))
            .collect::<IndexMap<_, _>>();
        let (chosen, held) = request_media_candidates(&content);
        assert_eq!(
            chosen.keys().map(String::as_str).collect::<Vec<_>>(),
            candidates,
            "{keys:?}"
        );
        assert_eq!(held, withheld, "{keys:?}");
    }
}

/// The first element ends at the first comma outside a quoted-string; a backslash escapes only
/// inside one.
#[test]
fn first_list_element_follows_the_table() {
    for (list, first) in [
        ("a/b, c/d", "a/b"),
        (" text/plain ", "text/plain"),
        (
            "text/plain; name=\"a, b\", c/d",
            "text/plain; name=\"a, b\"",
        ),
        (
            "text/plain; name=\"a\\\", b\", c/d",
            "text/plain; name=\"a\\\", b\"",
        ),
        ("a/b\\, c/d", "a/b\\"),
        ("text/plain; name=\"a, b", "text/plain; name=\"a, b"),
    ] {
        assert_eq!(first_list_element(list), first, "{list}");
    }
}

/// A field name is one or more RFC 9110 `tchar`s.
#[test]
fn is_field_name_follows_the_table() {
    for (name, valid) in [
        ("X-Rate-Limit", true),
        ("a!#$%&'*+-.^_`|~9", true),
        ("", false),
        ("a b", false),
        ("a:b", false),
        ("a\"b", false),
        ("a(b", false),
    ] {
        assert_eq!(is_field_name(name), valid, "{name:?}");
    }
}

/// A field value is visible ASCII or `obs-text`, with spaces and tabs only between two of them.
#[test]
fn is_field_value_follows_the_table() {
    for (value, valid) in [
        ("", true),
        ("abc", true),
        ("a b\tc", true),
        ("é", true),
        (" a", false),
        ("a ", false),
        ("\ta", false),
        ("a\u{1}b", false),
        ("a\u{7f}", false),
    ] {
        assert_eq!(is_field_value(value), valid, "{value:?}");
    }
}

/// The parameter grammar: what each well-formed list re-serializes to, what the multipart parser
/// cannot send, and what is malformed.
#[test]
fn media_type_with_parameters_follows_the_table() {
    let ok = |canonical: &str| Ok(canonical.to_owned());
    let unsendable = |canonical: &str| Err(ParameterFault::Unsendable(canonical.to_owned()));
    let malformed = Err(ParameterFault::Malformed);
    for (media, expected) in [
        ("text/plain", ok("text/plain")),
        (" text/plain ", ok("text/plain")),
        ("text/plain;charset=utf-8", ok("text/plain; charset=utf-8")),
        (
            "text/plain ; charset=utf-8 ;; format=flowed ;",
            ok("text/plain; charset=utf-8; format=flowed"),
        ),
        (
            "text/plain;\tcharset=utf-8",
            ok("text/plain; charset=utf-8"),
        ),
        (
            "text/plain; a=b; c=\"x y\"; d=e",
            ok("text/plain; a=b; c=\"x y\"; d=e"),
        ),
        ("text/plain; name=\"a\"", ok("text/plain; name=\"a\"")),
        ("text/plain; name=\"\\b\"", ok("text/plain; name=\"\\b\"")),
        (
            "text/plain; name=\"a\\ b\"",
            ok("text/plain; name=\"a\\ b\""),
        ),
        ("text/plain; name=\"é\"", ok("text/plain; name=\"é\"")),
        ("text/plain; name=\"\"", unsendable("text/plain; name=\"\"")),
        (
            "text/plain; name=\"a\\\"b\"",
            unsendable("text/plain; name=\"a\\\"b\""),
        ),
        (
            "text/plain; name=\"a\tb\"",
            unsendable("text/plain; name=\"a\tb\""),
        ),
        (
            "text/plain; name=\"a\\\tb\"",
            unsendable("text/plain; name=\"a\\\tb\""),
        ),
        (
            "text/plain; a=\"\"; b=c",
            unsendable("text/plain; a=\"\"; b=c"),
        ),
        ("text/plain; name=\"abc", malformed.clone()),
        ("text/plain; name", malformed.clone()),
        ("text/plain; name;", malformed.clone()),
        ("text/plain; =x", malformed.clone()),
        ("text/plain; name=", malformed.clone()),
        ("text/plain; name=;", malformed.clone()),
        ("text/plain; name=\"a\u{1}\"", malformed.clone()),
        ("text/plain; name=\"a\\\u{1}\"", malformed.clone()),
        ("text/plain; name=\"a\\", malformed.clone()),
        ("text/plain; a=b c=d", malformed.clone()),
        ("text/plain; a=\"x\"y", malformed.clone()),
    ] {
        assert_eq!(media_type_with_parameters(media), expected, "{media:?}");
    }
}

/// Every arm of the classification ladder, matched case-insensitively, with the keys each arm's
/// guard must not take.
#[test]
fn classify_media_follows_the_table() {
    use MediaType::{
        EventStream, FormUrlEncoded, Json, JsonSequence, Multipart, Ndjson, OctetStream, Text, Xml,
    };
    for (essence, expected) in [
        ("application/json", Some((Json, 0))),
        ("application/vnd.github+json", Some((Json, 0))),
        ("APPLICATION/VND.GITHUB+JSON", Some((Json, 0))),
        ("text/x+json", Some((Text, 5))),
        ("application/xml", Some((Xml, 1))),
        ("text/xml", Some((Xml, 1))),
        ("multipart/form-data", Some((Multipart, 2))),
        (
            "application/x-www-form-urlencoded",
            Some((FormUrlEncoded, 3)),
        ),
        ("application/octet-stream", Some((OctetStream, 4))),
        ("application/cbor", Some((OctetStream, 4))),
        ("application/cose+cbor", Some((OctetStream, 4))),
        ("text/event-stream", Some((EventStream, 6))),
        ("TEXT/EVENT-STREAM", Some((EventStream, 6))),
        ("application/x-ndjson", Some((Ndjson, 6))),
        ("application/jsonl", Some((Ndjson, 6))),
        ("application/json-seq", Some((JsonSequence, 6))),
        ("application/geo+json-seq", Some((JsonSequence, 6))),
        ("text/x+json-seq", Some((Text, 5))),
        ("application/octocat-stream", Some((Text, 5))),
        ("text/plain", Some((Text, 5))),
        ("Text/Plain", Some((Text, 5))),
        ("text/*", Some((Text, 7))),
        ("image/*", Some((OctetStream, 8))),
        ("*/*", Some((OctetStream, 8))),
        ("image/png", Some((OctetStream, 9))),
        ("AUDIO/mpeg", Some((OctetStream, 9))),
        ("video/mp4", Some((OctetStream, 9))),
        ("image/svg+xml", None),
        ("application/pdf", None),
        ("application/*+cbor", None),
        ("text/plain/extra", None),
        ("application", None),
    ] {
        assert_eq!(classify_media(essence), expected, "{essence}");
    }
}

/// CBOR is `application/cbor` or a concrete subtype with a non-empty stem before `+cbor`.
#[test]
fn is_cbor_follows_the_table() {
    for (essence, cbor) in [
        ("application/cbor", true),
        ("APPLICATION/CBOR", true),
        ("application/cose+cbor", true),
        ("image/x+CBOR", true),
        ("application/+cbor", false),
        ("application/*+cbor", false),
        ("application/json", false),
        ("text/cbor", false),
        ("/cbor", false),
        ("cbor", false),
    ] {
        assert_eq!(is_cbor(essence), cbor, "{essence}");
    }
}

/// A range is a non-empty type before `/*`: `text` is raw UTF-8, every other family octets.
#[test]
fn classify_media_range_follows_the_table() {
    for (essence, expected) in [
        ("text/*", Some((MediaType::Text, 7))),
        ("TEXT/*", Some((MediaType::Text, 7))),
        ("image/*", Some((MediaType::OctetStream, 8))),
        ("*/*", Some((MediaType::OctetStream, 8))),
        ("/*", None),
        ("text/plain", None),
    ] {
        assert_eq!(classify_media_range(essence), expected, "{essence}");
    }
}

/// A concrete `image`, `audio` or `video` member with no `*` and no structured suffix.
#[test]
fn classify_binary_family_follows_the_table() {
    for (essence, binary) in [
        ("image/png", true),
        ("Video/mp4", true),
        ("audio/mpeg", true),
        ("image/", false),
        ("image/*", false),
        ("image/svg+xml", false),
        ("application/pdf", false),
        ("image", false),
    ] {
        assert_eq!(
            classify_binary_family(essence),
            binary.then_some((MediaType::OctetStream, 9)),
            "{essence}"
        );
    }
}
