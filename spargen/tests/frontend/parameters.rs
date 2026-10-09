//! Parameters: styles, `allowReserved` and `allowEmptyValue`, `querystring`, content parameters,
//! and uninhabited parameter schemas.

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
    assert!(code.contains("serialize_form"), "{code}");
    assert!(code.contains("build_url_with_query_string"), "{code}");
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

#[test]
fn oas32_cookie_style_generates_cookie_header_serialization() {
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /prefs:
    get:
      parameters:
        - name: prefs
          in: cookie
          style: cookie
          required: true
          schema:
            type: object
            properties: { theme: { type: string }, compact: { type: boolean } }
      responses:
        '200': { description: ok }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedParameterStyle),
        "{report:#?}"
    );
    assert!(code.contains("join(\"; \")"), "{code}");
}

#[test]
fn operation_parameters_override_matching_path_item_parameters() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pets:
    parameters:
      - { name: limit, in: query, schema: { type: integer } }
    get:
      parameters:
        - { name: limit, in: query, schema: { type: string } }
      responses: { '204': { description: ok } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(code.matches("pub limit:").count(), 1, "{code}");
}

#[test]
fn deep_object_query_style_generates() {
    // `style: deepObject` over an object of scalars is fully specified: `filter[key]=value`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: filter
          in: query
          style: deepObject
          explode: true
          schema:
            type: object
            additionalProperties: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedParameterStyle),
            "{report:#?}"
        );
    }
}

/// An object parameter's `required` name that no `properties` entry declares is a required field
/// (#140), typed by `additionalProperties`. With no value schema that field is unconstrained, and
/// an unconstrained JSON value has no `name[key]=value` or `key=value` serialization, so the
/// parameter rejects with `E010` — naming the property and why, rather than the generic "nested
/// arrays or objects", which describes nothing the author wrote. A value schema that is a scalar
/// gives the field a serialization, and the parameter generates.
#[test]
fn an_object_parameters_undeclared_required_name_needs_a_scalar_value_schema() {
    const TEMPLATE: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: filter
          in: query
          style: STYLE
          explode: true
          schema:
            type: object
            properties: { b: { type: string } }
            required: [a]
ADDITIONAL      responses:
        "204": { description: No Content }
"##;
    let spec = |style: &str, additional: &str| {
        TEMPLATE
            .replace("STYLE", style)
            .replace("ADDITIONAL", additional)
    };
    for style in ["deepObject", "form"] {
        for additional in ["", "            additionalProperties: true\n"] {
            let spec = spec(style, additional);
            for report in [generate(&spec), check(&spec)] {
                assert_eq!(report.outcome(), Outcome::Rejected, "{style}: {report:#?}");
                let messages = messages_for(&report, Code::UnsupportedParameterStyle);
                assert_eq!(messages.len(), 1, "{style}: {report:#?}");
                assert!(
                    messages[0].contains("`a`")
                        && messages[0].contains("no schema constrains its value")
                        && !messages[0].contains("nested arrays or objects"),
                    "{style}: {messages:?}"
                );
            }
        }
        let spec = spec(
            style,
            "            additionalProperties: { type: integer }\n",
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{style}: {report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedParameterStyle),
            "{report:#?}"
        );
        let types = types_module(&code);
        assert!(
            types
                .lines()
                .any(|line| line.trim() == "pub a: FilterAdditional,")
                && types
                    .lines()
                    .any(|line| line.trim() == "pub type FilterAdditional = i64;"),
            "{style}: the required name lost its field or its value type:\n{types}"
        );
        assert_ne!(check(&spec).outcome(), Outcome::Rejected, "{style}");
    }
    // The same object as a `oneOf`/`anyOf` member, directly or through a nested union, holds the
    // parameter's position (#435), so its unconstrained field is named the same way rather than as
    // nesting; a scalar value schema makes the union generate.
    const UNION_TEMPLATE: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: filter
          in: query
          style: STYLE
          explode: true
          schema: SCHEMA
      responses:
        "204": { description: No Content }
"##;
    let member = "{ type: object, properties: { b: { type: string } }, required: [a]ADDITIONAL }";
    let unions = [
        "{ oneOf: [MEMBER, { type: string }] }",
        "{ anyOf: [{ oneOf: [MEMBER, { type: integer }] }, { type: string }] }",
    ];
    let union_spec = |style: &str, union: &str, additional: &str| {
        UNION_TEMPLATE.replace("STYLE", style).replace(
            "SCHEMA",
            &union.replace("MEMBER", &member.replace("ADDITIONAL", additional)),
        )
    };
    for style in ["deepObject", "form"] {
        for union in unions {
            for additional in ["", ", additionalProperties: true"] {
                let spec = union_spec(style, union, additional);
                for report in [generate(&spec), check(&spec)] {
                    assert_eq!(
                        report.outcome(),
                        Outcome::Rejected,
                        "{style} {union}: {report:#?}"
                    );
                    let messages = messages_for(&report, Code::UnsupportedParameterStyle);
                    assert_eq!(messages.len(), 1, "{style} {union}: {report:#?}");
                    assert!(
                        messages[0].contains("`a`")
                            && messages[0].contains("no schema constrains its value")
                            && !messages[0].contains("nested arrays or objects"),
                        "{style} {union}: {messages:?}"
                    );
                }
            }
            let spec = union_spec(style, union, ", additionalProperties: { type: integer }");
            for report in [generate(&spec), check(&spec)] {
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{style} {union}: {report:#?}"
                );
            }
        }
    }
}

