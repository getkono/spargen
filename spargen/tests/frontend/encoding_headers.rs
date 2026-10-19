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
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
        assert!(
            !has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
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
        for report in [generate(&spec), check(&spec)] {
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
