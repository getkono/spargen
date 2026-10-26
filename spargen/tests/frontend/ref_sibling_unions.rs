//! A `$ref` beside a `oneOf`/`anyOf` sibling: the sibling is intersected with the target, a
//! collapse or merge of its branches keeps the target and the `oneOf` null rule, and a union-member
//! or `allOf`-member `$ref` with a union sibling is intersected with its target.
//! `ref_union_sibling_documents` gives the three sites the family is exercised at.

use super::*;

/// The `W011` a union sibling's second branch draws where the `$ref` target excludes it.
const EXCLUDED_MEMBER_1: &str = "union member 1 cannot satisfy the enclosing schema's own \
                                 constraints, so it is not a variant of the generated enum";

#[test]
fn schema_ref_siblings_are_intersected_not_dropped() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /extended:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Extended' }
components:
  schemas:
    Base:
      type: object
      properties: { id: { type: string } }
      required: [id]
    Extended:
      $ref: '#/components/schemas/Base'
      type: object
      properties: { extra: { type: integer } }
      required: [extra]
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("pub id"), "{code}");
    assert!(code.contains("pub extra"), "{code}");
}

/// The three positions a `$ref` with a union sibling is exercised at: a request body, a response
/// body, and a component property. Each is `(label, document, JSON Pointer of the `$ref` site)`,
/// with `SITE` standing for the schema `{ $ref: '#/components/schemas/Name', <sibling> }` and
/// `Name` a plain string.
pub(super) fn ref_union_sibling_documents(site: &str) -> Vec<(&'static str, String, &'static str)> {
    let name = "    Name: { type: string }\n";
    let request = format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    post:
      operationId: send
      requestBody:
        required: true
        content:
          application/json: {{ schema: {site} }}
      responses:
        '204': {{ description: ok }}
components:
  schemas:
{name}"##
    );
    let response = format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {site} }}
components:
  schemas:
{name}"##
    );
    let property = format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    Holder:
      type: object
      properties:
        label: {site}
      required: [label]
{name}"##
    );
    vec![
        (
            "request body",
            request,
            "/paths/~1p/post/requestBody/content/application~1json/schema",
        ),
        (
            "response body",
            response,
            "/paths/~1p/get/responses/200/content/application~1json/schema",
        ),
        (
            "component property",
            property,
            "/components/schemas/Holder/properties/label",
        ),
    ]
}

/// A `oneOf`/`anyOf` beside a `$ref` constrains the instance exactly as a `type` beside it does:
/// `$ref` is a 2020-12 applicator, so the value must satisfy the target AND the union. The
/// shape-constraint gate used to leave both keywords out, so a `$ref` whose only sibling was a union
/// took the bare-reference exit and the union was dropped with no diagnostic at all — `spargen
/// check` was clean and the position was typed as the plain target.
///
/// A union whose every branch contradicts the target admits no value, so it is `E013` at the
/// `$ref` site, through `check` and `generate` alike, in every position.
#[test]
fn a_ref_whose_union_sibling_contradicts_its_target_is_rejected() {
    for keyword in ["oneOf", "anyOf"] {
        let site =
            format!("{{ $ref: '#/components/schemas/Name', {keyword}: [ {{ type: integer }} ] }}");
        for (position, spec, pointer) in ref_union_sibling_documents(&site) {
            for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{keyword} in a {position} via {entry}: the sibling must not be dropped in \
                     silence: {report:#?}\n{spec}"
                );
                assert!(
                    report
                        .diagnostics()
                        .iter()
                        .any(|d| d.code == Code::AllOfIrreconcilable
                            && d.pointer.as_str() == pointer),
                    "{keyword} in a {position} via {entry}: E013 must point at `{pointer}`: \
                     {report:#?}"
                );
            }
        }
    }
}

/// The satisfiable counterpart: the union sibling narrows the target rather than being discarded.
/// Of `[{type: string, enum: [red, green]}, {type: integer}]` beside a string `Name`, only the enum
/// branch meets the target, so the position is that two-member enum — not `String`, which is what
/// the dropped sibling used to leave, and not a union still carrying the integer branch.
#[test]
fn a_ref_with_a_union_sibling_is_intersected_with_its_target() {
    for keyword in ["oneOf", "anyOf"] {
        let site = format!(
            "{{ $ref: '#/components/schemas/Name', {keyword}: [ {{ type: string, enum: [red, \
             green] }}, {{ type: integer }} ] }}"
        );
        for (position, spec, _) in ref_union_sibling_documents(&site) {
            let (report, code) = generate_with_code(&spec);
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{keyword} in a {position}: {report:#?}\n{spec}"
            );
            // The integer branch the target excludes is reported as the inline sibling spelling
            // reports it (#671).
            assert_eq!(
                messages_for(&report, Code::DeclarationHasNoEffect),
                [EXCLUDED_MEMBER_1],
                "{keyword} in a {position}: {report:#?}"
            );
            assert_eq!(
                codes(&report),
                ["W011"],
                "{keyword} in a {position}: {report:#?}"
            );
            let types = types_module(&code);
            // The Rust type the position is bound to, read from where the position is used.
            let bound: String = match position {
                "request body" => types
                    .split("body: &types::")
                    .nth(1)
                    .and_then(|rest| rest.split([',', ')', '\n']).next())
                    .map(str::to_owned),
                "response body" => types
                    .split("ResponseValue<types::")
                    .nth(1)
                    .and_then(|rest| rest.split('>').next())
                    .map(str::to_owned),
                _ => field_type(&types, "pub label"),
            }
            .unwrap_or_else(|| panic!("{keyword} in a {position}: no bound type found: {types}"));
            assert_eq!(
                enum_variants(&types, &bound),
                ["Red", "Green"],
                "{keyword} in a {position}: `{bound}` must be the enum branch alone: {types}"
            );
        }
    }
}

