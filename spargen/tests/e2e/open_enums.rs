//! Open string enums: the generated code for narrowed problem types, compiled and linted, decoding
//! listed and unlisted values, parsing, hashing and telling union variants apart. That every enum
//! is open by default, wherever it sits, and that `closed_enums` closes them all, are pinned in
//! `frontend/open_enums.rs` over the same spec; the closed shape compiles in `basic_crate.rs`.

use super::*;

/// The RFC 9457 shape from #268: each documented error status narrows the shared `Problem`'s
/// `type` with a `const`, spelled as an `allOf` member (`404`) and as `$ref` siblings (`409`).
/// Beside them: a union of narrowed problems (`400`), a narrowing inside a component (`410`), a
/// set narrowed against a `uuid` string in either member order (`428`, `429`), unions meeting a
/// set in either order (`422` to `427`), a union nesting a union (`431`), and a request body.
const OPEN_ENUMS_SPEC: &str = include_str!("specs/open_enums.yaml");

#[test]
fn an_open_enum_decodes_an_unlisted_value_and_keeps_it_typed() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "problems.yaml", OPEN_ENUMS_SPEC);

    let out = temp.path().join("client");
    let report =
        generate_configured_fixture_crate(&spec, &out, "open_problems", "2021", |spec| spec);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let status = fixture_cargo(&out)
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the open enums must pass clippy -D warnings"
    );

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/open.rs"),
        r##"use std::collections::HashSet;

use open_problems::types::ForbiddenType;
use open_problems::{Error, PostProblemsError, ResponseValue};

/// Decode `json` as the body type the variant constructor `_variant` carries, without naming the
/// generated (hash-disambiguated) type.
fn decode<T: serde::de::DeserializeOwned, E>(
    _variant: fn(Box<T>) -> E,
    json: &str,
) -> Result<T, serde_json::Error> {
    serde_json::from_str(json)
}

/// `decoded` failed because the type refused a well-formed value: a `Data` error, never a syntax
/// or end-of-input one, which would mean the test's own document was malformed and proved nothing.
#[track_caller]
fn assert_refused<T: std::fmt::Debug>(decoded: serde_json::Result<T>, what: &str) {
    match decoded {
        Ok(value) => panic!("{what}: the type admits no such value, yet produced {value:?}"),
        Err(error) => assert_eq!(
            error.classify(),
            serde_json::error::Category::Data,
            "{what}: refused for the wrong reason: {error}"
        ),
    }
}

