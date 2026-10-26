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
    // The embedded runtime's `pub use` list names both helpers in every generated module, so a
    // bare name proves nothing: assert the calls the operation body makes, whitespace-flattened.
    let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains(
            "query .extend( support::serialize_form( \"\", value, true, \
             support::PercentEncoding::Form, )"
        ),
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
    for report in [generate(spec), check(spec)] {
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
    // The one field is the operation's `string`, not the path item's `integer`: a count alone holds
    // whichever of the two wins.
    assert_eq!(
        field_type(&code, "pub limit:").as_deref(),
        Some("Option<types::Limit>"),
        "{code}"
    );
    assert_eq!(
        alias_target(&code, "Limit").as_deref(),
        Some("String"),
        "{code}"
    );
}

/// Each `parameters` list may name a `name`/`in` pair once. The two lists are checked apart,
/// because a pair the operation repeats from its path item is an override, not a duplicate.
#[test]
fn e011_a_parameter_list_naming_one_pair_twice() {
    for (level, message, path_item, operation) in [
        (
            "path item",
            "path-item parameters contain a duplicate name/location pair",
            "    parameters:\n      - { name: limit, in: query, schema: { type: integer } }\n      \
             - { name: limit, in: query, schema: { type: string } }\n",
            "",
        ),
        (
            "operation",
            "operation parameters contain a duplicate name/location pair",
            "",
            "      parameters:\n        - { name: limit, in: query, schema: { type: integer } }\n        \
             - { name: limit, in: query, schema: { type: string } }\n",
        ),
    ] {
        let spec = format!(
            "openapi: 3.1.0\n\
             info: {{ title: T, version: 1.0.0 }}\n\
             paths:\n  \
             /pets:\n{path_item}    \
             get:\n{operation}      \
             responses: {{ '204': {{ description: ok }} }}\n"
        );
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{level}: {report:#?}");
            assert_eq!(
                messages_for(&report, Code::InvalidInput),
                [message],
                "{level}: {report:#?}"
            );
        }
    }
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
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedParameterStyle),
        "{report:#?}"
    );
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
    }
    // The same object as a `oneOf`/`anyOf` member, directly or through a nested union, holds the
    // parameter's position (#435), so its unconstrained field is named the same way rather than as
    // nesting; a scalar value schema clears that fault (the union then generates under `form`,
    // and `deepObject` reports only its scalar member, as `assert_union_control` says).
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
            assert_union_control(style, &spec);
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
/// inhabited part generates under `form` (under `deepObject` only its scalar member is left to
/// report, as `assert_union_control` says).
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
        // The control: the same union with the part inhabited loses this fault.
        let spec = TEMPLATE.replace("STYLE", style).replace("SCHEMA", control);
        assert_union_control(style, &spec);
    }
}

/// The control of a fixture whose parameter schema is a union of an object and a scalar: with
/// the fault the fixture is about removed, `form` generates, and `deepObject` is left with only
/// the scalar member it cannot serialize (#696), reported as that and nothing else.
fn assert_union_control(style: &str, spec: &str) {
    for report in [generate(spec), check(spec)] {
        if style == "deepObject" {
            assert_eq!(report.outcome(), Outcome::Rejected, "{spec}: {report:#?}");
            let messages = messages_for(&report, Code::UnsupportedParameterStyle);
            assert_eq!(messages.len(), 1, "{spec}: {report:#?}");
            assert!(
                messages[0].contains("`style: deepObject` serializes only objects"),
                "{spec}: {messages:?}"
            );
        } else {
            assert_ne!(report.outcome(), Outcome::Rejected, "{spec}: {report:#?}");
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
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedParameterStyle),
        "{report:#?}"
    );
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
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
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
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedParameterStyle));
}

