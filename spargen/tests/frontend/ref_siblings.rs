//! A `$ref` beside sibling keywords: intersection with its target, union siblings, and the
//! rejections a contradiction draws.

use super::*;

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
fn ref_union_sibling_documents(site: &str) -> Vec<(&'static str, String, &'static str)> {
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
            assert!(
                report.diagnostics().is_empty(),
                "{keyword} in a {position}: {report:#?}"
            );
            let checked = check(&spec);
            assert!(
                checked.diagnostics().is_empty(),
                "{keyword} in a {position} via check: {checked:#?}"
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

/// The collapse above is reserved for a `$ref` whose own sibling is a `oneOf`/`anyOf`. A `$ref` to a
/// union component beside a non-union sibling (`U: anyOf[...]`, `P: {$ref: U, const: x}`) is an
/// intersection this change does not touch: its branches may intersect to one type, but it must
/// keep generating exactly what it did before — no `W001` at the `$ref`, and the same union shape.
#[test]
fn a_ref_to_a_union_with_a_non_union_sibling_is_not_collapsed() {
    for keyword in ["oneOf", "anyOf"] {
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
            assert!(
                !report.diagnostics().iter().any(|d| {
                    d.code == Code::ValidationKeywordIgnored
                        && d.pointer.as_str() == "/components/schemas/P"
                }),
                "{keyword} via {entry}: a `$ref` with no union sibling must not be collapsed or \
                 warned about: {report:#?}"
            );
        }
        let types = types_module(&code);
        assert!(
            types.contains("pub enum P "),
            "{keyword}: `P` must keep the union shape it generated before: {types}"
        );
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
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{target}: {site}, via {entry}: {report:#?}"
            );
        }
        let (_, code) = generate_with_code(&spec);
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
                assert!(
                    report.diagnostics().is_empty(),
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

/// The control: `not` beside a `$ref` was never silent — it is validation-only and says so with
/// `W001` — and admitting the union keywords to the shape gate must leave it exactly that, in every
/// position, with the position still typed as the target.
#[test]
fn a_ref_with_a_not_sibling_still_reports_it_as_validation_only() {
    let site = "{ $ref: '#/components/schemas/Name', not: { enum: [forbidden] } }";
    for (position, spec, _) in ref_union_sibling_documents(site) {
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{position} via {entry}: {report:#?}"
            );
            assert!(
                has_code(&report, Code::ValidationKeywordIgnored),
                "{position} via {entry}: `not` must still be reported: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::AllOfIrreconcilable),
                "{position} via {entry}: {report:#?}"
            );
        }
    }
}

/// The negative control for the rejection above: this change turns a previously-silent success
/// into a rejection, so what it must NOT do is reject a `$ref` that resolves. A `$ref` carrying
/// shape siblings is the narrow case — it is the one construct that reaches `ensure_component`
/// through `lower_schema_inner`, and it is an intersection, so its target contributes fields rather
/// than replacing it. Both sides must survive into the generated type.
#[test]
fn a_ref_with_shape_siblings_that_resolves_is_not_rejected() {
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
            application/json: { schema: { $ref: '#/components/schemas/Ext' } }
components:
  schemas:
    Ext:
      $ref: '#/components/schemas/Base'
      type: object
      properties: { extra: { type: string } }
    Base:
      type: object
      properties: { id: { type: string } }
      required: [id]
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::UnresolvedRef), "{report:#?}");
    // The reference was genuinely followed, not merely tolerated: the sibling's own property and
    // the referenced component's property are both present.
    assert!(code.contains("pub extra"), "{code}");
    assert!(code.contains("pub id"), "{code}");

    // check/generate parity on the clean path too.
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "{checked:#?}");
    assert!(!has_code(&checked, Code::UnresolvedRef), "{checked:#?}");
}

/// A `oneOf`/`anyOf` member written `{$ref: '#/components/schemas/Name', type: integer}` is the
/// conjunction `Name ∩ integer` — in 2020-12 `$ref` is an applicator — exactly as the same schema is
/// at a body or property position. With `Name` a string, nothing satisfies it.
///
/// The multi-member union used to lower a root-component member through its `$ref` string alone,
/// so the member's own `type` was discarded and the run came back clean with the branch typed as
/// the plain target (#279). The sole-member collapse and every other `$ref` spelling already
/// intersected it; the verdict is now the one they give, `E013` at the member.
#[test]
fn a_component_ref_union_member_whose_siblings_contradict_its_target_is_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /p:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                oneOf:
                  - { $ref: '#/components/schemas/Name', type: integer }
                  - { type: boolean }
components:
  schemas:
    Name: { type: string }
"##;
    let pointer = "/paths/~1p/get/responses/200/content/application~1json/schema/oneOf/0";
    for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
        assert_eq!(
            report.outcome(),
            Outcome::Rejected,
            "{entry}: the member's `type: integer` must not be dropped in silence: {report:#?}"
        );
        let e013: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::AllOfIrreconcilable)
            .collect();
        assert!(
            e013.iter().any(|d| d.pointer.as_str() == pointer),
            "{entry}: E013 must point at the member `{pointer}`, not at {:?}\n{report:#?}",
            e013.iter().map(|d| d.pointer.as_str()).collect::<Vec<_>>()
        );
    }
}

/// The satisfiable counterpart: `{$ref: Pet, properties: {owner: …}, required: [owner]}` narrows
/// `Pet` to the pets that carry an `owner`. The branch must be that narrowed type — `Pet`'s fields
/// plus a required `owner` — while still taking its variant name and its implicit discriminator tag
/// from the component it references, since the member is still written as that component.
#[test]
fn a_component_ref_union_member_with_siblings_is_narrowed_and_keeps_its_component_name() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Pet:
      type: object
      properties:
        kind: { type: string }
        name: { type: string }
    Dog:
      type: object
      required: [kind, bark]
      properties:
        kind: { type: string }
        bark: { type: boolean }
    Animal:
      oneOf:
        - $ref: '#/components/schemas/Pet'
          properties: { owner: { type: string } }
          required: [owner]
        - $ref: '#/components/schemas/Dog'
      discriminator:
        propertyName: kind
"##;
    let checked = check(spec);
    assert_ne!(checked.outcome(), Outcome::Rejected, "check: {checked:#?}");
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    let variants = enum_variants(&types, "Animal");
    let pet = variants
        .iter()
        .find_map(|variant| variant.strip_prefix("Pet(")?.strip_suffix(')'))
        .map(|payload| {
            payload
                .strip_prefix("Box<")
                .and_then(|inner| inner.strip_suffix('>'))
                .unwrap_or(payload)
        })
        .unwrap_or_else(|| panic!("the member lost its component name, got {variants:?}: {types}"));
    assert_ne!(
        pet, "Pet",
        "the branch is the plain target, so its siblings were discarded: {types}"
    );
    let mut fields = declared_fields(&types, pet);
    fields.sort();
    assert_eq!(
        fields,
        ["kind", "name", "owner"],
        "the narrowed branch must carry Pet's properties and the member's own: {types}"
    );
    let owner = types
        .split(&format!("pub struct {pet} "))
        .nth(1)
        .and_then(|body| field_type(body, "pub owner"));
    assert!(
        owner
            .as_deref()
            .is_some_and(|ty| !ty.starts_with("Option<")),
        "the member's `required: [owner]` must make `owner` required, got {owner:?}: {types}"
    );
    assert!(
        types.contains("\"Pet\""),
        "the implicit discriminator tag must stay the component name: {types}"
    );
}