/// A `simple`/`form`/`deepObject` parameter whose schema, or a property or item schema at a
/// position the style serializes, admits no value — `false`, or an `allOf` whose members give a
/// property disjoint types — rejects with `E010` naming that schema as uninhabited (#407). Before,
/// the only message was the generic "nested arrays or objects", which describes nothing the author
/// wrote. An uninhabited schema below a genuinely nested object is still reported as the nesting,
/// since that is the shape the style cannot serialize whatever the inner schema says.
#[test]
fn a_parameter_with_an_uninhabited_schema_is_reported_as_uninhabited() {
    const TEMPLATE: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: f
          in: query
          style: STYLE
          schema: SCHEMA
      responses:
        "204": { description: No Content }
"##;
    let conflicting = "{ allOf: [{ properties: { a: { type: string } } }, \
                       { properties: { a: { type: integer } } }] }";
    let cases = [
        ("deepObject", "false", "`f`"),
        ("form", "false", "`f`"),
        ("deepObject", conflicting, "`f.a`"),
        ("form", conflicting, "`f.a`"),
        (
            "deepObject",
            "{ type: object, additionalProperties: false, properties: { a: false } }",
            "`f.a`",
        ),
        ("form", "{ type: array, items: false }", "`f[]`"),
    ];
    for (style, schema, named) in cases {
        let spec = TEMPLATE.replace("STYLE", style).replace("SCHEMA", schema);
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{style} {schema}: {report:#?}"
            );
            let messages = messages_for(&report, Code::UnsupportedParameterStyle);
            assert_eq!(messages.len(), 1, "{style} {schema}: {report:#?}");
            assert!(
                messages[0].contains(named)
                    && messages[0].contains("uninhabited")
                    && !messages[0].contains("nested arrays or objects"),
                "{style} {schema}: {messages:?}"
            );
        }
    }
    // A union with an uninhabited member still admits its other members' values, so it is not
    // uninhabited; only the member is, and the message names that member rather than the union.
    let partly = [
        ("form", "{ oneOf: [{ type: string }, false] }", "`f`"),
        (
            "deepObject",
            "{ type: object, properties: { a: { anyOf: [{ type: integer }, false] } } }",
            "`f.a`",
        ),
    ];
    for (style, schema, named) in partly {
        let spec = TEMPLATE.replace("STYLE", style).replace("SCHEMA", schema);
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{schema}: {report:#?}");
            let messages = messages_for(&report, Code::UnsupportedParameterStyle);
            assert_eq!(messages.len(), 1, "{schema}: {report:#?}");
            assert!(
                messages[0].contains(&format!(
                    "{named} has a `oneOf`/`anyOf` member that is uninhabited"
                )) && !messages[0].contains(&format!("{named} is uninhabited"))
                    && !messages[0].contains("nested arrays or objects"),
                "{schema}: {messages:?}"
            );
        }
    }
    // One `E010` per parameter, for the first cause in a fixed order: an uninhabited part, then
    // an unconstrained property, then nesting. So an uninhabited property is reported while a
    // nested sibling is not, and the sibling surfaces once the first is fixed.
    let both = TEMPLATE.replace("STYLE", "deepObject").replace(
        "SCHEMA",
        "{ type: object, properties: { a: false, b: { type: object } } }",
    );
    for report in [generate(&both), check(&both)] {
        let messages = messages_for(&report, Code::UnsupportedParameterStyle);
        assert_eq!(messages.len(), 1, "{report:#?}");
        assert!(
            messages[0].contains("`f.a` is uninhabited")
                && !messages[0].contains("nested arrays or objects"),
            "{messages:?}"
        );
    }
    let nested = TEMPLATE.replace("STYLE", "deepObject").replace(
        "SCHEMA",
        "{ type: object, properties: { a: { type: object, properties: { b: false } } } }",
    );
    for report in [generate(&nested), check(&nested)] {
        let messages = messages_for(&report, Code::UnsupportedParameterStyle);
        assert_eq!(messages.len(), 1, "{report:#?}");
        assert!(
            messages[0].contains("nested arrays or objects")
                && !messages[0].contains("uninhabited"),
            "{messages:?}"
        );
    }
}

