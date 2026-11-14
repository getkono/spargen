//! Integer and boolean `enum`/`const` sets (#1327): each is a checked newtype over the width its
//! `format` names, listing its values as associated `const`s, rather than an alias of the scalar
//! that enforced no member of the set. A boolean set listing both values narrows nothing and stays
//! `bool`. A member its `format` cannot hold is `E008`.

use super::*;

/// The lines of `code`, trimmed, joined by single spaces, so a multi-line item reads as one line.
fn flat(code: &str) -> String {
    code.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A document whose component `Level` is `schema` (one line of YAML flow syntax).
fn level_document(schema: &str) -> String {
    with_schemas("3.1.0", &format!("    Level: {schema}\n"))
}

/// Each integer `format` holds the set in its own width, an unformatted one (and `number`) in
/// `i64`, and every set is a nominal newtype whose associated `const`s name its values and whose
/// `TryFrom` and decode refuse any other.
#[test]
fn an_integer_set_is_a_checked_newtype_over_its_formats_width() {
    for (schema, width) in [
        ("{ type: integer, format: int32, enum: [1, 2, 3] }", "i32"),
        ("{ type: integer, format: int64, enum: [1, 2, 3] }", "i64"),
        ("{ type: integer, enum: [1, 2, 3] }", "i64"),
        ("{ type: integer, format: uint8, enum: [1, 2, 3] }", "u8"),
        ("{ type: integer, format: uint16, enum: [1, 2, 3] }", "u16"),
        ("{ type: integer, format: uint32, enum: [1, 2, 3] }", "u32"),
        ("{ type: integer, format: uint64, enum: [1, 2, 3] }", "u64"),
        ("{ type: number, enum: [1, 2, 3] }", "i64"),
        ("{ enum: [1, 2, 3] }", "i64"),
    ] {
        let (report, code) = generate_with_code(&level_document(schema));
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{schema}: {report:#?}"
        );
        assert!(report.diagnostics().is_empty(), "{schema}: {report:#?}");
        let types = flat(&types_module(&code));
        for item in [
            format!("pub struct Level({width});"),
            "pub const V1: Self = Self(1);".to_owned(),
            "pub const V3: Self = Self(3);".to_owned(),
            "pub const ALL: &'static [Self] = &[Self::V1, Self::V2, Self::V3];".to_owned(),
            format!("pub const fn get(self) -> {width} {{ self.0 }}"),
            format!("impl TryFrom<{width}> for Level {{"),
            format!("impl From<Level> for {width} {{"),
            "Self::try_from(value).map_err(serde::de::Error::custom)".to_owned(),
            "///Holds exactly one of: `1`, `2`, `3`.".to_owned(),
        ] {
            assert!(types.contains(&item), "{schema}: no `{item}` in {types}");
        }
        assert!(!types.contains("pub type Level"), "{schema}: {types}");
    }
}

/// A `const` is a one-value set, and a negative member is named apart from its absolute value.
#[test]
fn an_integer_const_and_negative_members_name_their_values() {
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        "    One: { type: integer, format: int32, const: 1 }\n    \
         Signed: { type: integer, enum: [-1, 0, 1] }\n",
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let types = flat(&types_module(&code));
    for item in [
        "pub struct One(i32);",
        "pub const ALL: &'static [Self] = &[Self::V1];",
        "pub struct Signed(i64);",
        "pub const V_MINUS_1: Self = Self(-1);",
        "pub const V0: Self = Self(0);",
        "pub const ALL: &'static [Self] = &[Self::V_MINUS_1, Self::V0, Self::V1];",
    ] {
        assert!(types.contains(item), "no `{item}` in {types}");
    }
}

/// A boolean `const`, or an `enum` listing one value, narrows `bool` and is a checked newtype; one
/// listing both values narrows nothing and is `bool`.
#[test]
fn a_boolean_set_is_checked_unless_it_lists_both_values() {
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        "    Yes: { type: boolean, const: true }\n    \
         No: { enum: [false] }\n    \
         Either: { type: boolean, enum: [true, false] }\n",
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
    let types = flat(&types_module(&code));
    for item in [
        "pub struct Yes(bool);",
        "pub const TRUE: Self = Self(true);",
        "pub struct No(bool);",
        "pub const FALSE: Self = Self(false);",
        "pub type Either = bool;",
    ] {
        assert!(types.contains(item), "no `{item}` in {types}");
    }
    assert!(!types.contains("pub struct Either"), "{types}");
}

/// A member the set's `format` cannot hold satisfies `enum` and never `format`, so no value is it:
/// `E008`, naming the member and the format, rather than a set that would not compile.
#[test]
fn a_member_outside_its_formats_range_is_rejected() {
    for (schema, member, format) in [
        (
            "{ type: integer, format: int32, enum: [1, 3000000000] }",
            "3000000000",
            "int32",
        ),
        (
            "{ type: integer, format: uint8, enum: [1, 256] }",
            "256",
            "uint8",
        ),
        (
            "{ type: integer, format: uint64, enum: [-1, 1] }",
            "-1",
            "uint64",
        ),
    ] {
        let report = generate(&level_document(schema));
        assert_eq!(report.outcome(), Outcome::Rejected, "{schema}: {report:#?}");
        let messages = messages_for(&report, Code::NonScalarEnum);
        assert_eq!(
            messages,
            [format!(
                "enum/const value {member} is outside the range of `format: {format}`, so no \
                 integer that format admits is that value"
            )],
            "{schema}: {report:#?}"
        );
    }
}

/// An `allOf` meets an integer set with an integer `format` at the narrower width, keeping the
/// members it holds, and two sets at the narrower of theirs. A member the narrower width cannot
/// hold is outside the intersection, as a member one set lists and the other does not is, so it is
/// dropped where the same member beside its own schema's `format` is `E008`.
#[test]
fn an_intersection_holds_an_integer_set_in_the_narrower_width() {
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        "    Narrowed:\n      \
         allOf:\n        \
         - { type: integer, format: int32 }\n        \
         - { enum: [1, 3000000000] }\n    \
         Sets:\n      \
         allOf:\n        \
         - { type: integer, format: uint8, enum: [1, 2] }\n        \
         - { enum: [2, 300] }\n",
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let types = flat(&types_module(&code));
    for item in [
        "pub struct Narrowed(i32);",
        "pub const ALL: &'static [Self] = &[Self::V1];",
        "pub struct Sets(u8);",
        "pub const ALL: &'static [Self] = &[Self::V2];",
    ] {
        assert!(types.contains(item), "no `{item}` in {types}");
    }
}

/// An `allOf` meeting an integer set at a width that holds none of its members has no common
/// value, so it is `E013` at the composition, at a component root and inside a property alike, as
/// two sets sharing no member are.
#[test]
fn an_intersection_whose_width_holds_no_member_is_irreconcilable() {
    let report = generate(&with_schemas(
        "3.1.0",
        "    Holder:\n      \
         type: object\n      \
         properties:\n        \
         gone:\n          \
         allOf: [{ type: integer, format: int32 }, { enum: [3000000000] }]\n    \
         Empty:\n      \
         allOf: [{ type: integer, format: int32 }, { enum: [3000000000] }]\n",
    ));
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let e013: Vec<(&str, &str)> = report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == Code::AllOfIrreconcilable)
        .map(|diagnostic| (diagnostic.pointer.as_str(), diagnostic.message.as_str()))
        .collect();
    let message = "`allOf` scalar members have an empty or unrepresentable intersection";
    assert_eq!(
        e013,
        [
            ("/components/schemas/Holder/properties/gone", message),
            ("/components/schemas/Empty", message),
        ],
        "{report:#?}"
    );
}

