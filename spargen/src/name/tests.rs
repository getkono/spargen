use super::{allocate, Ident, Names};
use crate::diag::{JsonPointer, Provenance};
use crate::ir::{
    Api, BodyEncoding, Info, MediaType, Method, Operation, OperationId, ParamLoc, ParamStyle,
    Parameter, PathSegment, PathTemplate, Prim, RequestBody, Responses, Ty, TypeDef, TypeGraph,
    TypeKind,
};
use indexmap::IndexMap;

fn parameter(name: &str, location: ParamLoc, required: bool) -> Parameter {
    Parameter {
        name: name.to_owned(),
        location,
        ty: Ty {
            id: crate::ir::TypeId(0),
            nullable: false,
            boxed: false,
        },
        required,
        style: match location {
            ParamLoc::Path | ParamLoc::Header => ParamStyle::Simple,
            ParamLoc::Query | ParamLoc::QueryString | ParamLoc::Cookie => ParamStyle::Form,
        },
        allow_reserved: false,
        explode: false,
        deprecated: false,
        default_display: None,
    }
}

/// An API with one `GET /items/{id}` operation carrying `params` and, when `body` is set, a
/// typed JSON request body, so the `body` binding is allocated too.
fn api(params: Vec<Parameter>, body: bool) -> Api {
    let mut types = TypeGraph::default();
    let string = types.insert(TypeDef {
        name_hint: "Text".to_owned(),
        kind: TypeKind::Primitive(Prim::String),
        docs: Default::default(),
        provenance: Provenance::new(JsonPointer::root(), None),
        document: String::new(),
    });
    let ty = Ty {
        id: string,
        nullable: false,
        boxed: false,
    };
    let params = params
        .into_iter()
        .map(|parameter| Parameter { ty, ..parameter })
        .collect();
    Api {
        info: Info {
            title: "T".to_owned(),
            version: "1.0.0".to_owned(),
            description: None,
        },
        servers: Vec::new(),
        operations: vec![Operation {
            id: OperationId("getItem".to_owned()),
            method: Method::Get,
            path: PathTemplate {
                raw: "/items/{id}".to_owned(),
                segments: vec![
                    PathSegment::Literal("items".to_owned()),
                    PathSegment::Param("id".to_owned()),
                ],
            },
            params,
            request_body: body.then(|| RequestBody {
                media: MediaType::Json,
                content_type: "application/json".to_owned(),
                ty: Some(ty),
                required: true,
                encoding: BodyEncoding::default(),
            }),
            responses: Responses {
                by_status: Vec::new(),
                default: None,
            },
            security: Vec::new(),
            deprecated: false,
            docs: Default::default(),
            server: None,
            provenance: Provenance::new(
                JsonPointer::root()
                    .push("paths")
                    .push("/items/{id}")
                    .push("get"),
                None,
            ),
            bound_headers: Vec::new(),
        }],
        types,
        security_schemes: IndexMap::new(),
    }
}

fn names(params: Vec<Parameter>, body: bool) -> Names {
    allocate(&api(params, body))
}

/// The allocated identifier of every parameter, keyed by what identifies a parameter in
/// OpenAPI (`(in, name)`), so allocations of differently ordered lists compare directly.
fn by_parameter(params: &[Parameter], names: &Names) -> Vec<((u8, String), Ident)> {
    let idents = &names.operations[&OperationId("getItem".to_owned())].parameters;
    assert_eq!(idents.len(), params.len(), "one identifier per parameter");
    let mut keyed: Vec<_> = params
        .iter()
        .zip(idents)
        .map(|(parameter, ident)| {
            let location = match parameter.location {
                ParamLoc::Path => 0u8,
                ParamLoc::Query => 1,
                ParamLoc::QueryString => 2,
                ParamLoc::Header => 3,
                ParamLoc::Cookie => 4,
            };
            ((location, parameter.name.clone()), ident.clone())
        })
        .collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    keyed
}

/// Parameters whose names escape to the same spelling in each scope: `id` twice and `ID`
/// among the method arguments, `page-size`, `page_size` and `Page Size` among the fields.
fn colliding() -> Vec<Parameter> {
    vec![
        parameter("page_size", ParamLoc::Header, false),
        parameter("id", ParamLoc::Query, true),
        parameter("Page Size", ParamLoc::Cookie, false),
        parameter("ID", ParamLoc::Header, true),
        parameter("id", ParamLoc::Path, true),
        parameter("page-size", ParamLoc::Query, false),
    ]
}

fn assert_distinct_per_scope(params: &[Parameter], names: &Names) {
    let idents = &names.operations[&OperationId("getItem".to_owned())].parameters;
    for required in [true, false] {
        let scope: Vec<&str> = params
            .iter()
            .zip(idents)
            .filter(|(parameter, _)| parameter.required == required)
            .map(|(_, ident)| ident.as_str())
            .collect();
        let mut unique = scope.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            scope.len(),
            "required={required} identifiers collide: {scope:?}"
        );
    }
}

