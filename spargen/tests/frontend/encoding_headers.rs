//! The `headers` of an Encoding Object: the extra part headers a `multipart` property sends, the
//! name and literal each must be, and where a declared header has no effect.

use super::*;

/// `encoding.headers` accepts a Header Object or a `$ref` to one. The reference was never
/// resolved, so a target that pins a `const` was reported as pinning no value (`W011`) and the
/// part shipped without the header. Resolving it makes the two spellings equivalent.
#[test]
fn encoding_header_ref_is_resolved_and_pins_its_const() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                profile: { type: string }
            encoding:
              profile:
                contentType: text/plain
                headers:
                  X-Part-Kind: { $ref: '#/components/headers/PartKind' }
      responses:
        '204': { description: ok }
components:
  headers:
    PartKind:
      description: which part this is
      schema: { type: string, const: profile }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::DeclarationHasNoEffect),
        "a resolved header that pins a const has an effect: {report:#?}"
    );
    assert!(code.contains("X-Part-Kind"), "{code}");
    assert!(code.contains("profile"), "{code}");
}

/// The `headers` of an Encoding Object apply "either with or without the RFC6570-style
/// serialization fields" (OpenAPI 3.2 § Encoding Object). A property in RFC 6570 mode built its
/// parts without them, so a pinned header was dropped with no diagnostic (#812). It is sent, and
/// nothing reports it as having no effect.
#[test]
fn an_encoding_header_is_sent_on_an_rfc6570_serialized_part() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                tags: { type: array, items: { type: string } }
            encoding:
              tags:
                style: form
                explode: true
                headers:
                  X-Part-Kind:
                    schema: { type: string, const: tag }
      responses:
        '204': { description: ok }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::DeclarationHasNoEffect),
        "a pinned header on a style-serialized part has an effect: {report:#?}"
    );
    assert!(code.contains("serialize_multipart_values"), "{code}");
    assert!(code.contains("X-Part-Kind"), "{code}");
    assert!(code.contains(r#"HeaderValue::try_from("tag")"#), "{code}");
}

/// A multipart encoding header whose `schema` is a `$ref`: one schema component holding the
/// literal to send, written with `encoding.headers.X.schema`.
fn encoding_header_schema_document(schema: &str, components: &str) -> String {
    format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                file: {{ type: string }}
            encoding:
              file:
                headers:
                  X-Version:
                    schema: {schema}
      responses:
        '204': {{ description: ok }}
components:
  schemas:
{components}
"##
    )
}

