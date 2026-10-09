//! Responses: status-keyed enums, response keys, response headers, and response components.

use super::*;

#[test]
fn response_description_requirement_is_version_gated() {
    let body = r#"
info: { title: T, version: 1.0.0 }
paths:
  /items:
    get:
      responses:
        '200': { summary: ok }
"#;
    let oas31 = generate(&format!("openapi: 3.1.0\n{body}"));
    assert_eq!(oas31.outcome(), Outcome::Rejected, "{oas31:#?}");
    assert!(has_code(&oas31, Code::InvalidInput), "{oas31:#?}");

    let oas32 = generate(&format!("openapi: 3.2.0\n{body}"));
    assert_ne!(oas32.outcome(), Outcome::Rejected, "{oas32:#?}");
}

#[test]
fn response_component_aliases_resolve_and_cycles_reject() {
    let base = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /item:
    get:
      responses:
        '200': { $ref: '#/components/responses/A' }
components:
  responses:
    A: { $ref: '#/components/responses/B' }
    B:
      summary: shared result
      description: ok
      content:
        application/json: { schema: { type: string } }
"##;
    let (report, code) = generate_with_code(base);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("shared result"), "{code}");

    let cycle = base.replace(
        "B:\n      summary: shared result\n      description: ok\n      content:\n        application/json: { schema: { type: string } }",
        "B: { $ref: '#/components/responses/A' }",
    );
    let report = generate(&cycle);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
}

#[test]
fn multi_status_success_bodies_generate_a_typed_enum_not_a_degraded_value() {
    // Two success statuses with DIFFERENT bodies used to degrade to `serde_json::Value` (W003).
    // W003 is retired: the success type is now a typed per-operation response enum, generated with
    // no diagnostic at all.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/BodyA" }
        "201":
          description: Created
          content:
            application/json:
              schema: { $ref: "#/components/schemas/BodyB" }
components:
  schemas:
    BodyA:
      type: object
      properties:
        a: { type: string }
    BodyB:
      type: object
      properties:
        b: { type: string }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    // No diagnostics at all — the retired W003 must not fire under any code.
    assert!(report.diagnostics().is_empty(), "{report:#?}");
}

#[test]
fn multi_status_error_bodies_generate_a_typed_enum_not_a_degraded_value() {
    // Two error statuses with DIFFERENT bodies likewise generate a typed error enum, no W003.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { type: string }
        "404":
          description: Not Found
          content:
            application/json:
              schema: { $ref: "#/components/schemas/ErrA" }
        "409":
          description: Conflict
          content:
            application/json:
              schema: { $ref: "#/components/schemas/ErrB" }
components:
  schemas:
    ErrA:
      type: object
      properties:
        a: { type: string }
    ErrB:
      type: object
      properties:
        b: { type: string }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
}