/// `spaceDelimited`/`pipeDelimited` are defined only for arrays and objects, and `deepObject`
/// only for objects: the specification's serialization table gives no value for a primitive or
/// an undefined (`null`) value under them (#696). A schema admitting such a value used to
/// generate a client whose call failed at request time with the runtime's `ExpectedComposite`
/// or `ExpectedObject` whenever it was sent — on every call for a scalar schema. So a schema
/// that admits any value the style cannot serialize rejects with `E010`: a scalar, a
/// `oneOf`/`anyOf` with a scalar member, `null` on a required parameter, or a `oneOf` member
/// that is `null` (here through a `$ref`) on any parameter. An optional nullable parameter is
/// fine, since its `None` is simply not sent.
#[test]
fn e010_a_query_style_over_a_schema_admitting_a_value_it_cannot_serialize() {
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
          required: REQUIRED
          schema: SCHEMA
      responses:
        "204": { description: No Content }
"##;
    let spec = |style: &str, required: bool, schema: &str| {
        TEMPLATE
            .replace("STYLE", style)
            .replace("REQUIRED", &required.to_string())
            .replace("SCHEMA", schema)
    };
    let array = "{ type: array, items: { type: string } }";
    let object = "{ type: object, properties: { a: { type: string } } }";
    let rejected = [
        ("spaceDelimited", false, "{ type: string }"),
        ("pipeDelimited", true, "{ type: integer }"),
        ("spaceDelimited", false, "{ enum: [a, b] }"),
        (
            "pipeDelimited",
            false,
            "{ oneOf: [{ type: array, items: { type: string } }, { type: string }] }",
        ),
        (
            "spaceDelimited",
            true,
            "{ type: [array, 'null'], items: { type: string } }",
        ),
        ("deepObject", false, "{ type: string }"),
        ("deepObject", true, array),
        (
            "deepObject",
            false,
            "{ anyOf: [{ type: object, properties: { a: { type: string } } }, { type: integer }] }",
        ),
        (
            "deepObject",
            true,
            "{ type: [object, 'null'], properties: { a: { type: string } } }",
        ),
    ];
    for (style, required, schema) in rejected {
        let spec = spec(style, required, schema);
        let shape = if style == "deepObject" {
            "only objects"
        } else {
            "only arrays and objects"
        };
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{style} {schema}: {report:#?}"
            );
            let messages = messages_for(&report, Code::UnsupportedParameterStyle);
            assert_eq!(messages.len(), 1, "{style} {schema}: {report:#?}");
            assert!(
                messages[0].contains(&format!("`style: {style}` serializes {shape}"))
                    && messages[0].contains("`f`"),
                "{style} {schema}: {messages:?}"
            );
        }
    }
    let accepted = [
        ("spaceDelimited", true, array.to_owned()),
        ("pipeDelimited", false, object.to_owned()),
        (
            "spaceDelimited",
            false,
            format!("{{ oneOf: [{array}, {object}] }}"),
        ),
        (
            "pipeDelimited",
            false,
            "{ type: [array, 'null'], items: { type: string } }".to_owned(),
        ),
        (
            "spaceDelimited",
            false,
            format!("{{ oneOf: [{array}, {{ type: 'null' }}] }}"),
        ),
        ("deepObject", true, object.to_owned()),
        (
            "deepObject",
            false,
            "{ type: [object, 'null'], properties: { a: { type: string } } }".to_owned(),
        ),
    ];
    for (style, required, schema) in accepted {
        let spec = spec(style, required, &schema);
        for report in [generate(&spec), check(&spec)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{style} {schema}: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::UnsupportedParameterStyle),
                "{style} {schema}: {report:#?}"
            );
        }
    }
    // A `null` member reached through a `$ref` is not folded into the root the way the inline
    // `{ type: 'null' }` control above is: it stays a union member, whose `null` is the whole
    // value and is sent even on an optional parameter, so it rejects.
    let member_null = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      parameters:
        - name: f
          in: query
          style: spaceDelimited
          required: false
          schema: { oneOf: [{ $ref: '#/components/schemas/Arr' }, { $ref: '#/components/schemas/Nil' }] }
      responses:
        "204": { description: No Content }
components:
  schemas:
    Arr: { type: array, items: { type: string } }
    Nil: { type: 'null' }
"##;
    for report in [generate(member_null), check(member_null)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::UnsupportedParameterStyle);
        assert_eq!(messages.len(), 1, "{report:#?}");
        // The parameter is optional, so the message and remedy must name the union's `null`
        // member as the cause, not only a scalar or a required parameter's root `null`.
        assert!(
            messages[0].contains("`style: spaceDelimited` serializes only arrays and objects")
                && messages[0].contains("`f`")
                && messages[0].contains(
                    "a `oneOf`/`anyOf` member that is `null`, which is sent even on an optional \
                     parameter"
                ),
            "{messages:?}"
        );
        let remedies: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::UnsupportedParameterStyle)
            .filter_map(|d| d.remedy.as_deref())
            .collect();
        assert!(
            remedies.len() == 1
                && remedies[0].contains("with no `null` `oneOf`/`anyOf` member")
                && !remedies[0].contains("nullable only if the parameter is optional"),
            "{remedies:?}"
        );
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
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
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
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedParameterStyle),
        "{report:#?}"
    );
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
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::DeclarationHasNoEffect),
        "{report:#?}"
    );
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
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
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
    let report = generate(spec);
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
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
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
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::DeclarationHasNoEffect),
        "{report:#?}"
    );
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