/// A union sibling whose branches intersect with the target to one and the same type — here
/// branches that state nothing, which lower to no shape of their own — must not be
/// emitted as a union. Every branch would be the target, so a `oneOf` of them rejects every value
/// and an `anyOf` of them is the target itself. The position keeps the target's shape and the
/// branch distinctions it cannot carry are reported as `W001` at the `$ref`, through `generate` and
/// `check` alike, for `oneOf` and `anyOf`, as a component and as a property.
#[test]
fn a_ref_with_a_union_sibling_whose_branches_collapse_keeps_the_target_and_warns() {
    for keyword in ["oneOf", "anyOf"] {
        let site = format!("{{ $ref: '#/components/schemas/Base', {keyword}: [ {{}}, {{}} ] }}");
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    Base:
      type: object
      properties:
        a: {{ type: string }}
        b: {{ type: string }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
        inline: {site}
      required: [pick, inline]
"##
        );
        let (report, code) = generate_with_code(&spec);
        for (entry, report) in [("generate", &report), ("check", &check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{keyword} via {entry}: {report:#?}"
            );
            for pointer in [
                "/components/schemas/Pick",
                "/components/schemas/Holder/properties/inline",
            ] {
                assert!(
                    report.diagnostics().iter().any(|d| {
                        d.code == Code::ValidationKeywordIgnored && d.pointer.as_str() == pointer
                    }),
                    "{keyword} via {entry}: W001 must point at `{pointer}`: {report:#?}"
                );
            }
        }
        let types = types_module(&code);
        assert_eq!(
            declared_fields(&types, "Pick"),
            ["a", "b"],
            "{keyword}: `Pick` must keep `Base`'s shape: {types}"
        );
        let inline = field_type(&types, "pub inline")
            .unwrap_or_else(|| panic!("{keyword}: no `inline` field: {types}"));
        assert_eq!(
            declared_fields(&types, &inline),
            ["a", "b"],
            "{keyword}: the `inline` property must keep `Base`'s shape: {types}"
        );
    }
}

/// The `$ref` spelling of the merge above gives the same answer when its branches share a generated
/// type only structurally (#402). `$ref: Int` beside `oneOf: [{enum: [1]}, {enum: [2]}]` meets to
/// two distinct `i64`-alias enums, one generated type: emitted as two variants, every value would
/// match both and fail exactly-one, so the position is that one type and `W001` reports it. Where
/// only some branches share a type after the meet, they become one variant and the others stand.
#[test]
fn a_ref_sibling_one_of_whose_branches_share_a_type_only_structurally_collapses() {
    for (shape, body, expect_variants) in [
        (
            "fully shared",
            "{ $ref: '#/components/schemas/Int', oneOf: [ { enum: [1] }, { enum: [2] } ] }",
            None,
        ),
        (
            "partly shared",
            "{ $ref: '#/components/schemas/Free', oneOf: [ { type: integer, enum: [1] }, { type: \
             integer, enum: [2] }, { type: string } ] }",
            Some(2),
        ),
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/U' }} }}
components:
  schemas:
    Int: {{ type: integer }}
    Free: {{ description: any value }}
    U: {body}
"##
        );
        let (report, code) = generate_with_code(&spec);
        for (entry, report) in [("generate", &report), ("check", &check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{shape} via {entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|d| {
                    d.code == Code::ValidationKeywordIgnored
                        && d.pointer.as_str() == "/components/schemas/U"
                }),
                "{shape} via {entry}: the collapse must warn at `U`: {report:#?}"
            );
        }
        let types = types_module(&code);
        match expect_variants {
            Some(count) => assert_eq!(
                enum_variants(&types, "U").len(),
                count,
                "{shape}: the two integer branches must be one variant: {types}"
            ),
            None => assert!(
                !types.contains("pub enum U "),
                "{shape}: `U` must not be a union of indistinguishable variants: {types}"
            ),
        }
    }
}

/// The partly shared `$ref`-sibling merge keeps the `oneOf` null rule (#402). Beside a nullable
/// object `NB`, two branches that state nothing each meet `NB` to one nullable struct, and the
/// third branch's extra property `c` makes a second, so the union keeps two variants. `null` then
/// matches both merged branches and fails exactly-one, so it is invalid: the merged variant does
/// not carry `Option<_>`, and neither does the position. The third branch is written once
/// non-nullable and once nullable, so the union itself would otherwise accept `null` in the second
/// case and the position's own nullability is pinned too.
#[test]
fn a_partly_shared_ref_sibling_one_of_beside_a_nullable_target_is_not_nullable() {
    for third in ["object", "[object, 'null']"] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    NB:
      type: [object, 'null']
      properties:
        a: {{ type: string }}
        b: {{ type: string }}
    Holder:
      type: object
      properties:
        x:
          $ref: '#/components/schemas/NB'
          oneOf:
            - {{}}
            - {{}}
            - {{ type: {third}, properties: {{ c: {{ type: integer }} }} }}
      required: [x]
"##
        );
        let (report, code) = generate_with_code(&spec);
        for (entry, report) in [("generate", &report), ("check", &check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "third `type: {third}` via {entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|d| {
                    d.code == Code::ValidationKeywordIgnored
                        && d.pointer.as_str() == "/components/schemas/Holder/properties/x"
                }),
                "third `type: {third}` via {entry}: the partial merge must warn at `x`: \
                 {report:#?}"
            );
        }
        let types = types_module(&code);
        let x = field_type(&types, "pub x")
            .unwrap_or_else(|| panic!("third `type: {third}`: no `x` field: {types}"));
        assert!(
            !x.starts_with("Option<"),
            "third `type: {third}`: `x` is `{x}`, but `null` matches both merged branches, so it \
             is invalid: {types}"
        );
        let variants = enum_variants(&types, &x);
        assert_eq!(
            variants.len(),
            2,
            "third `type: {third}`: the two required-only branches must be one variant beside \
             the `c` branch: {types}"
        );
        assert!(
            !variants.iter().any(|variant| variant.contains("(Option<")),
            "third `type: {third}`: no variant may accept `null`, which matches both merged \
             branches: {variants:?}"
        );
    }
}

