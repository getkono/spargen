//! Schema lowering outside composition: enums, tuples, binary strings, `patternProperties`, and
//! validation-only keywords.

use super::*;

#[test]
fn boolean_false_schema_lowers_to_an_uninhabited_type() {
    let spec = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /forbidden:
    get:
      responses:
        '200':
          description: impossible
          content:
            application/json:
              schema: false
"#;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("enum ResponseBody"), "{code}");
}

#[test]
fn conditional_schema_keywords_warn_and_their_children_are_audited() {
    let warned = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Conditional:
      type: object
      if: { required: [kind] }
      then: { properties: { value: { type: string } } }
"##;
    let report = generate(warned);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );

    let dynamic = warned.replace(
        "then: { properties: { value: { type: string } } }",
        "then: { $dynamicRef: '#node' }",
    );
    let report = generate(&dynamic);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::DynamicRefRejected), "{report:#?}");
}

#[test]
fn pattern_properties_lowers_to_typed_map_with_w001() {
    // A representable `patternProperties` now GENERATES a typed overflow map instead of being
    // rejected. Two inline `{type: string}` value schemas under different patterns collapse to one
    // `BTreeMap<String, String>` (bounded structural equivalence over leaf primitives). The key
    // regexes are validation-only and acknowledged as `W001`, never silently dropped.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      patternProperties:
        "^x-": { type: string }
        "^y-": { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::PatternPropertiesRejected),
        "{report:#?}"
    );
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    // check/generate parity: the same disposition is reached without emitting.
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(
        has_code(&checked, Code::ValidationKeywordIgnored),
        "{checked:#?}"
    );
}

#[test]
fn pattern_properties_cyclic_array_values_terminate() {
    // Mutually-recursive array value schemas (`A = [B]`, `B = [A]`) form a cycle in the structural
    // homogeneity comparison. The visited-pair guard must terminate (return an outcome, never abort
    // with a stack overflow) and, since both patterns lower to the same array type, GENERATE one
    // typed overflow map. The check/generate parity assertion catches a regression that reintroduces
    // the crash.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A: { type: array, items: { $ref: "#/components/schemas/B" } }
    B: { type: array, items: { $ref: "#/components/schemas/A" } }
    Thing:
      type: object
      patternProperties:
        "^a-": { $ref: "#/components/schemas/A" }
        "^b-": { $ref: "#/components/schemas/B" }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::PatternPropertiesRejected),
        "{report:#?}"
    );
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
}

#[test]
fn e005_pattern_properties_heterogeneous_rejected() {
    // Two pattern value schemas that lower to different types cannot share one typed map → E005.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A: { type: string }
    B: { type: integer }
    Thing:
      type: object
      patternProperties:
        "^s-": { $ref: "#/components/schemas/A" }
        "^i-": { $ref: "#/components/schemas/B" }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::PatternPropertiesRejected));
    // check/generate parity: the rejection fires in `check` too.
    let checked = check(spec);
    assert_eq!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(has_code(&checked, Code::PatternPropertiesRejected));
}

#[test]
fn e005_pattern_properties_with_deny_rejected() {
    // `patternProperties` + `additionalProperties: false` cannot be faithfully represented → E005.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Thing:
      type: object
      additionalProperties: false
      patternProperties:
        "^x-": { type: string }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::PatternPropertiesRejected));
}

#[test]
fn e008_non_scalar_enum() {
    // Mixed scalar kinds with no null are genuinely unrepresentable: still E008.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Mixed:
      enum: ["a", 1]
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonScalarEnum));
}

#[test]
fn e008_stays_for_object_member_enum() {
    // Object (or array) enum members have no scalar-variant representation: still E008.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Structured:
      enum: [{ a: 1 }]
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonScalarEnum));
    let messages = messages_for(&report, Code::NonScalarEnum);
    assert!(
        messages.iter().all(|m| m.contains("object/array members")),
        "{messages:#?}"
    );
}

#[test]
fn e008_names_an_integer_member_above_i64_max() {
    // An integer above `i64::MAX` parses as a `u64` but has no `i64` discriminant: E008, and the
    // message names the range rather than blaming object/array members. check/generate parity.
    // `e008_names_a_yaml_integer_member_above_i64_max` pins the same message for a YAML document.
    let document = serde_json::from_str(
        r#"{
            "openapi": "3.1.0",
            "info": { "title": "T", "version": "1.0.0" },
            "paths": {},
            "components": { "schemas": { "Huge": {
                "type": "integer", "enum": [1, 18446744073709551615]
            } } }
        }"#,
    )
    .unwrap();
    let (generated, checked) = run_placement(&[("openapi.json", document)]);
    for report in [generated, checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::NonScalarEnum);
        assert!(
            !messages.is_empty()
                && messages.iter().all(|m| {
                    m.contains("18446744073709551615 exceeds i64::MAX") && !m.contains("object")
                }),
            "{messages:#?}"
        );
    }
}

