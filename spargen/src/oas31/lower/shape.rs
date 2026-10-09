//! Whether a schema's keywords give it a shape of its own, which decides whether a `$ref`'s
//! siblings are intersected with its target.

use crate::oas31::Schema;

/// Whether an `allOf` member not read as an object still constrains its instances, so it is
/// lowered whole as a scalar contribution — as opposed to a pure annotation member (`{}` /
/// `{description: ...}`) that constrains nothing.
///
/// True for any `type` (including `null` and `array`), an `enum` or `const`, `contentEncoding`,
/// `format: binary`, or a `oneOf`/`anyOf` (which the member's own object keywords then refine
/// rather than turn it into an object). Untyped array applicators (`items`, `prefixItems`) alone
/// answer `false`: they constrain only arrays, so the `allOf` paths beside a union route such a
/// member to the scoped refiners before this is asked, and a plain `allOf` reads it as one after
/// (`Contribution::Refiner`, #607).
pub(super) fn schema_imposes_scalar(schema: &Schema) -> bool {
    !schema.types.types.is_empty()
        || schema.enum_values.is_some()
        || schema.const_value.is_some()
        || schema.content_encoding.is_some()
        || schema.format.as_deref() == Some("binary")
        || !schema.one_of.is_empty()
        || !schema.any_of.is_empty()
}