/// In JSON Schema 2020-12 `$ref` is an applicator, so a `$ref`'s shape-bearing siblings are
/// intersected with the referenced schema rather than discarded. When that intersection is empty no
/// value can satisfy the schema, which is a document error the author must hear about: before this
/// was pinned, `spargen check` reported `clean` and the construct simply vanished — a request body
/// whose method then took no body argument at all. Every construct that reaches the `$ref` arm of
/// `LowerCtx::lower_schema_inner` must report `E013`, through both `generate` and `check`.
#[test]
fn e013_fires_when_a_ref_sibling_contradicts_its_target() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    // `Name` is a string; every site below intersects it with `type: integer`, which is empty.
    const TAIL: &str = "components:\n  schemas:\n    Name: { type: string }\n";

    // The issue's exact reproduction: the body vanished and `upload` lost its body argument.
    let request_body = format!(
        "{HEAD}{}{TAIL}",
        r##"paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: '#/components/schemas/Name', type: integer }
      responses: { '204': { description: ok } }
"##
    );
    let response_body = format!(
        "{HEAD}{}{TAIL}",
        r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Name', type: integer }
"##
    );
    let parameter = format!(
        "{HEAD}{}{TAIL}",
        r##"paths:
  /u:
    get:
      operationId: fetch
      parameters:
        - name: filter
          in: query
          schema: { $ref: '#/components/schemas/Name', type: integer }
      responses: { '204': { description: ok } }
"##
    );
    // A component property, which reaches the same arm through `object_body`/`ensure_component`.
    let component_property = format!(
        "{HEAD}paths: {{}}\n{}",
        r##"components:
  schemas:
    Name: { type: string }
    Holder:
      type: object
      properties:
        field: { $ref: '#/components/schemas/Name', type: integer }
      required: [field]
"##
    );

    // The pointer is not decoration: `compat::carve_rules` maps it to the smallest omittable
    // construct, so a diagnostic carrying the document root instead of the offending node yields no
    // carve rule and turns a carvable rejection into an un-carvable residual. Each site therefore
    // pins the exact pointer it must produce, and `carve.rs` proves the consequence end to end.
    for (site, spec, pointer) in [
        (
            "request body",
            &request_body,
            "/paths/~1u/post/requestBody/content/application~1json/schema",
        ),
        (
            "response body",
            &response_body,
            "/paths/~1u/get/responses/200/content/application~1json/schema",
        ),
        (
            "parameter",
            &parameter,
            "/paths/~1u/get/parameters/0/schema",
        ),
        (
            "component property",
            &component_property,
            "/components/schemas/Holder/properties/field",
        ),
    ] {
        for (entry, report) in [("generate", generate(spec)), ("check", check(spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{site}` was not rejected by {entry}: {report:#?}"
            );
            let pointers: Vec<&str> = report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::AllOfIrreconcilable)
                .map(|d| d.pointer.as_str())
                .collect();
            assert_eq!(
                pointers,
                vec![pointer],
                "`{site}` through {entry} must report E013 once, at the offending schema: \
                 {report:#?}"
            );
        }
    }
}

/// `intersect_types` fails for TWO conditions: the intersection is empty (`NoMeet::Empty`), so no
/// value satisfies both sides, and the intersection is inhabited but has no single Rust type
/// (`NoMeet::Unrepresentable` — `Bytes` against a `uuid` or date string, say). The emitted message must
/// not claim the first when it may be the second: `{$ref: Id, contentEncoding: base64}` over a
/// `uuid` string `Id` is satisfied by a base64 UUID, and the `E013` explain and the `allOf` scalar
/// site both already say "empty or unrepresentable". This pins the site to the same honest wording. It asserts
/// what the message may NOT say as well as what it must, because the defect this replaced was a
/// message that named the wrong one of the two.
#[test]
fn the_ref_sibling_rejection_does_not_claim_more_than_it_knows() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths: {}
components:
  schemas:
    Name: { type: string }
    Holder:
      type: object
      properties:
        field: { $ref: '#/components/schemas/Name', type: integer }
      required: [field]
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let messages = messages_for(&report, Code::AllOfIrreconcilable);
    assert_eq!(messages.len(), 1, "{report:#?}");
    assert!(
        messages[0].contains("empty or unrepresentable intersection"),
        "the site must use the same wording as the `allOf` scalar site and the E013 explain: {:?}",
        messages[0]
    );
    // `None` is not proof of emptiness, so the message may not assert unsatisfiability.
    assert!(
        !messages[0].contains("no value can satisfy"),
        "the message asserts unsatisfiability, which `intersect_types` returning `None` does not \
         establish: {:?}",
        messages[0]
    );
    // The remedy is the author's only route out of a rejection, so it is pinned too: it must name
    // the construct and offer the omit escape the taxonomy promises.
    let remedy = report
        .diagnostics()
        .iter()
        .find(|d| d.code == Code::AllOfIrreconcilable)
        .and_then(|d| d.remedy.clone())
        .unwrap_or_default();
    assert!(remedy.contains("`$ref` target"), "{remedy:?}");
    assert!(remedy.contains("spargen::omit!"), "{remedy:?}");
}

/// The shape-bearing sibling keywords `E013`'s explain names, READ OUT of the published text
/// rather than copied into the fixture beside it. Two independent lists cannot pin each other: a
/// keyword deleted from the prose simply disappears, and a hard-coded copy goes on passing.
fn shape_bearing_keywords_the_explain_names() -> Vec<String> {
    const LEAD: &str = "A sibling bears a shape of its own through ";
    let explain = Code::AllOfIrreconcilable.explain();
    let start = explain.find(LEAD).unwrap_or_else(|| {
        panic!("E013's explain no longer enumerates its shape-bearing sibling keywords: {explain}")
    }) + LEAD.len();
    let sentence = &explain[start..];
    let sentence = &sentence[..sentence
        .find(". ")
        .unwrap_or_else(|| panic!("E013's shape-bearing sentence never ends: {sentence}"))];
    // The keywords are the backticked spans of that one sentence.
    sentence
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// `E013`'s explain names the sibling keywords intersected with a `$ref`'s target rather than
/// discarded. That is a published promise, held to the code at two levels. Which keywords the
/// private gate `schema_has_shape_constraint` reads is pinned in-crate: its `SHAPE_KEYWORDS` table
/// and this explain name one set, in both directions (`oas31::lower`'s tests). What this fixture
/// adds is behaviour, which the table cannot show: a sibling that clears the gate but lowers to
/// `TypeKind::Any` intersects as identity and is discarded anyway.
///
/// The list under test is DERIVED from `explain()`, not repeated here, so deleting a keyword from
/// the prose fails this fixture rather than quietly shrinking what it checks. And each row is a
/// DIFFERENTIAL — the same document with and without the keyword — so the keyword is what flips the
/// outcome. An earlier revision paired several keywords with a `type` against a target of another
/// category, where the `type` alone already rejected and the keyword beside it was never
/// load-bearing; three rows proved nothing at all.
///
/// The `$ref` sits at a response body rather than a component root, so each row exercises the
/// sibling gate alone. A component root whose value is a `$ref` with only non-shape-bearing
/// siblings once tripped a release-level `assert_eq!` inside `ensure_component` (#148), which
/// turned every "without" control into an opaque panic instead of this fixture's own message.
///
/// Each target is chosen so the intersection is *genuinely empty*, never merely unrepresentable:
/// the `contentEncoding`/`format: binary` rows sit against an integer, not a string, because a
/// plain string target intersects to `bytes::Bytes` and generates (see
/// `a_binary_string_lowers_to_bytes_however_the_binary_constraint_is_spelled`).
#[test]
fn every_sibling_keyword_the_explain_names_is_actually_intersected() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const BODY: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Target'
SIBLING
"##;

    // (keyword, the target it must contradict, the sibling spelling of that keyword)
    let cases: &[(&str, &str, &str)] = &[
        ("type", "{ type: string }", "type: integer"),
        // `properties` alone, with no `type` beside it to account for the rejection. The target
        // REQUIRES `a`, so the conflicting property genuinely empties the composition — without
        // that, an optional conflicting property is representable and correctly generates.
        (
            "properties",
            "{ type: object, required: [a], properties: { a: { type: string } } }",
            "properties: { a: { type: integer } }",
        ),
        (
            "patternProperties",
            "{ type: string }",
            "patternProperties: { '^a': { type: string } }",
        ),
        // The four refining keywords, each ALONE: no `type`, no `properties`. They used to clear
        // `schema_has_shape_constraint` and then lower to `TypeKind::Any`, which intersects as
        // identity, so each was discarded with no diagnostic (#140). Each now establishes the
        // category it applies to, so against a string it empties the intersection.
        ("required", "{ type: string }", "required: [a]"),
        (
            "additionalProperties",
            "{ type: string }",
            "additionalProperties: false",
        ),
        ("items", "{ type: string }", "items: { type: integer }"),
        (
            "prefixItems",
            "{ type: string }",
            "prefixItems: [{ type: integer }]",
        ),
        ("enum", "{ type: integer }", "enum: ['a']"),
        ("const", "{ type: integer }", "const: 'a'"),
        (
            "contentEncoding",
            "{ type: integer }",
            "contentEncoding: base64",
        ),
        ("format: binary", "{ type: integer }", "format: binary"),
        ("allOf", "{ type: string }", "allOf: [{ type: integer }]"),
        ("oneOf", "{ type: string }", "oneOf: [{ type: integer }]"),
        ("anyOf", "{ type: string }", "anyOf: [{ type: integer }]"),
    ];

    // The fixture's table and the published text must name the same keywords, in the same order.
    let named = shape_bearing_keywords_the_explain_names();
    let covered: Vec<String> = cases
        .iter()
        .map(|(keyword, ..)| (*keyword).to_owned())
        .collect();
    assert_eq!(
        named, covered,
        "`E013`'s explain and this fixture disagree about which sibling keywords bear a shape; \
         whichever moved, the other must move with it"
    );

    let mut unconstrained = Vec::new();
    let mut spurious = Vec::new();
    for (keyword, target, sibling) in cases {
        let spec = |sibling: &str| {
            format!(
                "{HEAD}{}components:\n  schemas:\n    Target: {target}\n",
                BODY.replace("SIBLING", sibling)
            )
        };
        let with = generate(&spec(&format!("                {sibling}")));
        if with.outcome() != Outcome::Rejected || !has_code(&with, Code::AllOfIrreconcilable) {
            unconstrained.push(*keyword);
        }
        // The control: the identical document without the keyword must generate, so the rejection
        // above is attributable to the keyword and to nothing else in the row.
        let without = generate(&spec(""));
        if without.outcome() == Outcome::Rejected {
            spurious.push(*keyword);
        }
    }
    assert!(
        unconstrained.is_empty(),
        "the explain names these sibling keywords as intersected, but a `$ref` carrying one against \
         an irreconcilable target still generates: {unconstrained:?}"
    );
    assert!(
        spurious.is_empty(),
        "these rows reject even without their keyword, so they pin the rest of the row rather than \
         the keyword the explain names: {spurious:?}"
    );

    // Tier one proves each refiner constrains against a target of ANOTHER category, where an empty
    // intersection is the only possible verdict. It cannot see a refiner that establishes its
    // category and then refines nothing — a `required` that became an empty open struct would
    // still reject against a string. This tier is a differential instead — the same document with
    // and without the refining keyword, against a target of the category it applies to, so the
    // only thing that can move the lowering is the keyword itself. The rows with nothing beside the
    // refiner are #140's leak; the rows with an establishing keyword pin that the category a
    // `type`/`properties` gives it is the same one it now implies alone.
    //
    // (keyword, the establishing keywords beside it, the refiner, the target it agrees with)
    let refiners: &[(&str, &str, &str, &str)] = &[
        (
            "required",
            "",
            "required: [a]",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        // A name the target does not declare either: the requirement is the key's presence, and
        // it must survive as a required field rather than vanish for want of a property to mark.
        ("required", "", "required: [a]", "{ type: object }"),
        (
            "additionalProperties",
            "",
            "additionalProperties: false",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        (
            "items",
            "",
            "items: { type: integer }",
            "{ type: array, items: { type: number } }",
        ),
        (
            "prefixItems",
            "",
            "prefixItems: [{ type: integer }]",
            "{ type: array, items: { type: number } }",
        ),
        (
            "additionalProperties",
            "type: object",
            "additionalProperties: false",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        (
            "items",
            "type: array",
            "items: { type: integer }",
            "{ type: array, items: { type: number } }",
        ),
        (
            "prefixItems",
            "type: array",
            "prefixItems: [{ type: integer }]",
            "{ type: array, items: { type: number } }",
        ),
        // `required` beside `properties` participates: `object_body` consumes it per declared
        // property, and the property is declared.
        (
            "required",
            "properties: { a: { type: string } }",
            "required: [a]",
            "{ type: object, properties: { a: { type: string } } }",
        ),
        // And beside a bare `type: object`, which declares no property for it to mark. It used to
        // be dropped there — `object_body` consumed `required` only as a per-property flag — so
        // the generated type accepted and could emit `{}`, which the description forbids.
        (
            "required",
            "type: object",
            "required: [a]",
            "{ type: object, properties: { a: { type: string } } }",
        ),
    ];
    // What is compared is `Sibling`'s OWN emitted definition, not the whole `types` module.
    //
    // The module also holds the intermediate aliases — `SiblingConstraint`,
    // `SiblingReferenceIntersection` — which are emitted whenever the sibling clears
    // `schema_has_shape_constraint`, whether the refining keyword participates in the intersection
    // or not. So a whole-module comparison answers "did adding this keyword change ANY emitted
    // byte", which is liveness; it does not answer "did it change the type", which is the claim in
    // the failure message. Measured under the sibling-discarded mutation: `Sibling` itself is
    // byte-identical with and without the refiner, and the whole-module comparison passed anyway.
    let lowering = |establishing: &str, refiner: &str, target: &str| {
        let spec = format!(
            "{HEAD}components:\n  schemas:\n    Target: {target}\n    Sibling:\n      \
             $ref: '#/components/schemas/Target'\n      {establishing}\n{refiner}"
        );
        let (report, code) = generate_with_code(&spec);
        // `Sibling`'s own item, ATTRIBUTES INCLUDED — `#[serde(deny_unknown_fields)]` sits above
        // the declaration and is exactly what `additionalProperties` contributes. A rejection
        // yields no item at all, which is itself a difference worth seeing.
        let types = types_module(&code);
        let lines: Vec<&str> = types.lines().collect();
        let declares = |line: &str| {
            let t = line.trim_start();
            ["pub struct ", "pub type ", "pub enum "]
                .iter()
                .filter_map(|decl| t.strip_prefix(decl))
                .any(|rest| {
                    rest.split([' ', '<', '{', '(', ';', '='])
                        .next()
                        .is_some_and(|name| name == "Sibling")
                })
        };
        let definition = lines
            .iter()
            .position(|line| declares(line))
            .map(|decl| {
                // Walk back over the item's attributes and rustdoc.
                let mut start = decl;
                while start > 0 {
                    let prev = lines[start - 1].trim_start();
                    if prev.starts_with("#[") || prev.starts_with("///") {
                        start -= 1;
                    } else {
                        break;
                    }
                }
                // Forward to the end of the item.
                let mut end = decl;
                if lines[decl].trim_end().ends_with(';') {
                    end = decl + 1;
                } else {
                    while end < lines.len() {
                        end += 1;
                        if lines[end - 1].trim_end() == "}" {
                            break;
                        }
                    }
                }
                lines[start..end].join("\n")
            })
            .unwrap_or_default();
        (report.outcome(), definition)
    };
    for (keyword, establishing, refiner, target) in refiners {
        let with = lowering(establishing, &format!("      {refiner}\n"), target);
        let without = lowering(establishing, "", target);
        // Every target here AGREES with the refiner's category, so a rejection is not the
        // refiner taking part — it is the refiner being mistaken for a contradiction. Without
        // this, a rejection would satisfy the differential below by yielding no item at all.
        assert_ne!(
            with.0,
            Outcome::Rejected,
            "`{keyword}` beside `{establishing}` rejected against `{target}`, a target of the \
             category it applies to"
        );
        assert_ne!(
            with, without,
            "`{keyword}` beside `{establishing}` changed nothing about the lowering, so the \
             explain's claim that it takes part is not true"
        );
    }
}

/// #140's two remaining halves of the category rule `E013`'s explain states for an untyped `$ref`
/// sibling. The object and array applicators say nothing about `null` — in 2020-12 they are
/// vacuously satisfied by it — so a nullable target stays nullable through the intersection; and a
/// sibling carrying both kinds with no `type` to pick one is rejected rather than lowered to either
/// category, which would silently discard the other kind's keywords.
#[test]
fn a_ref_sibling_applicator_establishes_its_category_and_keeps_the_targets_null() {
    const HEAD: &str = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: \
                        'https://e.com' }]\npaths: {}\ncomponents:\n  schemas:\n";
    let holder =
        "    Holder:\n      type: object\n      required: [p]\n      properties:\n        \
                  p: { $ref: '#/components/schemas/Sibling' }\n";

    for (keyword, target, sibling) in [
        (
            "required",
            "{ type: [object, 'null'], properties: { a: { type: string } } }",
            "required: [a]",
        ),
        (
            "properties",
            "{ type: [object, 'null'], properties: { a: { type: string } } }",
            "properties: { b: { type: integer } }",
        ),
        (
            "items",
            "{ type: [array, 'null'], items: { type: number } }",
            "items: { type: integer }",
        ),
    ] {
        let spec = format!(
            "{HEAD}    Target: {target}\n    Sibling:\n      $ref: \
             '#/components/schemas/Target'\n      {sibling}\n{holder}"
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "{keyword}: {report:#?}"
        );
        let types = types_module(&code);
        assert!(
            types.contains("pub p: Option<Sibling>"),
            "an untyped `{keyword}` sibling dropped the nullable target's `null`, though the \
             keyword says nothing about it:\n{types}"
        );
    }

    // Keeping the target's `null` must not rescue a category contradiction. Against a nullable
    // target of ANOTHER category the inferred `[category, null]` shares only `null` with it, and
    // typing that as the exact JSON null (`pub type Sibling = ();`) would silently replace a
    // nullable string with a type that decodes nothing else. It is the empty intersection the
    // same sibling has against a non-null string, so it rejects the same way; an untyped
    // `properties` sibling rejected here before the category was inferred at all.
    for (keyword, target, sibling) in [
        ("required", "{ type: [string, 'null'] }", "required: [q]"),
        (
            "properties",
            "{ type: [string, 'null'] }",
            "properties: { b: { type: integer } }",
        ),
        (
            "items",
            "{ type: [string, 'null'] }",
            "items: { type: integer }",
        ),
        (
            "additionalProperties",
            "{ type: [array, 'null'], items: { type: string } }",
            "additionalProperties: { type: integer }",
        ),
    ] {
        let spec = format!(
            "{HEAD}    Target: {target}\n    Sibling:\n      $ref: \
             '#/components/schemas/Target'\n      {sibling}\n{holder}"
        );
        for report in [generate(&spec), check(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "an untyped `{keyword}` sibling against a nullable target of another category \
                 did not reject: {report:#?}"
            );
            assert_eq!(
                messages_for(&report, Code::AllOfIrreconcilable).len(),
                1,
                "{keyword}: {report:#?}"
            );
        }
    }

    // The exemption from the rule above: a target that is itself exactly `null`. There the
    // intersection's `null` is the target's whole type, not a remnant of a contradiction, and the
    // untyped applicators are vacuously satisfied by `null`, so the sibling keeps the target's type
    // rather than rejecting.
    let spec = format!(
        "{HEAD}    Target: {{ type: 'null' }}\n    Sibling:\n      $ref: \
         '#/components/schemas/Target'\n      required: [q]\n{holder}"
    );
    for report in [generate(&spec), check(&spec)] {
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "an untyped `required` sibling on a `$ref` to an exactly-`null` target rejected: \
             {report:#?}"
        );
        assert!(
            messages_for(&report, Code::AllOfIrreconcilable).is_empty(),
            "{report:#?}"
        );
    }
    let (_, code) = generate_with_code(&spec);
    let types = types_module(&code);
    assert!(
        types.contains("pub type Sibling = ();"),
        "an untyped `required` sibling changed an exactly-`null` target's type:\n{types}"
    );

    // A union target whose branches do not all share the inferred category. Intersecting the whole
    // union with the category would drop every branch of another category (the string one here),
    // so `Sibling` would reject the strings `Target` accepts. The applicators refine the branches of
    // their own category and keep the rest, as beside an inline union (#282): `Sibling` is still a
    // two-branch union, and it differs from `Target` in the branch the keyword reaches. Object and
    // array keywords together are no contradiction there: each set refines its own branches.
    for (keyword, target, sibling) in [
        (
            "required",
            "{ oneOf: [{ type: string }, { type: object, properties: { a: { type: string } } }] }",
            "required: [a]",
        ),
        (
            "items",
            "{ oneOf: [{ type: string }, { type: array, items: { type: number } }] }",
            "items: { type: integer }",
        ),
        (
            "required + items",
            "{ oneOf: [{ type: object, properties: { a: { type: string } } }, { type: array, \
             items: { type: number } }] }",
            "required: [a]\n      items: { type: integer }",
        ),
    ] {
        let spec = format!(
            "{HEAD}    Target: {target}\n    Sibling:\n      $ref: \
             '#/components/schemas/Target'\n      {sibling}\n{holder}"
        );
        for report in [generate(&spec), check(&spec)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "an untyped `{keyword}` sibling on a mixed union target rejected: {report:#?}"
            );
            for code in [Code::AllOfIrreconcilable, Code::DeclarationHasNoEffect] {
                assert!(!has_code(&report, code), "{keyword}: {report:#?}");
            }
        }
        let (_, code) = generate_with_code(&spec);
        let types = types_module(&code);
        let target = enum_variants(&types, "Target");
        let refined = enum_variants(&types, "Sibling");
        assert_eq!(
            refined.len(),
            2,
            "an untyped `{keyword}` sibling dropped a branch of its union target:\n{types}"
        );
        assert_ne!(
            target, refined,
            "an untyped `{keyword}` sibling changed no branch of its union target:\n{types}"
        );
    }
    // Where no branch of the target union has the category, the sibling constrains nothing the
    // target accepts: `Sibling` is the target's union unchanged, and the keyword is `W011`, as it is
    // beside the inline union.
    let spec = format!(
        "{HEAD}    Target: {{ oneOf: [{{ type: string }}, {{ type: integer }}] }}\n    Sibling:\n      \
         $ref: '#/components/schemas/Target'\n      required: [a]\n{holder}"
    );
    for report in [generate(&spec), check(&spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
        let messages = messages_for(&report, Code::DeclarationHasNoEffect);
        assert_eq!(messages.len(), 1, "{report:#?}");
        assert!(
            messages[0].contains("no branch of its target union has that category"),
            "{messages:?}"
        );
    }
    let (_, code) = generate_with_code(&spec);
    let types = types_module(&code);
    assert_eq!(
        enum_variants(&types, "Target"),
        enum_variants(&types, "Sibling"),
        "{types}"
    );
    // Object and array keywords together beside a target branch that states no category have no
    // single category to establish for it.
    let spec = format!(
        "{HEAD}    Target: {{ oneOf: [{{}}, {{ type: string }}] }}\n    Sibling:\n      $ref: \
         '#/components/schemas/Target'\n      required: [a]\n      items: {{ type: integer \
         }}\n{holder}"
    );
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::AllOfIrreconcilable);
        assert_eq!(messages.len(), 1, "{report:#?}");
        assert!(
            messages[0].contains("states no JSON category"),
            "{messages:?}"
        );
    }
    // A union whose every branch has the inferred category loses no branch, so it still composes.
    let spec = format!(
        "{HEAD}    A: {{ type: object, required: [kind], properties: {{ kind: {{ type: string }}, \
         a: {{ type: string }} }} }}\n    B: {{ type: object, required: [kind], properties: {{ \
         kind: {{ type: string }}, a: {{ type: string }} }} }}\n    Target:\n      oneOf: [{{ \
         $ref: '#/components/schemas/A' }}, {{ $ref: '#/components/schemas/B' }}]\n      \
         discriminator: {{ propertyName: kind }}\n    Sibling:\n      $ref: \
         '#/components/schemas/Target'\n      required: [a]\n{holder}"
    );
    let report = generate(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");

    let spec = format!(
        "{HEAD}    Target: {{ type: object, properties: {{ a: {{ type: string }} }} }}\n    \
         Sibling:\n      $ref: '#/components/schemas/Target'\n      required: [a]\n      \
         items: {{ type: integer }}\n"
    );
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let messages = messages_for(&report, Code::AllOfIrreconcilable);
        assert_eq!(messages.len(), 1, "{report:#?}");
        assert!(
            messages[0].contains("both object keywords") && messages[0].contains("array keywords"),
            "{messages:?}"
        );
    }
}