#[test]
fn e008_names_a_yaml_integer_member_above_i64_max() {
    // Issue #540: the YAML parser classifies an integer literal above `i64::MAX` as a `u64`, as
    // the JSON parser does, so the E008 message names the range rather than calling the member a
    // float. check/generate parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Huge:
      type: integer
      enum: [1, 18446744073709551615]
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::NonScalarEnum);
        assert!(
            !messages.is_empty()
                && messages.iter().all(|m| {
                    m.contains("18446744073709551615 exceeds i64::MAX") && !m.contains("object")
                }),
            "{messages:#?}"
        );
    }
}

/// `ty` with every `pub type` alias the generated `types` module declares expanded, recursively,
/// down to the Rust type the alias chain finally names — `Holderblob` to `bytes::Bytes`, `Coord` to
/// `(f64, f64)`.
///
/// Different spellings of one schema emit differently named intermediate aliases (`Data`,
/// `HolderblobConstraint`, `ResponseBody`, …), so comparing names compares spellings. What the
/// spellings must agree on is the type a consumer finally holds, and that is the expansion.
fn expand_aliases(types: &str, ty: &str) -> String {
    let aliases: std::collections::HashMap<&str, &str> = types
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("pub type "))
        .filter_map(|rest| rest.split_once(" = "))
        .map(|(name, rhs)| (name, rhs.trim_end_matches(';')))
        .collect();
    let mut current = ty.replace("types::", "");
    // A bound rather than a fixpoint test alone: a cyclic alias set must fail the fixture, not
    // hang it.
    for _ in 0..32 {
        let mut next = String::new();
        let mut word = String::new();
        for ch in current.chars().chain(std::iter::once(' ')) {
            if ch.is_alphanumeric() || ch == '_' || ch == ':' {
                word.push(ch);
                continue;
            }
            next.push_str(aliases.get(word.as_str()).copied().unwrap_or(&word));
            word.clear();
            next.push(ch);
        }
        next.pop();
        let next = next.replace("types::", "");
        if next == current {
            return current;
        }
        current = next;
    }
    panic!("the alias chain from `{ty}` does not terminate: {current}");
}

/// What one spelling lowered to: the expanded `(property, body)` Rust types, or the codes it was
/// rejected with.
type SpellingOutcome = Result<(String, String), String>;

/// Generate one document per spelling of one schema — the schema placed as a required JSON
/// property, as a `201` response body under `body_media`, and (when `multipart`) as a multipart
/// request part, under both 3.1 and 3.2 — and return, per spelling and version, the expanded Rust
/// type of the property and of the body. A spelling that rejects is an `Err` naming its codes.
///
/// `components` is the shared `components.schemas` block (indented four spaces); every spelling is
/// written as a YAML flow mapping, so it slots into every position unchanged. The positions are
/// parameters because not every shape is valid in every one: a tuple is not an octet-stream body
/// in any spelling, the inline one included (`E009`), so the tuple fixtures use a JSON body.
fn lower_each_spelling(
    spellings: &[(&str, &str)],
    components: &str,
    body_media: &str,
    multipart: bool,
) -> Vec<(String, SpellingOutcome)> {
    let mut outcomes = Vec::new();
    for version in ["3.1.0", "3.2.0"] {
        for (label, spelling) in spellings {
            let request = if multipart {
                format!(
                    "      requestBody:\n        required: true\n        content:\n          \
                     multipart/form-data:\n            schema:\n              type: object\n              \
                     properties:\n                part: {spelling}\n              required: [part]\n"
                )
            } else {
                String::new()
            };
            let spec = format!(
                r##"openapi: {version}
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /u:
    post:
      operationId: fetch
{request}      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: {{ $ref: '#/components/schemas/Holder' }}
        '201':
          description: ok
          content:
            {body_media}:
              schema: {spelling}
components:
  schemas:
{components}
    Holder:
      type: object
      properties:
        field: {spelling}
      required: [field]
"##
            );
            let key = format!("{version} {label}");
            let (report, code) = generate_with_code(&spec);
            let checked = check(&spec);
            assert_eq!(
                report.outcome() == Outcome::Rejected,
                checked.outcome() == Outcome::Rejected,
                "`{key}`: check and generate disagree: {report:#?} {checked:#?}"
            );
            if report.outcome() == Outcome::Rejected {
                let codes: Vec<Code> = report.diagnostics().iter().map(|d| d.code).collect();
                outcomes.push((key, Err(format!("rejected with {codes:?}"))));
                continue;
            }
            let types = types_module(&code);
            let field = field_type(&types, "pub field:")
                .unwrap_or_else(|| panic!("`{key}`: `Holder.field` was not emitted: {types}"));
            let body = types
                .lines()
                .map(str::trim_start)
                .find_map(|line| line.strip_prefix("Status201("))
                .and_then(|rest| rest.strip_suffix("),"))
                .map(|payload| {
                    payload
                        .strip_prefix("Box<")
                        .and_then(|inner| inner.strip_suffix('>'))
                        .unwrap_or(payload)
                        .to_owned()
                })
                .unwrap_or_else(|| {
                    panic!("`{key}`: the `201` body variant was not emitted: {types}")
                });
            outcomes.push((
                key,
                Ok((
                    expand_aliases(&types, &field),
                    expand_aliases(&types, &body),
                )),
            ));
        }
    }
    outcomes
}