/// One row of [`SHAPE_KEYWORDS`]: a keyword's published spelling, and whether a schema carries it.
type ShapeKeyword = (&'static str, fn(&Schema) -> bool);

/// Every keyword [`schema_has_shape_constraint`] reads, as the published spelling beside the test
/// that recognises it. The gate is exactly "any row matches", so this table IS the gate: a keyword
/// enters or leaves it here and nowhere else.
///
/// `E013`'s explain publishes this set, less `$ref` (a `$ref`'s siblings are this gate's input with
/// the reference already stripped, so `$ref` is never one of them); it is split there into the
/// keywords that establish a shape, the ones that refine one, and `required`. The in-module tests
/// hold that text equal to this table in both directions, and hold the gate to reading nothing the
/// table does not name.
const SHAPE_KEYWORDS: &[ShapeKeyword] = &[
    ("type", |schema| !schema.types.types.is_empty()),
    ("properties", |schema| !schema.properties.is_empty()),
    ("patternProperties", |schema| {
        !schema.pattern_properties.is_empty()
    }),
    ("additionalProperties", |schema| {
        schema.additional_properties.is_some()
    }),
    ("required", |schema| !schema.required.is_empty()),
    ("items", |schema| schema.items.is_some()),
    ("prefixItems", |schema| !schema.prefix_items.is_empty()),
    ("enum", |schema| schema.enum_values.is_some()),
    ("const", |schema| schema.const_value.is_some()),
    ("contentEncoding", |schema| {
        schema.content_encoding.is_some()
    }),
    ("format: binary", |schema| {
        schema.format.as_deref() == Some("binary")
    }),
    ("$ref", |schema| schema.reference.is_some()),
    ("allOf", |schema| !schema.all_of.is_empty()),
    // `oneOf`/`anyOf` count exactly as `allOf` does: in 2020-12 each is an applicator constraining
    // the instance, so a union beside a `$ref` narrows the target like a `type` beside it would,
    // and `schema_imposes_scalar` already treats them so. Leaving them out made a `$ref` whose only
    // sibling was a union take the bare-reference exit, discarding the union with no diagnostic.
    ("oneOf", |schema| !schema.one_of.is_empty()),
    ("anyOf", |schema| !schema.any_of.is_empty()),
];

/// Whether a schema carries any keyword that gives it a shape of its own, which decides whether a
/// `$ref`'s siblings are intersected with its target or the `$ref` is simply its target. Defined
/// by [`SHAPE_KEYWORDS`] alone; add a keyword there, never as another clause here.
pub(super) fn schema_has_shape_constraint(schema: &Schema) -> bool {
    SHAPE_KEYWORDS.iter().any(|(_, bears)| bears(schema))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{schema_has_shape_constraint, Schema, SHAPE_KEYWORDS};
    use crate::diag::{Code, Diagnostics, FileId, JsonPointer};

    fn schema(yaml: &str) -> Schema {
        let mut diags = Diagnostics::default();
        let value = crate::source::parse_yaml(FileId(0), yaml, &mut diags)
            .unwrap_or_else(|_| panic!("probe does not parse: {yaml}"));
        crate::oas31::deserialize::parse_schema(&value, &JsonPointer::root(), &mut diags)
            .unwrap_or_else(|| panic!("probe is not a schema: {yaml}"))
    }

    /// The backticked spans of the one sentence of `explain` that `lead` begins, `lead` included.
    fn backticked_in_sentence(explain: &str, lead: &str) -> Vec<String> {
        let start = explain
            .find(lead)
            .unwrap_or_else(|| panic!("E013's explain no longer says {lead:?}: {explain}"));
        let rest = &explain[start..];
        let sentence = &rest[..rest
            .find(". ")
            .unwrap_or_else(|| panic!("E013's sentence {lead:?} never ends"))];
        sentence
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    /// The sibling keywords `E013`'s explain publishes as taking part in a `$ref`-sibling
    /// intersection: the one sentence that lists every keyword bearing a shape of its own.
    fn published_sibling_keywords() -> Vec<String> {
        // Named by its code string: a `Code::<Variant>` mention of an enumerating code is read as
        // an emission site by `diag`'s case-marker test, and this reads the text, emitting nothing.
        let explain = "E013".parse::<Code>().expect("E013 is a code").explain();
        backticked_in_sentence(explain, "A sibling bears a shape of its own ")
    }

    /// Issue #155: the explain's keyword list and the gate that decides are one set. Read in both
    /// directions — a keyword the gate reads that the text omits, and one the text names that the
    /// gate never reads — so neither can move alone. `$ref` is the one row the text does not name,
    /// because the gate sees a `$ref`'s siblings with the reference already stripped.
    #[test]
    fn e013_explain_names_exactly_the_keywords_the_gate_reads() {
        let listed = published_sibling_keywords();
        let mut published = BTreeSet::new();
        for keyword in &listed {
            assert!(
                published.insert(keyword.as_str()),
                "E013's explain names `{keyword}` twice"
            );
        }
        let gate: BTreeSet<&str> = SHAPE_KEYWORDS
            .iter()
            .map(|(keyword, _)| *keyword)
            .filter(|keyword| *keyword != "$ref")
            .collect();
        assert_eq!(
            published, gate,
            "E013's explain and `SHAPE_KEYWORDS` disagree about which sibling keywords take part in \
             a `$ref` intersection (explain lists: {listed:?})"
        );
        assert_eq!(
            SHAPE_KEYWORDS.len(),
            gate.len() + 1,
            "`SHAPE_KEYWORDS` repeats a keyword, or lost `$ref`"
        );
    }

    /// Each row's name is the keyword its test recognises: a schema carrying that keyword alone
    /// clears the gate through that row and no other. A table with a predicate filed under the
    /// wrong name would pass the explain comparison above while publishing the wrong rule.
    #[test]
    fn every_shape_keyword_row_recognises_the_keyword_it_names() {
        let probe = |keyword: &str| match keyword {
            "type" => "type: string",
            "properties" => "properties: { a: { type: string } }",
            "patternProperties" => "patternProperties: { '^a': { type: string } }",
            "additionalProperties" => "additionalProperties: false",
            "required" => "required: [a]",
            "items" => "items: { type: string }",
            "prefixItems" => "prefixItems: [{ type: string }]",
            "enum" => "enum: [a]",
            "const" => "const: a",
            "contentEncoding" => "contentEncoding: base64",
            "format: binary" => "format: binary",
            "$ref" => "$ref: '#/components/schemas/A'",
            "allOf" => "allOf: [{ type: string }]",
            "oneOf" => "oneOf: [{ type: string }]",
            "anyOf" => "anyOf: [{ type: string }]",
            other => panic!("`SHAPE_KEYWORDS` row `{other}` has no probe here; add one"),
        };
        for (keyword, _) in SHAPE_KEYWORDS {
            let schema = schema(probe(keyword));
            let matched: Vec<&str> = SHAPE_KEYWORDS
                .iter()
                .filter(|(_, bears)| bears(&schema))
                .map(|(name, _)| *name)
                .collect();
            assert_eq!(
                matched,
                [*keyword],
                "a schema carrying only `{keyword}` should clear exactly its own row"
            );
        }
    }

    /// The gate reads nothing the table does not name. A schema carrying every other keyword the
    /// parser keeps — validation, annotations, content, `$defs` — does not clear it, so a
    /// clause added to `schema_has_shape_constraint` beside the table (the `maxLength` mutation
    /// #155 measured surviving) fails here rather than widening the published rule unseen.
    #[test]
    fn the_gate_reads_only_the_keywords_its_table_names() {
        let everything_else = schema(
            "discriminator: { propertyName: kind }\n\
             $defs: { A: { type: string } }\n\
             not: { type: string }\n\
             if: { type: string }\n\
             then: { type: string }\n\
             else: { type: string }\n\
             format: date-time\n\
             contentMediaType: application/json\n\
             contentSchema: { type: string }\n\
             xml: { name: a }\n\
             pattern: '^a'\n\
             minimum: 1\n\
             maximum: 2\n\
             exclusiveMinimum: 0\n\
             exclusiveMaximum: 3\n\
             multipleOf: 1\n\
             minLength: 1\n\
             maxLength: 2\n\
             minItems: 1\n\
             maxItems: 2\n\
             uniqueItems: true\n\
             minProperties: 1\n\
             maxProperties: 2\n\
             default: a\n\
             deprecated: true\n\
             readOnly: true\n\
             writeOnly: true\n\
             title: t\n\
             description: d\n",
        );
        assert!(
            !schema_has_shape_constraint(&everything_else),
            "the gate cleared a schema that carries none of `SHAPE_KEYWORDS`: it reads a keyword the \
             table (and so E013's explain) does not name"
        );
    }
}
