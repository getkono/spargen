//! `open_narrowing`: the generated code for a narrowed problem type, compiled and linted, decoding
//! listed and unlisted values. Which positions the option opens, and that it reports nothing of
//! its own, are lowering decisions pinned in `frontend/open_narrowing.rs` over the same spec.

use super::*;

/// The RFC 9457 shape from #268: each documented error status narrows the shared `Problem`'s
/// `type` with a `const`, spelled as an `allOf` member (`404`) and as `$ref` siblings (`409`). The
/// positions `open_narrowing` leaves closed sit beside them: a union of narrowed problems (`400`),
/// a narrowing inside a component (`410`), a set narrowed against a `uuid` string in either member
/// order (`428`, `429`), and a request body.
const OPEN_NARROWING_SPEC: &str = include_str!("specs/open_narrowing.yaml");

#[test]
fn open_narrowing_decodes_an_unlisted_problem_type_and_keeps_it_typed() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("problems.yaml");
    std::fs::write(&spec, OPEN_NARROWING_SPEC).unwrap();

    let out = temp.path().join("client");
    let report = generate_configured_fixture_crate(&spec, &out, "open_problems", "2021", |spec| {
        spec.open_narrowing(true)
    });
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
        r##"use open_problems::{Error, PostProblemsError, ResponseValue};

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
        r#"Other("https://example.com/probs/moved")"#
    );
    assert_eq!(unlisted.r#type.to_string(), "https://example.com/probs/moved");
    // It is still a string on the wire, both ways.
    let wire = serde_json::to_value(&unlisted).unwrap();
    assert_eq!(wire["type"], "https://example.com/probs/moved");
    // A non-string is refused: the open set's domain is the `string` it narrowed.
    assert_refused(
        decode(PostProblemsError::Status404, r#"{"type":7,"title":"t"}"#),
        "a non-string type",
    );
}

#[test]
fn the_ref_sibling_spelling_opens_and_a_value_spelled_other_keeps_its_name() {
    let other = decode(PostProblemsError::Status409, r#"{"type":"other","title":"t"}"#).unwrap();
    // The listed `other` keeps `Other`; the catch-all takes the next name.
    assert_eq!(format!("{:?}", other.r#type), "Other");
    let unlisted =
        decode(PostProblemsError::Status409, r#"{"type":"elsewhere","title":"t"}"#).unwrap();
    assert!(
        format!("{:?}", unlisted.r#type).ends_with(r#"("elsewhere")"#),
        "{unlisted:?}"
    );
}

#[test]
fn a_union_and_a_component_stay_closed() {
    // Each union variant still refuses the other's type, so exactly one matches...
    let b = decode(
        PostProblemsError::Status400,
        r#"{"type":"https://example.com/probs/b","title":"t"}"#,
    )
    .expect("the variant listing `b` decodes it");
    assert_eq!(
        format!("{b:?}"),
        r#"ResponseBodyVariant1(ResponseBodyVariant1 { type: HttpsExampleComProbsB, title: "t", detail: None })"#
    );
    // ...and a type neither lists matches neither.
    assert_refused(
        decode(
            PostProblemsError::Status400,
            r#"{"type":"https://example.com/probs/c","title":"t"}"#,
        ),
        "400: a type no variant lists",
    );
    assert_refused(
        decode(
            PostProblemsError::Status410,
            r#"{"type":"https://example.com/probs/moved","title":"t"}"#,
        ),
        "410: a type the closed set does not list",
    );
    // A narrowing through a `$ref`'d value opens a copy in the response and leaves the component
    // closed for its other uses.
    let copy = decode(
        PostProblemsError::Status403,
        r#"{"type":"https://example.com/probs/moved","title":"t"}"#,
    )
    .expect("the response's own copy is open");
    assert_eq!(copy.r#type.as_str(), "https://example.com/probs/moved");
    assert_refused(
        serde_json::from_str::<open_problems::types::ForbiddenType>(
            r#""https://example.com/probs/moved""#,
        ),
        "the component itself stays closed",
    );
    // A request body is never opened.
    assert_refused(
        serde_json::from_str::<open_problems::types::RequestBody>(
            r#"{"type":"https://example.com/probs/moved","title":"t"}"#,
        ),
        "a request body stays closed",
    );
}

#[test]
fn a_union_meeting_an_open_set_decodes_in_either_member_order() {
    // `422` writes the union last, after the set has opened; `423` writes it first. Either way
    // each variant stays closed, so a listed value matches exactly one of them.
    for (order, decoded) in [
        ("union last", decode(PostProblemsError::Status422, r#"{"kind":"a"}"#).map(|_| ())),
        ("union last", decode(PostProblemsError::Status422, r#"{"kind":"b"}"#).map(|_| ())),
        ("union first", decode(PostProblemsError::Status423, r#"{"kind":"a"}"#).map(|_| ())),
        ("union first", decode(PostProblemsError::Status423, r#"{"kind":"b"}"#).map(|_| ())),
    ] {
        decoded.unwrap_or_else(|error| panic!("{order}: a value one variant lists: {error}"));
    }
    // A value the set lists but no variant does, and one nothing lists, match no variant.
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
    // A union that narrows to one branch is no union: in either order the result is the response's
    // own open set, so an unlisted value is kept.
    for kind in ["a", "z"] {
        let json = format!(r#"{{"kind":"{kind}"}}"#);
        let last = decode(PostProblemsError::Status424, &json).expect("union last");
        let first = decode(PostProblemsError::Status425, &json).expect("union first");
        assert_eq!(last.kind.as_str(), kind);
        assert_eq!(first.kind.as_str(), kind);
    }
    // A plain `string` branch kept beside another stays closed in either order: `b` matches it
    // alone, `a` matches both branches (which `oneOf` forbids), and `z` matches neither.
    macro_rules! closed_beside_another {
        ($status:expr, $order:literal) => {
            decode($status, r#"{"kind":"b"}"#).unwrap_or_else(|error| panic!("{}: {error}", $order));
            assert_refused(decode($status, r#"{"kind":"a"}"#), &format!("{}: a", $order));
            assert_refused(decode($status, r#"{"kind":"z"}"#), &format!("{}: z", $order));
        };
    }
    closed_beside_another!(PostProblemsError::Status426, "union last");
    closed_beside_another!(PostProblemsError::Status427, "union first");
}

#[test]
fn a_set_narrowed_against_a_uuid_stays_closed_in_either_member_order() {
    // `428` opens the set before the `uuid` string meets it; `429` meets the `uuid` string first.
    // Either way the listed value decodes and an unlisted one, a uuid or not, is refused.
    macro_rules! closed_against_uuid {
        ($status:expr, $order:literal) => {
            decode($status, r#"{"kind":"00000000-0000-0000-0000-000000000001"}"#)
                .unwrap_or_else(|error| panic!("{}: listed: {error}", $order));
            assert_refused(
                decode($status, r#"{"kind":"00000000-0000-0000-0000-000000000002"}"#),
                &format!("{}: unlisted uuid", $order),
            );
            assert_refused(
                decode($status, r#"{"kind":"z"}"#),
                &format!("{}: unlisted string", $order),
            );
        };
    }
    closed_against_uuid!(PostProblemsError::Status428, "uuid last");
    closed_against_uuid!(PostProblemsError::Status429, "uuid first");
}

#[test]
fn the_problem_reader_reads_the_opened_type() {
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

/// A response body that is `allOf: [{type: array}, member]`, where `member` refines the array's
/// items with a string `enum` narrowed against a plain `string`, written in place or as `It`.
fn array_refined_by(member: &str) -> String {
    format!(
        r##"
openapi: 3.1.0
info: {{ title: Refined, version: 1.0.0 }}
paths:
  /items:
    get:
      operationId: getItems
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                allOf:
                  - {{ type: array }}
                  - {member}
components:
  schemas:
    It:
      items:
        allOf: [{{ type: string }}, {{ enum: [a, b] }}]
"##
    )
}

/// Under `open_narrowing`, a `$ref` member of untyped array applicators alone (#610) is lowered as
/// the `$ref` target it is, where the option is out of effect, so its item set stays closed; the
/// same member written in the response body is in the body's own position, so its set opens.
#[test]
fn a_ref_member_of_untyped_array_applicators_keeps_its_narrowing_closed() {
    let inline = "{ items: { allOf: [{ type: string }, { enum: [a, b] }] } }";
    let reference = "{ $ref: '#/components/schemas/It' }";
    let open = |spec: Spec| spec.open_narrowing(true);
    let catch_all = "Other(String)";

    // The option off: neither spelling opens anything.
    for member in [inline, reference] {
        let module = spec_module("openapi.yaml", &array_refined_by(member), |spec| spec);
        assert!(!module.contains(catch_all), "{member}:\n{module}");
    }
    // The option on: the inline member opens its set, the `$ref` member keeps it closed.
    let module = spec_module("openapi.yaml", &array_refined_by(inline), open);
    assert!(
        module.contains(catch_all),
        "the inline member opens:\n{module}"
    );
    let module = spec_module("openapi.yaml", &array_refined_by(reference), open);
    assert!(
        !module.contains(catch_all),
        "the `$ref` member stays closed:\n{module}"
    );
    // The body's items are exactly the closed `[a, b]` enum: the embedded runtime declares enums
    // and unit variants of its own, so the items' type is asserted by name.
    assert!(
        module.contains("pub type ResponseBody = Vec<ResponseBodyMember1ConstraintItem>;")
            && module.contains(
                "pub enum ResponseBodyMember1ConstraintItem {\n        \
                 #[serde(rename = \"a\")]\n        A,\n        \
                 #[serde(rename = \"b\")]\n        B,\n    }"
            ),
        "the `$ref` member still narrows the items to `[a, b]`:\n{module}"
    );
}
