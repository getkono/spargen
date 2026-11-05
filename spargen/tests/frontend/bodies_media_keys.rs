//! Media keys: the media-range and media-key grammar, structured-suffix ranges and the `W014` a
//! withheld range draws, and what a rejected or accepted response selection claims.

use super::*;

#[test]
fn a_concrete_media_type_outranks_a_range_that_precedes_it() {
    // Ranges rank below every codec, so a concrete sibling wins wherever it sits in the document
    // (the one type ranked below the ranges is a concrete `image`/`audio`/`video` member, and only
    // on a response; pinned elsewhere). Two ranges at the same rank still tie by source order, as
    // equal-ranked concrete media already do.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /thing:
    get:
      operationId: getThing
      responses:
        "200":
          description: OK
          content:
            "*/*": { schema: {} }
            application/json: { schema: { type: object, properties: { a: { type: string } } } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    // JSON won, so the alternative that was dropped is the range — and that one really is a
    // narrowing, because a JSON body and an opaque one decode differently.
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
    assert!(
        !code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );
}

#[test]
fn e009_a_media_range_cannot_be_a_request_content_type() {
    // A range describes what a server may return, not what a client sends. A generated request puts
    // its media key on the wire verbatim, and `Content-Type: video/*` is not a dispatchable header
    // — while choosing a concrete member of the family would be spargen inventing what the document
    // declined to say.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          video/*: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
}

#[test]
fn a_binary_body_whose_schema_failed_to_lower_is_not_retyped_to_bytes() {
    // Retyping to `Bytes` applies to a body that *said nothing*. A body that declared a schema
    // which then failed to lower has already reported its own failure, and quietly handing it back
    // as bytes would turn that error into a silently different type.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: { $ref: "#/components/schemas/Loop" } }
components:
  schemas:
    Loop: { $ref: "#/components/schemas/Loop" }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
}

#[test]
fn a_media_range_is_matched_case_insensitively() {
    // Media types are case-insensitive (RFC 9110 § 8.3.1). Reading `TEXT/*` as a binary family
    // would be silently wrong rather than loudly unsupported.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /note:
    get:
      operationId: getNote
      responses:
        "200":
          description: OK
          content:
            TEXT/*: { schema: { type: string } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !code.contains("pub type ResponseBody = bytes::Bytes;"),
        "an upper-case text range is still text: {code}"
    );
}

#[test]
fn e009_a_range_naming_no_family_is_unsupported() {
    // `/*` has no type before the slash, so it names nothing. It stays unsupported rather than
    // being read as opaque octets on the strength of its suffix alone.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "/*": { schema: {} }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
}

#[test]
fn e009_a_media_range_with_an_extra_slash_is_unsupported() {
    // `a/b/*` ends in `/*`, but what precedes the suffix is `a/b`, which is not a type name. It was
    // read as the family `a/b` and generated as opaque octets. A key that is not a media range names
    // no family, so it is unsupported like any other key that is not a media type.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "a/b/*": { schema: {} }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
}

/// Assert that each `(key, request, schema)` case, as the sole `content` key of a request body
/// (`request`) or a response, is rejected with `E009` through both `generate` and `check`.
fn assert_each_media_key_is_unsupported(cases: &[(&str, bool, &str)]) {
    fn document(key: &str, request: bool, schema: &str) -> String {
        if request {
            format!(
                r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    post:
      operationId: postX
      requestBody:
        required: true
        content:
          "{key}": {{ schema: {schema} }}
      responses:
        "204": {{ description: No Content }}
"##
            )
        } else {
            format!(
                r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "{key}": {{ schema: {schema} }}
"##
            )
        }
    }
    for &(key, request, schema) in cases {
        let spec = document(key, request, schema);
        let report = generate(&spec);
        assert_eq!(report.outcome(), Outcome::Rejected, "`{key}`: {report:#?}");
        assert!(
            has_code(&report, Code::UnsupportedMediaType),
            "`{key}`: {report:#?}"
        );
    }
}

#[test]
fn e009_a_media_key_that_is_not_a_restricted_name_is_unsupported() {
    // A media type is exactly one `/` between two RFC 6838 § 4.2 restricted names. Each key below
    // breaks that, yet most reached a codec through an arm that matched only part of the key: the
    // `text/` prefix, the `application/…+json` suffix, or a range's `/*`. `image/jpeg/extra` pins
    // the concrete binary family, which must not be read as `image` octets.
    // One byte past the 127-byte limit on a restricted name.
    let too_long = format!("text/{}", "a".repeat(128));
    assert_each_media_key_is_unsupported(&[
        ("image/jpeg/extra", true, "{}"),
        ("text/plain/extra", false, "{ type: string }"),
        ("application/vnd.a/b+json", true, "{ type: object }"),
        ("text/", false, "{ type: string }"),
        ("text/pl ain", false, "{ type: string }"),
        ("**/*", false, "{}"),
        (too_long.as_str(), false, "{ type: string }"),
    ]);
}