#[test]
fn colliding_parameter_identifiers_are_distinct_within_each_scope() {
    let params = colliding();
    let names = names(params.clone(), false);
    assert_distinct_per_scope(&params, &names);

    // The earliest location keeps the bare spelling; the others take a suffix.
    let keyed = by_parameter(&params, &names);
    let ident = |location: u8, name: &str| {
        keyed
            .iter()
            .find(|(key, _)| *key == (location, name.to_owned()))
            .map(|(_, ident)| ident.as_str().to_owned())
            .expect("parameter is allocated")
    };
    assert_eq!(ident(0, "id"), "id");
    assert_ne!(ident(1, "id"), "id");
    assert_ne!(ident(3, "ID"), "id");
    assert_eq!(ident(1, "page-size"), "page_size");
    assert_ne!(ident(3, "page_size"), "page_size");
    assert_ne!(ident(4, "Page Size"), "page_size");
}

#[test]
fn parameter_identifiers_do_not_depend_on_listing_order() {
    let params = colliding();
    let expected = by_parameter(&params, &names(params.clone(), false));
    for rotation in 0..params.len() {
        for reversed in [false, true] {
            let mut reordered = params.clone();
            reordered.rotate_left(rotation);
            if reversed {
                reordered.reverse();
            }
            let names = names(reordered.clone(), false);
            assert_distinct_per_scope(&reordered, &names);
            assert_eq!(
                by_parameter(&reordered, &names),
                expected,
                "rotation {rotation}, reversed {reversed}"
            );
        }
    }
}

/// `OperationNames::params`, the lookup codegen and the surface read parameter identifiers
/// through, pairs each of the operation's own parameters, in `Operation::params` order, with the
/// identifier allocated for it.
#[test]
fn params_pairs_each_parameter_with_its_allocated_identifier() {
    let api = api(colliding(), false);
    let names = allocate(&api);
    let operation = &api.operations[0];
    let allocated = &names.operations[&operation.id].parameters;
    let paired: Vec<_> = names.operation(&operation.id).params(operation).collect();
    assert_eq!(
        paired.len(),
        operation.params.len(),
        "one pair per parameter"
    );
    for (index, (parameter, ident)) in paired.into_iter().enumerate() {
        assert!(
            std::ptr::eq(parameter, &operation.params[index]),
            "pair {index} borrows the operation's own parameter"
        );
        assert_eq!(ident, &allocated[index], "pair {index}'s identifier");
    }
}

#[test]
fn generator_bindings_yield_to_every_parameter() {
    let bindings = [
        "params",
        "body",
        "path",
        "query",
        "raw_query",
        "url",
        "request",
        "reconnect_request",
        "cookies",
        "replay",
    ];
    let mut params: Vec<Parameter> = bindings
        .iter()
        .map(|name| parameter(name, ParamLoc::Query, true))
        .collect();
    // An optional parameter, so the `params` binding is allocated at all.
    params.push(parameter("limit", ParamLoc::Query, false));
    let names = names(params.clone(), true);
    let operation = OperationId("getItem".to_owned());
    let idents = &names.operations[&operation].parameters;

    // Every required parameter keeps its bare spelling.
    for (parameter, ident) in params.iter().zip(idents) {
        assert_eq!(ident.as_str(), parameter.name);
    }

    let allocated = &names.operations[&operation].bindings;
    let generated = [
        allocated.params.as_ref().expect("an optional parameter"),
        allocated.body.as_ref().expect("a typed request body"),
        &allocated.path,
        &allocated.query,
        &allocated.raw_query,
        &allocated.url,
        &allocated.request,
        &allocated.reconnect_request,
        &allocated.cookies,
        &allocated.replay,
    ];
    for (binding, ident) in bindings.iter().zip(generated) {
        assert_ne!(
            ident.as_str(),
            *binding,
            "`{binding}` kept its bare spelling"
        );
        assert!(
            params
                .iter()
                .zip(idents)
                .filter(|(parameter, _)| parameter.required)
                .all(|(_, parameter)| parameter != ident),
            "binding `{ident}` shadows a parameter"
        );
    }
    let mut unique: Vec<&str> = generated.iter().map(|ident| ident.as_str()).collect();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        generated.len(),
        "bindings collide: {generated:?}"
    );
}

/// An operation whose method already spells another operation's `{method}_url` keeps it, in
/// either listing order, and the accessor it would have taken is suffixed instead.
#[test]
fn an_operation_method_keeps_a_spelling_a_url_accessor_wants() {
    for accessor_first in [false, true] {
        let mut api = api(Vec::new(), false);
        let mut spelled = api.operations[0].clone();
        spelled.id = OperationId("getItemUrl".to_owned());
        spelled.provenance = Provenance::new(
            JsonPointer::root()
                .push("paths")
                .push("/items/{id}/url")
                .push("get"),
            None,
        );
        if accessor_first {
            api.operations.insert(0, spelled);
        } else {
            api.operations.push(spelled);
        }
        let names = allocate(&api);
        let method = |id: &str| {
            names.operations[&OperationId(id.to_owned())]
                .method
                .as_str()
        };
        let accessor = |id: &str| {
            names.operations[&OperationId(id.to_owned())]
                .url_accessor
                .as_str()
        };
        assert_eq!(method("getItem"), "get_item");
        assert_eq!(method("getItemUrl"), "get_item_url");
        assert_eq!(accessor("getItemUrl"), "get_item_url_url");
        // The stem plus the suffix seeded from `getItem`'s own pointer.
        assert_eq!(accessor("getItem"), "get_item_url_17dbc4cb");
    }
}