/// A header `schema` that is a `$ref` was never resolved, so a target pinning a `const` or a
/// `default` was reported as pinning no value (`W011`) and the part shipped without the header
/// (#701). The reference is read through to the schema it names: directly, through a component
/// alias chain, and from a `$ref` with a sibling keyword, which stays a schema with a `$ref` in it.
#[test]
fn encoding_header_schema_ref_is_resolved_and_pins_its_literal() {
    let cases = [
        // A `const` on the target.
        (
            "{ $ref: '#/components/schemas/V' }",
            "    V: { const: \"1\" }",
            "1",
        ),
        // A `default` on the target.
        (
            "{ $ref: '#/components/schemas/V' }",
            "    V: { type: string, default: \"2\" }",
            "2",
        ),
        // An alias chain: `V` names `W`, which names `X`, which pins the value.
        (
            "{ $ref: '#/components/schemas/V' }",
            "    V: { $ref: '#/components/schemas/W' }\n    W: { $ref: \
             '#/components/schemas/X' }\n    X: { const: \"3\" }",
            "3",
        ),
        // A shape-bearing sibling keeps the `$ref` inside a schema; its target still pins.
        (
            "{ $ref: '#/components/schemas/V', maxLength: 8 }",
            "    V: { type: string, const: \"4\" }",
            "4",
        ),
        // A target that is itself a `$ref` beside a sibling keyword is followed to what its own
        // reference names.
        (
            "{ $ref: '#/components/schemas/V' }",
            "    V: { $ref: '#/components/schemas/W', type: string }\n    W: { const: \"7\" }",
            "7",
        ),
        // A literal beside the `$ref` is the one sent, as it is without the `$ref`.
        (
            "{ $ref: '#/components/schemas/V', const: \"5\" }",
            "    V: { type: string, default: \"0\" }",
            "5",
        ),
    ];
    for (schema, components, sent) in cases {
        let spec = encoding_header_schema_document(schema, components);
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{schema}: {report:#?}");
        assert!(
            !has_code(&report, Code::DeclarationHasNoEffect),
            "{schema}: a resolved schema that pins a literal has an effect: {report:#?}"
        );
        let value = format!("HeaderValue::try_from({sent:?})");
        assert!(code.contains("X-Version"), "{schema}: {code}");
        assert!(code.contains(&value), "{schema} sends {value}: {code}");
    }
}

/// Resolving the header `schema` changes the reason only where the target pins something: a
/// target that pins nothing is still the one `W011`, and the header is not sent.
#[test]
fn w011_an_encoding_header_schema_ref_whose_target_pins_nothing_has_no_effect() {
    let spec = encoding_header_schema_document(
        "{ $ref: '#/components/schemas/V' }",
        "    V: { type: string }",
    );
    let (report, code) = generate_with_code(&spec);
    assert!(!code.contains("X-Version"), "{code}");
    for report in [report, check(&spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(
            messages_for(&report, Code::DeclarationHasNoEffect),
            [
                "`encoding.file.headers.X-Version` pins no value, so there is nothing for the \
                 client to send"
            ],
            "{report:#?}"
        );
    }
}

/// A reference in an encoding header's `schema` that resolves to nothing was reported only as
/// pinning no value (`W011`), and the document generated around a dangling `$ref`. It is
/// `E004`, as every other unresolvable reference is, and only `E004`: the broken link is reported
/// once, whether it is the written reference or a component alias the chain passes through.
#[test]
fn e004_an_unresolvable_encoding_header_schema_ref_does_not_also_warn() {
    for (schema, components) in [
        (
            "{ $ref: '#/components/schemas/Missing' }",
            "    V: { type: string }",
        ),
        (
            "{ $ref: '#/components/schemas/V' }",
            "    V: { $ref: '#/components/schemas/Missing' }",
        ),
    ] {
        let spec = encoding_header_schema_document(schema, components);
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{schema}: {report:#?}");
            let unresolved = messages_for(&report, Code::UnresolvedRef);
            assert_eq!(unresolved.len(), 1, "{schema}: {report:#?}");
            assert!(
                unresolved[0].contains("`#/components/schemas/Missing`"),
                "{schema}: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::DeclarationHasNoEffect),
                "{schema}: {report:#?}"
            );
        }
    }
}

/// A header schema read through a cycle of `$ref`s that each carry a sibling keyword never
/// reaches a target. That is the cycle's defect, `E013`, reported where the components are
/// lowered, and not also "pins no value".
#[test]
fn e013_an_encoding_header_schema_ref_cycle_does_not_also_warn() {
    let spec = encoding_header_schema_document(
        "{ $ref: '#/components/schemas/V' }",
        "    V: { $ref: '#/components/schemas/W', type: string }\n    W: { $ref: \
         '#/components/schemas/V', type: string }",
    );
    let report = generate(&spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
    assert!(
        !has_code(&report, Code::DeclarationHasNoEffect),
        "{report:#?}"
    );
}

/// An `encoding.headers` reference that resolves to nothing is `E004` — and only `E004`, so one
/// defect does not also collect a "pins no value" warning naming a second, wrong reason.
#[test]
fn e004_unresolvable_encoding_header_ref_does_not_also_warn() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                profile: { type: string }
            encoding:
              profile:
                contentType: text/plain
                headers:
                  X-Part-Kind: { $ref: '#/components/headers/Missing' }
      responses:
        '204': { description: ok }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
    assert!(
        !has_code(&report, Code::DeclarationHasNoEffect),
        "{report:#?}"
    );
}

/// An `encoding.headers` entry that pins a literal is sent as a part header, so its name must be
/// an RFC 9110 § 5.6.2 token and its literal an RFC 9110 § 5.5 field value. The generated client
/// once skipped a header whose name or value its HTTP stack refused, on every request, with
/// nothing reported at generation or at call time. Each case below is now `E009`.
#[test]
fn e009_an_encoding_header_that_is_not_an_http_field_is_unsupported() {
    let cases = [
        // A space is no `tchar`.
        ("Bad Name", r#""1""#),
        // `:` would end the field name on the wire.
        ("X-A:B", r#""1""#),
        // A control character is no `field-vchar`.
        ("X-Part", r#""a\u0001b""#),
        // A line break would end the field.
        ("X-Part", r#""a\nb""#),
        // DEL is no `field-vchar` either.
        ("X-Part", r#""a\u007fb""#),
        // A field value neither begins nor ends with whitespace.
        ("X-Part", r#"" padded""#),
        ("X-Part", r#""padded\t""#),
    ];
    for (name, literal) in cases {
        let spec = format!(
            r##"
openapi: 3.2.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                file: {{ type: string }}
            encoding:
              file:
                headers:
                  "{name}":
                    schema: {{ type: string, const: {literal} }}
      responses:
        '204': {{ description: ok }}
"##
        );
        let report = generate(&spec);
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "{name}: {literal}: {report:#?}"
        );
        assert!(
            has_code(&report, Code::UnsupportedMediaType),
            "{name}: {literal}: {report:#?}"
        );
    }
}

/// The field-name rule admits every `tchar` (RFC 9110 § 5.6.2), and the field-value rule what
/// RFC 9110 § 5.5 does: interior spaces and tabs, every visible ASCII character (0x21 through
/// 0x7E, the non-`tchar` delimiters among them), and `obs-text` (bytes above 0x7F, here UTF-8).
/// Each generates.
#[test]
fn an_encoding_header_that_is_an_http_field_generates() {
    // Every `tchar`: the fifteen punctuation characters, then digits and letters.
    let name = "!#$%&'*+-.^_`|~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    // Every visible ASCII character, as a YAML double-quoted scalar.
    let visible: String = (0x21u8..=0x7e)
        .map(char::from)
        .map(|c| match c {
            '"' | '\\' => format!("\\{c}"),
            c => c.to_string(),
        })
        .collect();
    for literal in [
        r#""a b\tc""#.to_owned(),
        format!("\"{visible}\""),
        r#""café""#.to_owned(),
        "7".to_owned(),
        "true".to_owned(),
    ] {
        let spec = format!(
            r##"
openapi: 3.2.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                file: {{ type: string }}
            encoding:
              file:
                headers:
                  "{name}":
                    schema: {{ const: {literal} }}
      responses:
        '204': {{ description: ok }}
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{literal}: {report:#?}"
        );
        assert!(code.contains(name), "{literal}: {code}");
    }
}

/// Every offending header of one property is reported, not only the first, so one generation
/// names every header the author must fix before the body is refused.
#[test]
fn e009_every_encoding_header_that_is_not_an_http_field_is_reported() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                file: { type: string }
            encoding:
              file:
                headers:
                  "Bad Name":
                    schema: { type: string, const: "1" }
                  X-Part:
                    schema: { type: string, const: "a\nb" }
      responses:
        '204': { description: ok }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_with_code(&report, Code::UnsupportedMediaType);
        assert_eq!(messages.len(), 2, "{report:#?}");
        assert!(
            messages.iter().any(|m| m.contains("Bad Name")),
            "{messages:#?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("headers.X-Part")),
            "{messages:#?}"
        );
    }
}

/// A form-urlencoded field is one name/value pair with no headers of its own, so `headers` there
/// is acknowledged and dropped. The header pins a `const`, so nothing but this rule reports it.
#[test]
fn w011_encoding_headers_on_a_form_urlencoded_body_have_no_effect() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /form:
    post:
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              properties:
                profile: { type: string }
            encoding:
              profile:
                headers:
                  X-Part-Kind: { schema: { type: string, const: profile } }
      responses:
        '204': { description: ok }
"##;
    let (report, code) = generate_with_code(spec);
    assert!(!code.contains("X-Part-Kind"), "{code}");
    for report in [report, check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(
            messages_for(&report, Code::DeclarationHasNoEffect),
            ["`encoding.profile.headers` applies only to `multipart` content"],
            "{report:#?}"
        );
    }
}

/// A Header Object describes a header and carries no value; one whose schema pins none gives the
/// client nothing to send, so it is acknowledged rather than sent empty or dropped in silence.
#[test]
fn w011_an_encoding_header_that_pins_no_value_has_no_effect() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                profile: { type: string }
            encoding:
              profile:
                headers:
                  X-Part-Kind: { schema: { type: string } }
      responses:
        '204': { description: ok }
"##;
    let (report, code) = generate_with_code(spec);
    assert!(!code.contains("X-Part-Kind"), "{code}");
    for report in [report, check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(
            messages_for(&report, Code::DeclarationHasNoEffect),
            [
                "`encoding.profile.headers.X-Part-Kind` pins no value, so there is nothing for \
                 the client to send"
            ],
            "{report:#?}"
        );
    }
}