#[test]
fn e009_a_wildcard_inside_a_name_is_unsupported() {
    // `*` is a whole-name wildcard, never part of a name: `*/json` is no range (a range fixes the
    // type and wildcards the subtype), and `image/pn*` is no type at all. Neither ever reached an
    // arm, so `text/pl*in` and `application/vn*+json` are here to discriminate: without the rule,
    // the `text/` prefix arm and the `+json` suffix arm would accept them.
    assert_each_media_key_is_unsupported(&[
        ("*/json", false, "{ type: object }"),
        ("image/pn*", false, "{}"),
        ("text/pl*in", false, "{ type: string }"),
        ("application/vn*+json", false, "{ type: object }"),
    ]);
}

#[test]
fn e009_a_name_starting_with_a_symbol_is_unsupported() {
    // `.`, `-` and the other symbols RFC 6838 § 4.2 permits may follow the first byte of a
    // restricted name but may not be it, in the type position or the subtype position.
    assert_each_media_key_is_unsupported(&[
        (".type/x", false, "{ type: string }"),
        ("-x/y", false, "{ type: string }"),
        ("text/.plain", false, "{ type: string }"),
        ("text/-plain", false, "{ type: string }"),
    ]);
}

#[test]
fn w014_a_malformed_key_beside_a_well_formed_sibling_is_ignored() {
    // A malformed key does not reject the whole map when a well-formed sibling exists: only that
    // key is dropped, and it is named under W014 like any other alternative that is not generated.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "text/plain/extra": { schema: { type: string } }
            application/json:
              schema: { type: object, required: [id], properties: { id: { type: integer } } }
"##;
    let (report, code) = generate_with_code(spec);
    let checked = check(spec);
    for report in [&report, &checked] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(report, Code::UnsupportedMediaType), "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|diagnostic| {
                diagnostic.code == Code::AlternativeMediaIgnored
                    && diagnostic.message
                        == "`application/json` is selected; the alternative media type(s) \
                            `text/plain/extra` are not"
            }),
            "{report:#?}"
        );
    }
    assert!(code.contains("pub type ResponseBodyid = i64;"), "{code}");
}

#[test]
fn well_formed_media_keys_still_generate() {
    // The restricted-name check must not reject what real descriptions write: dotted vendor `+json`
    // types, a key with parameters (stripped before the check), both kinds of range, every
    // non-alphanumeric byte RFC 6838 permits, and a subtype at exactly the 127-byte limit.
    let at_limit = format!("text/{}", "a".repeat(127));
    let keys = [
        ("application/vnd.github+json", "{ type: object }"),
        ("application/vnd.github.v3.star+json", "{ type: object }"),
        ("text/plain; charset=utf-8", "{ type: string }"),
        ("*/*", "{}"),
        ("application/*", "{}"),
        ("text/x-a!b#c$d&e^f_g.h+i", "{ type: string }"),
        (at_limit.as_str(), "{ type: string }"),
    ];
    // One operation per key, each with a single content entry, so no key competes with another.
    let mut spec = String::from("openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths:\n");
    for (index, (key, schema)) in keys.into_iter().enumerate() {
        spec += &format!(
            r##"  /op{index}:
    get:
      operationId: op{index}
      responses:
        "200":
          description: OK
          content:
            "{key}": {{ schema: {schema} }}
"##
        );
    }
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    // Seven operations share the `ResponseBody` name, so each alias carries a disambiguating suffix.
    let opaque = code
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.starts_with("pub type ResponseBody") && line.ends_with(" = bytes::Bytes;")
        })
        .count();
    assert_eq!(
        opaque, 2,
        "exactly the `*/*` and `application/*` ranges are opaque octets: {code}"
    );
}