/// A `oneOf`/`anyOf` parameter schema serializes as whichever member the value is, so an
/// uninhabited property, item, or tuple position of a struct, array, or tuple member is a part of
/// the parameter and is named as one (#435). Before, the part walk stopped at the union, and the
/// only message was "nested arrays or objects", though nothing is nested: the same union with an
/// inhabited part generates.
#[test]
fn a_parameter_union_member_with_an_uninhabited_part_is_reported_as_uninhabited() {
    const TEMPLATE: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: f
          in: query
          style: STYLE
          schema: SCHEMA
      responses:
        "204": { description: No Content }
"##;
    // (style, schema, the same schema with the part inhabited, what the message names)
    let cases = [
        (
            "deepObject",
            "{ oneOf: [{ type: object, properties: { a: false } }, { type: string }] }",
            "{ oneOf: [{ type: object, properties: { a: { type: integer } } }, { type: string }] }",
            "`f.a` is uninhabited",
        ),
        (
            "form",
            "{ oneOf: [{ type: object, properties: { a: false } }, { type: string }] }",
            "{ oneOf: [{ type: object, properties: { a: { type: integer } } }, { type: string }] }",
            "`f.a` is uninhabited",
        ),
        (
            "form",
            "{ anyOf: [{ type: object, additionalProperties: false, properties: { a: false } }, \
             { type: integer }] }",
            "{ anyOf: [{ type: object, additionalProperties: false, \
             properties: { a: { type: integer } } }, { type: integer }] }",
            "`f.a` is uninhabited",
        ),
        (
            "form",
            "{ oneOf: [{ type: array, items: false }, { type: boolean }] }",
            "{ oneOf: [{ type: array, items: { type: string } }, { type: boolean }] }",
            "`f[]` is uninhabited",
        ),
        (
            "form",
            "{ oneOf: [{ type: array, prefixItems: [{ type: string }, false], items: false }, \
             { type: boolean }] }",
            "{ oneOf: [{ type: array, prefixItems: [{ type: string }, { type: integer }], \
             items: false }, { type: boolean }] }",
            "`f[1]` is uninhabited",
        ),
        (
            "deepObject",
            "{ anyOf: [{ type: object, properties: { a: { oneOf: [{ type: integer }, false] } } }, \
             { type: string }] }",
            "{ anyOf: [{ type: object, properties: { a: { type: integer } } }, { type: string }] }",
            "`f.a` has a `oneOf`/`anyOf` member that is uninhabited",
        ),
        // A union nested directly inside a union member holds the same position, so its members'
        // parts are searched with the same paths.
        (
            "deepObject",
            "{ oneOf: [{ oneOf: [{ type: object, properties: { a: false } }, { type: integer }] }, \
             { type: string }] }",
            "{ oneOf: [{ oneOf: [{ type: object, properties: { a: { type: integer } } }, \
             { type: integer }] }, { type: string }] }",
            "`f.a` is uninhabited",
        ),
    ];
    for (style, schema, control, named) in cases {
        let spec = TEMPLATE.replace("STYLE", style).replace("SCHEMA", schema);
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{style} {schema}: {report:#?}"
            );
            let messages = messages_for(&report, Code::UnsupportedParameterStyle);
            assert_eq!(messages.len(), 1, "{style} {schema}: {report:#?}");
            assert!(
                messages[0].contains(named) && !messages[0].contains("nested arrays or objects"),
                "{style} {schema}: {messages:?}"
            );
        }
        // The control: the same union with the part inhabited is a supported shape.
        let spec = TEMPLATE.replace("STYLE", style).replace("SCHEMA", control);
        for report in [generate(&spec), check(&spec)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{style} {control}: {report:#?}"
            );
        }
    }
}