/// The guard on the two rejections above: only an EMPTY intersection is an error. A sibling that
/// merely narrows its target still lowers to the narrower type and generates, so the new rejection
/// cannot creep into the ordinary applicator case.
#[test]
fn a_compatible_ref_sibling_still_generates() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Count', type: number }
components:
  schemas:
    Count: { type: integer }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(!has_code(&report, Code::AllOfIrreconcilable), "{report:#?}");
    assert_no_untyped_value(&code);

    // The claim in the doc comment above — "lowers to the NARROWER type" — and the whole point of
    // treating `$ref` as an applicator. Outcome assertions cannot see it: they pin when the tool
    // refuses, and this is what it emits when it proceeds.
    //
    // `integer` ∧ `number` is `integer`, so the body must be `i64`. Two substitutions were green
    // before this: taking the SIBLING alone, which is the exact defect this pull request exists to
    // fix, and taking the TARGET alone, which is the behaviour before it. The first widens the body
    // to `f64` — a generated type that accepts values the description forbids — and neither changes
    // an outcome, a code, or introduces `serde_json::Value`.
    let types = types_module(&code);
    let body = code
        .split("ResponseValue<types::")
        .nth(1)
        .and_then(|rest| rest.split('>').next())
        .unwrap_or_else(|| panic!("no typed response: {code}"))
        .trim()
        .to_owned();
    assert!(
        types.contains(&format!("pub type {body} = i64;")),
        "`integer` narrowed by `number` must stay `i64`; `f64` would accept values the document \
         forbids: {types}"
    );
    // And the sibling is not simply discarded either: the intersection is taken, so the derived
    // type exists rather than the response naming the target component directly.
    assert!(
        types.contains("pub type Count = i64;"),
        "the target must still be emitted under its own name: {types}"
    );

    // The mirror, and the row that catches the OTHER substitution. Above, `integer` is both the
    // intersection and the target, so taking the target alone happens to give the right answer and
    // is invisible. Here the SIBLING is the narrower side: `number` narrowed by `integer` is
    // `integer`, so target-alone would emit `f64` and widen the body.
    let mirrored = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Measure', type: integer }
