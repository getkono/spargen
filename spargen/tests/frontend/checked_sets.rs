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
/// members it holds, and two sets at the narrower of theirs; a set no member of which the width
/// holds is empty, the uninhabited type an empty meet is.
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