/// Two integer sets listing different values are distinct checked newtypes a value tells apart, so
/// a `oneOf` of them is a union of both, reported as nothing: no member of either set is a member
/// of the other.
#[test]
fn a_one_of_of_disjoint_integer_sets_keeps_both_branches() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths: {}
components:
  schemas:
    Int: { type: integer }
    U: { $ref: '#/components/schemas/Int', oneOf: [ { enum: [1] }, { enum: [2] } ] }
"##;
    let (report, code) = generate_with_code(spec);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");
    let types = flat(&types_module(&code));
    assert!(types.contains("pub enum U {"), "{types}");
    assert_eq!(
        types.matches("pub const ALL: &'static [Self]").count(),
        2,
        "{types}"
    );
}

/// A value listed twice (`1` and `1.0` under `type: integer`, or `true` twice) is one member of the
/// set, named by one `const`, so the newtype's impl compiles.
#[test]
fn a_repeated_value_is_one_member_of_the_set() {
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        "    Repeated: { type: integer, enum: [1, 1.0, 2, 1] }\n    \
         Twice: { type: boolean, enum: [true, true] }\n",
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let types = flat(&types_module(&code));
    for (item, count) in [
        ("pub const V1: Self = Self(1);", 1),
        ("pub const V2: Self = Self(2);", 1),
        ("pub const ALL: &'static [Self] = &[Self::V1, Self::V2];", 1),
        ("pub const TRUE: Self = Self(true);", 1),
        ("pub const ALL: &'static [Self] = &[Self::TRUE];", 1),
    ] {
        assert_eq!(types.matches(item).count(), count, "`{item}` in {types}");
    }
}

