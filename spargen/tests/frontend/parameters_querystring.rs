//! `in: querystring` parameters: the typed argument, form-urlencoded Encoding Objects, the
//! `querystring`/`query` exclusion and the shape of its one content entry.

use super::*;

#[test]
fn oas32_querystring_param_generates_a_typed_argument() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /search:
    get:
      operationId: search
      parameters:
        - name: q
          in: querystring
          content:
            application/x-www-form-urlencoded:
              schema:
                type: object
      responses:
        '200': { description: ok }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::Oas32ConstructIgnored),
        "{report:#?}"
    );
    assert!(!has_code(&report, Code::InvalidInput), "{report:#?}");
    assert!(code.contains("pub q: Option<types::Q>"), "{code}");
    // The embedded runtime's `pub use` list names both helpers in every generated module, so a
    // bare name proves nothing: assert the calls the operation body makes, whitespace-flattened.
    let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
    // A form-urlencoded whole query string is rendered by the form body's serializer, property by
    // property through its resolved Encoding Objects.
    assert!(
        flat.contains("support::serialize_form_body( form_value, QUERYSTRING_PROPERTIES, None, )"),
        "{code}"
    );
    assert!(
        flat.contains(
            "let url = support::build_url_with_query_string_on( &self.core, None, &path, \
             &query, raw_query.as_deref(), )"
        ),
        "{code}"
    );
}

/// A form-urlencoded `in: querystring` parameter, whose one property `field` has `schema` and the
/// Encoding Object `encoding` (`{}` for none).
fn form_querystring_spec(schema: &str, encoding: &str) -> String {
    format!(
        r##"
openapi: 3.2.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /a:
    get:
      operationId: getA
      parameters:
        - name: q
          in: querystring
          content:
            application/x-www-form-urlencoded:
              schema:
                type: object
                properties:
                  field: {schema}
              encoding:
                field: {encoding}
      responses:
        '204': {{ description: ok }}
"##
    )
}

/// The specification says an `in: querystring` parameter's form-urlencoded content uses Encoding
/// Objects "in the same way as with request bodies of that media type", so each property is
/// lowered by the body's rules: an RFC 6570 field selects style mode, a `contentType` media mode,
/// and a property with no Encoding Object takes the default for its type. Every Encoding Object
/// used to be dropped unreported, and every property sent as `style: form, explode: true`.
#[test]
fn a_form_querystring_parameter_applies_its_encoding_objects() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /a:
    get:
      operationId: getA
      parameters:
        - name: q
          in: querystring
          content:
            application/x-www-form-urlencoded:
              schema:
                type: object
                properties:
                  tags: { type: array, items: { type: string } }
                  at: { type: object, properties: { x: { type: string } } }
                  ids: { type: array, items: { type: integer } }
                  term: { type: string }
              encoding:
                tags: { style: form, explode: false }
                at: { contentType: application/json }
      responses:
        '204': { description: ok }
"##;
    let (report, code) = generate_with_code(spec);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
    let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
    for property in [
        "support::FormProperty { name: \"tags\", mode: support::FormMode::Style { style: \
         support::FormStyle::Form, explode: false, encoding: support::PercentEncoding::Form, }, }",
        "support::FormProperty { name: \"at\", mode: support::FormMode::Json, }",
        // No Encoding Object: an array defaults to `application/json`, a string to `text/plain`.
        "support::FormProperty { name: \"ids\", mode: support::FormMode::Json, }",
        "support::FormProperty { name: \"term\", mode: support::FormMode::Text, }",
    ] {
        assert!(flat.contains(property), "{property}: {flat}");
    }
}

/// An Encoding Object field that has no effect on a form-urlencoded request body has none on a
/// form-urlencoded querystring either, and is reported the same way.
#[test]
fn w011_an_inert_encoding_field_on_a_form_querystring_is_reported() {
    let spec = form_querystring_spec(
        "{ type: string }",
        "{ headers: { X-A: { schema: { const: a } } } }",
    );
    let report = generate(&spec);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert_eq!(
        messages_for(&report, Code::DeclarationHasNoEffect),
        ["`encoding.field.headers` applies only to `multipart` content"],
        "{report:#?}"
    );
}