/// A binary string has one representation, `bytes::Bytes`, and spargen already emits it for the
/// inline spelling and for a `$ref` to a binary component. `{$ref: Data, format: binary}` over a
/// plain string `Data` is the same conjunction — a string that is binary — spelled with the
/// constraint on the other side of the `$ref`, and it used to reject with `E013`, because
/// `intersect_non_null` had no arm for `(Bytes, Primitive(String))` and its catch-all `None` reads
/// as an irreconcilable composition. Every spelling must now reach the same type, in every
/// position (a JSON property, a raw response body, a multipart part) and under both versions.
#[test]
fn a_binary_string_lowers_to_bytes_however_the_binary_constraint_is_spelled() {
    let components = "    Data: { type: string }\n    \
                      Blob: { type: string, format: binary }\n    \
                      Blob64: { type: string, contentEncoding: base64 }";
    let spellings: &[(&str, &str)] = &[
        ("inline", "{ type: string, format: binary }"),
        ("ref to binary", "{ $ref: '#/components/schemas/Blob' }"),
        (
            "ref to string + format: binary",
            "{ $ref: '#/components/schemas/Data', format: binary }",
        ),
        (
            "ref to string + contentEncoding",
            "{ $ref: '#/components/schemas/Data', contentEncoding: base64 }",
        ),
        (
            "ref to base64 + type: string",
            "{ $ref: '#/components/schemas/Blob64', type: string }",
        ),
        (
            "allOf",
            "{ allOf: [{ $ref: '#/components/schemas/Data' }, { format: binary }] }",
        ),
    ];
    let expected = Ok(("bytes::Bytes".to_owned(), "bytes::Bytes".to_owned()));
    let disagreeing: Vec<_> =
        lower_each_spelling(spellings, components, "application/octet-stream", true)
            .into_iter()
            .filter(|(_, outcome)| *outcome != expected)
            .collect();
    assert!(
        disagreeing.is_empty(),
        "every spelling of a binary string must lower to `bytes::Bytes` in every position, \
         but these did not: {disagreeing:#?}"
    );
}

/// The boundary of the arm above, pinned so that moving it is a visible decision: a string with a
/// format spargen DECODES (`uuid`, `date-time`, `date`) has a Rust representation of its own, and
/// `bytes::Bytes` cannot also be it, so the binary conjunction there stays `E013` rather than
/// silently picking one of the two.
#[test]
fn a_binary_constraint_on_a_decoded_string_format_still_rejects() {
    for format in ["uuid", "date-time", "date"] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths: {{}}
components:
  schemas:
    Formatted: {{ type: string, format: {format} }}
    Holder:
      type: object
      properties:
        field: {{ $ref: '#/components/schemas/Formatted', contentEncoding: base64 }}
      required: [field]
"##
        );
        let report = generate(&spec);
        assert_eq!(report.outcome(), Outcome::Rejected, "{format}: {report:#?}");
        assert!(
            has_code(&report, Code::AllOfIrreconcilable),
            "{format}: {report:#?}"
        );
    }
}