#[test]
fn a_bodyless_error_status_beside_one_error_body_is_its_own_variant() {
    // Issue #204: a documented bodyless error entry beside exactly one documented error body was
    // dropped from the generated error type — a newtype over that one body — with no diagnostic,
    // so a real `403` arrived as `UnexpectedStatus` and a bodyless `304` under a bodied `default`
    // had its empty body decoded as the `default` model. Every declared error entry is now a
    // variant once there is more than one, as on the success side (issue #121): the bodyless one
    // a unit variant. A lone bodied error entry is still the newtype.
    const PROBLEM: &str = "{ description: problem, content: { application/json: { schema: { $ref: '#/components/schemas/Problem' } } } }";
    const PET: &str = "{ description: pet, content: { application/json: { schema: { $ref: '#/components/schemas/Pet' } } } }";
    const XML: &str = "{ description: problem, content: { application/xml: { schema: { $ref: '#/components/schemas/Problem' } } } }";
    const NONE: &str = "{ description: none }";
    let spec = |responses: &[(&str, &str)]| {
        let responses: String = responses
            .iter()
            .map(|(key, response)| format!("        '{key}': {response}\n"))
            .collect();
        format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths:\n  /x:\n    get:\n      \
             operationId: getX\n      responses:\n{responses}components:\n  schemas:\n    \
             Pet: {{ type: object, properties: {{ name: {{ type: string }} }} }}\n    \
             Problem: {{ type: object, properties: {{ detail: {{ type: string }} }} }}\n"
        )
    };
    for (responses, expected) in [
        // The issue's repro: a bodyless `403` beside a bodied `404`.
        (
            &[("200", PET), ("403", NONE), ("404", PROBLEM)][..],
            &["Status403", "Status404(Box<types::Problem>)"][..],
        ),
        // A bodyless range beside one bodied exact status, in either document order.
        (
            &[("200", PET), ("5XX", NONE), ("404", PROBLEM)][..],
            &["Status404(Box<types::Problem>)", "Status5xx"][..],
        ),
        // A bodyless `default` beside one bodied status is the catch-all unit variant, as it
        // already was beside two.
        (
            &[("200", PET), ("404", PROBLEM), ("default", NONE)][..],
            &["Status404(Box<types::Problem>)", "Default"][..],
        ),
        // A bodyless `304` beside a bodied `default`, with no success status declared: the `304`
        // is its own variant rather than a status the `default` body is decoded for.
        (
            &[("304", NONE), ("default", PROBLEM)][..],
            &["Status304", "Default(Box<types::Problem>)"][..],
        ),
        // One XML error body beside a bodyless status is still one body to decode as XML — not
        // the rejected two-XML-bodies shape (narrowed `E009`).
        (
            &[("200", PET), ("403", NONE), ("404", XML)][..],
            &["Status403", "Status404(Box<types::Problem>)"][..],
        ),
    ] {
        let spec = spec(responses);
        let (report, code) = generate_with_code(&spec);
        assert_eq!(report.outcome(), Outcome::Generated, "{spec}\n{report:#?}");
        assert!(report.diagnostics().is_empty(), "{spec}\n{report:#?}");
        assert_eq!(enum_variants(&code, "GetXError"), expected, "{spec}");
        assert!(!code.contains("pub struct GetXError("), "{spec}");
        let checked = check(&spec);
        assert_eq!(checked.outcome(), Outcome::Clean, "{checked:#?}");
        assert!(checked.diagnostics().is_empty(), "{checked:#?}");
    }

    // A single bodied error entry and nothing else on the error side is still the newtype.
    for responses in [
        &[("200", PET), ("404", PROBLEM)][..],
        &[("200", PET), ("default", PROBLEM)][..],
    ] {
        let (report, code) = generate_with_code(&spec(responses));
        assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
        assert!(
            code.contains("pub struct GetXError(pub types::Problem);"),
            "{responses:?}"
        );
    }
}