/// A keyword operation's method is raw (`r#type`); its accessor is spelled from the bare
/// keyword, which `_url` makes an ordinary identifier.
#[test]
fn a_keyword_operations_url_accessor_drops_the_raw_prefix() {
    let mut api = api(Vec::new(), false);
    api.operations[0].id = OperationId("type".to_owned());
    let names = allocate(&api);
    let id = OperationId("type".to_owned());
    assert_eq!(names.operations[&id].method.as_str(), "r#type");
    assert_eq!(names.operations[&id].url_accessor.as_str(), "type_url");
}

/// Two operations contending for one error type name: the plain `{Method}Error` form beats a
/// form widened past a root re-export (`request` widens to `RequestOperationError`), and
/// between two plain forms (`get_v2`, `get_v_2`) the lexically smaller method wins, in either
/// listing order. The loser is suffixed from its own pointer.
#[test]
fn a_plain_error_form_beats_a_widened_one_and_the_smaller_method_wins() {
    for (first, second, winner, loser) in [
        (
            "request",
            "requestOperation",
            "RequestOperationError",
            "RequestOperationErrorDdb6e353",
        ),
        ("get_v_2", "getV2", "GetV2Error", "GetV2Error4d12aa9a"),
    ] {
        for reversed in [false, true] {
            let mut api = api(Vec::new(), false);
            let template = api.operations.remove(0);
            for id in [first, second] {
                let mut operation = template.clone();
                operation.id = OperationId(id.to_owned());
                operation.provenance = Provenance::new(
                    JsonPointer::root().push("paths").push(&format!("/{id}")),
                    None,
                );
                api.operations.push(operation);
            }
            if reversed {
                api.operations.reverse();
            }
            let names = allocate(&api);
            let error = |id: &str| names.operations[&OperationId(id.to_owned())].error.as_str();
            assert_eq!(
                error(second),
                winner,
                "{first} / {second}, reversed: {reversed}"
            );
            assert_eq!(
                error(first),
                loser,
                "{first} / {second}, reversed: {reversed}"
            );
        }
    }
}

/// Two operations whose ids escape to one method spelling (`getItem` and `get_item`): the
/// lexically smaller `operationId` keeps the bare method, URL accessor, `…Params` struct and
/// response-header struct in either listing order, and the other's names are suffixed, the
/// same in both orders (#1120).
#[test]
fn operation_names_do_not_depend_on_operation_order() {
    use crate::ir::{Response, ResponseHeader, StatusSpec};
    let mut losers = Vec::new();
    for reversed in [false, true] {
        let mut api = api(Vec::new(), false);
        let template = api.operations.remove(0);
        for id in ["getItem", "get_item"] {
            let mut operation = template.clone();
            operation.id = OperationId(id.to_owned());
            operation.params = vec![parameter("limit", ParamLoc::Query, false)];
            operation.provenance = Provenance::new(
                JsonPointer::root().push("paths").push(&format!("/{id}")),
                None,
            );
            let header = ResponseHeader {
                name: "x-rate-limit".to_owned(),
                ty: operation.params[0].ty,
                required: false,
                explode: false,
                shape: crate::ir::HeaderShape::Scalar,
                deprecated: false,
                docs: Default::default(),
            };
            let response = Response {
                body: None,
                media: None,
                stream: None,
                accept: None,
                headers: vec![header],
            };
            operation.responses.by_status = [(StatusSpec::Exact(200), response)].into();
            api.operations.push(operation);
        }
        if reversed {
            api.operations.reverse();
        }
        let names = allocate(&api);
        let record = |id: &str| &names.operations[&OperationId(id.to_owned())];
        let spelled = |id: &str| {
            let record = record(id);
            [
                record.method.as_str().to_owned(),
                record.url_accessor.as_str().to_owned(),
                record.params_struct.as_str().to_owned(),
                record.response_headers["Status200"].ty.as_str().to_owned(),
            ]
        };
        assert_eq!(
            spelled("getItem"),
            [
                "get_item",
                "get_item_url",
                "GetItemParams",
                "GetItemStatus200Headers"
            ],
            "reversed: {reversed}"
        );
        let loser = spelled("get_item");
        for (bare, suffixed) in [
            ("get_item", &loser[0]),
            ("GetItemParams", &loser[2]),
            ("GetItemStatus200Headers", &loser[3]),
        ] {
            assert!(
                suffixed.len() > bare.len() && suffixed.starts_with(bare),
                "{suffixed} is not a suffixed {bare}, reversed: {reversed}"
            );
        }
        // The loser's accessor is spelled from its own, suffixed, method.
        assert_eq!(
            loser[1],
            format!("{}_url", loser[0]),
            "reversed: {reversed}"
        );
        losers.push(loser);
    }
    assert_eq!(
        losers[0], losers[1],
        "the loser's names move with the listing order"
    );
}

