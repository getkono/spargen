//! An `allOf` member of untyped array applicators (`items` or `prefixItems`, no `type`), inline or
//! through a `$ref`, refines the merge's arrays; `alias`, `resolve` and `element` read the
//! resulting types out of the generated code.

use super::*;

/// The right-hand side of `pub type ty = …;`, whitespace removed, which the formatter may have
/// broken across lines (a long element name does).
fn alias(code: &str, ty: &str) -> Option<String> {
    let head = format!("pub type {ty} = ");
    let start = code.find(&head)? + head.len();
    let end = start + code[start..].find(';')?;
    Some(code[start..end].split_whitespace().collect::<String>())
}

/// The type `ty` names once every bare alias on the way is followed.
fn resolve(code: &str, ty: &str) -> String {
    let mut ty = ty.trim().to_owned();
    while let Some(target) = alias(code, &ty) {
        if target.contains(['<', '(']) {
            return target;
        }
        ty = target;
    }
    ty
}

/// The element type of the `Vec` that `ty` resolves to, resolved in turn.
fn element(code: &str, ty: &str, what: &str) -> String {
    let vec = resolve(code, ty);
    let inner = vec
        .strip_prefix("Vec<")
        .and_then(|rest| rest.strip_suffix('>'))
        .map(|inner| inner.trim_end_matches(','))
        .unwrap_or_else(|| panic!("{what}: `{ty}` is `{vec}`, not a `Vec`: {code}"));
    resolve(code, inner)
}