#[test]
fn multi_status_enum_precedence_emits_exact_arm_before_range_and_a_bodyless_unit_variant() {
    // Both classes list a RANGE before an overlapping EXACT in document order (and mix in a bodyless
    // 204). The emitter must reorder to exact-before-range so a real 200/409 dispatches to its exact
    // variant, and the bodyless 204 must appear as a payload-free unit variant — never a silent drop.
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec_path,
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /p:
    get:
      operationId: getP
      responses:
        "2XX":
          description: RangeOk
          content: { application/json: { schema: { $ref: "#/components/schemas/RangeOk" } } }
        "200":
          description: ExactOk
          content: { application/json: { schema: { $ref: "#/components/schemas/ExactOk" } } }
        "204":
          description: No Content
        "4XX":
          description: RangeErr
          content: { application/json: { schema: { $ref: "#/components/schemas/RangeErr" } } }
        "409":
          description: Conflict
          content: { application/json: { schema: { $ref: "#/components/schemas/Conflict" } } }
components:
  schemas:
    RangeOk: { type: object, properties: { r: { type: string } } }
    ExactOk: { type: object, properties: { e: { type: string } } }
    RangeErr: { type: object, properties: { x: { type: string } } }
    Conflict: { type: object, properties: { c: { type: string } } }
"##,
    )
    .unwrap();
    let out = temp.path().join("client.rs");
    let report = run_generate(&build(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        Utf8PathBuf::from_path_buf(out.clone()).unwrap(),
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");

    let code = std::fs::read_to_string(&out).unwrap();
    // Success dispatch: the exact 200 arm is emitted (and thus checked) before the 2XX range arm.
    let exact_200 = code.find("Exact(200u16)").expect("exact 200 selector");
    let range_2xx = code.find("Range(2u8)").expect("2XX range selector");
    assert!(
        exact_200 < range_2xx,
        "exact 200 must precede the 2XX range in the emitted decode chain"
    );
    // Error classification: the exact 409 arm precedes the 4XX range arm.
    let exact_409 = code.find("Exact(409u16)").expect("exact 409 selector");
    let range_4xx = code.find("Range(4u8)").expect("4XX range selector");
    assert!(
        exact_409 < range_4xx,
        "exact 409 must precede the 4XX range in the emitted classification chain"
    );
    // The bodyless 204 is a payload-free unit variant, not dropped and not a `serde_json::Value`.
    assert!(
        code.contains("Status204,"),
        "bodyless 204 must emit a unit variant"
    );
}

#[test]
fn multi_status_enum_precedence_emits_exact_and_range_arms_in_ascending_order() {
    // Issue #138: every selector class is listed in DESCENDING document order — the exact
    // successes, the exact errors, and the two error ranges — with `default` first. A key that
    // orders only by class keeps document order within each class, so only the emitted arm order
    // below tells ascending from document order. `4XX` and `5XX` are the only two ranges one sort
    // can hold (`2XX` is the lone success range), so the error chain is where "range ascending" is
    // observable at all.
    let temp = tempfile::tempdir().unwrap();
    let spec_path = temp.path().join("openapi.yaml");
    std::fs::write(
        &spec_path,
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /p:
    get:
      operationId: getP
      responses:
        default:
          description: Other
          content: { application/json: { schema: { $ref: "#/components/schemas/Other" } } }
        "201":
          description: Created
          content: { application/json: { schema: { $ref: "#/components/schemas/Created" } } }
        "200":
          description: Ok
          content: { application/json: { schema: { $ref: "#/components/schemas/Ok" } } }
        "5XX":
          description: ServerErr
          content: { application/json: { schema: { $ref: "#/components/schemas/ServerErr" } } }
        "4XX":
          description: ClientErr
          content: { application/json: { schema: { $ref: "#/components/schemas/ClientErr" } } }
        "409":
          description: Conflict
          content: { application/json: { schema: { $ref: "#/components/schemas/Conflict" } } }
        "404":
          description: Missing
          content: { application/json: { schema: { $ref: "#/components/schemas/Missing" } } }
components:
  schemas:
    Other: { type: object, properties: { o: { type: string } } }
    Created: { type: object, properties: { c: { type: string } } }
    Ok: { type: object, properties: { k: { type: string } } }
    ServerErr: { type: object, properties: { s: { type: string } } }
    ClientErr: { type: object, properties: { l: { type: string } } }
    Conflict: { type: object, properties: { f: { type: string } } }
    Missing: { type: object, properties: { m: { type: string } } }
"##,
    )
    .unwrap();
    let out = temp.path().join("client.rs");
    let report = run_generate(&build(
        Utf8PathBuf::from_path_buf(spec_path).unwrap(),
        Utf8PathBuf::from_path_buf(out.clone()).unwrap(),
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");

    let code = std::fs::read_to_string(&out).unwrap();
    let at = |selector: &str| {
        assert_eq!(
            code.matches(selector).count(),
            1,
            "{selector} must appear exactly once, as its dispatch arm"
        );
        code.find(selector).unwrap()
    };
    // Success dispatch: 200 before 201, though the document lists 201 first.
    assert!(
        at("Exact(200u16)") < at("Exact(201u16)"),
        "exact successes must dispatch in ascending code order"
    );
    // Error classification: 404 < 409 < 4XX < 5XX, each listed after its successor in the document.
    let errors = [
        at("Exact(404u16)"),
        at("Exact(409u16)"),
        at("Range(4u8)"),
        at("Range(5u8)"),
    ];
    assert!(
        errors.is_sorted(),
        "error arms must be exact ascending, then range ascending: {errors:?}"
    );
}

#[test]
fn documented_response_headers_get_typed_accessors() {
    // Regression: `response.headers` was dropped entirely, with no diagnostic.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "200":
          description: OK
          headers:
            X-RateLimit-Remaining:
              required: true
              schema: { type: integer }
            X-Next:
              $ref: "#/components/headers/Next"
            Content-Type:
              schema: { type: string }
          content:
            application/json:
              schema: { type: array, items: { type: string } }
components:
  headers:
    Next:
      description: The cursor for the next page.
      schema: { type: string }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("ListPetsStatus200Headers"), "{code}");
    // A required header is a plain field; an optional one is an Option. Inline header schemas get
    // a synthesized named type, exactly as inline schemas elsewhere do.
    assert!(
        code.contains("pub x_rate_limit_remaining: types::HeaderXRateLimitRemaining"),
        "{code}"
    );
    assert!(code.contains("pub x_next: Option<"), "{code}");
    assert!(code.contains("from_response"), "{code}");
    // A documented `Content-Type` is ignored per the specification, and said so.
    assert!(
        has_code(&report, Code::DeclarationHasNoEffect),
        "{report:#?}"
    );
    assert!(
        !code.contains("content_type:"),
        "a documented Content-Type header must not become a field: {code}"
    );
}

#[test]
fn set_cookie_response_header_is_a_list_of_lines() {
    // RFC 9110 s5.3 exempts `Set-Cookie` from the rule that lets a repeated header be folded into a
    // comma-separated line, and OpenAPI 3.2 gives it a section saying each value stays on its own
    // line. The declared schema describes one cookie, so the accessor is a list of them — the
    // generic path would have joined the occurrences and split them back apart on every comma,
    // fragmenting any cookie with an `Expires=Wed, 09 Jun ...` attribute.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "204":
          description: No Content
          headers:
            Set-Cookie:
              required: true
              schema: { type: string }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        code.contains("support::HeaderShape::SetCookie"),
        "a documented Set-Cookie header must use the non-joining shape: {code}"
    );
    assert!(
        code.contains("Vec<String>"),
        "the accessor must be a list of per-line values: {code}"
    );
}

#[test]
fn an_image_or_audio_range_as_the_only_response_key_is_a_byte_body() {
    // The repro from #82 verbatim: `image/*` (and `audio/*`) as the *sole* `content` key, with
    // `schema: {}`. It was rejected outright, and only generated when an unrelated
    // `application/octet-stream` sibling happened to be listed.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /artwork/{id}:
    get:
      operationId: getArtwork
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      responses:
        "200":
          description: An image
          content:
            image/*: { schema: {} }
  /clip:
    get:
      operationId: getClip
      responses:
        "200":
          description: A clip
          content:
            audio/*: { schema: {} }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert_ne!(check(spec).outcome(), Outcome::Rejected);
    assert_eq!(code.matches("= bytes::Bytes;").count(), 2, "{code}");
    assert!(!code.contains("= serde_json::Value;"), "{code}");
}

#[test]
fn w011_a_response_header_with_binary_family_content_is_acknowledged() {
    // A response header's `content` keyed `image/png` now classifies (as opaque octets) instead of
    // failing to classify, and lands on the existing header rule either way: octets are not a
    // header value spargen decodes, so the accessor is dropped with `W011` and the operation
    // generates — the disposition `*/*` already has there.
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
          headers:
            X-Thumbnail:
              content:
                image/png: { schema: {} }
          content:
            application/json: { schema: { type: string } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::DeclarationHasNoEffect && d.message.contains("`X-Thumbnail`")
            }),
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_malformed_response_header_content_key_is_unsupported() {
    // A response header's `content` key is classified like any other, so a key that is not a type
    // is reported rather than decoded as text on the strength of its `text/` prefix.
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
          headers:
            X-Detail:
              content:
                "text/plain/extra": { schema: { type: string } }
          content:
            application/json: { schema: { type: string } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|diagnostic| {
                diagnostic.code == Code::UnsupportedMediaType
                    && diagnostic.message == "media type `text/plain/extra` is not supported"
            }),
            "{report:#?}"
        );
    }
}

#[test]
fn a_textual_content_response_header_gets_a_typed_accessor() {
    // `Content-Range` on a ranged response is routinely documented with `content:` rather than
    // `schema:`. A textual content entry describes the field value itself, so it decodes exactly
    // like the `schema:` spelling — refusing it cost the accessor and reported `W011` on a shape
    // that is neither rare nor wrong.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /clip:
    get:
      operationId: getClip
      responses:
        "206":
          description: Partial Content
          headers:
            Content-Range:
              content:
                text/plain: { schema: { type: string } }
          content:
            application/octet-stream: { schema: {} }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::DeclarationHasNoEffect),
        "{report:#?}"
    );
    assert!(code.contains("content_range"), "{code}");
    assert_ne!(check(spec).outcome(), Outcome::Rejected, "{report:#?}");
}

#[test]
fn w011_response_header_content_media_spargen_cannot_decode_is_reported() {
    // The other side of the change, and a raise site that had no fixture: a `content` media type
    // that is neither JSON nor textual still drops the accessor, and still says so. `*/*` is here
    // because it used to be rejected outright (`E009`, unclassifiable) and now classifies as a
    // binary range — the operation generates and the header is acknowledged instead.
    for media in ["application/xml", r#""*/*""#] {
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
          headers:
            X-Detail:
              content:
                {media}: {{ schema: {{ type: string }} }}
          content:
            application/json: {{ schema: {{ type: string }} }}
"##
        );
        for report in [generate(&spec), check(&spec)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
            assert!(
                has_code(&report, Code::DeclarationHasNoEffect),
                "{report:#?}"
            );
        }
    }
}