#[test]
fn a_declared_root_document_schema_takes_a_type_name_from_a_synthesized_one() {
    // A definition is anonymous only when it has neither a document nor a pointer. A schema
    // declared in the root document has an empty document and a pointer of its own, so it
    // must still outrank the synthesized `Text` (root pointer) that `api` inserts first.
    let mut api = api(Vec::new(), false);
    let synthesized = api.types.iter().next().expect("the text type").0;
    let declared = api.types.insert(TypeDef {
        name_hint: "Text".to_owned(),
        kind: TypeKind::Primitive(Prim::String),
        docs: Default::default(),
        provenance: Provenance::new(
            JsonPointer::root()
                .push("components")
                .push("schemas")
                .push("Text"),
            None,
        ),
        document: String::new(),
    });

    let names = allocate(&api);

    assert_eq!(names.types[&declared].as_str(), "Text");
    assert_ne!(names.types[&synthesized].as_str(), "Text");
}

#[test]
fn a_component_keeps_its_type_name_from_an_inline_type_spelling_it() {
    // `Pet`'s `id` property hints `PetId`, which a component declares too. The inline pointer
    // sorts before the component's (`/` before `I`), so only the declared-schema rank keeps the
    // bare name with the component, in either insertion order.
    for component_first in [false, true] {
        let mut api = api(Vec::new(), false);
        let def = |pointer: JsonPointer| TypeDef {
            name_hint: "PetId".to_owned(),
            kind: TypeKind::Primitive(Prim::String),
            docs: Default::default(),
            provenance: Provenance::new(pointer, None),
            document: String::new(),
        };
        let schemas = JsonPointer::root().push("components").push("schemas");
        let component = def(schemas.push("PetId"));
        let inline = def(schemas.push("Pet").push("properties").push("id"));
        let (component, inline) = if component_first {
            let component = api.types.insert(component);
            (component, api.types.insert(inline))
        } else {
            let inline = api.types.insert(inline);
            (api.types.insert(component), inline)
        };

        let names = allocate(&api);

        assert_eq!(names.types[&component].as_str(), "PetId");
        assert_ne!(names.types[&inline].as_str(), "PetId");
    }
}

#[test]
fn a_closed_string_enum_variant_never_takes_the_all_constants_spelling() {
    // `a_l_l` cases to `ALL`, the closed enum's inherent constant, which the variant would shadow.
    // An open enum emits no such constant, so its variant keeps the spelling.
    use crate::ir::{Openness, ScalarEnum, ScalarRepr, ScalarValue};
    let mut api = api(Vec::new(), false);
    let mut enumeration = |hint: &str, openness: Openness| {
        api.types.insert(TypeDef {
            name_hint: hint.to_owned(),
            kind: TypeKind::Enum(ScalarEnum {
                repr: ScalarRepr::String,
                variants: vec![ScalarValue::String("a_l_l".to_owned())],
                openness,
            }),
            docs: Default::default(),
            provenance: Provenance::new(JsonPointer::root().push(hint), None),
            document: String::new(),
        })
    };
    let closed = enumeration("Closed", Openness::Closed);
    let open = enumeration("Open", Openness::Open);

    let names = allocate(&api);

    let variant = |id| names.variants[&(id, "a_l_l".to_owned())].as_str();
    assert_eq!(super::CLOSED_ENUM_ALL, "ALL");
    assert_ne!(variant(closed), "ALL");
    assert_eq!(variant(open), "ALL");
}

/// Does `text` lex as exactly one Rust identifier token, spelled as itself? `Ident`'s
/// `ToTokens` hands the text to `proc_macro2::Ident::new`, which panics on anything else.
fn is_legal(text: &str) -> bool {
    let Ok(stream) = text.parse::<proc_macro2::TokenStream>() else {
        return false;
    };
    let mut tokens = stream.into_iter();
    matches!(tokens.next(), Some(proc_macro2::TokenTree::Ident(ident)) if ident == text)
        && tokens.next().is_none()
}

/// Assert that `idents` are pairwise distinct legal identifiers, none of them spelled as one of
/// `reserved`. `scope` names the scope in the failure message.
fn assert_scope<'a>(
    scope: &str,
    idents: impl IntoIterator<Item = &'a Ident>,
    reserved: &[&str],
) -> Result<(), proptest::test_runner::TestCaseError> {
    let mut seen = std::collections::BTreeSet::new();
    for ident in idents {
        let text = ident.as_str();
        proptest::prop_assert!(is_legal(text), "{scope}: {text:?} is not one identifier");
        proptest::prop_assert!(
            !reserved.contains(&text),
            "{scope}: {text:?} takes a reserved spelling"
        );
        proptest::prop_assert!(seen.insert(text), "{scope}: {text:?} is allocated twice");
    }
    Ok(())
}