#[test]
fn a_structured_suffix_range_response_generates_json() {
    // `application/*+json` is a media range over every structured JSON subtype (RFC 9110's
    // media-range grammar, with RFC 6838 § 4.2.8 structured syntax suffixes). The support matrix
    // promises it as JSON, and a response offering it alone is decoded as JSON rather than
    // rejected for its `*`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "application/*+json":
              schema: { type: object, required: [id], properties: { id: { type: integer } } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        code.contains("pub struct ResponseBody {")
            && code.contains("pub type ResponseBodyid = i64;"),
        "a typed JSON body, not bytes or text: {code}"
    );
}

#[test]
fn w014_a_structured_suffix_range_ties_its_codec_on_a_response() {
    // Unlike a family range, which every concrete sibling outranks, `application/*+json` ranks
    // level with `application/json` (the support matrix and `E009` say so). On a response nothing
    // withholds it, so the tie is broken by source order: whichever is listed first is generated,
    // and the other is reported as the alternative that is not. The two schemas differ so the
    // generated body shows which key won.
    let range = r#""application/*+json": { schema: { type: object, required: [fromRange], properties: { fromRange: { type: integer } } } }"#;
    let json = r#""application/json": { schema: { type: object, required: [fromJson], properties: { fromJson: { type: integer } } } }"#;
    for (first, second, generated, other, field, other_field) in [
        (
            range,
            json,
            "application/*+json",
            "application/json",
            "from_range",
            "from_json",
        ),
        (
            json,
            range,
            "application/json",
            "application/*+json",
            "from_json",
            "from_range",
        ),
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            {first}
            {second}
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert!(
            code.contains(&format!("pub {field}: "))
                && !code.contains(&format!("pub {other_field}: ")),
            "`{generated}` listed first is generated, `{other}` is not: {code}"
        );
        let expected =
            format!("`{generated}` is selected; the alternative media type(s) `{other}` are not");
        for report in [report, check(&spec)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
            assert!(
                report.diagnostics().iter().any(|diagnostic| {
                    diagnostic.code == Code::AlternativeMediaIgnored
                        && diagnostic.message == expected
                }),
                "{report:#?}"
            );
        }
    }
}

#[test]
fn e009_a_structured_suffix_range_cannot_be_a_request_content_type() {
    // A request puts its media key on the wire verbatim, and `Content-Type: application/*+json`
    // names a family rather than a type, exactly like `video/*`. It is rejected as a range.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      operationId: postX
      requestBody:
        required: true
        content:
          "application/*+json": { schema: { type: object } }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == Code::UnsupportedMediaType
                    && diagnostic.message.contains("is a media range")),
            "{report:#?}"
        );
    }
}

/// A request body whose `content` lists `application/*+json` first and then `sibling`.
fn suffix_range_request_document(sibling: &str, sibling_schema: &str) -> String {
    format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    post:
      operationId: postX
      requestBody:
        required: true
        content:
          "application/*+json": {{ schema: {{ type: object }} }}
          "{sibling}": {{ schema: {sibling_schema} }}
      responses:
        "204": {{ description: No Content }}
"##
    )
}

