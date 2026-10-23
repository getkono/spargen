//! Labels against the generated code.
//!
//! `canon_ty` renders the types `spargen diff` labels a change with, and codegen declares them;
//! nothing else ties the two. The property below generates one specification per case, reads the
//! type codegen declared for a field, and compares it with the label the diff gives that field.

use super::*;

/// A schema tree for one field: leaves cover every rendering `canon_ty` has a rule for that a
/// field can carry (scalars and their formats, scalar enums, nominal objects, untyped, `null`),
/// composed by arrays, closed tuples and nullability.
#[derive(Debug, Clone)]
enum Schema {
    Leaf(&'static str),
    Array(Box<Schema>),
    Tuple(Vec<Schema>),
    Nullable(Box<Schema>),
}

const NULL: &str = r#"{"type":"null"}"#;

impl Schema {
    fn json(&self) -> String {
        match self {
            Schema::Leaf(leaf) => (*leaf).to_owned(),
            Schema::Array(item) => format!(r#"{{"type":"array","items":{}}}"#, item.json()),
            Schema::Tuple(items) => {
                let items: Vec<String> = items.iter().map(Schema::json).collect();
                format!(
                    r#"{{"type":"array","prefixItems":[{}],"items":false}}"#,
                    items.join(",")
                )
            }
            Schema::Nullable(inner) => format!(r#"{{"anyOf":[{},{NULL}]}}"#, inner.json()),
        }
    }
}

fn schema() -> impl proptest::strategy::Strategy<Value = Schema> {
    use proptest::prelude::*;
    let leaf = proptest::sample::select(vec![
        r#"{"type":"string"}"#,
        r#"{"type":"integer","format":"int32"}"#,
        r#"{"type":"integer"}"#,
        r#"{"type":"number"}"#,
        r#"{"type":"boolean"}"#,
        r#"{"type":"string","format":"uuid"}"#,
        r#"{"type":"string","format":"date-time"}"#,
        r#"{"type":"string","format":"date"}"#,
        r#"{"type":"string","enum":["a","b"]}"#,
        r#"{"type":"integer","enum":[1,2]}"#,
        r#"{"type":"boolean","enum":[true]}"#,
        r#"{}"#,
        NULL,
        r##"{"$ref":"#/components/schemas/Leaf"}"##,
        r#"{"type":"object","properties":{"x":{"type":"string"}}}"#,
    ])
    .prop_map(Schema::Leaf);
    leaf.prop_recursive(3, 16, 3, |inner| {
        prop_oneof![
            inner.clone().prop_map(|item| Schema::Array(Box::new(item))),
            proptest::collection::vec(inner.clone(), 1..4).prop_map(Schema::Tuple),
            inner
                .prop_filter("one `null` member at a time", |schema| {
                    !matches!(schema, Schema::Nullable(_) | Schema::Leaf(NULL))
                })
                .prop_map(|schema| Schema::Nullable(Box::new(schema))),
        ]
    })
}

/// A specification whose `Pet` has one required field, `subject`, of schema `subject`.
fn subject_spec(subject: &str) -> String {
    format!(
        r##"{{"openapi":"3.1.0","info":{{"title":"T","version":"1.0.0"}},
"paths":{{"/pets":{{"get":{{"operationId":"getPet","responses":{{"200":{{"description":"ok",
"content":{{"application/json":{{"schema":{{"$ref":"#/components/schemas/Pet"}}}}}}}}}}}}}}}},
"components":{{"schemas":{{
"Pet":{{"type":"object","required":["subject"],"properties":{{"subject":{subject}}}}},
"Marker":{{"type":"object","properties":{{"m":{{"type":"string"}}}}}},
"Leaf":{{"type":"object","properties":{{"x":{{"type":"integer"}}}}}}}}}}}}"##
    )
}

/// Every `type` alias the generated module declares outside the embedded runtime, by name.
fn aliases(items: &[syn::Item], out: &mut std::collections::BTreeMap<String, syn::Type>) {
    for item in items {
        match item {
            syn::Item::Type(alias) => {
                out.insert(alias.ident.to_string(), (*alias.ty).clone());
            }
            syn::Item::Mod(module) if module.ident != "support" => {
                if let Some((_, items)) = &module.content {
                    aliases(items, out);
                }
            }
            _ => {}
        }
    }
}

/// The declared type of `struct_name`'s field `field` in the generated module.
fn field_type(items: &[syn::Item], struct_name: &str, field: &str) -> Option<syn::Type> {
    items.iter().find_map(|item| match item {
        syn::Item::Struct(item) if item.ident == struct_name => item
            .fields
            .iter()
            .find(|candidate| candidate.ident.as_ref().is_some_and(|ident| ident == field))
            .map(|field| field.ty.clone()),
        syn::Item::Mod(module) if module.ident != "support" => module
            .content
            .as_ref()
            .and_then(|(_, items)| field_type(items, struct_name, field)),
        _ => None,
    })
}

/// Render `ty` in the label's notation, expanding every alias in `aliases`. The normalisations are
/// the ones `canon_ty`'s documentation states: a path is named by its last segment
/// (`types::Leaf`, `bytes::Bytes`, `serde_json::Value`), `Box` is not rendered, and a `format`
/// scalar is labelled by its format (`Uuid`, `DateTime`, `Date`) whatever Rust type the `uuid` and
/// `time` mappings choose for it, so both sides spell those `String` here.
fn normalise(ty: &syn::Type, aliases: &std::collections::BTreeMap<String, syn::Type>) -> String {
    match ty {
        syn::Type::Tuple(tuple) => {
            let elements: Vec<String> = tuple
                .elems
                .iter()
                .map(|element| normalise(element, aliases))
                .collect();
            match elements.as_slice() {
                [only] => format!("({only},)"),
                _ => format!("({})", elements.join(", ")),
            }
        }
        syn::Type::Path(path) => {
            let segment = path.path.segments.last().expect("a path has a segment");
            let name = segment.ident.to_string();
            let arguments: Vec<String> = match &segment.arguments {
                syn::PathArguments::AngleBracketed(arguments) => arguments
                    .args
                    .iter()
                    .map(|argument| match argument {
                        syn::GenericArgument::Type(ty) => normalise(ty, aliases),
                        other => panic!(
                            "unexpected generic argument `{}`",
                            quote::ToTokens::to_token_stream(other)
                        ),
                    })
                    .collect(),
                syn::PathArguments::None => Vec::new(),
                other => panic!(
                    "unexpected path arguments `{}`",
                    quote::ToTokens::to_token_stream(other)
                ),
            };
            match (name.as_str(), arguments.as_slice()) {
                ("Box", [inner]) => inner.clone(),
                ("Uuid" | "DateTime" | "Date", []) => "String".to_owned(),
                (_, []) => match aliases.get(&name) {
                    Some(target) => normalise(target, aliases),
                    None => name,
                },
                (_, arguments) => format!("{name}<{}>", arguments.join(", ")),
            }
        }
        other => format!("<unexpected {}>", quote::ToTokens::to_token_stream(other)),
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 48,
        failure_persistence: None,
        ..proptest::test_runner::Config::default()
    })]

    /// The type `spargen diff` labels a field with is the type codegen declares for it, once the
    /// declared aliases are expanded and the documented normalisations applied: the same nesting,
    /// the same tuple arity (a one-position tuple is `(T,)` on both sides, #449), and the same
    /// nullability.
    #[test]
    fn a_field_type_label_is_the_type_codegen_declares(subject in schema()) {
        let old = subject_spec(r##"{"$ref":"#/components/schemas/Marker"}"##);
        let new = subject_spec(&subject.json());
        let source = generated_source(&new, false);
        let file = syn::parse_file(&source).expect("the generated module parses");
        let mut declared = std::collections::BTreeMap::new();
        aliases(&file.items, &mut declared);
        let field = field_type(&file.items, "Pet", "subject").expect("`Pet.subject` is emitted");
        let generated = normalise(&field, &declared);

        let report = unsnapshotted_diff(&old, &new, |spec| spec, |spec| spec);
        let labels: Vec<&str> = report
            .changes
            .iter()
            .filter(|change| change.kind == ChangeKind::FieldTypeChanged)
            .map(|change| change.detail.as_str())
            .collect();
        let [label] = labels[..] else {
            panic!("one field-type change expected: {:?}", report.changes);
        };
        let canonical = label
            .strip_prefix("field type `Marker` -> `")
            .and_then(|rest| rest.strip_suffix('`'))
            .unwrap_or_else(|| panic!("unexpected label {label:?}"));
        let parsed: syn::Type = syn::parse_str(canonical)
            .unwrap_or_else(|error| panic!("the label {canonical:?} is not a type: {error}"));
        let labelled = normalise(&parsed, &std::collections::BTreeMap::new());
        proptest::prop_assert_eq!(
            labelled,
            generated,
            "label {:?}, schema {}",
            canonical,
            subject.json()
        );
    }
}
