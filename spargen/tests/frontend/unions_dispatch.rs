//! Union variants and dispatch: a member that is the union itself, or a sole member irreconcilable
//! with its siblings, is rejected; overlapping and disjoint branches generate with typed trial
//! matching; combined `oneOf`/`anyOf` applicators are `E007`; and nullable variants and null enums
//! hoist to `Option` or the exact null type.

use super::*;

/// A `oneOf` one of whose members is the union itself.
///
/// The member is a cycle-closing back-edge, so its `Ty` points at the union's own reservation. The
/// emitted `Deserialize` therefore opens with `serde_json::from_value::<Box<Loop>>(value.clone())`
/// — **the same impl, on the same value, with no base case** — so every decode recurses until the
/// stack is exhausted. It compiles, and the `e2e` gate compiles generated output rather than
/// decoding through every type, so nothing could have caught it.
///
/// A variant that *is* the whole union constrains nothing and cannot be decoded, so the right answer
/// is a rejection. Both spellings and the root document are pinned; as with the sibling case above,
/// the root form reproduces byte-identically on `2aa5ada`.
#[test]
fn a_union_variant_that_is_the_union_itself_is_rejected() {
    const LIB: &str = r##"
components:
  schemas:
    Loop:
      oneOf:
        - { $ref: 'PREFIX#/components/schemas/Loop' }
        - { type: string }
"##;

    for (spelling, prefix) in [("bare", ""), ("explicit", "./lib.yaml")] {
        let (generated, checked, code) = split(
            "./lib.yaml#/components/schemas/Loop",
            &LIB.replace("PREFIX", prefix),
        );
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{spelling}/{entry}: a variant that is the whole union decodes by re-entering its \
                 own `Deserialize` on the same value: {report:#?}"
            );
            assert!(
                has_code(report, Code::NonDisjointUnion),
                "{spelling}/{entry}: {report:#?}"
            );
        }
        // The runtime shape itself: nothing may emit a `Deserialize` arm that calls back into the
        // same type on the same value. That is what makes this a hang rather than a wrong type.
        assert!(
            !code.contains("from_value::<Box<Loop>>"),
            "{spelling}: the emitted decoder re-enters itself with no base case: {code}"
        );
    }

    let root = format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Loop' }} }}
{}"##,
        LIB.replace("PREFIX", "")
    );
    let report = generate(&root);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonDisjointUnion), "{report:#?}");
}

/// A union that collapses to its sole non-null member, whose intersection with the enclosing
/// schema's own sibling keywords is irreconcilable (`type: object` against `type: string`), leaves
/// the union with no variant. Before the rejection existed, the collapse `?`-propagated the failed
/// intersection with no diagnostic, so the run came back clean with the response body silently
/// dropped — the fourth behaviour. It must be refused with `E007` at the schema that carries the
/// union, through both entry points.
#[test]
fn a_sole_union_member_irreconcilable_with_its_siblings_is_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    get:
      operationId: getU
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  a: { type: string }
                oneOf:
                  - { type: string }
                  - { type: "null" }