/// After the meet with a `$ref` target, each `oneOf` branch keeps its own nullability, so branches
/// that emit one Rust type need not agree on `null` (#402). Beside `NI: {type: [integer, 'null']}`,
/// `{minimum: 0}` says nothing about `null` and meets `NI` to a nullable integer, while
/// `{type: integer}` meets it to a plain one: every integer matches both, so they are one type
/// however `null` falls, and `W001` reports it. `null` is valid exactly when one branch accepts it:
/// with one untyped branch it matches that branch alone, and with two it matches both and fails
/// exactly-one.
#[test]
fn a_ref_sibling_one_of_whose_branches_differ_only_in_nullability_collapses() {
    for (branches, nullable) in [
        ("[ { minimum: 0 }, { type: integer } ]", true),
        (
            "[ { minimum: 0 }, { maximum: 10 }, { type: integer } ]",
            false,
        ),
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    NI: {{ type: [integer, 'null'] }}
    Holder:
      type: object
      properties:
        x:
          $ref: '#/components/schemas/NI'
          oneOf: {branches}
      required: [x]
"##
        );
        let (report, code) = generate_with_code(&spec);
        for (entry, report) in [("generate", &report), ("check", &check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{branches} via {entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|d| {
                    d.code == Code::ValidationKeywordIgnored
                        && d.pointer.as_str() == "/components/schemas/Holder/properties/x"
                }),
                "{branches} via {entry}: the collapse must warn at `x`: {report:#?}"
            );
        }
        let types = types_module(&code);
        let x = field_type(&types, "pub x")
            .unwrap_or_else(|| panic!("{branches}: no `x` field: {types}"));
        assert_eq!(
            x.starts_with("Option<"),
            nullable,
            "{branches}: `x` is `{x}`, but `null` is {} here: {types}",
            if nullable { "valid" } else { "invalid" }
        );
        let inner = x
            .strip_prefix("Option<")
            .and_then(|inner| inner.strip_suffix('>'))
            .unwrap_or(&x);
        assert!(
            inner == "i64" || types.contains(&format!("pub type {inner} = i64;")),
            "{branches}: `x` must be the one integer every branch lowers to, not a union: {types}"
        );
    }

    // Partly shared: beside `NS: {type: [string, 'null']}`, `{maxLength: 0}` meets it to a nullable
    // string and `{type: string, maxLength: 0}` to a plain one, while `{enum: [abc]}` is a string
    // enum of its own, which neither of the others admits. The first two are one variant, and
    // `null` matches the untyped one alone, so that variant accepts it.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: { schema: { $ref: '#/components/schemas/Holder' } }
components:
  schemas:
    NS: { type: [string, 'null'] }
    Holder:
      type: object
      properties:
        x:
          $ref: '#/components/schemas/NS'
          oneOf:
            - { maxLength: 0 }
            - { type: string, maxLength: 0 }
            - { enum: [abc] }
      required: [x]
"##;
    let (report, code) = generate_with_code(spec);
    for (entry, report) in [("generate", &report), ("check", &check(spec))] {
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "via {entry}: {report:#?}"
        );
        assert!(
            report.diagnostics().iter().any(|d| {
                d.code == Code::ValidationKeywordIgnored
                    && d.pointer.as_str() == "/components/schemas/Holder/properties/x"
            }),
            "partly shared via {entry}: the merge must warn at `x`: {report:#?}"
        );
    }
    let types = types_module(&code);
    let x = field_type(&types, "pub x").unwrap_or_else(|| panic!("no `x` field: {types}"));
    let variants = enum_variants(&types, &x);
    assert_eq!(
        variants.len(),
        2,
        "the two `maxLength: 0` branches must be one variant beside the enum branch: {types}"
    );
    assert_eq!(
        variants
            .iter()
            .filter(|variant| variant.contains("(Option<"))
            .count(),
        1,
        "the merged variant accepts `null`, which only its untyped branch matches: {variants:?}"
    );
}

/// The partly shared `$ref`-sibling merge counts `null` across the whole union (#528), as the
/// all-collapse path does: the union's own `null` member and every branch of every merged set are
/// each a source, and `null` stays valid, where it was, only when exactly one source accepts it.
/// Beside `NI: {type: [integer, 'null']}`, an untyped branch meets `NI` to a nullable integer and
/// `{type: integer}` to a plain one, so `{minimum: 0}` and `{type: integer}` are one variant, while
/// an `int32` branch makes a second. Each case is `(branches, the position is Option<_>, variants that are
/// Option<_>)`:
/// - a `null` member beside a nullable merged branch is two sources, so `null` is invalid;
/// - a `null` member beside no nullable branch is one, so the position keeps it;
/// - a nullable branch in each of two sets is two, so neither variant keeps it.
#[test]
fn a_partly_shared_ref_sibling_one_of_counts_null_across_the_whole_union() {
    for (branches, position_nullable, nullable_variants) in [
        (
            "[ { type: 'null' }, { minimum: 0 }, { type: integer }, \
             { type: integer, format: int32 } ]",
            false,
            0,
        ),
        (
            "[ { type: 'null' }, { type: integer, minimum: 0 }, { type: integer }, \
             { type: integer, format: int32 } ]",
            true,
            0,
        ),
        (
            "[ { minimum: 0 }, { type: integer }, \
             { type: [integer, 'null'], format: int32 } ]",
            false,
            0,
        ),
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    NI: {{ type: [integer, 'null'] }}
    Holder:
      type: object
      properties:
        x:
          $ref: '#/components/schemas/NI'
          oneOf: {branches}
      required: [x]
"##
        );
        let (report, code) = generate_with_code(&spec);
        for (entry, report) in [("generate", &report), ("check", &check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{branches} via {entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|d| {
                    d.code == Code::ValidationKeywordIgnored
                        && d.pointer.as_str() == "/components/schemas/Holder/properties/x"
                }),
                "{branches} via {entry}: the partial merge must warn at `x`: {report:#?}"
            );
        }
        let types = types_module(&code);
        let x = field_type(&types, "pub x")
            .unwrap_or_else(|| panic!("{branches}: no `x` field: {types}"));
        assert_eq!(
            x.starts_with("Option<"),
            position_nullable,
            "{branches}: `x` is `{x}`: {types}"
        );
        let inner = x
            .strip_prefix("Option<")
            .and_then(|inner| inner.strip_suffix('>'))
            .unwrap_or(&x);
        let variants = enum_variants(&types, inner);
        assert_eq!(
            variants.len(),
            2,
            "{branches}: the two `i64` branches must be one variant beside `int32`: {types}"
        );
        assert_eq!(
            variants
                .iter()
                .filter(|variant| variant.contains("(Option<"))
                .count(),
            nullable_variants,
            "{branches}: `null` has more than one source, or its one source is the position: \
             {variants:?}"
        );
    }
}

