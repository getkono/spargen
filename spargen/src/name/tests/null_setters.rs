//! The explicit-`null` setter stems: a nullable model member's `with_{field}_null` and an
//! explicit-null parameter's `{field}_null`.

use super::{allocate, api, parameter};
use crate::diag::{JsonPointer, Provenance};
use crate::ir::{MediaType, ParamLoc, ParamStyle, Parameter, Ty, TypeDef, TypeKind};

/// Issue #1414: an optional nullable member's explicit-`null` setter stem is `{field}_null`, which
/// yields to a declared property spelling it, in either declaration order, and a member `new`
/// takes or whose type admits no `null` gets none.
#[test]
fn a_null_setter_stem_yields_to_a_property_spelling_it() {
    use crate::ir::{Field, PropertyName, Struct};
    let field = |wire: &str, nullable: bool, required: bool| Field {
        name: PropertyName {
            wire: wire.to_owned(),
        },
        ty: Ty {
            id: crate::ir::TypeId(0),
            nullable,
            boxed: false,
        },
        required,
        deprecated: false,
        read_only: false,
        write_only: false,
        default: None,
        xml: Default::default(),
        undeclared: false,
    };
    for stem_first in [false, true] {
        let mut fields = vec![
            field("nickname", true, false),
            field("age", false, false),
            field("id", true, true),
        ];
        let spelled = field("nickname_null", true, false);
        if stem_first {
            fields.insert(0, spelled);
        } else {
            fields.push(spelled);
        }
        let mut api = api(Vec::new(), false);
        let id = api.types.insert(TypeDef {
            name_hint: "Patch".to_owned(),
            kind: TypeKind::Struct(Struct {
                fields,
                additional: crate::ir::AdditionalProps::Allow,
            }),
            docs: Default::default(),
            provenance: Provenance::new(JsonPointer::root().push("Patch"), None),
            document: String::new(),
        });
        let names = allocate(&api);
        let key = |wire: &str| (id, wire.to_owned());
        assert_eq!(
            names.fields[&key("nickname_null")].as_str(),
            "nickname_null"
        );
        let stem = names.null_setters[&key("nickname")].as_str();
        assert!(
            stem.len() > "nickname_null".len() && stem.starts_with("nickname_null_"),
            "{stem}"
        );
        assert_eq!(
            names.null_setters[&key("nickname_null")].as_str(),
            "nickname_null_null"
        );
        assert!(!names.null_setters.contains_key(&key("age")));
        assert!(!names.null_setters.contains_key(&key("id")));
    }
}

/// Issue #1414: a parameter sending an explicit `null` gets a `{field}_null` setter, which yields
/// to a parameter spelling it, in either listing order; every other parameter gets none.
#[test]
fn a_parameter_null_setter_yields_to_a_parameter_spelling_it() {
    let explicit_null = |name: &str| Parameter {
        ty: Ty {
            id: crate::ir::TypeId(0),
            nullable: true,
            boxed: false,
        },
        style: ParamStyle::Content(MediaType::Json),
        ..parameter(name, ParamLoc::Query, false)
    };
    for setter_first in [false, true] {
        let mut params = vec![
            explicit_null("filter"),
            parameter("id", ParamLoc::Path, true),
        ];
        let spelled = parameter("filter_null", ParamLoc::Header, false);
        if setter_first {
            params.insert(0, spelled);
        } else {
            params.push(spelled);
        }
        let mut api = api(params, false);
        // `api` gives every parameter its non-nullable text type; restore the nullable one.
        for parameter in &mut api.operations[0].params {
            parameter.ty.nullable = parameter.name == "filter";
        }
        let names = allocate(&api);
        let operation = &api.operations[0];
        let record = &names.operations[&operation.id];
        for (index, (parameter, ident)) in record.params(operation).enumerate() {
            match parameter.name.as_str() {
                "filter" => {
                    let setter = record.null_setter(index).as_str();
                    assert!(
                        setter.len() > "filter_null".len() && setter.starts_with("filter_null_"),
                        "{setter}"
                    );
                }
                "filter_null" => assert_eq!(ident.as_str(), "filter_null"),
                _ => {}
            }
            assert_eq!(
                record.null_setters[index].is_some(),
                parameter.sends_explicit_null()
            );
        }
    }
}