components:
  schemas:
    Measure: { type: number }
"##;
    let (report, code) = generate_with_code(mirrored);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let types = types_module(&code);
    let body = code
        .split("ResponseValue<types::")
        .nth(1)
        .and_then(|rest| rest.split('>').next())
        .unwrap_or_else(|| panic!("no typed response: {code}"))
        .trim()
        .to_owned();
    assert!(
        types.contains(&format!("pub type {body} = i64;")),
        "`number` narrowed by `integer` must be `i64`; `f64` is the target alone, which is the \
         behaviour before `$ref` was treated as an applicator: {types}"
    );
}

/// Over-rejection is the whole risk of reporting where the code used to drop, and the fixture above
/// pins one shape only — primitive narrowing. These are the other shapes that reach the `$ref` arm
/// and must keep generating. Each is a direction the rejection could creep in, and each lands on a
/// different mechanism: the early exit before any intersection, an intersection that succeeds
/// unchanged, and `intersect_types`' both-sides-nullable rescue, which returns the exact JSON null
/// type rather than `None` and so never reaches the new rejection at all.
#[test]
fn the_ref_sibling_rejection_does_not_creep_into_the_shapes_that_still_generate() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Name', SIBLING }
"##;

    // (what it exercises, the sibling keywords, the target, what must appear in the output)
    let cases: &[(&str, &str, &str, &str)] = &[
        // Validation-only siblings bear no shape, so the `$ref` arm exits before intersecting. They
        // are reported as ignored (`W001`), never as irreconcilable.
        (
            "validation-only siblings",
            "maxLength: 5, pattern: '^a'",
            "{ type: string }",
            "String",
        ),
        // The sibling agrees with its target: the intersection succeeds and is the target's type.
        (
            "an agreeing type",
            "type: string",
            "{ type: string }",
            "String",
        ),
        // Both sides accept null and nothing else is shared. `intersect_types` returns the exact
        // JSON null type — `null` genuinely is the only satisfying value — so this must NOT reject.
        // The decision record lists "collapse to Null when nullable" as rejected; the code does it,
        // and this fixture is why the record now says the code is right.
        (
            "a nullable-only intersection",
            "type: [integer, 'null']",
            "{ type: [string, 'null'] }",
            "()",
        ),
    ];

    for (what, sibling, target, expected) in cases {
        let spec = format!(
            "{HEAD}{}components:\n  schemas:\n    Name: {target}\n",
            PATH.replace("SIBLING", sibling)
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "`{what}` was rejected, so the new rejection has crept: {report:#?}"
        );
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "`{what}` reported E013: {report:#?}"
        );
        // The RESPONSE BODY's own type, resolved through the operation signature. The earlier
        // revision asserted `code.contains("String")` and `code.contains("()")`, and both of those
        // are true of every generated client — `pub fn url(&self) -> String` and `Server0::new()`
        // are always emitted — so the column could not distinguish any behaviour at all.
        let types = types_module(&code);
        let body = code
            .split("ResponseValue<types::")
            .nth(1)
            .and_then(|rest| rest.split('>').next())
            .unwrap_or_else(|| panic!("`{what}` emitted no typed response: {code}"))
            .trim()
            .to_owned();
        assert!(
            types.contains(&format!("pub type {body} = {expected};")),
            "`{what}` must lower its response body to `{expected}`, but `{body}` is not: {types}"
        );
    }

    // The validation-only case is also the one that must still be *acknowledged*: bearing no shape
    // is not the same as being silently dropped.
    let validation_only = format!(
        "{HEAD}{}components:\n  schemas:\n    Name: {{ type: string }}\n",
        PATH.replace("SIBLING", "maxLength: 5, pattern: '^a'")
    );
    assert!(
        has_code(&generate(&validation_only), Code::ValidationKeywordIgnored),
        "a validation-only sibling must still be acknowledged"
    );
}