/// A representable `default` on an optional field whose type is a checked newtype is the newtype
/// over the listed value, so the provider compiles against the field's type.
#[test]
fn a_default_on_a_checked_field_is_the_newtype_over_its_value() {
    let (report, code) = generate_with_code(&with_schemas(
        "3.1.0",
        "    Settings:\n      \
         type: object\n      \
         properties:\n        \
         level: { type: integer, format: int32, enum: [1, 2], default: 2 }\n        \
         flag: { type: boolean, const: true, default: true }\n",
    ));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let types = flat(&types_module(&code));
    for item in ["Some(SettingsLevel(2))", "Some(SettingsFlag(true))"] {
        assert!(types.contains(item), "no `{item}` in {types}");
    }
}

/// Under `open_narrowing`, an integer or boolean set narrowing a plain `integer` or `boolean`
/// inside a response body's own schema is open, as a string set narrowing a plain `string` is:
/// its decode accepts any value of its width, while `TryFrom` still accepts only the listed ones.
/// Without the option, and in a request body, the set stays closed.
#[test]
fn open_narrowing_opens_an_integer_or_boolean_set_in_a_response_body() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /status:
    post:
      operationId: putStatus
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                code:
                  allOf: [{ type: integer, format: int32 }, { enum: [1, 2] }]
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  code:
                    allOf: [{ type: integer, format: int32 }, { enum: [1, 2] }]
                  ok:
                    allOf: [{ type: boolean }, { const: true }]
"##;
    let temp = tempfile::tempdir().unwrap();
    let path = write_spec(temp.path(), "openapi.yaml", spec);
    for (narrowing, open) in [
        (spargen::Narrowing::Open, true),
        (spargen::Narrowing::Exact, false),
    ] {
        let (report, code) =
            generate_spec_with_code(Spec::new(path.clone()).with_open_narrowing(narrowing));
        assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
        let types = flat(&types_module(&code));
        let opened = types
            .matches("<i32 as serde::Deserialize>::deserialize(deserializer)?; Ok(Self(value))")
            .count()
            + types
                .matches(
                    "<bool as serde::Deserialize>::deserialize(deserializer)?; Ok(Self(value))",
                )
                .count();
        let closed = types
            .matches("Self::try_from(value).map_err(serde::de::Error::custom)")
            .count();
        if open {
            // The response's `code` and `ok` open; the request's `code` stays closed.
            assert_eq!((opened, closed), (2, 1), "{types}");
            assert!(
                types.contains("///Lists `1`, `2`. Decoding also accepts any other value"),
                "{types}"
            );
        } else {
            assert_eq!((opened, closed), (0, 3), "{types}");
        }
    }
}

/// The `E013` diagnostics of `report`, as `(message, remedy)`.
fn e013s(report: &Report) -> Vec<(&str, &str)> {
    report
        .diagnostics()
        .iter()
        .filter(|d| d.code == Code::AllOfIrreconcilable)
        .map(|d| (d.message.as_str(), d.remedy.as_deref().unwrap_or_default()))
        .collect()
}