/// A `$ref` to a union component beside a non-union sibling (`U: oneOf/anyOf [...]`,
/// `P: {$ref: U, const: x}`) is collapsed only where the meet with the sibling leaves branches
/// alike that the target kept apart (#1014). `U`'s `oneOf` branches are one generated type on
/// their own, so `U`'s lowering merges them and reports `W001` at `U`: `P` meets one `String`,
/// is the const enum, and draws no second `W001`. `U`'s `anyOf` branches are distinct aliases,
/// which an `anyOf`'s identity comparison keeps apart, and the `const` meets both to one type:
/// emitted, `P` was an enum of that one payload type twice, and now it is that type, with the
/// merge reported at `P`.
#[test]
fn a_ref_to_a_union_with_a_non_union_sibling_merges_only_what_the_meet_made_alike() {
    for (keyword, warned_at_p) in [("oneOf", false), ("anyOf", true)] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    U:
      {keyword}:
        - {{ type: string, minLength: 1 }}
        - {{ type: string, maxLength: 9 }}
    P: {{ $ref: '#/components/schemas/U', const: x }}
    Holder:
      type: object
      properties:
        p: {{ $ref: '#/components/schemas/P' }}
      required: [p]
"##
        );
        let (report, code) = generate_with_code(&spec);
        for (entry, report) in [("generate", &report), ("check", &check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{keyword} via {entry}: {report:#?}"
            );
            let merged_at = |pointer: &str| {
                report.diagnostics().iter().any(|d| {
                    d.code == Code::ValidationKeywordIgnored
                        && d.pointer.as_str() == pointer
                        && (d.message.contains("differ only in keywords")
                            || d.message.contains("same generated type"))
                })
            };
            assert_eq!(
                merged_at("/components/schemas/P"),
                warned_at_p,
                "{keyword} via {entry}: `P` reports a merge exactly when its meet made two \
                 branches alike: {report:#?}"
            );
            assert_eq!(
                merged_at("/components/schemas/U"),
                !warned_at_p,
                "{keyword} via {entry}: `U` reports the merge its own branches take: {report:#?}"
            );
        }
        let types = types_module(&code);
        assert!(
            types.contains("pub enum P "),
            "{keyword}: `P` must be the const enum: {types}"
        );
        assert_eq!(
            enum_variants(&types, "P"),
            ["X"],
            "{keyword}: `P` must be the const enum, not a union of it: {types}"
        );
    }
}

/// A `$ref` to a union beside a non-union sibling meets the sibling with each branch of the
/// target, and branches the target keeps distinct can meet to one generated type (#1014): with
/// `T: oneOf [{ type: [string, integer] }, { type: string }]`, the sibling `type: string` leaves
/// `String` twice. Emitted as two variants, every string would match both and fail exactly-one,
/// so the met branches are merged and `W001` reported at the `$ref`, as the union-sibling
/// spelling `{ $ref: C, oneOf: [...] }` merges them. Where every branch meets to one type the
/// position takes that type; where only some do, they become one variant and the others stand.
/// An `anyOf` target compares its met branches by identity, as every met `anyOf` does, which the
/// test above pins.
#[test]
fn a_ref_to_a_union_merges_the_branches_its_non_union_sibling_leaves_alike() {
    let holder = "    Holder:\n      type: object\n      properties:\n        pick: { $ref: \
                  '#/components/schemas/Pick' }\n      required: [pick]\n";
    for (target, sibling, variants) in [
        (
            "{ oneOf: [ { type: [string, integer] }, { type: string } ] }",
            "type: string",
            None,
        ),
        (
            "{ oneOf: [ { type: [string, integer] }, { type: string }, { type: boolean } ] }",
            "type: [string, boolean]",
            Some(2),
        ),
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    T: {target}\n    Pick: {{ $ref: '#/components/schemas/T', {sibling} \
                 }}\n{holder}"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        for (entry, report) in [("generate", &report), ("check", &check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{target} via {entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|d| {
                    d.code == Code::ValidationKeywordIgnored
                        && d.pointer.as_str() == "/components/schemas/Pick"
                        && d.message
                            .contains("this `$ref`'s target and its sibling keywords")
                }),
                "{target} via {entry}: the merge must be reported at `Pick`: {report:#?}"
            );
        }
        let types = types_module(&code);
        let repeated = unions_with_a_repeated_payload(&types);
        assert!(
            repeated.is_empty(),
            "{target}: a union carries one payload type twice: {repeated:#?}\n{types}"
        );
        match variants {
            None => assert!(
                types.contains("pub type Pick = String;"),
                "{target}: `Pick` must be the one type its branches met to: {types}"
            ),
            Some(count) => assert_eq!(
                enum_variants(&types, "Pick").len(),
                count,
                "{target}: the two `String` branches must be one variant: {types}"
            ),
        }
    }
}