/// A component whose root is a `$ref` carrying only siblings that bear no shape — an annotation such
/// as `description`/`title`, or a validation keyword such as `maxLength` — is an alias for its
/// target, exactly as the bare `{$ref: T}` component is. It used to panic in release builds
/// (`component root was not the last inserted def`) when the target was declared first, and when
/// the target was declared second it passed the assertion by lifting the TARGET's def out from
/// under the target's own component entry, and the IR invariant check then rejected a valid
/// document (`response body references missing type`).
///
/// Every existing no-shape fixture put the `$ref` in a response body, which never reserves a
/// component root, so none of them could reach this. Both declaration orders are driven because
/// they fail in different ways, and the output is compared with the bare-`$ref` spelling of the
/// same alias: the siblings must change nothing the generated types say.
#[test]
fn a_component_root_ref_with_only_shapeless_siblings_is_an_alias_for_its_target() {
    const HEAD: &str = r##"openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /alias:
    get:
      operationId: getAlias
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Alias' }
  /target:
    get:
      operationId: getTarget
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Target' }
components:
  schemas:
"##;
    let targets = [
        "{ type: string }",
        "{ type: object, required: [id], properties: { id: { type: integer } } }",
    ];
    // `default` rides beside `description` because alone it is the bare spelling: the parser folds a
    // lone `$ref`+`default` into the alias and reports it as `W005` there.
    let siblings = [
        "description: hello",
        "title: Hello",
        "maxLength: 5",
        "description: hello, default: x",
    ];

    for target in targets {
        for sibling in siblings {
            for target_first in [true, false] {
                let target_line = format!("    Target: {target}\n");
                let spec = |alias: &str| {
                    let alias_line = format!("    Alias: {alias}\n");
                    if target_first {
                        format!("{HEAD}{target_line}{alias_line}")
                    } else {
                        format!("{HEAD}{alias_line}{target_line}")
                    }
                };
                let with_sibling = spec(&format!(
                    "{{ $ref: '#/components/schemas/Target', {sibling} }}"
                ));
                let bare = spec("{ $ref: '#/components/schemas/Target' }");
                let what = format!(
                    "`{sibling}` beside a `$ref` to `{target}`, target first: {target_first}"
                );

                let checked = check(&with_sibling);
                assert_ne!(
                    checked.outcome(),
                    Outcome::Rejected,
                    "{what} was rejected by check: {checked:#?}"
                );
                let (report, code) = generate_with_code(&with_sibling);
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{what} was rejected by generate: {report:#?}"
                );
                let (bare_report, bare_code) = generate_with_code(&bare);
                assert_ne!(bare_report.outcome(), Outcome::Rejected, "{bare_report:#?}");
                assert_eq!(
                    types_module(&code),
                    types_module(&bare_code),
                    "{what} must lower exactly as the bare `$ref` alias does"
                );
                if sibling.starts_with("maxLength") {
                    assert!(
                        has_code(&report, Code::ValidationKeywordIgnored),
                        "{what}: a validation-only sibling must still be acknowledged: {report:#?}"
                    );
                }
                // The alias has no type of its own to document a default on, so it is dropped —
                // and must say so, as the bare `$ref`+`default` spelling does.
                assert_eq!(
                    has_code(&report, Code::SchemaDefaultNotApplied),
                    sibling.contains("default"),
                    "{what}: `W005` must fire exactly when a default is dropped: {report:#?}"
                );
                assert_eq!(
                    has_code(&checked, Code::SchemaDefaultNotApplied),
                    sibling.contains("default"),
                    "{what}: check must agree with generate on `W005`: {checked:#?}"
                );
            }
        }
    }

    // A self-referential alias names no type at all. The bare spelling reports the cycle as `E004`;
    // a sibling beside it must not change that into a generated placeholder or a panic.
    for alias in [
        "{ $ref: '#/components/schemas/Loop' }",
        "{ $ref: '#/components/schemas/Loop', description: hello }",
    ] {
        let spec = format!(
            "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\nservers: [{{ url: 'https://e.com' }}]\n\
             paths:\n  /u:\n    get:\n      operationId: fetch\n      responses:\n        '200':\n          \
             description: ok\n          content:\n            application/json:\n              \
             schema: {{ $ref: '#/components/schemas/Loop' }}\ncomponents:\n  schemas:\n    Loop: {alias}\n"
        );
        for report in [check(&spec), generate(&spec)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{alias}`: {report:#?}"
            );
            assert!(
                has_code(&report, Code::UnresolvedRef),
                "`{alias}` must report the alias cycle as E004: {report:#?}"
            );
        }
    }

    // The guard hands the alias to `chain_component_alias`, which routes a relative-file target
    // through `ensure_resolved` rather than `ensure_component`: a file target must lower exactly as
    // its bare spelling does too, in both declaration orders of the root document's two uses.
    let lib = "components:\n  schemas:\n    \
               Target: { type: object, required: [id], properties: { id: { type: integer } } }\n";
    let file_target = "./lib.yaml#/components/schemas/Target";
    for sibling in ["description: hello", "description: hello, default: x"] {
        for target_first in [true, false] {
            let run = |alias: &str| {
                let temp = tempfile::tempdir().unwrap();
                let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
                let alias_path = "  /alias:\n    get:\n      operationId: getAlias\n      \
                    responses:\n        '200':\n          description: ok\n          content:\n            \
                    application/json:\n              schema: { $ref: '#/components/schemas/Alias' }\n";
                let target_path = format!(
                    "  /target:\n    get:\n      operationId: getTarget\n      \
                     responses:\n        '200':\n          description: ok\n          content:\n            \
                     application/json:\n              schema: {{ $ref: '{file_target}' }}\n"
                );
                let paths = if target_first {
                    format!("{target_path}{alias_path}")
                } else {
                    format!("{alias_path}{target_path}")
                };
                std::fs::write(
                    dir.join("openapi.yaml"),
                    format!(
                        "openapi: 3.1.0\ninfo: {{ title: T, version: 1.0.0 }}\n\
                         servers: [{{ url: 'https://e.com' }}]\npaths:\n{paths}\
                         components:\n  schemas:\n    Alias: {alias}\n"
                    ),
                )
                .unwrap();
                std::fs::write(dir.join("lib.yaml"), lib).unwrap();
                let out = dir.join("client.rs");
                let generated = run_generate(&build(dir.join("openapi.yaml"), out.clone()));
                let code = std::fs::read_to_string(&out).unwrap_or_default();
                let checked = run_check(&Spec::new(dir.join("openapi.yaml")));
                (generated, checked, code)
            };
            let what = format!(
                "`{sibling}` beside a `$ref` to a relative-file target, target first: {target_first}"
            );
            let (generated, checked, code) =
                run(&format!("{{ $ref: '{file_target}', {sibling} }}"));
            let (bare_generated, _, bare_code) = run(&format!("{{ $ref: '{file_target}' }}"));
            for (entry, report) in [
                ("generate", &generated),
                ("check", &checked),
                ("bare generate", &bare_generated),
            ] {
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{what}: {entry}: {report:#?}"
                );
            }
            assert_eq!(
                types_module(&code),
                types_module(&bare_code),
                "{what} must lower exactly as the bare `$ref` alias does"
            );
            for (entry, report) in [("generate", &generated), ("check", &checked)] {
                assert_eq!(
                    has_code(report, Code::SchemaDefaultNotApplied),
                    sibling.contains("default"),
                    "{what}: {entry}: `W005` must fire exactly when a default is dropped: {report:#?}"
                );
            }
        }
    }
}

/// When a `$ref` is BOTH unresolvable and carries a contradictory sibling, exactly one code must
/// win and it must be `E004`: `ensure_component` returns `None` before the intersection is reached,
/// so the missing component is reported and the sibling never gets a second, confusing diagnostic
/// about a target that does not exist. The two sites are twenty lines apart in the same block, which
/// is why this is pinned rather than assumed.
///
/// The absence of `E013` is only a SYMPTOM of that ordering, and a weak one: letting the miss fall
/// through to a `TypeKind::Any` placeholder and reach the intersection looks identical from
/// outside, because an `Any` sibling identity-intersects and emits nothing. So the ordering itself
/// is measured, with a sibling whose own LOWERING would report — `patternProperties` whose value
/// schemas disagree is `E005`. If the sibling is never lowered, `E005` cannot fire; the control
/// proves it fires the moment the same sibling is lowered against a target that does exist.
#[test]
fn an_unresolvable_ref_reports_only_e004_even_when_its_sibling_contradicts() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: '#/components/schemas/Nope', type: integer }
      responses: { '204': { description: ok } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
        assert!(
            !has_code(&report, Code::AllOfIrreconcilable),
            "a missing component must not also be reported as an irreconcilable intersection — \
             there is no target to intersect with: {report:#?}"
        );
    }

    // The mechanism. `SIBLING` is shape-bearing, so it clears `schema_has_shape_constraint` and
    // would be lowered if the `$ref` arm ever got that far.
    const TRACED: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [{ url: 'https://e.com' }]
paths:
  /u:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/TARGET'
              type: object
              patternProperties: { '^a': { type: string }, '^b': { type: integer } }
      responses: { '204': { description: ok } }
components:
  schemas:
    Present: { type: object }
"##;
    for report in [
        generate(&TRACED.replace("TARGET", "Nope")),
        check(&TRACED.replace("TARGET", "Nope")),
    ] {
        assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
        assert!(
            !has_code(&report, Code::PatternPropertiesRejected),
            "the sibling of an unresolvable `$ref` was lowered, so `ensure_component` no longer \
             returns `None` before the intersection is reached and the ordering this fixture \
             names is gone: {report:#?}"
        );
    }
    // The control: the identical sibling against a target that resolves IS lowered, and reports.
    let present = generate(&TRACED.replace("TARGET", "Present"));
    assert!(
        has_code(&present, Code::PatternPropertiesRejected),
        "the marker sibling no longer reports when it is lowered, so its absence above proves \
         nothing: {present:#?}"
    );
}

/// A union sibling gets a say in the union's nullability only where it makes a statement about
/// null, and a sibling that carries no `type` makes none.
///
/// `properties` and `patternProperties` are OBJECT APPLICATORS in 2020-12: they constrain an
/// object and are vacuously satisfied by every non-object, `null` included. They nonetheless lower
/// to a non-nullable `Struct`, so reading `Ty::nullable` off one and letting it decide removed an
/// acceptance the sibling never denied. An independent Draft 2020-12 validator says `null` is valid
/// for all three rows below; they went non-optional, so a `200` of literal `null` that used to
/// decode began failing at runtime with nothing reported at generate time.
///
/// The controls matter as much as the rows: a sibling that DOES carry a `type` is entitled to
/// remove the acceptance, and must keep doing so.
#[test]
fn a_union_sibling_without_a_type_does_not_decide_nullability() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;

    // (what it exercises, the schema body, whether the response must be optional)
    let cases: &[(&str, &str, bool)] = &[
        // No `type` on the sibling: it says nothing about null, so the union's own acceptance wins.
        (
            "a `properties`-only sibling beside a null member",
            "properties: { a: { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a `patternProperties`-only sibling beside a null member",
            "patternProperties: { '^a': { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a `properties`-only sibling beside a nullable sole member",
            "properties: { a: { type: string } }\n                oneOf: [{ type: [object, 'null'] }]",
            true,
        ),
        (
            "a `required`-only sibling beside a null member",
            "required: [a]\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "an `additionalProperties`-only sibling beside a null member",
            "additionalProperties: false\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        // The sibling carries a `type` that excludes null, so it IS entitled to remove the
        // acceptance — this is the case the round-2 change correctly fixed and must keep fixing.
        (
            "a sibling whose `type` excludes null, beside a null member",
            "type: object\n                properties: { a: { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            false,
        ),
        (
            "a sibling whose `type` array excludes null, beside a null member",
            "type: [string]\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "a sibling whose `type` excludes null, beside a nullable sole member",
            "type: [string]\n                oneOf: [{ type: [string, 'null'] }]",
            false,
        ),
        // And a `type` that ADMITS null must not remove it either.
        (
            "a sibling whose `type` array admits null, beside a null member",
            "type: [string, 'null']\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
    ];

    for (what, body, optional) in cases {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "`{what}`: {report:#?}");
        assert_eq!(
            code.contains("ResponseValue<Option<types::"),
            *optional,
            "`{what}` must {} an optional response body — `null` is {} under this schema: {code}",
            if *optional { "have" } else { "not have" },
            if *optional { "valid" } else { "invalid" }
        );
    }
}

/// The question is "does the SIBLING make a statement about `null`?", and the gate asked the
/// ENCLOSING schema's `type`. The two differ in two ways, and each produces a wrong answer in the
/// opposite direction.
///
/// `lower_union_sibling` deletes the enclosing `type` array whenever it holds more than one non-null
/// type, because no single lowered type represents it. That deletion also throws away the array's
/// `"null"`, so the lowered sibling reads as null-rejecting even where the array admitted null —
/// **an under-accept**, whose compiled client fails on a
/// spec-legal `null` with `invalid type: null, expected struct …`. In the other direction, a sibling
/// that speaks about null through `enum` or `const` rather than `type` was treated as silent, so the
/// union's acceptance survived a sibling that denied it.
///
/// Both oracles — Python `jsonschema` 4.26 and the Rust crate 0.49.3 — agree on every row.
#[test]
fn the_nullability_gate_asks_the_sibling_not_the_enclosing_schema() {
    const HEAD: &str =
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\nservers: [{ url: 'https://e.com' }]\n";
    const PATH: &str = r##"paths:
  /u:
    get:
      operationId: fetch
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                BODY
"##;

    // (what it exercises, the schema body, whether `null` satisfies it)
    let cases: &[(&str, &str, bool)] = &[
        // The sibling carries a WIDE type array that admits null. Lowering deletes the array, so
        // before this the sibling read as null-rejecting and the response went non-optional.
        (
            "a `properties`-only sibling under a wide nullable type array",
            "type: [object, array, 'null']\n                properties: { a: { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a `patternProperties`-only sibling under a wide nullable type array",
            "type: [object, array, 'null']\n                patternProperties: { '^a': { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        (
            "a `required`-only sibling under a wide nullable type array",
            "type: [object, array, 'null']\n                required: [a]\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        // The narrow spelling of the first row: one non-null type, so the array survives lowering.
        // It was already right, and the wide spelling must now agree with it.
        (
            "the same sibling under a narrow nullable type array",
            "type: [object, 'null']\n                properties: { a: { type: string } }\n                oneOf: [{ type: object }, { type: 'null' }]",
            true,
        ),
        // The sibling speaks about null through `enum`/`const`, not `type`. It denies null, and the
        // gate must let it.
        (
            "an `enum` sibling that excludes null",
            "enum: ['a', 'b']\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "a `const` sibling",
            "const: 'a'\n                oneOf: [{ type: string }, { type: 'null' }]",
            false,
        ),
        (
            "an `allOf` sibling that excludes null",
            "allOf: [{ type: object }]\n                oneOf: [{ type: object }, { type: 'null' }]",
            false,
        ),
        // An `enum` that lists null admits it.
        (
            "an `enum` sibling that includes null",
            "enum: ['a', null]\n                oneOf: [{ type: string }, { type: 'null' }]",
            true,
        ),
    ];

    for (what, body, null_satisfies) in cases {
        let spec = format!("{HEAD}{}", PATH.replace("BODY", body));
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "`{what}`: {report:#?}");
        assert_eq!(
            code.contains("ResponseValue<Option<types::"),
            *null_satisfies,
            "`{what}` must {} an optional response body — `null` is {} under this schema: {code}",
            if *null_satisfies { "have" } else { "not have" },
            if *null_satisfies { "valid" } else { "invalid" }
        );
    }

    // The third symptom of the same root, and the sharpest: a document whose ONLY valid instance is
    // `null` was REJECTED, with a message asserting an empty intersection when the intersection is
    // `{null}`. The one-non-null-type spelling of the same instance set already generated `()`.
    let only_null = format!(
        "{HEAD}{}",
        PATH.replace(
            "BODY",
            "type: [integer, boolean, 'null']\n                properties: { a: { type: string } }\n                oneOf: [{ type: string }, { type: 'null' }]"
        )
    );
    for (entry, report) in [
        ("generate", generate(&only_null)),
        ("check", check(&only_null)),
    ] {
        assert_ne!(
            report.outcome(),
            Outcome::Rejected,
            "a schema whose only valid instance is `null` must not be rejected as having no \
             variant by {entry}: {report:#?}"
        );
        assert!(!has_code(&report, Code::NonDisjointUnion), "{report:#?}");
    }
    let (_, code) = generate_with_code(&only_null);
    let body = code
        .split("ResponseValue<types::")
        .nth(1)
        .and_then(|rest| rest.split('>').next())
        .unwrap_or_else(|| panic!("no typed response: {code}"))
        .trim()
        .to_owned();
    assert!(
        code.contains(&format!("pub type {body} = ();")),
        "the intersection is `{{null}}`, so the exact JSON null type is the answer, not a \
         dropped body and not `Option<()>`: {code}"
    );
}