// E009 case: non-json-structured
#[test]
fn e009_a_form_querystring_object_property_declaring_a_non_json_content_type() {
    // As in a form body, a property that is not a scalar is serialized into its field only as
    // JSON, so a `contentType` naming another syntax cannot be honoured. It used to be ignored,
    // and the nested object refused only at request time.
    let spec = form_querystring_spec(
        "{ type: object, properties: { x: { type: string } } }",
        "{ contentType: text/plain }",
    );
    let report = generate(&spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        messages_for(&report, Code::UnsupportedMediaType),
        [
            "property `field` declares `contentType: text/plain`, but it is not a scalar value, \
             so spargen can serialize it into a form field only as JSON; the field would not be \
             in the syntax the document declares"
        ],
        "{report:#?}"
    );
}

// E009 case: form-binary
#[test]
fn e009_a_binary_form_querystring_property() {
    // A form field is a single URL-encoded string, in a query string as in a body, so raw bytes
    // have no representation there.
    let spec = form_querystring_spec("{ type: string, contentEncoding: base64 }", "{}");
    let report = generate(&spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        messages_for(&report, Code::UnsupportedMediaType),
        [
            "property `field` is binary, which has no `application/x-www-form-urlencoded` \
             representation"
        ],
        "{report:#?}"
    );
}

// E009 case: undefined-rfc6570
#[test]
fn e009_a_form_querystring_property_under_an_undefined_rfc6570_combination() {
    let spec = form_querystring_spec(
        "{ type: array, items: { type: string } }",
        "{ style: pipeDelimited, explode: true }",
    );
    let report = generate(&spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        messages_for(&report, Code::UnsupportedMediaType),
        [
            "`encoding.field.style: pipeDelimited` with `explode: true` is undefined: the \
             specification's serialization table gives no value for that combination"
        ],
        "{report:#?}"
    );
}

#[test]
fn oas32_querystring_and_named_query_are_rejected_together() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /search:
    get:
      parameters:
        - name: whole
          in: querystring
          content:
            application/json:
              schema: { type: object }
        - name: page
          in: query
          schema: { type: integer }
      responses:
        '200': { description: ok }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
}

/// The 3.2 metaschema refuses `querystring` beside `query` within one `parameters` list, so the
/// spec above never reaches lowering's own rule. Split across the Path Item and the Operation, the
/// metaschema admits the pair, and only the merged parameter set the operation lowers has both.
#[test]
fn oas32_querystring_and_named_query_are_rejected_across_path_item_and_operation() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /search:
    parameters:
      - name: whole
        in: querystring
        content:
          application/json:
            schema: { type: object }
    get:
      parameters:
        - name: page
          in: query
          schema: { type: integer }
      responses:
        '200': { description: ok }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        messages_for(&report, Code::InvalidInput),
        [
            "an operation may declare at most one `in: querystring` parameter and may not \
             combine it with `in: query` parameters"
        ],
        "{report:#?}"
    );
}

#[test]
fn e011_querystring_parameter_without_one_content_entry() {
    // The official 3.2 document schema requires `content` on an `in: querystring` parameter and
    // holds `content` to exactly one entry, so neither shape reaches lowering's own `E010` arm.
    let without_content = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      parameters:
        - name: q
          in: querystring
          schema: { type: object }
      responses:
        "204": { description: No Content }
"##;
    let two_entries = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      parameters:
        - name: q
          in: querystring
          content:
            application/json: { schema: { type: object } }
            application/x-www-form-urlencoded: { schema: { type: object } }
      responses:
        "204": { description: No Content }
"##;
    for spec in [without_content, two_entries] {
        let report = generate(spec);
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedParameterStyle),
            "{report:#?}"
        );
    }
}

/// A `querystring` parameter is one typed argument, so its content entry needs a schema to type
/// it, and a form-urlencoded entry an object schema whose properties become the query's pairs.
/// Each shortfall is `E010` in its own words.
// E010 case: querystring-content
#[test]
fn e010_a_querystring_content_entry_with_no_usable_schema_is_unsupported() {
    for (content, expected) in [
        (
            "application/json: {}",
            "querystring content requires a schema for a typed client argument",
        ),
        (
            "application/x-www-form-urlencoded: { schema: { type: string } }",
            "form-urlencoded querystring parameters require an object schema",
        ),
    ] {
        let report = generate(&format!(
            "openapi: 3.2.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths:\n  /search:\n    \
             get:\n      operationId: search\n      parameters:\n        - name: q\n          \
             in: querystring\n          content: {{ {content} }}\n      responses:\n        \
             '204': {{ description: ok }}\n"
        ));
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "{content}: {report:#?}"
        );
        assert_eq!(
            messages_for(&report, Code::UnsupportedParameterStyle),
            [expected],
            "{content}: {report:#?}"
        );
    }
}