/// The diagnostics a `$ref` whose siblings are a union and further keywords draws once those
/// keywords are met apart from the union (#538), in the `$ref` spelling's own words: typed
/// keywords the target or the union cannot meet are `E013` with the `$ref`-sibling remedy, untyped
/// keywords of a category no branch has are `W011` naming the `$ref`'s siblings, and object and
/// array keywords together beside a branch of no category are `E013` naming them too. A sibling
/// `allOf` of untyped members leaves the target's `null` to it (#562), so it survives an `anyOf`.
#[test]
fn a_ref_union_sibling_beside_other_keywords_reports_in_the_ref_spelling() {
    let spec = |site: &str| {
        format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    NB: {{ type: [object, 'null'], properties: {{ a: {{ type: string }}, b: {{ type: string }} }} }}
    Free: {{ description: anything }}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        )
    };
    let nb = "$ref: '#/components/schemas/NB'";
    let ref_sibling_remedy =
        "restructure the schema so the `$ref` target and its sibling keywords \
                              describe one representable type";
    let union_sibling_remedy = "give the sibling keywords a `type`";
    let rejected = |site: &str, expected: &str, remedy_head: &str| {
        let spec = spec(site);
        let (report, _) = generate_with_code(&spec);
        assert_eq!(report.outcome(), Outcome::Rejected, "{site}: {report:#?}");
        let found: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::AllOfIrreconcilable)
            .collect();
        assert_eq!(found.len(), 1, "{site}: {report:#?}");
        assert!(found[0].message.contains(expected), "{site}: {report:#?}");
        assert!(
            found[0]
                .remedy
                .as_deref()
                .is_some_and(|remedy| remedy.starts_with(remedy_head)),
            "{site}: {report:#?}"
        );
    };
    // Typed keywords meet the target, and the union none of whose branches meets that is `E013`.
    rejected(
        &format!(
            "{{ {nb}, type: object, properties: {{ c: {{ type: integer }} }}, oneOf: [ {{ type: \
             string }}, {{ type: integer }} ] }}"
        ),
        "the `$ref` target, this schema's own sibling keywords and the `oneOf`/`anyOf` beside \
         them all apply",
        ref_sibling_remedy,
    );
    // Typed keywords the target itself cannot meet are `E013` before the union is reached.
    rejected(
        &format!("{{ {nb}, type: string, oneOf: [ {{ minLength: 1 }}, {{ maxLength: 3 }} ] }}"),
        "the `$ref` target and this schema's own sibling keywords have an empty or \
         unrepresentable intersection",
        ref_sibling_remedy,
    );
    // Object and array keywords together beside a branch that states no category.
    rejected(
        "{ $ref: '#/components/schemas/Free', required: [c], items: { type: integer }, oneOf: [ \
         {}, { type: string } ] }",
        "a branch of the union beside this `$ref` states no JSON category, and this `$ref`'s \
         sibling untyped keywords are both object keywords and array keywords",
        union_sibling_remedy,
    );

    // Untyped array keywords beside object branches refine none of them (`W011`), and the met
    // branches, alike, collapse with `W001`. `null` matches both branches, so the `oneOf` rejects
    // it.
    let spec_text = spec(&format!(
        "{{ {nb}, items: {{ type: string }}, oneOf: [ {{ required: [a] }}, {{ required: [a] }} ] }}"
    ));
    let (report, code) = generate_with_code(&spec_text);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let unreached = messages_for(&report, Code::DeclarationHasNoEffect);
    assert_eq!(unreached.len(), 1, "{report:#?}");
    assert!(
        unreached[0].starts_with(
            "this `$ref`'s sibling untyped array keywords (`items`, `prefixItems`) constrain only \
             the instances of their own category, and no branch of the union beside the `$ref` \
             has that category"
        ),
        "{unreached:?}"
    );
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    let types = types_module(&code);
    assert_eq!(declared_fields(&types, "Pick"), ["a", "b"], "{types}");
    assert_eq!(
        field_type(&types, "pub pick").as_deref(),
        Some("Pick"),
        "{types}"
    );

    // A sibling `allOf` of untyped members meets the target as the other keywords do, admitting
    // its `null` without deciding it (#562), so the `anyOf`, whose branches all accept `null`,
    // keeps it once its branches collapse.
    let spec_text = spec(&format!(
        "{{ {nb}, allOf: [ {{ required: [a] }} ], anyOf: [ {{}}, {{}} ] }}"
    ));
    let (report, code) = generate_with_code(&spec_text);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::ValidationKeywordIgnored),
        "{report:#?}"
    );
    let types = types_module(&code);
    assert_eq!(declared_fields(&types, "Pick"), ["a", "b"], "{types}");
    assert_eq!(
        field_type(&types, "pub pick").as_deref(),
        Some("Option<Pick>"),
        "a nested `allOf` beside a `$ref`'s `anyOf` denied the target's `null`: {types}"
    );
}