/// Wrapping a parameter's schema in a union does not change the cause its diagnostic names
/// (#435): for `S = {type: object, properties: {a: false}}` under `form` and `deepObject`, `S`
/// itself and `oneOf: [S, {type: string}]` both report the one `E010` message naming `f.a` in the
/// uninhabited wording, and neither the nesting one. `S` is held both inline and as a component.
#[test]
fn wrapping_a_parameter_schema_in_a_union_names_the_same_uninhabited_part() {
    const TEMPLATE: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: f
          in: query
          style: STYLE
          schema: SCHEMA
      responses:
        "204": { description: No Content }
components:
  schemas:
    S: { type: object, properties: { a: false } }
"##;
    const S: &str = "{ type: object, properties: { a: false } }";
    const S_REF: &str = "{ $ref: '#/components/schemas/S' }";
    for style in ["form", "deepObject"] {
        for s in [S, S_REF] {
            let wrapped = format!("{{ oneOf: [{s}, {{ type: string }}] }}");
            let mut named = Vec::new();
            for schema in [s, wrapped.as_str()] {
                let spec = TEMPLATE.replace("STYLE", style).replace("SCHEMA", schema);
                for report in [generate(&spec), check(&spec)] {
                    assert_eq!(
                        report.outcome(),
                        Outcome::Rejected,
                        "{style} {schema}: {report:#?}"
                    );
                    let messages = messages_for(&report, Code::UnsupportedParameterStyle);
                    assert_eq!(messages.len(), 1, "{style} {schema}: {report:#?}");
                    assert!(
                        messages[0].contains("`f.a` is uninhabited")
                            && !messages[0].contains("nested arrays or objects"),
                        "{style} {schema}: {messages:?}"
                    );
                    named.push(messages[0].to_owned());
                }
            }
            // The bare and the wrapped schema, each through both entry points, give one message:
            // the union adds nothing to what is named.
            assert_eq!(named.len(), 4, "{style} {s}: {named:?}");
            assert!(
                named.iter().all(|message| *message == named[0]),
                "{style} {s}: {named:#?}"
            );
        }
    }
}

#[test]
fn matrix_and_label_path_styles_generate() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /map/{position}/{ext}:
    get:
      parameters:
        - name: position
          in: path
          required: true
          style: matrix
          schema:
            type: array
            items: { type: integer }
        - name: ext
          in: path
          required: true
          style: label
          schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedParameterStyle),
            "{report:#?}"
        );
    }
}

#[test]
fn delimited_query_styles_generate() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: spaced
          in: query
          style: spaceDelimited
          explode: false
          schema:
            type: array
            items: { type: string }
        - name: piped
          in: query
          style: pipeDelimited
          explode: false
          schema:
            type: array
            items: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    }
}