#[test]
fn w014_a_structured_suffix_range_yields_to_a_sendable_request_sibling() {
    // `application/*+json` ranks with the concrete JSON types, so listed first it would win the
    // tie by source order and then be refused as a range, rejecting a body that offered something
    // sendable. While a concrete sibling can be sent, the range is not a candidate: the sibling is
    // generated and the range is reported as the alternative that is not, whatever the sibling's
    // rank (`text/plain` ranks below every JSON type).
    for (sibling, schema) in [
        ("application/json", "{ type: object }"),
        ("application/merge-patch+json", "{ type: object }"),
        ("text/plain", "{ type: string }"),
    ] {
        let spec = suffix_range_request_document(sibling, schema);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::UnsupportedMediaType),
                "`{sibling}` through {entry}: {report:#?}"
            );
            let expected = format!(
                "`{sibling}` is selected; the alternative media type(s) `application/*+json` are not"
            );
            assert!(
                report.diagnostics().iter().any(|diagnostic| {
                    diagnostic.code == Code::AlternativeMediaIgnored
                        && diagnostic.message == expected
                }),
                "`{sibling}` through {entry}: {report:#?}"
            );
        }
    }
}

#[test]
fn e009_a_structured_suffix_range_with_no_sendable_request_sibling_is_unsupported() {
    // With nothing beside it that a request could send, the suffix range is still what the body
    // offers, and it is still refused as a range. Neither another range nor a streaming media
    // counts as sendable.
    for (sibling, schema) in [("video/*", "{}"), ("text/event-stream", "{ type: string }")] {
        let spec = suffix_range_request_document(sibling, schema);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|diagnostic| {
                    diagnostic.code == Code::UnsupportedMediaType
                        && diagnostic
                            .message
                            .starts_with("media type `application/*+json` is a media range")
                }),
                "`{sibling}` through {entry}: {report:#?}"
            );
        }
    }
}

#[test]
fn w014_a_suffix_range_listed_after_a_sendable_request_sibling_is_withheld() {
    // Order does not decide it. A sendable sibling listed before the range is generated and the
    // range is named as not generated, even for a sibling whose rank the range's rank 0 would
    // otherwise beat (`text/plain`).
    for (sibling, schema) in [
        ("application/json", "{ type: object }"),
        ("text/plain", "{ type: string }"),
    ] {
        let spec = request_body_document(&[
            (sibling, schema),
            ("application/*+json", "{ type: object }"),
        ]);
        let expected = format!(
            "`{sibling}` is selected; the alternative media type(s) `application/*+json` are not"
        );
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::UnsupportedMediaType),
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert_eq!(
                messages_for(&report, Code::AlternativeMediaIgnored),
                [expected.as_str()],
                "`{sibling}` through {entry}: {report:#?}"
            );
        }
    }
}