/// A closed `prefixItems` tuple conjoined with a homogeneous array is the tuple, each position
/// narrowed by the array's item schema: `[1.0, 2.0]` satisfies both `{$ref: Coord, type: array}`
/// and `Coord`. That pair also fell to `intersect_non_null`'s catch-all and rejected with `E013`.
/// Every spelling — the tuple on either side of the `$ref`, and through `allOf` — must reach the
/// type the inline tuple reaches.
#[test]
fn a_tuple_conjoined_with_an_array_lowers_to_the_tuple_however_it_is_spelled() {
    let components =
        "    Coord: { type: array, prefixItems: [{ type: number }, { type: number }], \
                      items: false }\n    \
                      Numbers: { type: array, items: { type: number } }";
    let spellings: &[(&str, &str)] = &[
        (
            "inline",
            "{ type: array, prefixItems: [{ type: number }, { type: number }], items: false }",
        ),
        ("ref to tuple", "{ $ref: '#/components/schemas/Coord' }"),
        (
            "ref to tuple + type: array",
            "{ $ref: '#/components/schemas/Coord', type: array }",
        ),
        (
            "ref to tuple + typed items",
            "{ $ref: '#/components/schemas/Coord', type: array, items: { type: number } }",
        ),
        (
            "ref to array + prefixItems",
            "{ $ref: '#/components/schemas/Numbers', type: array, \
             prefixItems: [{ type: number }, { type: number }], items: false }",
        ),
        (
            "allOf",
            "{ allOf: [{ $ref: '#/components/schemas/Coord' }, { type: array }] }",
        ),
    ];
    let expected = Ok(("(f64, f64)".to_owned(), "(f64, f64)".to_owned()));
    let disagreeing: Vec<_> = lower_each_spelling(spellings, components, "application/json", false)
        .into_iter()
        .filter(|(_, outcome)| *outcome != expected)
        .collect();
    assert!(
        disagreeing.is_empty(),
        "every spelling of a two-number tuple must lower to `(f64, f64)` in every position, but \
         these did not: {disagreeing:#?}"
    );
}

/// The array's item schema is a real constraint on every tuple position, not a formality the
/// tuple absorbs: an `integer` item narrows `number` positions to `i64`, and a position the item
/// contradicts leaves an intersection no Rust type holds — `prefixItems` does not require an array
/// to reach that position, so the arrays that stop short of it (`[]` among them) satisfy both
/// sides, and neither a narrowed tuple nor an uninhabited one admits exactly those — so that
/// composition is still `E013`, as unrepresentable rather than empty.
#[test]
fn an_array_item_schema_narrows_each_tuple_position_or_empties_the_tuple() {
    let narrowed = lower_each_spelling(
        &[(
            "integer items",
            "{ $ref: '#/components/schemas/Coord', type: array, items: { type: integer } }",
        )],
        "    Coord: { type: array, prefixItems: [{ type: number }, { type: number }], \
         items: false }",
        "application/json",
        false,
    );
    for (key, outcome) in narrowed {
        assert_eq!(
            outcome,
            Ok(("(i64, i64)".to_owned(), "(i64, i64)".to_owned())),
            "`{key}`"
        );
    }

    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths: {}
components:
  schemas:
    Pair: { type: array, prefixItems: [{ type: string }, { type: number }], items: false }
    Holder:
      type: object
      properties:
        field: { $ref: '#/components/schemas/Pair', type: array, items: { type: number } }
      required: [field]
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
}

/// The union path intersects the enclosing schema's sibling constraints with every branch, and a
/// branch whose intersection is empty (`NoMeet::Empty`) is dropped with `W011`; one whose
/// intersection is inhabited but unrepresentable (`NoMeet::Unrepresentable`) is `E013` instead.
/// Before the two pairs above had
/// arms, a tuple branch under a sibling `type: array`, and a binary branch under a sibling
/// `type: string`, were dropped as excluded although the sibling admits them — and the union
/// silently lost a member (`U` became `Vec<String>`, or `uuid::Uuid`). Both branches now survive,
/// with no `W011`, so the union keeps both variants; and a union whose only branches were those
/// used to be `E007` and now generates the lone surviving branch.
#[test]
fn a_sibling_constraint_keeps_a_tuple_or_binary_union_branch_it_admits() {
    let spec = |union: &str| {
        format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\nservers: [{{ url: 'https://e.com' }}]\n\
             paths: {{}}\ncomponents:\n  schemas:\n    U: {union}\n"
        )
    };
    for (label, union) in [
        (
            "tuple under type: array",
            "{ type: array, oneOf: [{ type: array, prefixItems: [{ type: number }, { type: number }], \
             items: false }, { type: array, items: { type: string } }] }",
        ),
        (
            "binary under type: string",
            "{ type: string, anyOf: [{ contentEncoding: base64 }, { type: string, format: uuid }] }",
        ),
    ] {
        let (report, code) = generate_with_code(&spec(union));
        assert_ne!(report.outcome(), Outcome::Rejected, "{label}: {report:#?}");
        assert!(
            !has_code(&report, Code::DeclarationHasNoEffect),
            "{label}: a branch the sibling admits was dropped as excluded: {report:#?}"
        );
        assert_eq!(
            enum_variants(&types_module(&code), "U").len(),
            2,
            "{label}: both branches must survive as variants: {}",
            types_module(&code)
        );
    }

    // A union whose every other branch the sibling excludes: the admitted branch is what remains.
    let (report, code) = generate_with_code(&spec(
        "{ type: string, oneOf: [{ format: binary }, { type: integer }] }",
    ));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(
        expand_aliases(&types_module(&code), "U"),
        "bytes::Bytes",
        "{}",
        types_module(&code)
    );
}