/// A member of a kind the schema's own `type` does not name satisfies `enum` and never `type`, so
/// no value is it (#1406). Beside members the `type` does admit it is `E008`, naming the member,
/// as a member outside the set's `format` is; where it leaves no value at all, `null` included,
/// the schema is `E013`.
// E013 case: own-keywords-empty
#[test]
fn a_member_the_schemas_own_type_excludes_is_rejected() {
    for (schema, member, kind) in [
        ("{ type: string, enum: [a, 1] }", "1", "an integer"),
        ("{ type: integer, enum: [1, a] }", "\"a\"", "a string"),
        (
            "{ type: boolean, enum: [true, \"false\"] }",
            "\"false\"",
            "a string",
        ),
        (
            "{ type: [string, 'null'], enum: [1, null] }",
            "1",
            "an integer",
        ),
    ] {
        let report = generate(&level_document(schema));
        assert_eq!(report.outcome(), Outcome::Rejected, "{schema}: {report:#?}");
        assert_eq!(
            messages_for(&report, Code::NonScalarEnum),
            [format!(
                "enum/const value {member} is {kind}, which the schema's `type` does not admit, \
                 so no value the schema admits is that member"
            )],
            "{schema}: {report:#?}"
        );
        assert!(e013s(&report).is_empty(), "{schema}: {report:#?}");
    }
    for schema in [
        "{ type: string, enum: [1, 2] }",
        "{ type: integer, enum: [a] }",
        "{ type: string, const: 5 }",
        "{ type: boolean, enum: [\"true\", \"false\"] }",
        "{ type: string, enum: [1, null] }",
    ] {
        let report = generate(&level_document(schema));
        assert_eq!(report.outcome(), Outcome::Rejected, "{schema}: {report:#?}");
        assert_eq!(
            e013s(&report),
            [(
                "every value this schema's own `enum` or `const` lists is of a kind its `type` \
                 does not admit, so no value satisfies the schema",
                "write each value as the `type` names it (`\"1\"`, not `1`, under `type: \
                 string`), or correct the `type`"
            )],
            "{schema}: {report:#?}"
        );
    }
}

/// An integer member satisfies `type: number`, and an integral float `type: integer`: the `type`
/// admits both, so neither is reported.
#[test]
fn a_member_its_type_admits_by_json_schemas_number_rules_is_kept() {
    for schema in [
        "{ type: number, enum: [1, 2] }",
        "{ type: integer, enum: [1, 2.0] }",
        "{ type: [integer, string], enum: [1, 2] }",
    ] {
        let report = generate(&level_document(schema));
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{schema}: {report:#?}"
        );
        assert!(report.diagnostics().is_empty(), "{schema}: {report:#?}");
    }
}

/// A schema's own `enum` and `const` both apply, so its values are the ones both list, as the
/// `allOf` spelling of the same pair reads them: `{enum: [a, b], const: a}` is the one-value set
/// `{enum: [a]}` is (#1406).
#[test]
fn an_enum_beside_a_const_is_their_intersection() {
    let types_of = |schema: &str| {
        let (report, code) = generate_with_code(&level_document(schema));
        assert_eq!(
            report.outcome(),
            Outcome::Generated,
            "{schema}: {report:#?}"
        );
        assert!(report.diagnostics().is_empty(), "{schema}: {report:#?}");
        types_module(&code)
    };
    for (both, alone, all_of) in [
        (
            "{ type: string, enum: [a, b], const: a }",
            "{ type: string, enum: [a] }",
            "{ allOf: [{ type: string, enum: [a, b] }, { const: a }] }",
        ),
        (
            "{ type: integer, enum: [1, 2], const: 2.0 }",
            "{ type: integer, enum: [2] }",
            "{ allOf: [{ type: integer, enum: [1, 2] }, { const: 2 }] }",
        ),
        (
            "{ type: [string, 'null'], enum: [a, null], const: null }",
            "{ type: [string, 'null'], const: null }",
            "{ allOf: [{ type: [string, 'null'], enum: [a, null] }, { const: null }] }",
        ),
    ] {
        let intersected = types_of(both);
        assert!(intersected == types_of(alone), "{both} is not {alone}");
        assert!(intersected == types_of(all_of), "{both} is not {all_of}");
    }
}

/// An `enum` and a `const` on one schema that share no value leave none to satisfy it: `E013`, as
/// the `allOf` spelling of the same pair is.
// E013 case: own-keywords-empty
#[test]
fn an_enum_and_a_const_sharing_no_value_are_irreconcilable() {
    for schema in [
        "{ type: string, enum: [a, b], const: c }",
        "{ enum: [a, b], const: 1 }",
        "{ enum: [null], const: a }",
    ] {
        let report = generate(&level_document(schema));
        assert_eq!(report.outcome(), Outcome::Rejected, "{schema}: {report:#?}");
        assert_eq!(
            e013s(&report),
            [(
                "this schema's own `enum` and `const` share no value, so no value satisfies the \
                 schema",
                "make the `const` one of the `enum`'s values, or drop one of the two keywords"
            )],
            "{schema}: {report:#?}"
        );
        assert!(
            messages_for(&report, Code::NonScalarEnum).is_empty(),
            "{schema}: {report:#?}"
        );
    }
}