#[test]
fn w011_a_textual_content_response_header_that_is_not_a_single_value_is_reported() {
    // Textual content carries the field value verbatim, so a list says nothing about how the value
    // is framed — and `simple` is not that framing.
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
          headers:
            X-Detail:
              content:
                text/plain: { schema: { type: array, items: { type: string } } }
          content:
            application/json: { schema: { type: string } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::DeclarationHasNoEffect),
        "{report:#?}"
    );
}

/// A Responses key the specification does not define, reached through a Path Item `$ref`.
///
/// `references/3.2.0.md` closes the grammar — *"Only the following range definitions are allowed:
/// `1XX`, `2XX`, `3XX`, `4XX`, and `5XX`"* — and the metaschema spells it `^[1-5](?:[0-9]{2}|XX)$`.
/// Before the key was checked behind a `$ref`, `0XX` lowered to `StatusSpec::Range(0)`, which was
/// then the sentinel `default` itself lowered to (it is now `StatusSpec::Default`): the
/// operation's error enum got **two** `Default` variants and `rustc` refused the emitted module
/// with `E0428` — at outcome `Generated`, with zero diagnostics. That is the fourth, silent
/// behavior the contract forbids, on a construct the specification explicitly closes.
#[test]
fn e011_out_of_grammar_response_key_behind_a_ref_is_rejected() {
    let (generated, checked) = generate_and_check_refd_path_item(
        "get:\n  operationId: getPet\n  responses:\n    '200': { description: ok }\n    '0XX': { description: collides with the default sentinel }\n    default: { description: fallback }\n",
    );
    assert_eq!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
    assert!(has_code(&generated, Code::InvalidInput), "{generated:#?}");
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::InvalidInput), "{checked:#?}");
}