/// An `allOf` member of untyped array applicators alone (`items` or `prefixItems`, no `type`) was
/// read as neither an object nor a scalar, so it was dropped as an annotation with no diagnostic:
/// `allOf: [{type: array}, {items: {type: string}}]` generated `Vec<serde_json::Value>` (#607).
/// Its applicators constrain only arrays, so they now refine the merge's arrays, in either member
/// order, through a `$ref` member's array target, from a nested `allOf` and from a sub-file
/// target, and the array branch of a multi-type member; they establish the array category where
/// no other member constrains; and they leave every other category as it is, so beside a string
/// or an object member they change nothing. Beside a union, a nested member refines the union's
/// array branch as the same member written directly does, rather than dropping its string branch,
/// and beside a union member with no array branch it is `W011`, nested or not. A memoised sub-file
/// target first expanded inside a union branch the meet drops still replays at its next use.
#[test]
fn an_all_of_member_of_untyped_array_applicators_refines_the_merge_s_arrays() {
    let narrowing = [
        (
            "the array member first",
            "    Probe:\n      allOf: [ { type: array }, { items: { type: string } } ]\n",
        ),
        (
            "the items member first",
            "    Probe:\n      allOf: [ { items: { type: string } }, { type: array } ]\n",
        ),
        (
            "a `$ref` member to an array component",
            "    Probe:\n      allOf: [ { $ref: '#/components/schemas/Arr' }, \
             { items: { type: string } } ]\n    Arr: { type: array }\n",
        ),
        (
            "a nested `allOf`",
            "    Probe:\n      allOf: [ { type: array }, { allOf: [ { items: { type: string } } \
             ] } ]\n",
        ),
        (
            "no other constraining member",
            "    Probe:\n      allOf: [ { items: { type: string } }, { description: d } ]\n",
        ),
    ];
    for (what, schemas) in narrowing {
        let (report, code) = generate_with_code(&with_schemas("3.1.0", schemas));
        assert_eq!(report.outcome(), Outcome::Generated, "{what}: {report:#?}");
        assert!(codes(&report).is_empty(), "{what}: {report:#?}");
        assert_eq!(element(&code, "Probe", what), "String", "{what}");
    }

    // A sub-file target is gathered as an inline member, so its applicators refine as one does.
    let root = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  \
                schemas:\n    Probe:\n      allOf: [ { type: array }, { $ref: './lib.yaml#/It' } \
                ]\n";
    let lib = "It: { items: { type: string } }\n";
    let (generated, checked, code) =
        generate_and_check_files(&[("openapi.yaml", root), ("lib.yaml", lib)]);
    assert_check_agrees(&generated, &checked);
    assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
    assert_eq!(element(&code, "Probe", "a sub-file target"), "String");

    // The sub-file target's contribution is memoised where it is first expanded, here inside a
    // union branch the composition beside it excludes, so the union's unused lowering is elided
    // after the meet. Its refiner halves stay as prune roots: elided, the replay at `Zpet` met a
    // missing type and was rejected (`E011`).
    let root = "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths: {}\ncomponents:\n  \
                schemas:\n    Apet:\n      allOf: [ { type: string } ]\n      oneOf: [ { type: \
                string }, { allOf: [ { type: array }, { $ref: './lib.yaml#/It' } ] } ]\n    \
                Zpet:\n      allOf: [ { type: array }, { $ref: './lib.yaml#/It' } ]\n";
    let (generated, checked, code) =
        generate_and_check_files(&[("openapi.yaml", root), ("lib.yaml", lib)]);
    assert_check_agrees(&generated, &checked);
    assert_eq!(generated.outcome(), Outcome::Generated, "{generated:#?}");
    // The excluded array branch is reported as the sibling spelling reports it (#671).
    assert_eq!(codes(&generated), ["W011"], "{generated:#?}");
    assert_eq!(resolve(&code, "Apet"), "String", "{code}");
    assert_eq!(
        element(&code, "Zpet", "a replayed sub-file target"),
        "String"
    );

    // `prefixItems` narrows the array to the tuple it describes.
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        "    Probe:\n      allOf: [ { type: array }, { prefixItems: [ { type: string } ], \
         items: false } ]\n",
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let tuple = resolve(&code, "Probe");
    let position = tuple
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(",)"))
        .unwrap_or_else(|| panic!("`Probe` is `{tuple}`, not a one-tuple: {code}"));
    assert_eq!(resolve(&code, position), "String");

    // The array branch of a multi-type member is refined; its string branch is kept.
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        "    Probe:\n      allOf: [ { type: [array, string] }, { items: { type: string } } ]\n",
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let mut branches: Vec<String> = enum_variants(&code, "Probe")
        .iter()
        .map(|variant| {
            let inner = variant
                .split_once("(Box<")
                .and_then(|(_, rest)| rest.strip_suffix(">)"))
                .unwrap_or_else(|| panic!("variant `{variant}`: {code}"));
            match resolve(&code, inner).as_str() {
                "String" => "string".to_owned(),
                _ => format!("array of {}", element(&code, inner, "the array branch")),
            }
        })
        .collect();
    branches.sort();
    assert_eq!(branches, ["array of String", "string"], "{code}");

    // `null` stays where the array member admits it.
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        "    Probe:\n      allOf: [ { type: [array, 'null'] }, { items: { type: string } } ]\n    \
         Holder: { type: object, required: [p], properties: { p: { $ref: \
         '#/components/schemas/Probe' } } }\n",
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert_eq!(element(&code, "Probe", "a nullable array"), "String");
    assert_eq!(
        field_type(&types_module(&code), "pub p:").as_deref(),
        Some("Option<Probe>"),
        "{code}"
    );

    // Beside members of another category the applicators constrain nothing.
    let vacuous = [
        (
            "a string member",
            "    Probe:\n      allOf: [ { type: string }, { items: { type: string } } ]\n",
            "String",
        ),
        (
            "an object member",
            "    Probe:\n      allOf: [ { type: object, properties: { a: { type: string } } }, \
             { items: { type: string } } ]\n",
            "struct",
        ),
    ];
    for (what, schemas, expected) in vacuous {
        let (report, code) = generate_with_code(&with_schemas("3.1.0", schemas));
        assert_eq!(report.outcome(), Outcome::Generated, "{what}: {report:#?}");
        assert!(codes(&report).is_empty(), "{what}: {report:#?}");
        if expected == "struct" {
            assert_eq!(declared_fields(&code, "Probe"), ["a"], "{what}: {code}");
        } else {
            assert_eq!(resolve(&code, "Probe"), expected, "{what}: {code}");
        }
    }

    // Beside a union, nested or not, the member refines the union's array branch alone.
    for (what, all_of) in [
        ("written directly", "[ { items: { type: string } } ]"),
        ("nested", "[ { allOf: [ { items: { type: string } } ] } ]"),
    ] {
        let (report, code) = generate_with_code(&with_schemas(
            "3.1.0",
            &format!(
                "    Probe:\n      allOf: {all_of}\n      oneOf: [ {{ type: string }}, {{ type: \
                 array }} ]\n"
            ),
        ));
        assert_eq!(report.outcome(), Outcome::Generated, "{what}: {report:#?}");
        let variants = enum_variants(&code, "Probe");
        assert_eq!(
            variants.len(),
            2,
            "{what}: the string branch must stay: {code}"
        );
        let array = variants
            .iter()
            .filter_map(|variant| {
                variant
                    .split_once("(Box<")
                    .and_then(|(_, rest)| rest.strip_suffix(">)"))
            })
            .find(|inner| resolve(&code, inner) != "String")
            .unwrap_or_else(|| panic!("{what}: no array branch: {code}"));
        assert_eq!(element(&code, array, what), "String", "{what}");
    }

    // Beside a union member with no array branch, the member reaches nothing the union accepts:
    // `W011` at the member, written directly or nested, and the union generates as it is. The
    // nested spelling took the scalar shortcut and was met silently.
    for (what, refiner, pointer) in [
        (
            "written directly",
            "{ items: { type: string } }",
            "/components/schemas/Probe/allOf/1",
        ),
        (
            "nested",
            "{ allOf: [ { items: { type: string } } ] }",
            "/components/schemas/Probe/allOf/1/allOf/0",
        ),
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!(
                "    Probe:\n      allOf: [ {{ oneOf: [ {{ type: string }}, {{ type: integer }} \
                 ] }}, {refiner} ]\n"
            ),
        );
        let (report, code) = generate_with_code(&spec);
        assert_eq!(report.outcome(), Outcome::Generated, "{what}: {report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|d| d.code == Code::DeclarationHasNoEffect && d.pointer.as_str() == pointer),
            "{what}: W011 must sit at `{pointer}`: {report:#?}"
        );
        assert_eq!(enum_variants(&code, "Probe").len(), 2, "{what}: {code}");
    }
}