#[test]
fn e010_a_content_parameter_whose_media_entry_has_no_schema() {
    // Issue #699: a schema-less `content` entry lowered to nothing through a bare `?`, and the
    // operation skips a parameter that lowers to nothing, so the argument vanished from the
    // generated method with no diagnostic. Every location reports it, as the `querystring` arm
    // already did, and a `text/*` entry as a JSON one.
    for (location, name, path, media) in [
        ("query", "q", "/x", "application/json"),
        ("header", "X-Q", "/x", "application/json"),
        ("cookie", "q", "/x", "text/plain"),
        ("path", "q", "/x/{q}", "application/json"),
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  {path}:
    get:
      operationId: getX
      parameters:
        - name: {name}
          in: {location}
          required: true
          content:
            {media}: {{}}
      responses:
        "204": {{ description: No Content }}
"##
        );
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{location}: {report:#?}"
            );
            assert_eq!(
                messages_for(&report, Code::UnsupportedParameterStyle),
                [format!(
                    "`content` parameter `{name}` has no schema for its `{media}` entry, so the \
                     client has no type to serialize it from"
                )],
                "{location}: {report:#?}"
            );
        }
    }
}

#[test]
fn an_optional_nullable_json_content_parameter_can_send_an_explicit_null() {
    // A JSON `content` parameter's codec renders `null` as the text `null`, which a server tells
    // apart from an absent parameter (#996), so its `…Params` field is `Option<Option<T>>` and its
    // setter takes `Option<T>`. A nullable schema-styled or `text/*` parameter has no token for
    // `null`, so it keeps the single `Option`; a required nullable JSON one is the `Option<T>`
    // argument it always was, its `None` already sent as `null`.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      parameters:
        - name: jq
          in: query
          content:
            application/json: { schema: { type: [integer, 'null'] } }
        - name: X-Jh
          in: header
          content:
            application/json: { schema: { type: [integer, 'null'] } }
        - name: jc
          in: cookie
          content:
            application/json: { schema: { type: [integer, 'null'] } }
        - name: X-Th
          in: header
          content:
            text/plain: { schema: { type: [string, 'null'] } }
        - name: sq
          in: query
          schema: { type: [integer, 'null'] }
        - name: X-Req
          in: header
          required: true
          content:
            application/json: { schema: { type: [integer, 'null'] } }
      responses:
        "204": { description: No Content }
  /y:
    get:
      operationId: getY
      parameters:
        - name: whole
          in: querystring
          content:
            application/json: { schema: { type: [integer, 'null'] } }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(report.diagnostics().is_empty(), "no warning: {report:#?}");
    }
    let (_, code) = generate_with_code(spec);
    let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
    for (field, ty) in [
        ("jq", "Jq"),
        ("x_jh", "XJh"),
        ("jc", "Jc"),
        ("whole", "Whole"),
    ] {
        assert!(
            flat.contains(&format!("pub {field}: Option<Option<types::{ty}>>,")),
            "{field}: {code}"
        );
        assert!(
            flat.contains(&format!(
                "pub fn {field}(mut self, value: Option<types::{ty}>) -> Self {{ self.{field} = \
                 Some(value);"
            )),
            "{field}: {code}"
        );
    }
    for (field, ty) in [("x_th", "XTh"), ("sq", "Sq")] {
        assert!(
            flat.contains(&format!("pub {field}: Option<types::{ty}>,")),
            "{field}: {code}"
        );
        assert!(
            flat.contains(&format!(
                "pub fn {field}(mut self, value: types::{ty}) -> Self"
            )),
            "{field}: {code}"
        );
    }
    assert!(flat.contains("x_req: Option<types::XReq>,"), "{code}");
    assert!(!flat.contains("x_req: Option<Option"), "{code}");
}