/// Issue #567: a `$ref` whose siblings are a `type` that admits `null` and a `oneOf`/`anyOf` of
/// untyped object branches admits `null` as JSON Schema does. The split
/// (`split_union_sibling`) leaves the `type` with the keywords beside the union, so the union's
/// untyped branches lowered as non-null structs and the meet denied the `null` the target, the
/// `type` and every branch accept, where the `allOf`-member and inline spellings admit it. A union
/// held back for a meet now counts such a branch as accepting `null` wherever the meet admits it:
/// an `anyOf` keeps `null`, a `oneOf` two such branches share rejects it, and so does one beside a
/// `null` branch. Each row is written over a typed nullable target (`N`) and an untyped one (`U`,
/// which admits `null` beside a sibling `type` naming `object`, #566); a sibling `type: object`
/// keeps `null` out. The same count reaches a `$ref`, or an `allOf`, whose union has no `type`
/// beside it at all, over the nullable `N`, and a plain union's sole untyped branch beside a `null`
/// branch, which takes the nullable `type`'s `null` as an untyped `Value` branch does (#563).
///
/// The same `oneOf` written with the nullable `type` in the union's own schema (the
/// `allOf`-member and inline spellings, and a plain `{type: [object, 'null'], oneOf: [...]}`)
/// rejects `null` as well, and the plain `anyOf` keeps it (#579): the untyped branches take that
/// `type`'s `null` themselves (#574), so `null` matches both and fails exactly-one.
#[test]
fn a_ref_union_sibling_admits_the_null_its_sibling_type_admits() {
    let branches =
        "[ { properties: { a: { type: string } } }, { properties: { b: { type: string } \
                    } } ]";
    let beside_null = "[ { type: 'null' }, { properties: { a: { type: string } } } ]";
    let nullable = "[object, 'null']";
    let mut rows: Vec<(&str, String, bool)> = Vec::new();
    for target in ["N", "U"] {
        let target_ref = format!("$ref: '#/components/schemas/{target}'");
        for (ty, admits) in [(nullable, true), ("object", false)] {
            rows.push((
                target,
                format!("{{ {target_ref}, type: {ty}, anyOf: {branches} }}"),
                admits,
            ));
            rows.push((
                target,
                format!("{{ allOf: [ {{ {target_ref} }}, {{ type: {ty}, anyOf: {branches} }} ] }}"),
                admits,
            ));
            rows.push((
                target,
                format!(
                    "{{ type: {ty}, properties: {{ u: {{ type: string }} }}, anyOf: {branches} }}"
                ),
                admits,
            ));
            rows.push((
                target,
                format!("{{ {target_ref}, type: {ty}, oneOf: {branches} }}"),
                false,
            ));
            // The same `oneOf` with the `type` in the union's own schema (#579): `null` matches
            // both untyped branches there too, so the `oneOf` rejects it in every spelling.
            rows.push((
                target,
                format!("{{ allOf: [ {{ {target_ref} }}, {{ type: {ty}, oneOf: {branches} }} ] }}"),
                false,
            ));
            rows.push((
                target,
                format!(
                    "{{ type: {ty}, properties: {{ u: {{ type: string }} }}, oneOf: {branches} }}"
                ),
                false,
            ));
            rows.push((
                target,
                format!("{{ type: {ty}, anyOf: {branches} }}"),
                admits,
            ));
            rows.push((
                target,
                format!("{{ type: {ty}, oneOf: {branches} }}"),
                false,
            ));
            rows.push((
                target,
                format!("{{ {target_ref}, type: {ty}, anyOf: {beside_null} }}"),
                admits,
            ));
            rows.push((
                target,
                format!("{{ {target_ref}, type: {ty}, oneOf: {beside_null} }}"),
                false,
            ));
        }
    }
    let n = "$ref: '#/components/schemas/N'";
    for (keyword, admits) in [("anyOf", true), ("oneOf", false)] {
        rows.push((
            "N",
            format!("{{ type: {nullable}, {keyword}: {beside_null} }}"),
            admits,
        ));
        rows.push(("N", format!("{{ {n}, {keyword}: {branches} }}"), admits));
        rows.push((
            "N",
            format!("{{ allOf: [ {{ {n} }}, {{ {keyword}: {branches} }} ] }}"),
            admits,
        ));
    }
    // A `oneOf` of one untyped branch beside a branch that denies `null`, over the nullable `N`:
    // `null` is in the untyped branch alone, so exactly one branch takes it.
    for other in ["{ type: object }", "{ type: string }"] {
        let one_untyped = format!("[ {{ properties: {{ a: {{ type: string }} }} }}, {other} ]");
        rows.push(("N", format!("{{ {n}, oneOf: {one_untyped} }}"), true));
        rows.push((
            "N",
            format!("{{ allOf: [ {{ {n} }}, {{ oneOf: {one_untyped} }} ] }}"),
            true,
        ));
    }
    // Nothing in these compositions admits `null` by a `type`: the conjuncts are untyped objects
    // alone, or the untyped `U`, so every spelling keeps the non-null struct an untyped object
    // lowers to, and the held-back union's untyped branches take no `null` from the meet.
    let c = "{ properties: { c: { type: string } } }";
    let u = "$ref: '#/components/schemas/U'";
    for site in [
        format!("{{ allOf: [ {c} ], anyOf: {branches} }}"),
        format!("{{ allOf: [ {c}, {{ anyOf: {branches} }} ] }}"),
        format!("{{ properties: {{ c: {{ type: string }} }}, anyOf: {branches} }}"),
        format!("{{ {u}, anyOf: {branches} }}"),
        format!("{{ allOf: [ {{ {u} }} ], anyOf: {branches} }}"),
        format!("{{ allOf: [ {{ {u} }}, {{ anyOf: {branches} }} ] }}"),
    ] {
        rows.push(("U", site, false));
    }
    // Here the `null` branch admits `null` itself, and the untyped branch beside it is counted as
    // accepting it too (#563), so `null` is in two branches and fails exactly-one.
    rows.push((
        "U",
        format!("{{ allOf: [ {c} ], oneOf: {beside_null} }}"),
        false,
    ));
    let mut mismatches = Vec::new();
    for (target, site, admits) in rows {
        let target_body = if target == "N" {
            "{ type: [object, 'null'], properties: { u: { type: string } } }"
        } else {
            "{ properties: { u: { type: string } } }"
        };
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /p:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Holder' }} }}
components:
  schemas:
    {target}: {target_body}
    Pick: {site}
    Holder:
      type: object
      properties:
        pick: {{ $ref: '#/components/schemas/Pick' }}
      required: [pick]
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{target}: {site}: {report:#?}"
        );
        let types = types_module(&code);
        let pick = field_type(&types, "pub pick")
            .unwrap_or_else(|| panic!("{target}: {site}: no `pick` field: {types}"));
        if pick.starts_with("Option<") != admits {
            let validity = if admits { "valid" } else { "invalid" };
            mismatches.push(format!(
                "{target}: {site}: `pick` is `{pick}`, but `null` is {validity} here"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A `oneOf`/`anyOf` member written `{ $ref: Name, <union> }` is a `$ref` with a union sibling like
/// any other, so `lower_union_variant` lowers it through the same `$ref`-sibling intersection
/// rather than as the bare target. A union sibling that contradicts the target is `E013` at the
/// member; one that narrows it gives the member the narrowed type. Before the shape gate counted
/// the union keywords the member took the bare-target exit, so the first generated with the member
/// typed `String` and the second with no narrowing, both with no diagnostic.
#[test]
fn a_union_member_ref_with_a_union_sibling_is_intersected_with_its_target() {
    for outer in ["oneOf", "anyOf"] {
        for keyword in ["oneOf", "anyOf"] {
            let contradicting = single_component_document(&format!(
                "      {outer}:\n        - $ref: '#/components/schemas/Name'\n          {keyword}: \
                 [{{ type: integer }}]\n        - type: boolean\n"
            ));
            let pointer = format!("/components/schemas/U/{outer}/0");
            for (entry, report) in [
                ("generate", generate(&contradicting)),
                ("check", check(&contradicting)),
            ] {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{keyword} beside a {outer} member via {entry}: {report:#?}\n{contradicting}"
                );
                assert!(
                    report.diagnostics().iter().any(|d| {
                        d.code == Code::AllOfIrreconcilable && d.pointer.as_str() == pointer
                    }),
                    "{keyword} beside a {outer} member via {entry}: E013 must point at \
                     `{pointer}`: {report:#?}"
                );
            }

            let narrowing = single_component_document(&format!(
                "      {outer}:\n        - $ref: '#/components/schemas/Name'\n          {keyword}: \
                 [{{ type: string, enum: [red, green] }}, {{ type: integer }}]\n        - type: \
                 boolean\n"
            ));
            let (report, code) = generate_with_code(&narrowing);
            for (entry, report) in [("generate", &report), ("check", &check(&narrowing))] {
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{keyword} beside a {outer} member via {entry}: {report:#?}\n{narrowing}"
                );
                // The integer branch `Name` excludes is reported (#671).
                assert_eq!(
                    messages_for(report, Code::DeclarationHasNoEffect),
                    [EXCLUDED_MEMBER_1],
                    "{keyword} beside a {outer} member via {entry}: {report:#?}"
                );
                assert_eq!(
                    codes(report),
                    ["W011"],
                    "{keyword} beside a {outer} member via {entry}: {report:#?}"
                );
            }
            let types = types_module(&code);
            let variants = enum_variants(&types, "U");
            let payload = variants
                .iter()
                .find_map(|variant| {
                    variant
                        .strip_prefix("Name(")
                        .and_then(|rest| rest.strip_suffix(')'))
                        .map(|ty| {
                            ty.strip_prefix("Box<")
                                .and_then(|ty| ty.strip_suffix('>'))
                                .unwrap_or(ty)
                        })
                })
                .unwrap_or_else(|| {
                    panic!("{keyword} beside a {outer} member: no `Name` variant: {types}")
                });
            assert_eq!(
                enum_variants(&types, payload),
                ["Red", "Green"],
                "{keyword} beside a {outer} member: the member must be the enum branch alone: \
                 {types}"
            );
        }
    }
}

/// An `allOf` member written `{ $ref: X, <union> }` gathers its union sibling as a further
/// conjunct, as `gather_member` does for every shape-bearing sibling of a member's `$ref`. That
/// conjunct is a union, which the object merge does not intersect with object members: it is the
/// object/scalar-mix `E013`. The separate-member spelling `allOf: [{$ref: Base}, {oneOf: [...]}]`
/// no longer is: it is met as the `$ref` spelling is (#463, pinned by
/// `an_all_of_union_member_is_met_with_the_other_members_as_a_ref_sibling_union_is`). A union
/// written beside a member's own `$ref` is not a union member of the `allOf`, so this spelling
/// still takes the rejection rather than that meet. Before the shape
/// gate counted the union keywords both documents below generated as the bare target with the
/// union dropped in silence: the object one as `Base` merged with `c`, the scalar one as `String`
/// for a union that admits no string at all.
#[test]
fn an_all_of_member_ref_with_a_union_sibling_is_not_dropped() {
    for keyword in ["oneOf", "anyOf"] {
        let beside_object = single_component_document(&format!(
            "      allOf:\n        - $ref: '#/components/schemas/Base'\n          {keyword}: [{{ \
             required: [a] }}, {{ required: [b] }}]\n        - type: object\n          \
             properties:\n            c: {{ type: string }}\n"
        ));
        let contradicting = single_component_document(&format!(
            "      allOf:\n        - $ref: '#/components/schemas/Name'\n          {keyword}: [{{ \
             type: integer }}]\n"
        ));
        for (shape, spec) in [
            ("beside an object member", beside_object),
            ("contradicting its target", contradicting),
        ] {
            for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{keyword} {shape} via {entry}: the member's union must not be dropped: \
                     {report:#?}\n{spec}"
                );
                assert!(
                    report.diagnostics().iter().any(|d| {
                        d.code == Code::AllOfIrreconcilable
                            && d.pointer.as_str().starts_with("/components/schemas/U")
                    }),
                    "{keyword} {shape} via {entry}: E013 must point into `U`: {report:#?}"
                );
            }
        }
    }
}

/// Each union `types` declares two of whose variants carry the same payload type, with its
/// variants.
fn unions_with_a_repeated_payload(types: &str) -> Vec<String> {
    types
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("pub enum "))
        .filter_map(|rest| rest.split([' ', '<', '{']).next())
        .filter_map(|name| {
            let variants = enum_variants(types, name);
            let payloads: Vec<&str> = variants
                .iter()
                .filter_map(|variant| variant.split_once('(').map(|(_, payload)| payload))
                .collect();
            let repeated = payloads
                .iter()
                .enumerate()
                .any(|(at, payload)| payloads[at + 1..].contains(payload));
            repeated.then(|| format!("{name}: {variants:?}"))
        })
        .collect()
}

/// A `$ref` to a target of several `type`s meets its union sibling once per type the target
/// allows, which nests the sibling's union under each of the target's variants (#998). A nested
/// union whose branches that one type leaves alike is merged with `W001`, as the top-level union
/// is: here `{ type: [string, integer] }` and `{ type: string }` both meet the string variant as
/// `String`. Before, the string variant was a `oneOf` of two `String` aliases that rejected every
/// string as matching both, with no warning, and the `anyOf` spelling an enum of one payload type
/// twice. The same conjunction is written as a `$ref` with a shape sibling beside the union, and
/// as both `allOf` spellings, which reach the same meet through a second site.
#[test]
fn a_ref_to_a_multi_type_target_merges_the_branches_one_target_type_leaves_alike() {
    let target = "    C: { type: [string, integer, 'null'] }\n";
    let holder = "    Holder:\n      type: object\n      properties:\n        pick: { $ref: \
                  '#/components/schemas/Pick' }\n      required: [pick]\n";
    for (keyword, branches) in [
        ("oneOf", "{ type: [string, integer] }, { type: string }"),
        ("oneOf", "{ type: [string, integer] }, {}"),
        ("anyOf", "{}, { type: number }"),
    ] {
        let union = format!("{keyword}: [ {branches} ]");
        for (spelling, pick) in [
            (
                "a `$ref` beside the union",
                format!("{{ $ref: '#/components/schemas/C', {union} }}"),
            ),
            (
                "a `$ref` and a shape keyword beside the union",
                format!("{{ $ref: '#/components/schemas/C', maxLength: 64, {union} }}"),
            ),
            (
                "an `allOf` beside the union",
                format!("{{ allOf: [ {{ $ref: '#/components/schemas/C' }} ], {union} }}"),
            ),
        ] {
            let spec = with_schemas("3.1.0", &format!("{target}    Pick: {pick}\n{holder}"));
            let (report, code) = generate_with_code(&spec);
            assert_eq!(
                report.outcome(),
                Outcome::Generated,
                "{union} as {spelling}: {report:#?}"
            );
            let types = types_module(&code);
            let repeated = unions_with_a_repeated_payload(&types);
            assert!(
                repeated.is_empty(),
                "{union} as {spelling}: a union carries one payload type twice: \
                 {repeated:#?}\n{types}"
            );
            let hashed = hash_suffixed_types(&types);
            assert!(
                hashed.is_empty(),
                "{union} as {spelling}: a collapsed union lost its name: {hashed:?}\n{types}"
            );
            assert!(
                messages_for(&report, Code::ValidationKeywordIgnored)
                    .iter()
                    .any(|message| message.contains("differ only in keywords")),
                "{union} as {spelling}: the merge must be reported: {report:#?}"
            );
        }
    }
}

/// The other conjunct #998 named, `{ enum: [a, b, null] }` as a `$ref` target beside a `oneOf` or
/// `anyOf`, is one string enum rather than a union of target variants, so the sibling's union is
/// met once and nothing is nested: no union it generates carries a payload type twice. Pinned
/// across branches the enum keeps whole, keeps in part, keeps as `null` alone, or excludes.
#[test]
fn a_ref_to_a_nullable_enum_target_beside_a_union_repeats_no_payload_type() {
    let target = "    C: { enum: [a, b, null] }\n";
    let holder = "    Holder:\n      type: object\n      properties:\n        pick: { $ref: \
                  '#/components/schemas/Pick' }\n      required: [pick]\n";
    let branches = [
        "{ type: string }",
        "{ enum: [a] }",
        "{ enum: [a, b] }",
        "{ enum: [b, null] }",
        "{ const: a }",
        "{ type: 'null' }",
        "{}",
    ];
    for keyword in ["oneOf", "anyOf"] {
        for first in branches {
            for second in branches {
                let pick = format!(
                    "{{ $ref: '#/components/schemas/C', {keyword}: [ {first}, {second} ] }}"
                );
                let spec = with_schemas("3.1.0", &format!("{target}    Pick: {pick}\n{holder}"));
                let (report, code) = generate_with_code(&spec);
                let repeated = unions_with_a_repeated_payload(&types_module(&code));
                assert!(
                    repeated.is_empty(),
                    "{pick}: a union carries one payload type twice: {repeated:#?}\n{report:#?}"
                );
            }
        }
    }
}

/// A nested union the meet collapses keeps the name it has uncollapsed (#998), both where a
/// collapse one level further in rewrites its variants and where it merges only some of its own
/// branches. Each was emitted under a hash-suffixed name beside the definition it replaced, which
/// nothing emits but which kept the bare name.
///
/// In the first, `C` is an `anyOf` of two multi-type branches, so the `oneOf` sibling is met per
/// branch of `C` and then per `type` of that branch: two levels of nesting. In `C`'s first branch
/// the string variant meets both `oneOf` branches as `String`, so the innermost union collapses
/// first, to `String`, and that rewrites the union holding it, whose string and integer variants
/// stay. In the second, `C`'s object variant meets the first two `oneOf` branches as one struct
/// shape and the third as another, so two of its three branches merge.
#[test]
fn a_nested_union_the_meet_collapses_keeps_its_own_name() {
    let holder = "    Holder:\n      type: object\n      properties:\n        pick: { $ref: \
                  '#/components/schemas/Pick' }\n      required: [pick]\n";
    for (target, branches, collapsed, variants) in [
        (
            "{ anyOf: [ { type: [string, integer] }, { type: [boolean, object] } ] }",
            "[ { type: [string, integer, object], properties: { a: { type: string } } }, \
             { type: [string, object], properties: { a: { type: string } } } ]",
            "PickReferenceIntersectionVariant0",
            2,
        ),
        (
            "{ type: [string, object] }",
            "[ { type: [string, object], properties: { a: { type: string } } }, \
             { type: [integer, object], properties: { a: { type: string } } }, \
             { type: [string, object], properties: { b: { type: integer } }, required: [b] } ]",
            "PickReferenceIntersectionVariant1",
            2,
        ),
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    C: {target}\n    Pick: {{ $ref: '#/components/schemas/C', oneOf: \
                 {branches} }}\n{holder}"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{target}: {report:#?}"
        );
        let types = types_module(&code);
        let hashed = hash_suffixed_types(&types);
        assert!(
            hashed.is_empty(),
            "{target}: a collapsed union lost its name: {hashed:?}\n{types}"
        );
        assert_eq!(
            enum_variants(&types, collapsed).len(),
            variants,
            "{target}: `{collapsed}` is the collapsed union\n{types}"
        );
        let repeated = unions_with_a_repeated_payload(&types);
        assert!(
            repeated.is_empty(),
            "{target}: a union carries one payload type twice: {repeated:#?}\n{types}"
        );
        assert!(
            messages_for(&report, Code::ValidationKeywordIgnored)
                .iter()
                .any(|message| message.contains("same generated type")
                    || message.contains("differ only in keywords")),
            "{target}: the merge must be reported: {report:#?}"
        );
    }
}

/// The types `types` declares whose name ends in a disambiguating hash: a run of six or more
/// digits, which no hint in these fixtures ends in.
fn hash_suffixed_types(types: &str) -> Vec<String> {
    types
        .lines()
        .map(str::trim_start)
        .filter_map(|line| {
            ["pub enum ", "pub struct ", "pub type "]
                .iter()
                .find_map(|head| line.strip_prefix(head))
        })
        .filter_map(|rest| rest.split([' ', '<', '{', '(', ';', '=']).next())
        .filter(|name| name.chars().rev().take_while(char::is_ascii_digit).count() >= 6)
        .map(str::to_owned)
        .collect()
}