/// A name hint biased to collide: two letters around an optional separator in either case, so
/// distinct hints often case to one spelling, plus the keywords, prelude names and generator
/// bindings every scope reserves or escapes.
fn hint() -> impl proptest::strategy::Strategy<Value = String> {
    use proptest::prelude::*;
    prop_oneof![
        3 => "[a-cA-C][-_ ]?[a-cA-C]",
        1 => proptest::sample::select(vec![
            "", "type", "self", "Self", "super", "crate", "String", "Box", "Option", "Vec",
            "Default", "default", "Into", "new", "core", "inner", "with_credential", "params",
            "body", "path", "query", "url", "request", "cookies", "replay", "additional", "other",
            "Other", "unknown", "Unknown", "1a", "-", "requestOperation", "getV2", "get_v_2",
            "a_l_l",
        ])
        .prop_map(str::to_owned),
    ]
}

/// The shape of one synthetic definition: its member hints are unique per definition, as
/// lowering makes them (an object's property map and an enum's value set have no duplicates,
/// and union hints are made unique per union).
#[derive(Debug, Clone)]
enum Shape {
    Struct {
        fields: std::collections::BTreeSet<String>,
        overflow: bool,
    },
    Enum {
        values: std::collections::BTreeSet<String>,
        open: bool,
    },
    Union {
        hints: std::collections::BTreeSet<String>,
        /// A discriminated union open to unlisted tags, which allocates one more variant.
        open: bool,
    },
}

#[derive(Debug, Clone)]
struct Definition {
    hint: String,
    /// No `(document, pointer)` identity, as a synthesized type has.
    anonymous: bool,
    shape: Shape,
}

#[derive(Debug, Clone)]
struct SyntheticOperation {
    /// `(location, name)` → `required`: OpenAPI identifies a parameter by the pair.
    params: std::collections::BTreeMap<(u8, String), bool>,
    body: bool,
    /// Documented header names per response status.
    headers: std::collections::BTreeMap<u16, std::collections::BTreeSet<String>>,
}

#[derive(Debug, Clone)]
struct SyntheticServer {
    name: Option<String>,
    variables: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
}

fn hints(
    max: usize,
) -> impl proptest::strategy::Strategy<Value = std::collections::BTreeSet<String>> {
    proptest::collection::btree_set(hint(), 0..max)
}

fn definition() -> impl proptest::strategy::Strategy<Value = Definition> {
    use proptest::prelude::*;
    let shape = prop_oneof![
        (hints(8), any::<bool>()).prop_map(|(fields, overflow)| Shape::Struct { fields, overflow }),
        (hints(8), any::<bool>()).prop_map(|(values, open)| Shape::Enum { values, open }),
        (hints(6), any::<bool>()).prop_map(|(hints, open)| Shape::Union { hints, open }),
    ];
    (hint(), proptest::bool::weighted(0.2), shape).prop_map(|(hint, anonymous, shape)| Definition {
        hint,
        anonymous,
        shape,
    })
}

fn operation() -> impl proptest::strategy::Strategy<Value = SyntheticOperation> {
    use proptest::prelude::*;
    (
        proptest::collection::btree_map((0u8..5, hint()), any::<bool>(), 0..8),
        any::<bool>(),
        proptest::collection::btree_map(
            proptest::sample::select(vec![200u16, 201, 404]),
            hints(4),
            0..3,
        ),
    )
        .prop_map(|(params, body, headers)| SyntheticOperation {
            params,
            body,
            headers,
        })
}

fn server() -> impl proptest::strategy::Strategy<Value = SyntheticServer> {
    use proptest::strategy::Strategy;
    (
        proptest::option::of(hint()),
        proptest::collection::btree_map(hint(), hints(4), 0..4),
    )
        .prop_map(|(name, variables)| SyntheticServer { name, variables })
}

fn location(code: u8) -> ParamLoc {
    match code {
        0 => ParamLoc::Path,
        1 => ParamLoc::Query,
        2 => ParamLoc::QueryString,
        3 => ParamLoc::Header,
        _ => ParamLoc::Cookie,
    }
}