/// The other faces of the same defect, each reached through a `$ref`, where the metaschema once
/// never looked. `02XX` lowered to the same `StatusSpec` as `2XX`, and `0200`/`+200` to the same one as
/// `200` — a duplication `E022` cannot catch, because these are distinct map keys. `6XX`-`9XX`
/// lowered to a match arm no status can reach. `XX`, `2xx` and `banana` were dropped with no
/// diagnostic at all, handing the caller a client with no arm for a response they wrote down.
///
/// The last three keys are rejected by the grammar *only*: an empty key and a `200` with leading
/// or trailing whitespace parse as neither a range nor an exact status, so before this check they
/// too vanished silently, and nothing anywhere in the tree named them.
#[test]
fn e011_every_out_of_grammar_response_key_is_rejected_behind_a_ref() {
    for key in [
        "0XX", "02XX", "0200", "+200", "6XX", "9XX", "XX", "2xx", "banana", "200.0", "1000", "20X",
        "", " 200", "200 ",
    ] {
        let (generated, checked) = generate_and_check_refd_path_item(&format!(
            "get:\n  operationId: getPet\n  responses:\n    '200': {{ description: ok }}\n    '2XX': {{ description: range }}\n    '{key}': {{ description: out of grammar }}\n"
        ));
        assert_eq!(
            generated.outcome(),
            Outcome::Rejected,
            "`{key}` was accepted: {generated:#?}"
        );
        assert!(
            has_code(&generated, Code::InvalidInput),
            "`{key}`: {generated:#?}"
        );
        assert_eq!(
            checked.outcome(),
            Outcome::Rejected,
            "`{key}`: check disagreed with generate: {checked:#?}"
        );
        assert!(
            has_code(&checked, Code::InvalidInput),
            "`{key}`: {checked:#?}"
        );
    }
}