/// `object_body` consumed `required` only as a per-property flag, so a `required` name that no
/// `properties` entry declares was dropped and the generated type accepted, and could emit, an
/// object without that key (#140). It is carried as a required field, typed by what the object
/// says of an undeclared key.
#[test]
fn a_required_name_no_property_declares_is_still_required() {
    const HEAD: &str = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: \
                        'https://e.com' }]\npaths: {}\ncomponents:\n  schemas:\n";
    let field = |schema: &str, name: &str| {
        let (report, code) = generate_with_code(&format!("{HEAD}    Thing: {schema}\n"));
        assert_ne!(report.outcome(), Outcome::Rejected, "{schema}: {report:#?}");
        let types = types_module(&code);
        let prefix = format!("pub {name}: ");
        let ty = types
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix(&prefix))
            .unwrap_or_else(|| panic!("`{schema}` generated no `{name}` field:\n{types}"))
            .trim_end_matches(',')
            .to_owned();
        // The field is written through its own named alias; report what that alias names.
        let alias = format!("pub type {ty} = ");
        let resolved = types
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix(&alias))
            .map_or(ty.clone(), |rest| rest.trim_end_matches(';').to_owned());
        format!("pub {name}: {resolved},")
    };

    // Unconstrained: absent `additionalProperties`, `true`, and `patternProperties` whose patterns
    // cannot be matched against the name at generation time.
    for schema in [
        "{ type: object, required: [a] }",
        "{ type: object, additionalProperties: true, required: [a] }",
        "{ type: object, patternProperties: { '^x': { type: integer } }, required: [a] }",
        "{ type: object, properties: { b: { type: string } }, required: [a, b] }",
    ] {
        assert_eq!(field(schema, "a"), "pub a: serde_json::Value,", "{schema}");
    }
    // Typed by the `additionalProperties` schema, which is what the key's value must satisfy.
    assert_eq!(
        field(
            "{ type: object, additionalProperties: { type: integer }, required: [a] }",
            "a"
        ),
        "pub a: i64,"
    );
    // A value schema that closes a cycle back to the object: the map drops the `Box` its own
    // indirection makes unnecessary, and a plain required field has none, so it is boxed again.
    assert_eq!(
        field(
            "{ type: object, additionalProperties: { $ref: '#/components/schemas/Thing' }, \
             required: [child] }",
            "child"
        ),
        "pub child: Box<Thing>,"
    );
    // `additionalProperties: false` closes the object to the fields the type declares, as it does
    // everywhere else in lowering, and the required name is one of them.
    assert_eq!(
        field(
            "{ type: object, additionalProperties: false, required: [a] }",
            "a"
        ),
        "pub a: serde_json::Value,"
    );
    // So the closed `allOf` spelling of "require a property the base declares" keeps generating,
    // with the base's type for it.
    assert_eq!(
        field(
            "{ allOf: [{ type: object, properties: { a: { type: string } } }, \
             { additionalProperties: false, required: [a] }] }",
            "a"
        ),
        "pub a: String,"
    );

    // A member that only requires the name comes FIRST here, so its metadata-less field is the one
    // the merge meets first. The later declaration's metadata must still reach the field.
    let (report, code) = generate_with_code(&format!(
        "{HEAD}    Thing:\n      allOf:\n        - {{ type: object, required: [a] }}\n        - \
         {{ type: object, properties: {{ a: {{ type: string, deprecated: true }} }} }}\n"
    ));
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    let lines: Vec<&str> = types.lines().map(str::trim).collect();
    let at = lines
        .iter()
        .position(|line| line.starts_with("pub a: "))
        .unwrap_or_else(|| panic!("no `a` field:\n{types}"));
    assert!(
        lines[..at]
            .iter()
            .rev()
            .take_while(|line| line.starts_with("#[") || line.starts_with("///"))
            .any(|line| line.contains("Deprecated per the spec")),
        "the declaring member's `deprecated` was lost to the requiring member's placeholder:\n{types}"
    );
    assert!(!lines[at].contains("Option<"), "{types}");

    // The `allOf` spelling reaches the same `object_body`, so a member's undeclared requirement
    // survives the merge too.
    assert_eq!(
        field(
            "{ allOf: [{ type: object, properties: { b: { type: string } } }, \
             { type: object, required: [a] }] }",
            "a"
        ),
        "pub a: serde_json::Value,"
    );
}