/// Build an `Api` from the synthetic description. Operations are keyed by their (unique)
/// `operationId`, as lowering guarantees.
fn synthetic_api(
    definitions: &[Definition],
    operations: &std::collections::BTreeMap<String, SyntheticOperation>,
    servers: &[SyntheticServer],
) -> Api {
    use crate::ir::{
        AdditionalProps, Field, Openness, PropertyName, Response, ResponseHeader, ScalarEnum,
        ScalarRepr, ScalarValue, Server, ServerVariable, StatusSpec, Struct, Union, UnionMode,
        UnionStrategy, UnionVariant, VariantDispatch,
    };
    let mut api = api(Vec::new(), false);
    let text = Ty {
        id: api.types.iter().next().expect("the text type").0,
        nullable: false,
        boxed: false,
    };
    let template = api.operations.remove(0);
    for (index, definition) in definitions.iter().enumerate() {
        let kind = match &definition.shape {
            Shape::Struct { fields, overflow } => TypeKind::Struct(Struct {
                fields: fields
                    .iter()
                    .map(|wire| Field {
                        name: PropertyName { wire: wire.clone() },
                        ty: text,
                        required: false,
                        deprecated: false,
                        read_only: false,
                        write_only: false,
                        default: None,
                        xml: Default::default(),
                        undeclared: false,
                    })
                    .collect(),
                additional: if *overflow {
                    AdditionalProps::Typed(Box::new(text))
                } else {
                    AdditionalProps::Allow
                },
            }),
            Shape::Enum { values, open } => TypeKind::Enum(ScalarEnum {
                repr: ScalarRepr::String,
                variants: values.iter().cloned().map(ScalarValue::String).collect(),
                openness: if *open {
                    Openness::Open
                } else {
                    Openness::Closed
                },
            }),
            // An open union is discriminated, each variant selected by its hint as its tag.
            Shape::Union { hints, open } => TypeKind::Union(Union {
                variants: hints
                    .iter()
                    .map(|hint| UnionVariant {
                        name_hint: hint.clone(),
                        ty: text,
                        dispatch: if *open {
                            VariantDispatch::Discriminated {
                                tags: vec![hint.clone()],
                                category: None,
                                untagged: None,
                            }
                        } else {
                            VariantDispatch::Trial { priority: 0 }
                        },
                    })
                    .collect(),
                strategy: if *open {
                    UnionStrategy::Discriminated {
                        tag_field: "kind".to_owned(),
                        default_variant: None,
                        mode: UnionMode::OneOf,
                        open: true,
                    }
                } else {
                    UnionStrategy::Trial {
                        mode: UnionMode::OneOf,
                    }
                },
            }),
        };
        let pointer = if definition.anonymous {
            JsonPointer::root()
        } else {
            JsonPointer::root()
                .push("components")
                .push("schemas")
                .push(&index.to_string())
        };
        api.types.insert(TypeDef {
            name_hint: definition.hint.clone(),
            kind,
            docs: Default::default(),
            provenance: Provenance::new(pointer, None),
            document: String::new(),
        });
    }
    for (id, synthetic) in operations {
        let mut operation = template.clone();
        operation.id = OperationId(id.clone());
        operation.provenance = Provenance::new(
            JsonPointer::root()
                .push("paths")
                .push(&format!("/{id}"))
                .push("get"),
            None,
        );
        operation.params = synthetic
            .params
            .iter()
            .map(|((code, name), required)| Parameter {
                ty: text,
                ..parameter(name, location(*code), *required)
            })
            .collect();
        operation.request_body = synthetic.body.then(|| RequestBody {
            media: MediaType::Json,
            content_type: "application/json".to_owned(),
            ty: Some(text),
            required: true,
            encoding: BodyEncoding::default(),
        });
        operation.responses.by_status = synthetic
            .headers
            .iter()
            .map(|(status, headers)| {
                // An operation with a request body documents a body on each success status
                // too, so one documenting both `200` and `201` has an enum-shaped success.
                let bodied = synthetic.body && *status < 300;
                let response = Response {
                    body: bodied.then_some(text),
                    media: bodied.then_some(MediaType::Json),
                    stream: None,
                    accept: None,
                    headers: headers
                        .iter()
                        .map(|name| ResponseHeader {
                            name: name.clone(),
                            ty: text,
                            required: false,
                            explode: false,
                            shape: crate::ir::HeaderShape::Scalar,
                            deprecated: false,
                            docs: Default::default(),
                        })
                        .collect(),
                };
                (StatusSpec::Exact(*status), response)
            })
            .collect();
        api.operations.push(operation);
    }
    api.servers = servers
        .iter()
        .map(|server| Server {
            name: server.name.clone(),
            url: "https://example.com".to_owned(),
            segments: Vec::new(),
            variables: server
                .variables
                .iter()
                .map(|(name, values)| {
                    let variable = ServerVariable {
                        default: values.iter().next().cloned().unwrap_or_default(),
                        enum_values: values.iter().cloned().collect(),
                        description: None,
                    };
                    (name.clone(), variable)
                })
                .collect(),
            description: None,
        })
        .collect();
    api
}