"##;
    let pointer = "/paths/~1u/get/responses/200/content/application~1json/schema";
    for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let e007: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::NonDisjointUnion)
            .collect();
        assert!(
            e007.iter().any(
                |d| d.pointer.as_str() == pointer && d.message.contains("sole non-null member")
            ),
            "{entry}: E007 for the empty sole-member intersection must point at `{pointer}`, \
             not at {:?}\n{report:#?}",
            e007.iter().map(|d| d.pointer.as_str()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn overlapping_numeric_one_of_generates_with_typed_trial_matching() {
    // `integer | number` overlaps on integral payloads. The generated typed trial union enforces
    // exact-one matching at runtime (`1` is ambiguous; `1.5` selects number).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: integer
        - type: number
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");

    // The typed union the name promises: both branches survive, each with its own numeric type.
    let types = types_module(&code);
    assert_eq!(
        enum_variants(&types, "U"),
        ["Uvariant0(Box<Uvariant0>)", "Uvariant1(Box<Uvariant1>)"],
        "{types}"
    );
    assert_eq!(
        alias_target(&types, "Uvariant0").as_deref(),
        Some("i64"),
        "{types}"
    );
    assert_eq!(
        alias_target(&types, "Uvariant1").as_deref(),
        Some("f64"),
        "{types}"
    );
}

#[test]
fn overlapping_object_one_of_generates_with_typed_trial_matching() {
    // Object variants that overlap structurally use typed trial matching and exact-one semantics.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: object
          required: [kind]
          properties: { kind: { type: string }, a: { type: string } }
        - type: object
          required: [kind]
          properties: { kind: { type: string }, b: { type: string } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");

    // Both object branches survive as typed structs, each keeping its own distinguishing field.
    let types = types_module(&code);
    assert_eq!(
        enum_variants(&types, "U"),
        ["Uvariant0(Box<Uvariant0>)", "Uvariant1(Box<Uvariant1>)"],
        "{types}"
    );
    assert_eq!(
        declared_fields(&types, "Uvariant0"),
        ["kind", "a"],
        "{types}"
    );
    assert_eq!(
        declared_fields(&types, "Uvariant1"),
        ["kind", "b"],
        "{types}"
    );
}

#[test]
fn string_integer_union_generates() {
    // `string | integer` occupy distinct JSON type categories (string vs number) → provably disjoint
    // → GENERATES (this replaced the old, incorrect E007 fixture, which asserted rejection here).
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: integer
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
}

#[test]
fn union_sibling_constraints_intersect_every_branch() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    StringOnly:
      type: string
      oneOf:
        - type: string
        - type: integer
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");

    // The sibling `type: string` intersects each branch: the integer branch admits nothing a
    // string does, so what remains is the string branch alone, not a two-variant union.
    let types = types_module(&code);
    assert_eq!(
        alias_target(&types, "StringOnly").as_deref(),
        Some("String"),
        "{types}"
    );
    assert!(!types.contains("pub enum StringOnly "), "{types}");
}

#[test]
fn disjoint_string_array_union_generates() {
    // `string | string[]` occupy distinct JSON type categories (string vs array) → provably disjoint
    // (ollama's dominant shape). Generates without E007.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: array
          items: { type: string }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
}

#[test]
fn required_key_disjoint_objects_generate() {
    // Two CLOSED object variants (`additionalProperties: false`) each with a unique required key
    // (`a` / `b`) → provably disjoint by key presence → GENERATES with a content-inspecting custom
    // Deserialize. Closed is required for this fast path; open variants use typed trial matching.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A:
      type: object
      additionalProperties: false
      required: [a]
      properties: { a: { type: string } }
    B:
      type: object
      additionalProperties: false
      required: [b]
      properties: { b: { type: string } }
    U:
      oneOf:
        - $ref: "#/components/schemas/A"
        - $ref: "#/components/schemas/B"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
}

#[test]
fn open_object_union_generates_with_typed_trial_matching() {
    // Open objects cannot use the required-key fast path, so they use typed trial matching.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    A:
      type: object
      required: [a]
      properties: { a: { type: string } }
    B:
      type: object
      required: [b]
      properties: { b: { type: string } }
    U:
      oneOf:
        - $ref: "#/components/schemas/A"
        - $ref: "#/components/schemas/B"
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
}

#[test]
fn e007_combined_one_of_and_any_of_applicators_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: integer
      anyOf:
        - type: string
        - type: boolean
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::NonDisjointUnion));
}

#[test]
fn nullable_variant_hoists_to_option() {
    // A variant that is itself nullable (`{type: [string, "null"]}`) has its nullability HOISTED to
    // the union: the union becomes `Option<Enum>` and the string/array variants stay disjoint. A
    // `null` payload resolves at the outer `Option`, so the custom Deserialize only sees non-null.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: [string, "null"]
        - type: array
          items: { type: string }
    Holder:
      type: object
      required: [u]
      properties:
        u: { $ref: "#/components/schemas/U" }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");

    // The hoist itself: a required use of the union is `Option<U>` (the nullability the variant
    // carried), and the string variant's own type is the non-null `String`.
    let types = types_module(&code);
    assert_eq!(
        field_type(&types, "pub u:").as_deref(),
        Some("Option<U>"),
        "{types}"
    );
    assert_eq!(
        enum_variants(&types, "U"),
        ["Uvariant0(Box<Uvariant0>)", "Uvariant1(Box<Uvariant1>)"],
        "{types}"
    );
    assert_eq!(
        alias_target(&types, "Uvariant0").as_deref(),
        Some("String"),
        "{types}"
    );
}

#[test]
fn nullable_union_collapses_to_option() {
    // A 2-member union where one member is `{type: "null"}` strips the null and collapses to
    // `Option<String>` — no enum, no E007. Generates.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    U:
      oneOf:
        - type: string
        - type: "null"
    Holder:
      type: object
      required: [u]
      properties:
        u: { $ref: "#/components/schemas/U" }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");

    // The collapse: no enum, `U` names the string itself, and the stripped null reappears as the
    // `Option` a required use of `U` carries.
    let types = types_module(&code);
    assert!(!types.contains("pub enum U "), "{types}");
    assert_eq!(
        alias_target(&types, "U").as_deref(),
        Some("String"),
        "{types}"
    );
    assert_eq!(
        field_type(&types, "pub u:").as_deref(),
        Some("Option<U>"),
        "{types}"
    );
}

#[test]
fn null_mixed_scalar_enum_generates() {
    // A `null` member is stripped; the remaining homogeneous string scalars lower as a nullable
    // enum (`Option<Enum>`). No E008, and generation succeeds. check/generate parity.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Severity:
      type: [string, "null"]
      enum: [low, medium, high, null]
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonScalarEnum), "{report:#?}");
}

#[test]
fn all_null_enum_generates_as_exact_null() {
    // A value set of only `null` has no scalar remainder: it lowers to exact JSON null (`()`) rather
    // than an unconstrained value or E008.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Nothing:
      enum: [null]
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::NonScalarEnum), "{report:#?}");

    let types = types_module(&code);
    assert_eq!(
        alias_target(&types, "Nothing").as_deref(),
        Some("()"),
        "{types}"
    );
}