#[test]
fn w014_every_suffix_range_beside_a_sendable_request_sibling_is_withheld() {
    // Every structured-suffix range that classifies is withheld, not only the first, and all of
    // them are named in one report.
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("application/*+json-seq", "{ type: object }"),
        ("application/json", "{ type: object }"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
        assert_eq!(
            messages_for(&report, Code::AlternativeMediaIgnored),
            [
                "`application/json` is selected; the alternative media type(s) \
                 `application/*+json`, `application/*+json-seq` are not"
            ],
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_suffix_range_beside_only_an_unclassified_request_sibling_is_a_media_range() {
    // A sibling that does not classify is not sendable, so nothing is withheld: the range is still
    // the only thing the body offers, and it is refused as a range.
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("application/pdf", "{}"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            messages_for(&report, Code::UnsupportedMediaType)
                .iter()
                .any(|message| message
                    .starts_with("media type `application/*+json` is a media range")),
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_sendable_request_sibling_that_fails_its_own_gate_is_reported_for_itself() {
    // Sendable is decided by classification alone. A `text/plain` sibling carrying an object schema
    // is still chosen over the range, and then refused by the raw-text gate for its own reason
    // rather than the range's. Neither the withheld range nor anything else is then reported as
    // passed over for a `text/plain` selection that is then refused (#110).
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("text/plain", "{ type: object }"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
        let unsupported = messages_for(&report, Code::UnsupportedMediaType);
        assert!(
            unsupported
                .iter()
                .all(|message| !message.contains("is a media range")),
            "{report:#?}"
        );
        assert!(
            unsupported
                .iter()
                .any(|message| message.contains("text/plain")),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_a_withheld_suffix_range_beside_two_request_entries_is_reported_separately() {
    // Pinned as it stands. `choose_media` names the alternatives it passed over, and the withheld
    // range gets its own W014 right after, so this body carries two: both true, always in this
    // order, and both emitted only once the selection has passed every request-body gate.
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("application/json", "{ type: object }"),
        ("application/xml", "{ type: object }"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
        assert_eq!(
            messages_for(&report, Code::AlternativeMediaIgnored),
            [
                "`application/json` is selected; the alternative media type(s) \
                 `application/xml` are not",
                "`application/json` is selected; the alternative media type(s) \
                 `application/*+json` are not",
            ],
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_rejected_response_selection_claims_nothing_is_generated() {
    // The response side of #110: `text/plain` is selected over `text/html` (same rank, listed
    // first) and refused by the raw-text gate, so its `E009` alone reports it and no `W014` names
    // `text/html` as passed over.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /page:
    get:
      operationId: getPage
      responses:
        "200":
          description: OK
          content:
            text/plain: { schema: { type: object } }
            text/html: { schema: { type: object } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
        assert_eq!(
            messages_for(&report, Code::UnsupportedMediaType),
            ["media type `text/plain` requires a string-like or binary response schema"],
            "{report:#?}"
        );
    }
}

#[test]
fn w014_an_accepted_response_selection_still_reports_its_alternatives() {
    // The counterpart: the same shape with a string schema passes the gate, so the narrowing it
    // makes is still disclosed.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /page:
    get:
      operationId: getPage
      responses:
        "200":
          description: OK
          content:
            text/plain: { schema: { type: string } }
            text/html: { schema: { type: string } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(
            messages_for(&report, Code::AlternativeMediaIgnored),
            ["`text/plain` is selected; the alternative media type(s) `text/html` are not"],
            "{report:#?}"
        );
    }
}

#[test]
fn w014_on_a_document_rejected_elsewhere_claims_only_the_selection() {
    // `/page` passes every one of its own gates, so its `W014` is emitted; `/doc` is rejected, so
    // the run generates nothing — and `check` never generates on any document. The message must
    // therefore assert only what its emission site decides, the selection, and never that anything
    // "is generated" (#174). The `openai_openapi` corpus snapshot carries this shape at scale:
    // `Rejected` with `E009` beside many `W014`s.
    //
    // This pins the wording. The principle is held by `run_generate`/`run_check`, which compare
    // each diagnostic's structured `claim` with the run's outcome (#413); see
    // `a_generation_claim_on_w014_is_contradicted_wherever_174_found_it_false`.
    let spec = W014_REJECTED_ELSEWHERE;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
        assert_eq!(
            messages_for(&report, Code::AlternativeMediaIgnored),
            ["`text/plain` is selected; the alternative media type(s) `text/html` are not"],
            "{report:#?}"
        );
    }
}

#[test]
fn a_sequential_media_outranks_a_text_range() {
    // `text/*` used to classify as `Text` by accident of the `text/` prefix arm, at the same rank
    // as a concrete textual type and *above* sequential media — so this response was a whole-body
    // `String`. As a range it now ranks below both, and the concrete streaming type wins. That is a
    // change to the generated public API, so it is pinned rather than left to be rediscovered.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: getEvents
      responses:
        "200":
          description: OK
          content:
            text/*: { schema: { type: string } }
            text/event-stream:
              schema: { type: object, required: [seq], properties: { seq: { type: integer } } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("EventStream"), "{code}");
    // Two media types that decode differently: the range really is not generated.
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}