#[test]
fn w001_validation_keyword_ignored_still_generates() {
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /ping:
    get:
      responses:
        "204": { description: No Content }
components:
  schemas:
    Age:
      type: integer
      minimum: 0
"##,
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(has_code(&report, Code::ValidationKeywordIgnored));
}

#[test]
fn property_annotations_come_from_the_property_not_the_object() {
    // Regression: `deprecated`/`readOnly`/`writeOnly` were read from the enclosing object, so an
    // object-level `deprecated: true` marked every field and a property-level one was ignored.
    let (report, code) = generate_with_code(
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
              schema: { $ref: "#/components/schemas/Item" }
components:
  schemas:
    Item:
      type: object
      deprecated: true
      properties:
        current: { type: string }
        legacy: { type: string, deprecated: true }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("legacy"), "legacy field emitted: {code}");
    assert!(code.contains("current"), "current field emitted: {code}");
    // Exactly one item is deprecated — the property that says so — not every field of the
    // deprecated object, and not zero.
    assert_eq!(
        code.matches("#[deprecated]").count(),
        1,
        "exactly one item is deprecated, not every field of a deprecated object: {code}"
    );
}

#[test]
fn e015_items_beside_prefix_items() {
    let spec = r##"
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
              schema:
                type: array
                prefixItems:
                  - { type: string }
                items: { type: integer }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::TupleRestNotRepresentable),
            "{report:#?}"
        );
    }
}

#[test]
fn items_false_beside_prefix_items_is_a_closed_tuple() {
    let spec = r##"
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
              schema:
                type: array
                prefixItems:
                  - { type: string }
                  - { type: integer }
                items: false
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::TupleRestNotRepresentable),
            "{report:#?}"
        );
    }
}

/// The support matrix says homogeneous scalar enums are supported; floats are the boundary. They
/// have no representable Rust enum discriminant, so they are `E008` rather than a silent demotion,
/// and the matrix now states the boundary rather than implying floats are included.
#[test]
fn e008_float_enum_members_are_the_documented_boundary() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Ratio:
      type: number
      enum: [1.5, 2.5]
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::NonScalarEnum), "{report:#?}");
        // The message names the float, not object/array members, which this enum has none of.
        let messages = messages_for(&report, Code::NonScalarEnum);
        assert!(
            messages
                .iter()
                .all(|m| m.contains("1.5 is a floating-point number") && !m.contains("object")),
            "{messages:#?}"
        );
    }

    // Strings, integers, and booleans stay supported.
    for (kind, values) in [
        ("string", "[a, b]"),
        ("integer", "[1, 2]"),
        ("boolean", "[true, false]"),
    ] {
        let ok = format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\npaths: {{}}\ncomponents:\n  schemas:\n    Kind:\n      type: {kind}\n      enum: {values}\n"
        );
        let report = generate(&ok);
        assert_ne!(report.outcome(), Outcome::Rejected, "{kind}: {report:#?}");
    }
}