#[test]
fn a_listed_value_is_its_own_variant_and_an_unlisted_one_is_kept() {
    let listed = decode(
        PostProblemsError::Status404,
        r#"{"type":"https://example.com/probs/not-found","title":"t"}"#,
    )
    .expect("a listed type decodes");
    assert_eq!(format!("{:?}", listed.r#type), "HttpsExampleComProbsNotFound");
    assert_eq!(listed.r#type.as_str(), "https://example.com/probs/not-found");

    // A problem type the description does not list: it decodes, and keeps its value.
    let unlisted = decode(
        PostProblemsError::Status404,
        r#"{"type":"https://example.com/probs/moved","title":"t","detail":"d"}"#,
    )
    .expect("an unlisted type still decodes");
    assert_eq!(
        format!("{:?}", unlisted.r#type),
        r#"Unknown(UnknownValue("https://example.com/probs/moved"))"#
    );
    assert_eq!(unlisted.r#type.to_string(), "https://example.com/probs/moved");
    // It is still a string on the wire, both ways.
    let wire = serde_json::to_value(&unlisted).unwrap();
    assert_eq!(wire["type"], "https://example.com/probs/moved");
    // A non-string is refused: an open enum's domain is still a string.
    assert_refused(
        decode(PostProblemsError::Status404, r#"{"type":7,"title":"t"}"#),
        "a non-string type",
    );
}

#[test]
fn from_str_maps_a_listed_value_to_its_variant_and_never_fails() {
    let listed = "https://example.com/probs/forbidden";
    let parsed: Result<ForbiddenType, std::convert::Infallible> = listed.parse();
    assert_eq!(parsed, Ok(ForbiddenType::HttpsExampleComProbsForbidden));
    assert_eq!(ForbiddenType::ALL, [ForbiddenType::HttpsExampleComProbsForbidden]);

    // Any other string parses as `Unknown`, which compares equal to the same value decoded, and
    // never to a listed variant.
    let other: ForbiddenType = "https://example.com/probs/teapot".parse().unwrap();
    assert!(matches!(&other, ForbiddenType::Unknown(value) if value.as_str() == "https://example.com/probs/teapot"));
    assert_eq!(
        serde_json::from_str::<ForbiddenType>(r#""https://example.com/probs/teapot""#).unwrap(),
        other
    );
    assert_ne!(other, ForbiddenType::HttpsExampleComProbsForbidden);
    assert_eq!(serde_json::to_string(&other).unwrap(), r#""https://example.com/probs/teapot""#);
}

#[test]
fn an_open_enum_is_a_hash_key() {
    let set: HashSet<ForbiddenType> = [
        "https://example.com/probs/forbidden",
        "https://example.com/probs/teapot",
        "https://example.com/probs/teapot",
        "https://example.com/probs/gone",
    ]
    .into_iter()
    .map(|value| value.parse().unwrap())
    .collect();
    assert_eq!(set.len(), 3);
    assert!(set.contains(&ForbiddenType::HttpsExampleComProbsForbidden));
}

#[test]
fn an_open_defaulted_member_reads_through_its_accessor() {
    // `430`'s optional `detail` is an open set with a default: absent, the accessor gives the
    // default; set, even to an unlisted value, it gives the value.
    let absent = decode(PostProblemsError::Status430, r#"{"type":"t","title":"t"}"#).unwrap();
    assert!(absent.detail.is_none());
    assert_eq!(absent.detail_or_default().as_str(), "a");
    let unlisted = decode(
        PostProblemsError::Status430,
        r#"{"type":"t","title":"t","detail":"elsewhere"}"#,
    )
    .unwrap();
    assert_eq!(unlisted.detail_or_default().as_str(), "elsewhere");
}

#[test]
fn a_value_spelled_unknown_keeps_its_name() {
    let listed = decode(PostProblemsError::Status409, r#"{"type":"unknown","title":"t"}"#).unwrap();
    // The listed `unknown` keeps `Unknown`; the catch-all takes the next name.
    assert_eq!(format!("{:?}", listed.r#type), "Unknown");
    let unlisted =
        decode(PostProblemsError::Status409, r#"{"type":"elsewhere","title":"t"}"#).unwrap();
    assert!(
        format!("{:?}", unlisted.r#type).ends_with(r#"(UnknownValue("elsewhere"))"#),
        "{unlisted:?}"
    );
}

#[test]
fn a_union_tells_its_variants_apart_by_the_values_they_list() {
    // Each variant still refuses the other's listed type, so exactly one matches...
    let b = decode(
        PostProblemsError::Status400,
        r#"{"type":"https://example.com/probs/b","title":"t"}"#,
    )
    .expect("the variant listing `b` decodes it");
    assert_eq!(
        format!("{b:?}"),
        r#"ResponseBodyVariant1(ResponseBodyVariant1 { type: HttpsExampleComProbsB, title: "t", detail: None })"#
    );
    // ...and a type neither lists, which both would hold, matches no one of them.
    assert_refused(
        decode(
            PostProblemsError::Status400,
            r#"{"type":"https://example.com/probs/c","title":"t"}"#,
        ),
        "400: a type no variant lists",
    );
    // Written back, a listed value is checked against the same rule and passes.
    serde_json::to_value(&b).expect("a listed value matches exactly one variant");
}

#[test]
fn a_nested_union_keeps_the_enclosing_closed_reading() {
    // `431`'s first variant holds a union of its own. Decoded inside the outer union's closed
    // pass, that inner union must not try its variants open: `q`, which only the second variant
    // lists, would then match the first one too, and the pair would refuse a listed value.
    let q = decode(PostProblemsError::Status431, r#"{"v":{"e":"q"}}"#)
        .expect("the variant listing `q` decodes it");
    let p = decode(PostProblemsError::Status431, r#"{"v":{"e":"p"}}"#)
        .expect("the variant listing `p` decodes it");
    let (q_variant, p_variant) = (format!("{q:?}"), format!("{p:?}"));
    assert_ne!(
        q_variant.split('(').next(),
        p_variant.split('(').next(),
        "{q_variant} / {p_variant}"
    );
    serde_json::to_value(&q).expect("a listed value matches exactly one variant");
    serde_json::to_value(&p).expect("a listed value matches exactly one variant");
    // A value neither lists, both variants would hold open, so neither matches alone.
    assert_refused(
        decode(PostProblemsError::Status431, r#"{"v":{"e":"z"}}"#),
        "431: a value no variant lists",
    );
}

#[test]
fn a_component_and_a_request_body_are_open_too() {
    let gone = decode(
        PostProblemsError::Status410,
        r#"{"type":"https://example.com/probs/moved","title":"t"}"#,
    )
    .expect("a component's narrowing is open");
    assert_eq!(gone.r#type.as_str(), "https://example.com/probs/moved");
    let forbidden = decode(
        PostProblemsError::Status403,
        r#"{"type":"https://example.com/probs/moved","title":"t"}"#,
    )
    .expect("a narrowing through a `$ref`'d value is open");
    assert_eq!(forbidden.r#type.as_str(), "https://example.com/probs/moved");
    let body = serde_json::from_str::<open_problems::types::PostProblemsBody>(
        r#"{"type":"https://example.com/probs/moved","title":"t"}"#,
    )
    .expect("a request body's narrowing is open");
    assert_eq!(body.r#type.as_str(), "https://example.com/probs/moved");
}

#[test]
fn a_union_meeting_a_set_decodes_in_either_member_order() {
    // `422` writes the union last; `423` writes it first. Either way a listed value matches
    // exactly one variant.
    for (order, decoded) in [
        ("union last", decode(PostProblemsError::Status422, r#"{"kind":"a"}"#).map(|_| ())),
        ("union last", decode(PostProblemsError::Status422, r#"{"kind":"b"}"#).map(|_| ())),
        ("union first", decode(PostProblemsError::Status423, r#"{"kind":"a"}"#).map(|_| ())),
        ("union first", decode(PostProblemsError::Status423, r#"{"kind":"b"}"#).map(|_| ())),
    ] {
        decoded.unwrap_or_else(|error| panic!("{order}: a value one variant lists: {error}"));
    }
    // A value the set lists but no variant does, and one nothing lists, both variants would hold
    // as `Unknown`, so neither matches alone.
    for kind in ["c", "z"] {
        let json = format!(r#"{{"kind":"{kind}"}}"#);
        assert_refused(
            decode(PostProblemsError::Status422, &json),
            &format!("union last: {kind}"),
        );
        assert_refused(
            decode(PostProblemsError::Status423, &json),
            &format!("union first: {kind}"),
        );
    }
    // A union that narrows to one branch is no union: in either order the result is one open
    // set, so an unlisted value is kept.
    for kind in ["a", "z"] {
        let json = format!(r#"{{"kind":"{kind}"}}"#);
        let last = decode(PostProblemsError::Status424, &json).expect("union last");
        let first = decode(PostProblemsError::Status425, &json).expect("union first");
        assert_eq!(last.kind.as_str(), kind);
        assert_eq!(first.kind.as_str(), kind);
    }
    // A plain `string` branch kept beside another, in either order: `b` only it lists, `a` both
    // list (which `oneOf` forbids), and `z` both would hold as `Unknown`.
    macro_rules! beside_another {
        ($status:expr, $order:literal) => {
            decode($status, r#"{"kind":"b"}"#).unwrap_or_else(|error| panic!("{}: {error}", $order));
            assert_refused(decode($status, r#"{"kind":"a"}"#), &format!("{}: a", $order));
            assert_refused(decode($status, r#"{"kind":"z"}"#), &format!("{}: z", $order));
        };
    }
    beside_another!(PostProblemsError::Status426, "union last");
    beside_another!(PostProblemsError::Status427, "union first");
}

#[test]
fn a_set_narrowed_against_a_uuid_is_open_in_either_member_order() {
    // `428` meets the `uuid` string last; `429` meets it first. Either way the listed value is its
    // variant and an unlisted one is kept.
    macro_rules! open_against_uuid {
        ($status:expr) => {
            let listed =
                decode($status, r#"{"kind":"00000000-0000-0000-0000-000000000001"}"#).unwrap();
            assert!(!format!("{:?}", listed.kind).starts_with("Unknown"), "{listed:?}");
            for unlisted in ["00000000-0000-0000-0000-000000000002", "z"] {
                let json = format!(r#"{{"kind":"{unlisted}"}}"#);
                let decoded = decode($status, &json).unwrap();
                assert_eq!(decoded.kind.as_str(), unlisted);
            }
        };
    }
    open_against_uuid!(PostProblemsError::Status428);
    open_against_uuid!(PostProblemsError::Status429);
}

#[test]
fn the_problem_reader_reads_the_open_type() {
    let body = decode(
        PostProblemsError::Status404,
        r#"{"type":"https://example.com/probs/moved","title":"t","detail":"d"}"#,
    )
    .unwrap();
    let error = Error::Api(ResponseValue::new(
        reqwest::StatusCode::NOT_FOUND,
        Default::default(),
        PostProblemsError::Status404(Box::new(body)),
    ));
    let problem = error.problem().expect("an object body");
    assert_eq!(problem.problem_type.as_deref(), Some("https://example.com/probs/moved"));
    assert_eq!(problem.detail.as_deref(), Some("d"));
}
"##,
    )
    .unwrap();
    let status = fixture_cargo(&out)
        .args(["test", "--test", "open"])
        .status()
        .unwrap();
    assert!(status.success(), "the open enums must decode as documented");
}