#[test]
fn e010_delimited_style_with_explode_true() {
    // The specification's own serialization table marks this combination n/a, so there is no
    // correct wire form to emit.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: spaced
          in: query
          style: spaceDelimited
          explode: true
          schema:
            type: array
            items: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedParameterStyle));
    }
}

#[test]
fn e011_parameter_style_illegal_for_location() {
    // The official document schema enumerates the legal styles per location, so an illegal
    // pairing is caught structurally before lowering ever sees it.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: filter
          in: query
          style: label
          schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
    }
}

#[test]
fn allow_reserved_query_parameter_generates() {
    // `allowReserved: true` selects RFC 6570 reserved expansion — a different encoding set, not an
    // unrepresentable construct.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: expression
          in: query
          allowReserved: true
          schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedParameterStyle),
            "{report:#?}"
        );
    }
}

#[test]
fn w011_allow_reserved_has_no_effect_where_nothing_is_encoded() {
    // OpenAPI 3.1 scopes `allowReserved` to `in: query`, so its metaschema rejects it elsewhere
    // structurally. 3.2 broadens it to "wherever the location percent-encodes" — which makes it
    // declarable, but still inert, on a header and on `style: cookie`.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: X-Expression
          in: header
          allowReserved: true
          schema: { type: string }
        - name: session
          in: cookie
          style: cookie
          allowReserved: true
          schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn e011_allow_reserved_on_a_3_1_header_is_structurally_invalid() {
    // Pins the version difference above: 3.1 permits `allowReserved` only on a query parameter.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: X-Expression
          in: header
          allowReserved: true
          schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
    }
}

#[test]
fn e010_nested_parameter_value() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: matrix
          in: query
          schema:
            type: array
            items:
              type: array
              items: { type: integer }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedParameterStyle));

    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::UnsupportedParameterStyle));
}

#[test]
fn w011_reserved_header_parameters_are_ignored() {
    // `Accept`, `Content-Type`, and `Authorization` belong to the protocol layer.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: Accept
          in: header
          schema: { type: string }
        - name: authorization
          in: header
          schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(
            report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::DeclarationHasNoEffect)
                .count(),
            2,
            "one per reserved header, matched case-insensitively: {report:#?}"
        );
    }
}

#[test]
fn e009_content_parameter_with_an_unrenderable_media_type() {
    // An XML `content` parameter used to fall through to `simple` serialization and be sent in the
    // wrong format entirely.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: filter
          in: query
          content:
            application/xml:
              schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
    }
}

#[test]
fn w011_allow_empty_value_has_no_effect() {
    // Deprecated in 3.2 and inert for a typed client: an omitted optional parameter is not sent.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: flag
          in: query
          allowEmptyValue: true
          schema: { type: string }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn a_binary_family_parameter_content_is_rejected_by_its_position() {
    // Parameter `content` shares the body classifier, so `image/png` there classifies as octets
    // and meets each position's own rule instead of the generic "not supported": a querystring
    // takes only JSON or form content (`E010`, as `video/*` already gets there), and any other
    // `content` parameter needs a single-token codec (`E009`).
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
            image/png: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedParameterStyle
                    && d.message
                        .contains("querystring media type `image/png` is not supported")
            }),
            "{report:#?}"
        );
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
    }

    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /search:
    get:
      operationId: search
      parameters:
        - name: thumb
          in: query
          content:
            image/png: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::UnsupportedMediaType
                    && d.message.contains(
                        "`content` parameter media type `image/png` has no single-token \
                         serialization",
                    )
            }),
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_malformed_parameter_content_key_is_unsupported() {
    // Both parameter call sites classify their `content` key: a `content` parameter and a 3.2
    // `in: querystring` parameter. Neither may render a value through a key that is not a type.
    let content_parameter = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      parameters:
        - name: filter
          in: query
          content:
            "text/plain/extra": { schema: { type: string } }
      responses:
        "204": { description: No Content }
"##;
    let querystring_parameter = r##"
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
            "text/plain/extra": { schema: { type: object } }
      responses:
        "204": { description: No Content }
"##;
    for spec in [content_parameter, querystring_parameter] {
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
}