/// Untyped `required` or `additionalProperties` alone are object applicators, and establish the
/// object category as untyped `properties` and the same keywords beside a `$ref` already do
/// (#613). They used to fall to `serde_json::Value`, dropping the constraint with no diagnostic,
/// standalone and as a `oneOf`/`anyOf` branch alike.
#[test]
fn untyped_required_or_additional_properties_alone_lower_to_an_object() {
    for (case, body) in [
        ("required", "      required: [id]\n"),
        (
            "additionalProperties",
            "      additionalProperties: { type: string }\n",
        ),
        (
            "additionalProperties: false",
            "      additionalProperties: false\n",
        ),
        (
            "required beside additionalProperties",
            "      required: [id]\n      additionalProperties: { type: integer }\n",
        ),
        (
            "an anyOf branch",
            "      anyOf: [ { required: [id] }, { type: string } ]\n",
        ),
        (
            "a oneOf branch",
            "      oneOf: [ { required: [id] }, { type: string } ]\n",
        ),
    ] {
        let spec = with_schemas("3.1.0", &format!("    U:\n{body}"));
        let (report, code) = generate_with_code(&spec);
        assert_eq!(report.outcome(), Outcome::Generated, "{case}: {report:#?}");
        assert!(report.diagnostics().is_empty(), "{case}: {report:#?}");
        let types = types_module(&code);
        assert_eq!(alias_target(&types, "U"), None, "{case}: {types}");
        if body.contains("Of:") {
            assert_eq!(enum_variants(&types, "U").len(), 2, "{case}: {types}");
        } else {
            assert!(types.contains("pub struct U {"), "{case}: {types}");
        }
        if body.contains("required") {
            // The required key is a required field of the object the keyword establishes.
            let owner = field_owner(&types, "pub id:")
                .unwrap_or_else(|| panic!("{case}: no `id` field: {types}"));
            assert_eq!(declared_fields(&types, &owner)[0], "id", "{case}: {types}");
            let id = field_type(&types, "pub id:").unwrap_or_default();
            assert!(
                !id.starts_with("Option<"),
                "{case}: `id` is `{id}`: {types}"
            );
        }
        if body.contains("{ type: string }\n") && !body.contains("Of:") {
            // The undeclared keys are a typed map of the `additionalProperties` value.
            let map = field_type(&types, "pub additional:").unwrap_or_default();
            let value = map
                .strip_prefix("BTreeMap<String, ")
                .and_then(|rest| rest.strip_suffix('>'))
                .unwrap_or_else(|| panic!("{case}: `additional` is `{map}`: {types}"));
            assert_eq!(
                alias_target(&types, value).as_deref(),
                Some("String"),
                "{case}: {types}"
            );
        }
    }

    // Distinct `required` branches are distinct objects in every spelling of the meet with an
    // object target, so none collapses them (`W001`) into the target: each keeps two variants, one
    // requiring each key.
    let base =
        "    Base:\n      type: object\n      properties:\n        a: { type: string }\n        \
                b: { type: string }\n";
    let branches = "oneOf: [ { required: [a] }, { required: [b] } ]";
    for (spelling, site) in [
        (
            "$ref sibling",
            format!("{{ $ref: '#/components/schemas/Base', {branches} }}"),
        ),
        (
            "allOf member",
            format!("{{ allOf: [ {{ $ref: '#/components/schemas/Base' }}, {{ {branches} }} ] }}"),
        ),
        (
            "beside allOf",
            format!("{{ allOf: [ {{ $ref: '#/components/schemas/Base' }} ], {branches} }}"),
        ),
        (
            "inline",
            format!(
                "{{ type: object, properties: {{ a: {{ type: string }}, b: {{ type: string }} }}, \
                 {branches} }}"
            ),
        ),
    ] {
        let spec = with_schemas("3.1.0", &format!("{base}    Pick: {site}\n"));
        let (report, code) = generate_with_code(&spec);
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{spelling}: {report:#?}"
        );
        assert!(report.diagnostics().is_empty(), "{spelling}: {report:#?}");
        assert_eq!(
            enum_variants(&types_module(&code), "Pick").len(),
            2,
            "{spelling}: {code}"
        );
    }
}