/// A `$ref` `allOf` member whose target is untyped array applicators alone contributed the
/// target's standalone lowering, `serde_json::Value`, which met the array member as the identity
/// and dropped the target's `items` with no diagnostic (#610). The target is now read by its
/// keywords, as the same member written inline is (#607): it refines the merge's arrays in either
/// member order, through a component, a bare alias to one, and a component subschema; it is
/// vacuous beside a string or an object member; beside a union it refines the union's array
/// branch, and beside a union with none it is `W011` at the target.
#[test]
fn a_ref_all_of_member_to_untyped_array_applicators_refines_the_merge_s_arrays() {
    let narrowing = [
        (
            "the array member first",
            "    Probe:\n      allOf: [ { type: array }, { $ref: '#/components/schemas/It' } ]\n    \
             It: { items: { type: string } }\n",
            "String",
        ),
        (
            "the `$ref` member first",
            "    Probe:\n      allOf: [ { $ref: '#/components/schemas/It' }, { type: array } ]\n    \
             It: { items: { type: string } }\n",
            "String",
        ),
        (
            "a bare alias to the target",
            "    Probe:\n      allOf: [ { type: array }, { $ref: '#/components/schemas/Alias' } \
             ]\n    Alias: { $ref: '#/components/schemas/It' }\n    It: { items: { type: string } \
             }\n",
            "String",
        ),
        (
            "a component subschema",
            "    Probe:\n      allOf: [ { type: array }, { $ref: \
             '#/components/schemas/Bag/properties/x' } ]\n    Bag: { type: object, properties: \
             { x: { items: { type: integer } } } }\n",
            "i64",
        ),
        (
            "no other constraining member",
            "    Probe:\n      allOf: [ { $ref: '#/components/schemas/It' }, { description: d } \
             ]\n    It: { items: { type: string } }\n",
            "String",
        ),
    ];
    for (what, schemas, expected) in narrowing {
        let spec = with_schemas("3.1.0", schemas);
        let (report, code) = generate_with_code(&spec);
        assert_eq!(report.outcome(), Outcome::Generated, "{what}: {report:#?}");
        assert!(codes(&report).is_empty(), "{what}: {report:#?}");
        assert_eq!(element(&code, "Probe", what), expected, "{what}: {code}");
    }

    // Beside members of another category the target constrains nothing: beside an object member
    // its standalone `Value` was a scalar, so the mix was rejected (`E013`).
    let vacuous = [
        (
            "a string member",
            "    Probe:\n      allOf: [ { type: string }, { $ref: '#/components/schemas/It' } \
             ]\n    It: { items: { type: string } }\n",
        ),
        (
            "an object member",
            "    Probe:\n      allOf: [ { type: object, properties: { a: { type: string } } }, \
             { $ref: '#/components/schemas/It' } ]\n    It: { items: { type: string } }\n",
        ),
    ];
    for (what, schemas) in vacuous {
        let (report, code) = generate_with_code(&with_schemas("3.1.0", schemas));
        assert_eq!(report.outcome(), Outcome::Generated, "{what}: {report:#?}");
        assert!(codes(&report).is_empty(), "{what}: {report:#?}");
        if what == "an object member" {
            assert_eq!(declared_fields(&code, "Probe"), ["a"], "{what}: {code}");
        } else {
            assert_eq!(resolve(&code, "Probe"), "String", "{what}: {code}");
        }
    }

    // Beside a union, the target refines the union's array branch and keeps its string branch.
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        "    Probe:\n      allOf: [ { $ref: '#/components/schemas/It' } ]\n      oneOf: [ { type: \
         string }, { type: array } ]\n    It: { items: { type: string } }\n",
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let variants = enum_variants(&code, "Probe");
    assert_eq!(variants.len(), 2, "the string branch must stay: {code}");
    let array = variants
        .iter()
        .filter_map(|variant| {
            variant
                .split_once("(Box<")
                .and_then(|(_, rest)| rest.strip_suffix(">)"))
        })
        .find(|inner| resolve(&code, inner) != "String")
        .unwrap_or_else(|| panic!("no array branch: {code}"));
    assert_eq!(element(&code, array, "the union's array branch"), "String");

    // Beside a union member with no array branch, the target reaches nothing the union accepts.
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        "    Probe:\n      allOf: [ { oneOf: [ { type: string }, { type: integer } ] }, { $ref: \
         '#/components/schemas/It' } ]\n    It: { items: { type: string } }\n",
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|d| d.code == Code::DeclarationHasNoEffect
                && d.pointer.as_str() == "/components/schemas/It"),
        "W011 must sit at the target: {report:#?}"
    );
    assert_eq!(enum_variants(&code, "Probe").len(), 2, "{code}");
}