proptest::proptest! {
    /// Every identifier `allocate` hands out, over a whole synthetic `Api` whose hints are
    /// biased to collide, is a legal identifier and distinct within the scope it is emitted
    /// in: the `types` module (definitions and response-header structs, beside the names
    /// that module imports), the module root (the `…Params` structs, the error types, and the
    /// success enums, beside the runtime re-exports), the client methods and their URL
    /// accessors (beside the fixed ones), each struct's fields with its overflow map, each
    /// enum's and union's variants with an open enum's catch-all, each operation's method
    /// arguments with its generator bindings and its `…Params` fields, each header struct's
    /// fields, the `servers` module (the server builders and the server-variable enums
    /// `emit_servers` declares side by side), and each server's setters and each server
    /// variable's values. Every member is allocated, so the property cannot pass by allocating
    /// nothing.
    #[test]
    fn every_allocation_is_legal_and_distinct_within_its_scope(
        definitions in proptest::collection::vec(definition(), 0..10),
        operations in proptest::collection::btree_map(hint(), operation(), 0..6),
        servers in proptest::collection::vec(server(), 0..3),
    ) {
        use proptest::prop_assert_eq;
        let api = synthetic_api(&definitions, &operations, &servers);
        let names = allocate(&api);

        prop_assert_eq!(names.types.len(), api.types.iter().count());
        let header_structs: usize = operations
            .values()
            .map(|operation| operation.headers.values().filter(|h| !h.is_empty()).count())
            .sum();
        let records: Vec<&super::OperationNames> = names.operations.values().collect();
        prop_assert_eq!(
            records.iter().map(|record| record.response_headers.len()).sum::<usize>(),
            header_structs
        );
        assert_scope(
            "types module",
            names.types.values().chain(
                records
                    .iter()
                    .flat_map(|record| record.response_headers.values())
                    .map(|headers| &headers.ty),
            ),
            super::TYPES_MODULE_NAMES,
        )?;

        prop_assert_eq!(records.len(), operations.len());
        assert_scope(
            "client methods",
            records
                .iter()
                .map(|record| &record.method)
                .chain(records.iter().map(|record| &record.url_accessor)),
            super::CLIENT_METHODS,
        )?;
        let enum_successes = api
            .operations
            .iter()
            .filter(|operation| {
                matches!(operation.responses.success(), crate::ir::SuccessShape::Enum(_))
            })
            .count();
        prop_assert_eq!(
            records.iter().filter(|record| record.success_enum.is_some()).count(),
            enum_successes
        );
        for operation in &api.operations {
            prop_assert_eq!(
                names.operations[&operation.id].success_enum.is_some(),
                matches!(operation.responses.success(), crate::ir::SuccessShape::Enum(_))
            );
        }
        let reexported = [
            super::ROOT_REEXPORTS,
            super::STREAM_ROOT_REEXPORTS,
            super::DATETIME_ROOT_REEXPORTS,
        ]
        .concat();
        assert_scope(
            "module root",
            records
                .iter()
                .map(|record| &record.params_struct)
                .chain(records.iter().map(|record| &record.error))
                .chain(records.iter().filter_map(|record| record.success_enum.as_ref())),
            &reexported,
        )?;

        for (id, def) in api.types.iter() {
            match &def.kind {
                TypeKind::Struct(object) => {
                    let fields: Vec<&Ident> = object
                        .fields
                        .iter()
                        .map(|field| &names.fields[&(id, field.name.wire.clone())])
                        .chain(names.struct_overflow.get(&id))
                        .collect();
                    let overflow =
                        usize::from(matches!(object.additional, crate::ir::AdditionalProps::Typed(_)));
                    prop_assert_eq!(fields.len(), object.fields.len() + overflow);
                    assert_scope(&format!("fields of {}", def.name_hint), fields, &[])?;
                }
                TypeKind::Enum(enumeration) => {
                    let variants: Vec<&Ident> = enumeration
                        .variants
                        .iter()
                        .map(|value| match value {
                            crate::ir::ScalarValue::String(value) => {
                                &names.variants[&(id, value.clone())]
                            }
                            other => unreachable!("only string enums are generated: {other:?}"),
                        })
                        .chain(names.open_variants.get(&id))
                        .collect();
                    prop_assert_eq!(
                        variants.len(),
                        enumeration.variants.len() + usize::from(enumeration.is_open())
                    );
                    // A closed string enum's `ALL` constant shares its variants' namespace.
                    let reserved: &[&str] = if enumeration.is_open() {
                        &[]
                    } else {
                        &[super::CLOSED_ENUM_ALL]
                    };
                    assert_scope(&format!("variants of {}", def.name_hint), variants, reserved)?;
                }
                TypeKind::Union(union) => {
                    let variants: Vec<&Ident> = union
                        .variants
                        .iter()
                        .map(|variant| &names.variants[&(id, variant.name_hint.clone())])
                        .chain(names.open_variants.get(&id))
                        .collect();
                    prop_assert_eq!(
                        variants.len(),
                        union.variants.len() + usize::from(union.is_open())
                    );
                    assert_scope(&format!("variants of {}", def.name_hint), variants, &[])?;
                }
                // `synthetic_api` builds no reservation, and `allocate` refuses one.
                TypeKind::Reserved => unreachable!("a synthetic API holds no reservation"),
                _ => {}
            }
        }

        for operation in &api.operations {
            let idents = &names.operations[&operation.id].parameters;
            prop_assert_eq!(idents.len(), operation.params.len());
            let bindings = &names.operations[&operation.id].bindings;
            prop_assert_eq!(
                bindings.params.is_some(),
                operation.params.iter().any(|parameter| !parameter.required)
            );
            prop_assert_eq!(bindings.body.is_some(), operation.request_body.is_some());
            let arguments = operation
                .params
                .iter()
                .zip(idents)
                .filter(|(parameter, _)| parameter.required)
                .map(|(_, ident)| ident)
                .chain(bindings.params.as_ref())
                .chain(bindings.body.as_ref())
                .chain([
                    &bindings.path,
                    &bindings.query,
                    &bindings.raw_query,
                    &bindings.url,
                    &bindings.request,
                    &bindings.reconnect_request,
                    &bindings.cookies,
                    &bindings.replay,
                ]);
            assert_scope(&format!("arguments of {}", operation.id.0), arguments, &[])?;
            let fields = operation
                .params
                .iter()
                .zip(idents)
                .filter(|(parameter, _)| !parameter.required)
                .map(|(_, ident)| ident);
            assert_scope(&format!("params fields of {}", operation.id.0), fields, &[])?;
            for (status, response) in &operation.responses.by_status {
                if response.headers.is_empty() {
                    continue;
                }
                let label = super::status_label(*status);
                let headers = &names.operations[&operation.id].response_headers[&label];
                let fields = response.headers.iter().map(|header| &headers.fields[&header.name]);
                assert_scope(&format!("{label} headers of {}", operation.id.0), fields, &[])?;
            }
        }

        prop_assert_eq!(names.servers.len(), servers.len());
        assert_scope(
            "servers module",
            names.servers.iter().chain(names.server_variable_enums.values()),
            super::SERVERS_MODULE_NAMES,
        )?;
        for (index, server) in api.servers.iter().enumerate() {
            let fields = server
                .variables
                .keys()
                .map(|name| &names.server_variable_fields[&(index, name.clone())]);
            assert_scope(&format!("variables of server {index}"), fields, &[])?;
            for (name, variable) in &server.variables {
                let variants = variable.enum_values.iter().map(|value| {
                    &names.server_variable_variants[&(index, name.clone(), value.clone())]
                });
                assert_scope(&format!("values of {name}"), variants, &[])?;
            }
        }
    }

    /// Type names and every operation-level name (method, URL accessor, `…Params` struct, error
    /// type, success enum, response-header structs) do not depend on the order the definitions
    /// or the operations are listed in: listing both in reverse renames nothing, operations
    /// whose ids escape to one method spelling included.
    #[test]
    fn allocation_does_not_depend_on_definition_or_operation_order(
        definitions in proptest::collection::vec(definition(), 0..10),
        operations in proptest::collection::btree_map(hint(), operation(), 0..6),
    ) {
        let definitions: Vec<_> = definitions
            .into_iter()
            .map(|definition| Definition { anonymous: false, ..definition })
            .collect();
        let api = synthetic_api(&definitions, &operations, &[]);
        let mut reversed = api.clone();
        reversed.operations.reverse();
        reversed.types = TypeGraph::default();
        for (_, def) in api.types.iter().collect::<Vec<_>>().into_iter().rev() {
            reversed.types.insert(def.clone());
        }
        let type_names = |api: &Api, names: &Names| -> std::collections::BTreeMap<_, _> {
            api.types
                .iter()
                .map(|(id, def)| (def.provenance.pointer.to_string(), names.types[&id].clone()))
                .collect()
        };
        // Every name of each operation but its bindings and parameters, by operation.
        let operation_types = |names: &Names| -> std::collections::BTreeMap<_, _> {
            names
                .operations
                .iter()
                .map(|(id, record)| {
                    let methods = (record.method.clone(), record.url_accessor.clone());
                    let types = (
                        record.params_struct.clone(),
                        record.error.clone(),
                        record.success_enum.clone(),
                    );
                    let headers: std::collections::BTreeMap<_, _> = record
                        .response_headers
                        .iter()
                        .map(|(label, headers)| (label.clone(), headers.ty.clone()))
                        .collect();
                    (id.0.clone(), (methods, types, headers))
                })
                .collect()
        };
        let (forward, backward) = (allocate(&api), allocate(&reversed));
        proptest::prop_assert_eq!(type_names(&api, &forward), type_names(&reversed, &backward));
        proptest::prop_assert_eq!(operation_types(&forward), operation_types(&backward));
    }

    /// The parameter scopes alone, over the space the parameter collisions came from: names
    /// drawn from `[a-c][-_]?[a-c]` (so `a-b`, `a_b` and `ab` meet constantly) at random
    /// locations, required or not. Each kind is injective and legal, the bindings never take
    /// an argument's spelling, and each parameter's identifier does not depend on where the
    /// spec lists it.
    #[test]
    fn operation_identifiers_are_injective(
        params in proptest::collection::btree_map(
            (0u8..5, "[a-c][-_]?[a-c]"),
            proptest::bool::ANY,
            1..12,
        ),
        body in proptest::bool::ANY,
    ) {
        let params: Vec<Parameter> = params
            .iter()
            .map(|((code, name), required)| parameter(name, location(*code), *required))
            .collect();
        let names = names(params.clone(), body);
        let operation = OperationId("getItem".to_owned());
        let idents = &names.operations[&operation].parameters;
        let bindings = &names.operations[&operation].bindings;
        let arguments = params
            .iter()
            .zip(idents)
            .filter(|(parameter, _)| parameter.required)
            .map(|(_, ident)| ident)
            .chain(bindings.params.as_ref())
            .chain(bindings.body.as_ref())
            .chain([
                &bindings.path,
                &bindings.query,
                &bindings.raw_query,
                &bindings.url,
                &bindings.request,
                &bindings.reconnect_request,
                &bindings.cookies,
                &bindings.replay,
            ]);
        assert_scope("arguments", arguments, &[])?;
        let fields = params
            .iter()
            .zip(idents)
            .filter(|(parameter, _)| !parameter.required)
            .map(|(_, ident)| ident);
        assert_scope("params fields", fields, &[])?;

        let expected = by_parameter(&params, &names);
        let mut reversed = params.clone();
        reversed.reverse();
        proptest::prop_assert_eq!(
            by_parameter(&reversed, &self::names(reversed.clone(), body)),
            expected
        );
    }
}