/// Untyped `items` or `prefixItems` alone are array applicators, and establish the array category
/// as the same keywords beside a `$ref` or as an `allOf` member already do, and as untyped object
/// applicators alone do (#613). They used to fall to `serde_json::Value`, dropping the item types
/// with no diagnostic, standalone and as a `oneOf`/`anyOf` branch alike (#614). The untyped
/// spelling reads as `type: array` does: the same element types, and the same rejection of a typed
/// tuple remainder.
#[test]
fn untyped_items_or_prefix_items_alone_lower_to_an_array() {
    // The type an element alias names, or the element itself where it is no alias.
    fn element(types: &str, ty: &str) -> String {
        alias_target(types, ty).unwrap_or_else(|| ty.to_owned())
    }
    for (case, body, expected) in [
        ("items", "      items: { type: string }\n", vec!["String"]),
        (
            "prefixItems closed by items: false",
            "      prefixItems: [ { type: string }, { type: integer } ]\n      items: false\n",
            vec!["String", "i64"],
        ),
        (
            "prefixItems alone",
            "      prefixItems: [ { type: boolean } ]\n",
            vec!["bool"],
        ),
    ] {
        for ty in ["", "      type: array\n"] {
            let what = format!("{case}, {ty:?}");
            let spec = with_schemas("3.1.0", &format!("    U:\n{ty}{body}"));
            let (report, code) = generate_with_code(&spec);
            assert_eq!(report.outcome(), Outcome::Generated, "{what}: {report:#?}");
            assert!(report.diagnostics().is_empty(), "{what}: {report:#?}");
            let types = types_module(&code);
            let target = alias_target(&types, "U").unwrap_or_else(|| panic!("{what}: {types}"));
            let elements: Vec<String> = if let Some(inner) = target
                .strip_prefix("Vec<")
                .and_then(|rest| rest.strip_suffix('>'))
            {
                vec![element(&types, inner)]
            } else {
                target
                    .strip_prefix('(')
                    .and_then(|rest| rest.strip_suffix(')'))
                    .unwrap_or_else(|| panic!("{what}: `U` is `{target}`: {types}"))
                    .split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(|part| element(&types, part))
                    .collect()
            };
            assert_eq!(elements, expected, "{what}: {types}");
        }
    }

    // As a union branch the array keeps its element type beside the other branch.
    for keyword in ["anyOf", "oneOf"] {
        let spec = with_schemas(
            "3.1.0",
            &format!("    U:\n      {keyword}: [ {{ items: {{ type: string }} }}, {{ type: integer }} ]\n"),
        );
        let (report, code) = generate_with_code(&spec);
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{keyword}: {report:#?}"
        );
        assert!(report.diagnostics().is_empty(), "{keyword}: {report:#?}");
        let types = types_module(&code);
        assert_eq!(enum_variants(&types, "U").len(), 2, "{keyword}: {types}");
        assert_eq!(
            alias_target(&types, "Uvariant0").as_deref(),
            Some("Vec<Uvariant0Item>"),
            "{keyword}: {types}"
        );
        assert_eq!(
            alias_target(&types, "Uvariant0Item").as_deref(),
            Some("String"),
            "{keyword}: {types}"
        );
    }

    // A typed remainder beside `prefixItems` is the variable-length tuple `type: array` rejects,
    // and the untyped spelling is rejected with the same code.
    for ty in ["", "      type: array\n"] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    U:\n{ty}      prefixItems: [ {{ type: string }} ]\n      items: {{ type: \
                 integer }}\n"
            ),
        );
        let (report, _) = generate_with_code(&spec);
        assert_eq!(report.outcome(), Outcome::Rejected, "{ty:?}: {report:#?}");
        assert!(
            has_code(&report, Code::TupleRestNotRepresentable),
            "{ty:?}: {report:#?}"
        );
    }

    // The component is an array wherever it is used, so a property referencing it is that array,
    // and a `$ref` to it beside a scalar `type` meets an array, as a `$ref` to its `type: array`
    // spelling does: the intersection is empty (`E013`). An `allOf` member referencing it still
    // reads it by its keywords, vacuous beside the scalar (#612).
    let names = "    Names: { items: { type: string } }\n";
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        &format!(
            "    Owner:\n      type: object\n      properties:\n        a: {{ $ref: \
             '#/components/schemas/Names' }}\n{names}"
        ),
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let types = types_module(&code);
    assert_eq!(
        field_type(&types, "pub a:").as_deref(),
        Some("Option<Names>"),
        "{types}"
    );
    assert_eq!(
        alias_target(&types, "Names").as_deref(),
        Some("Vec<NamesItem>"),
        "{types}"
    );
    for (spelling, expected) in [
        (
            "{ $ref: '#/components/schemas/Names', type: string }",
            Outcome::Rejected,
        ),
        (
            "{ allOf: [ { $ref: '#/components/schemas/Names' }, { type: string } ] }",
            Outcome::Generated,
        ),
    ] {
        let spec = with_schemas("3.1.0", &format!("    X: {spelling}\n{names}"));
        let (report, code) = generate_with_code(&spec);
        assert_eq!(report.outcome(), expected, "{spelling}: {report:#?}");
        if expected == Outcome::Rejected {
            assert_eq!(codes(&report), ["E013"], "{spelling}: {report:#?}");
        } else {
            assert_eq!(
                alias_target(&types_module(&code), "X").as_deref(),
                Some("String"),
                "{spelling}: {code}"
            );
        }
    }
}