/// The parity property for this one construct: the *same* out-of-grammar key written inline
/// reaches the same verdict under the same code as behind a `$ref`. Both placements are now
/// decided by the one vendored pattern, so they cannot drift apart;
/// `a_construct_reaches_the_same_verdict_inline_and_behind_a_ref` states the property in general.
#[test]
fn an_out_of_grammar_response_key_rejects_identically_inline_and_behind_a_ref() {
    let inline = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\npaths:\n  /pet:\n    get:\n      operationId: getPet\n      responses:\n        '200': { description: ok }\n        '0XX': { description: out of grammar }\n        default: { description: fallback }\n";
    let (refd_generated, refd_checked) = generate_and_check_refd_path_item(
        "get:\n  operationId: getPet\n  responses:\n    '200': { description: ok }\n    '0XX': { description: out of grammar }\n    default: { description: fallback }\n",
    );
    for report in [
        &generate(inline),
        &check(inline),
        &refd_generated,
        &refd_checked,
    ] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(report, Code::InvalidInput), "{report:#?}");
    }
}

/// The keys the metaschema *does* admit keep working behind a `$ref`: both response-key shapes it
/// allows, the `default` sentinel, and a specification extension, which `specification-extensions`
/// admits under `^x-` and which parsing therefore skips. Without this, validating the referenced
/// file could over-reject with every other suite still green.
///
/// The verdict is asserted exactly rather than as "not rejected", and the diagnostics are asserted
/// empty: a change that started *warning* on in-grammar keys would otherwise pass here. And each
/// accepted key is asserted to reach the emitted module, so "no diagnostic" cannot be satisfied by
/// dropping the entry — which is the very failure mode this change exists to close.
#[test]
fn in_grammar_response_keys_and_extensions_still_generate_behind_a_ref() {
    // Every response carries a body, so the emitted client has to build a status→variant table and
    // the accepted keys become observable selectors in it rather than only doc comments.
    let body = "content: { application/json: { schema: { type: string } } }";
    let (generated, checked, code) = generate_and_check_refd_path_item_with_code(&format!(
        "get:\n  operationId: getPet\n  responses:\n    '200': {{ description: ok, {body} }}\n    '2XX': {{ description: range, {body} }}\n    '404': {{ description: gone, {body} }}\n    '5XX': {{ description: server, {body} }}\n    default: {{ description: fallback, {body} }}\n    x-internal-note: {{ description: a specification extension, not a response }}\n"
    ));
    assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
    assert!(generated.diagnostics().is_empty(), "{generated:#?}");
    assert_eq!(checked.outcome(), Outcome::Clean, "{checked:#?}");
    assert!(checked.diagnostics().is_empty(), "{checked:#?}");
    for selector in [
        "StatusSpec::Exact(200u16)",
        "StatusSpec::Range(2u8)",
        "StatusSpec::Exact(404u16)",
        "StatusSpec::Range(5u8)",
    ] {
        assert!(
            code.contains(selector),
            "accepted key's selector `{selector}` never reached the emitted client:\n{code}"
        );
    }
    // The skipped extension contributes nothing to the emitted client — not even the
    // `Response `x-…`: <description>` doc line an object-valued extension produced before the
    // skip, when it was parsed as a Response Object. It is not a response, so it is not
    // documented as one.
    for fragment in [
        "x-internal-note",
        "a specification extension, not a response",
    ] {
        assert!(
            !code.contains(fragment),
            "skipped extension leaked `{fragment}` into the emitted client:\n{code}"
        );
    }
}

/// The silent-drop face on its own. `XX` parses as neither a range nor an exact status, so
/// `parse_status` returned `None` and the entry simply vanished: outcome `Generated`, no
/// diagnostic, and a client missing an arm for a response the author had documented. It reports.
#[test]
fn an_unparseable_response_key_behind_a_ref_reports_rather_than_vanishing() {
    let (generated, checked) = generate_and_check_refd_path_item(
        "get:\n  operationId: getPet\n  responses:\n    '200': { description: ok }\n    'XX': { description: silently dropped }\n",
    );
    assert_eq!(generated.outcome(), Outcome::Rejected, "{generated:#?}");
    assert!(has_code(&generated, Code::InvalidInput), "{generated:#?}");
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::InvalidInput), "{checked:#?}");
}
