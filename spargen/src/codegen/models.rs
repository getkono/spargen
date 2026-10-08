//! The `types` module: one model per emitted definition — structs, enums, aliases, and (through
//! [`union`](super::union)) union enums — with their serde wiring.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::ir::{AdditionalProps, Api, Field, ScalarRepr, ScalarValue, Ty, TypeDef, TypeKind};
use crate::name::Names;

use super::docs::{doc_tokens, normalize_rustdoc};
use super::ty::{prim_tokens, ty_tokens};
use super::union::emit_union;
use super::CodegenOptions;

/// Emit the `types` (models) module for every type the graph emits (every one but those lowering
/// elided), in deterministic order. `uses_time` is whether the embedded runtime carries the RFC
/// 3339 date newtypes, which the module then imports.
pub(super) fn emit_models(
    api: &Api,
    names: &Names,
    options: &CodegenOptions,
    uses_time: bool,
) -> TokenStream {
    let items = api
        .types
        .emitted()
        .map(|(id, def)| emit_type_def(id, def, names, options));
    let decode_present = format_ident!("{DECODE_PRESENT}");
    // The RFC 3339 newtypes live beside `types`, so bring them into scope under the same bare names
    // `prim_tokens` emits; at the generated root the prelude re-export supplies them instead.
    let datetime_import = uses_time.then(|| {
        quote! { use super::{Date, DateTime}; }
    });
    quote! {
        #[forbid(unsafe_code)]
        #[allow(dead_code, unused_imports)]
        pub mod types {
            use serde::{Deserialize, Serialize};
            use std::collections::BTreeMap;
            #datetime_import

            /// The `deserialize_with` of every optional, non-nullable field. It is reached only when
            /// the field is present, and decodes the value — `null` included — as the field's own
            /// type, so a `null` the schema does not admit is rejected rather than read as absence.
            fn #decode_present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
            where
                D: serde::Deserializer<'de>,
                T: serde::Deserialize<'de>,
            {
                T::deserialize(deserializer).map(Some)
            }

            #(#items)*
        }
    }
}

/// Emit an open string enum (`open_narrowing`): one unit variant per listed value, as the closed
/// enum has, plus a variant holding any other string. It cannot derive serde with a catch-all and
/// stay a plain string on the wire, so `Serialize` writes `as_str()` and `Deserialize` reads a
/// string and matches it, reaching the catch-all only for a value no unit variant lists.
///
/// `display_arms` are the closed enum's `Display` arms, which map each unit variant to its value.
fn emit_open_string_enum(
    id: crate::ir::TypeId,
    ident: &crate::name::Ident,
    enumeration: &crate::ir::ScalarEnum,
    names: &Names,
    docs: TokenStream,
    deprecated: Option<TokenStream>,
    display_arms: Vec<TokenStream>,
) -> TokenStream {
    let other = names
        .open_variants
        .get(&id)
        .expect("open variant name allocated");
    let listed: Vec<(&String, &crate::name::Ident)> = enumeration
        .variants
        .iter()
        .map(|variant| {
            let ScalarValue::String(value) = variant else {
                unreachable!("an open enum is a string enum");
            };
            let variant_ident = names
                .variants
                .get(&(id, value.clone()))
                .expect("variant name allocated");
            (value, variant_ident)
        })
        .collect();
    let variants = listed
        .iter()
        .map(|(_, variant_ident)| quote! { #variant_ident, });
    let decode_arms = listed.iter().map(|(value, variant_ident)| {
        quote! { #value => #ident::#variant_ident, }
    });
    let other_doc = "A value the description does not list here. Decoding produces it only for a \
                     string no other variant names.";
    quote! {
        #docs
        #deprecated
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum #ident {
            #(#variants)*
            #[doc = #other_doc]
            #other(String),
        }

        impl #ident {
            /// The wire value.
            pub fn as_str(&self) -> &str {
                match self {
                    #(#display_arms)*
                    #ident::#other(value) => value.as_str(),
                }
            }
        }

        impl std::fmt::Display for #ident {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl serde::Serialize for #ident {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for #ident {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = <String as serde::Deserialize>::deserialize(deserializer)?;
                Ok(match value.as_str() {
                    #(#decode_arms)*
                    _ => #ident::#other(value),
                })
            }
        }
    }
}

/// Emit the model for one definition: a struct, a string enum (closed or open), an integer or
/// boolean enum's alias, an uninhabited enum, a union enum, or an alias of the type it lowered to.
fn emit_type_def(
    id: crate::ir::TypeId,
    def: &TypeDef,
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
    let ident = names.types.get(&id).expect("type name allocated");
    let docs = doc_tokens(&def.docs);
    let deprecated = def.docs.deprecated.then(|| quote! { #[deprecated] });
    match &def.kind {
        TypeKind::Struct(object) => {
            let deny_unknown = matches!(object.additional, AdditionalProps::Deny)
                .then(|| quote! { #[serde(deny_unknown_fields)] });
            let fields = object
                .fields
                .iter()
                .map(|field| emit_field(id, field, names));
            let providers = object
                .fields
                .iter()
                .filter_map(|field| emit_default_provider(id, field, names));
            let additional = match &object.additional {
                AdditionalProps::Typed(ty) => {
                    let ty = ty_tokens(**ty, names, false);
                    let overflow = names
                        .struct_overflow
                        .get(&id)
                        .expect("overflow field name allocated");
                    quote! { #[serde(flatten)] pub #overflow: BTreeMap<String, #ty>, }
                }
                AdditionalProps::Allow | AdditionalProps::Deny => quote! {},
            };
            quote! {
                #docs
                #deprecated
                #[derive(Debug, Clone, Serialize, Deserialize)]
                #deny_unknown
                pub struct #ident {
                    #(#fields)*
                    #additional
                }
                #(#providers)*
            }
        }
        TypeKind::Enum(enumeration) if enumeration.repr == ScalarRepr::String => {
            let variants = enumeration.variants.iter().map(|variant| {
                let value = match variant {
                    ScalarValue::String(value) => value,
                    _ => unreachable!("string repr has string variants"),
                };
                let ident = names
                    .variants
                    .get(&(id, value.clone()))
                    .expect("variant name allocated");
                quote! { #[serde(rename = #value)] #ident, }
            });
            let display_arms = enumeration.variants.iter().map(|variant| {
                let value = match variant {
                    ScalarValue::String(value) => value,
                    _ => unreachable!("string repr has string variants"),
                };
                let variant_ident = names
                    .variants
                    .get(&(id, value.clone()))
                    .expect("variant name allocated");
                quote! { #ident::#variant_ident => #value, }
            });
            if enumeration.is_open() {
                return emit_open_string_enum(
                    id,
                    ident,
                    enumeration,
                    names,
                    docs,
                    deprecated,
                    display_arms.collect(),
                );
            }
            quote! {
                #docs
                #deprecated
                #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
                pub enum #ident {
                    #(#variants)*
                }

                impl std::fmt::Display for #ident {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str(match self {
                            #(#display_arms)*
                        })
                    }
                }
            }
        }
        TypeKind::Enum(enumeration) => {
            let ty = match enumeration.repr {
                ScalarRepr::String => quote! { String },
                ScalarRepr::Int => quote! { i64 },
                ScalarRepr::Bool => quote! { bool },
            };
            quote! { #docs pub type #ident = #ty; }
        }
        TypeKind::Never => {
            let error = format!("no JSON value can inhabit schema {}", ident.as_str());
            quote! {
                #docs
                #deprecated
                #[derive(Debug, Clone)]
                pub enum #ident {}

                impl<'de> serde::Deserialize<'de> for #ident {
                    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
                    where
                        D: serde::Deserializer<'de>,
                    {
                        Err(serde::de::Error::custom(#error))
                    }
                }

                impl serde::Serialize for #ident {
                    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
                    where
                        S: serde::Serializer,
                    {
                        Err(serde::ser::Error::custom(#error))
                    }
                }
            }
        }
        TypeKind::Union(union) => emit_union(id, ident, union, names, docs, deprecated),
        // Named here rather than left to the alias arm: `type_kind_tokens` refuses it too, but a
        // reservation is not an alias and must not read as one at this site either.
        TypeKind::Reserved => {
            unreachable!(
                "a reservation reached codegen; `check_invariants` should have rejected it"
            )
        }
        _ => {
            let ty = type_kind_tokens(&def.kind, names, options);
            quote! { #docs pub type #ident = #ty; }
        }
    }
}

/// One struct field: its rustdoc notes, its serde attribute (wire name, default, and the
/// present-value decode an optional non-nullable field takes), and its declaration.
fn emit_field(id: crate::ir::TypeId, field: &Field, names: &Names) -> TokenStream {
    let ident = names
        .fields
        .get(&(id, field.name.wire.clone()))
        .expect("field name allocated");
    // An `xml.name`/`xml.attribute` hint overrides the serde wire name for XML bodies (an attribute
    // uses quick-xml's `@name` convention); otherwise the plain property wire name is used. The Rust
    // identifier and the names-table key stay keyed off the original property name, so only the wire
    // string changes.
    let wire = field
        .xml
        .wire_override(&field.name.wire)
        .unwrap_or_else(|| field.name.wire.clone());
    // `ty_tokens` already wraps a nullable type in `Option` (`"null"` in the type array), so only an
    // *optional* non-nullable field needs the extra `Option` here — wrapping a nullable field again
    // would yield `Option<Option<T>>`. A required nullable field stays a single `Option<T>` (present
    // but may be `null`); an optional field of either kind is a single `Option<T>`.
    let mut ty = ty_tokens(field.ty, names, false);
    if !field.required && !field.ty.nullable {
        ty = quote! { Option<#ty> };
    }
    // An optional field always deserializes an absent value; when the spec gives a representable
    // scalar default, point serde at a generated provider so the default fills in rather than
    // `None`. Otherwise fall back to `Option::default()` (`None`).
    let serde_default = if field.required {
        quote! {}
    } else if field
        .default
        .as_ref()
        .is_some_and(|default| default.applied.is_some())
    {
        let provider = default_provider_ident(id, ident).to_string();
        quote! { default = #provider, skip_serializing_if = "Option::is_none", }
    } else {
        quote! { default, skip_serializing_if = "Option::is_none", }
    };
    // The `Option` wrapping an optional non-nullable field stands for absence only, yet serde's
    // `Option<T>` maps a JSON `null` to `None` without ever calling `T::deserialize`, so
    // `{"x": null}` would decode as absent (and re-serialise as `{}`) although the schema admits no
    // `null` there. Route every *present* value, `null` included, to `T`'s own deserializer, which
    // rejects it wherever `T` does (an uninhabited `T` rejects every value); an absent field still
    // takes `default`. A nullable one keeps plain `Option`, since there `null` is an admitted value.
    let decode_present = (!field.required && !field.ty.nullable).then(|| {
        let path = DECODE_PRESENT;
        quote! { deserialize_with = #path, }
    });
    let mut notes: Vec<String> = Vec::new();
    if field.deprecated {
        notes.push("Deprecated per the spec.".to_owned());
    }
    if field.read_only {
        notes.push("Read-only: set by the server; ignored in requests.".to_owned());
    }
    if field.write_only {
        notes.push("Write-only: sent in requests; absent from responses.".to_owned());
    }
    if let Some(default) = &field.default {
        notes.push(default.doc_note.clone());
    }
    let notes = notes
        .iter()
        .map(|note| normalize_rustdoc(note))
        .map(|note| quote! { #[doc = #note] });
    quote! {
        #(#notes)*
        #[serde(rename = #wire, #serde_default #decode_present)]
        pub #ident: #ty,
    }
}

/// The name of the private function `types` carries for every optional, non-nullable field: a
/// `deserialize_with` target that decodes a present value, `null` included, as the field's own
/// type. Snake case, so it cannot collide with a `PascalCase` type, and not of the
/// `default_<id>_<field>` shape a default provider takes.
const DECODE_PRESENT: &str = "decode_present";

/// The deterministic identifier of a field's generated serde default-provider function. Derived
/// from the owning type's dense id plus the field's Rust identifier, so it is stable across runs
/// and cannot collide with a `PascalCase` type ident or another field's provider.
fn default_provider_ident(
    id: crate::ir::TypeId,
    field_ident: &crate::name::Ident,
) -> proc_macro2::Ident {
    format_ident!(
        "default_{}_{}",
        id.0,
        field_ident.as_str().trim_start_matches("r#")
    )
}

/// Emit a field's serde default-provider function, when its `default` is a representable scalar
/// wired through serde. The function returns `Option<T>` matching the (optional) field's Rust type.
fn emit_default_provider(
    id: crate::ir::TypeId,
    field: &Field,
    names: &Names,
) -> Option<TokenStream> {
    let applied = field.default.as_ref()?.applied.as_ref()?;
    let field_ident = names
        .fields
        .get(&(id, field.name.wire.clone()))
        .expect("field name allocated");
    let fn_ident = default_provider_ident(id, field_ident);
    let inner_ty = ty_tokens(field.ty, names, false);
    let value = default_value_tokens(applied, field.ty, names);
    Some(quote! {
        fn #fn_ident() -> Option<#inner_ty> {
            Some(#value)
        }
    })
}

/// Render a representable default as a Rust literal (or generated enum variant) for the field's
/// Rust type.
fn default_value_tokens(value: &crate::ir::DefaultValue, ty: Ty, names: &Names) -> TokenStream {
    use crate::ir::DefaultValue;
    match value {
        DefaultValue::Bool(value) => quote! { #value },
        DefaultValue::Int(value) => {
            let literal = proc_macro2::Literal::i64_unsuffixed(*value);
            quote! { #literal }
        }
        DefaultValue::Float(value) => {
            let literal = proc_macro2::Literal::f64_unsuffixed(*value);
            quote! { #literal }
        }
        DefaultValue::Str(value) => quote! { #value.to_owned() },
        DefaultValue::EnumVariant(value) => {
            let enum_ident = names.types.get(&ty.id).expect("enum type name allocated");
            let variant_ident = names
                .variants
                .get(&(ty.id, value.clone()))
                .expect("variant name allocated");
            quote! { #enum_ident::#variant_ident }
        }
    }
}

/// The type an unnamed definition (a primitive, array, tuple, bytes, null, or any) aliases.
fn type_kind_tokens(kind: &TypeKind, names: &Names, options: &CodegenOptions) -> TokenStream {
    match kind {
        TypeKind::Primitive(prim) => prim_tokens(*prim, options),
        TypeKind::Array(ty) => {
            let ty = ty_tokens(**ty, names, false);
            quote! { Vec<#ty> }
        }
        TypeKind::Tuple(items) => {
            let items = items.iter().map(|ty| ty_tokens(*ty, names, false));
            // Every position carries its comma: `(T,)` is a one-position tuple, while `(T)` is a
            // parenthesized `T` that decodes from a bare scalar instead of a one-element array.
            quote! { (#(#items,)*) }
        }
        TypeKind::Bytes => quote! { bytes::Bytes },
        TypeKind::Null => quote! { () },
        TypeKind::Any => quote! { serde_json::Value },
        // Codegen runs only on an `Api` that passed `check_invariants`, which rejects a surviving
        // reservation, so reaching here means a reserved id was never filled. Emitting anything at
        // all would put a shape on the wire that was never computed — which is how this variant's
        // predecessor produced `serde_json::Value` for a typed schema, silently, four times over.
        TypeKind::Reserved => {
            unreachable!(
                "a reservation reached codegen; `check_invariants` should have rejected it"
            )
        }
        TypeKind::Struct(_) | TypeKind::Enum(_) | TypeKind::Never | TypeKind::Union(_) => {
            unreachable!("named definitions emitted separately")
        }
    }
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::CodegenOptions;
    use crate::diag::{Diagnostics, JsonPointer, Provenance};
    use crate::ir::{Api, Docs, Info, Prim, Ty, TypeDef, TypeGraph, TypeKind};

    /// Every capitalised identifier in `tokens` that is written bare — not after `::` or `.`, and
    /// not inside an attribute — appended to `into`.
    fn bare_capitalised_idents(tokens: proc_macro2::TokenStream, into: &mut Vec<String>) {
        use proc_macro2::TokenTree;
        let mut previous: Option<TokenTree> = None;
        let mut before_previous: Option<TokenTree> = None;
        for token in tokens {
            match &token {
                TokenTree::Group(group) => {
                    let in_attribute = matches!(&previous, Some(TokenTree::Punct(p)) if p.as_char() == '#')
                        && group.delimiter() == proc_macro2::Delimiter::Bracket;
                    if !in_attribute {
                        bare_capitalised_idents(group.stream(), into);
                    }
                }
                TokenTree::Ident(ident) => {
                    let name = ident.to_string();
                    let after_path_separator = matches!(
                        (&before_previous, &previous),
                        (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b)))
                            if a.as_char() == ':' && b.as_char() == ':'
                    );
                    let after_dot =
                        matches!(&previous, Some(TokenTree::Punct(p)) if p.as_char() == '.');
                    if name.starts_with(|c: char| c.is_ascii_uppercase())
                        && !after_path_separator
                        && !after_dot
                    {
                        into.push(name);
                    }
                }
                TokenTree::Punct(_) | TokenTree::Literal(_) => {}
            }
            before_previous = previous.replace(token);
        }
    }

    /// The name every leaf of `tree` brings into scope, appended to `into`.
    fn use_leaves(tree: &syn::UseTree, into: &mut Vec<String>) {
        match tree {
            syn::UseTree::Path(path) => use_leaves(&path.tree, into),
            syn::UseTree::Name(name) => into.push(name.ident.to_string()),
            syn::UseTree::Rename(rename) => into.push(rename.rename.to_string()),
            syn::UseTree::Group(group) => {
                group.items.iter().for_each(|tree| use_leaves(tree, into))
            }
            syn::UseTree::Glob(_) => panic!("a glob import brings in names no list can reserve"),
        }
    }

    /// What the `types` module emitted for one alias per `(hint, kind)` beside a `Text` string
    /// definition: the names its `use` items bring in, the capitalised names it writes bare (the
    /// emitted functions' own type parameters excluded), and the names it defines.
    struct TypesModuleNames {
        imported: Vec<String>,
        written: Vec<String>,
        defined: std::collections::BTreeSet<String>,
    }

    /// The alias kinds [`types_module_names`] can emit: a date, a date-time, or a nullable boxed
    /// array of the `Text` definition, which writes `Vec`, `Box`, `Option`, and `String` bare.
    #[derive(Clone, Copy)]
    enum AliasKind {
        Date,
        DateTime,
        List,
    }

    fn types_module_names(aliases: &[(&str, AliasKind)]) -> TypesModuleNames {
        use quote::ToTokens;

        let mut types = TypeGraph::default();
        let mut define = |name: &str, kind| {
            types.insert(TypeDef {
                name_hint: name.to_owned(),
                kind,
                docs: Docs::default(),
                provenance: Provenance::new(JsonPointer::root().push(name), None),
                document: String::new(),
            })
        };
        let text = define("Text", TypeKind::Primitive(Prim::String));
        for (name, kind) in aliases {
            let kind = match kind {
                AliasKind::Date => TypeKind::Primitive(Prim::Date),
                AliasKind::DateTime => TypeKind::Primitive(Prim::DateTime),
                AliasKind::List => TypeKind::Array(Box::new(Ty {
                    id: text,
                    nullable: true,
                    boxed: true,
                })),
            };
            define(name, kind);
        }
        let api = Api {
            info: Info {
                title: "T".to_owned(),
                version: "1".to_owned(),
                description: None,
            },
            servers: Vec::new(),
            operations: Vec::new(),
            types,
            security_schemes: IndexMap::new(),
        };
        let names = crate::name::allocate(&api, &mut Diagnostics::default());
        let file: syn::File = syn::parse2(super::emit_models(
            &api,
            &names,
            &CodegenOptions::default(),
            api.uses_time(),
        ))
        .expect("the emitted types module parses");
        let [syn::Item::Mod(module)] = file.items.as_slice() else {
            panic!("the models are emitted as one module");
        };
        let (_, items) = module.content.as_ref().expect("the module is inline");

        let mut imported = Vec::new();
        let mut written = Vec::new();
        let mut defined = std::collections::BTreeSet::new();
        for item in items {
            match item {
                syn::Item::Use(import) => use_leaves(&import.tree, &mut imported),
                syn::Item::Type(alias) => {
                    defined.insert(alias.ident.to_string());
                    bare_capitalised_idents(alias.ty.to_token_stream(), &mut written);
                }
                syn::Item::Fn(function) => {
                    let generics: Vec<String> = function
                        .sig
                        .generics
                        .type_params()
                        .map(|param| param.ident.to_string())
                        .collect();
                    let mut idents = Vec::new();
                    bare_capitalised_idents(function.to_token_stream(), &mut idents);
                    written.extend(idents.into_iter().filter(|ident| !generics.contains(ident)));
                }
                other => panic!(
                    "an unexpected item in the types module: {}",
                    other.to_token_stream()
                ),
            }
        }
        TypesModuleNames {
            imported,
            written,
            defined,
        }
    }

    /// Issue #356: a model is emitted into the `types` module beside that module's own imports and
    /// the prelude types it writes bare, so a schema hinted `Date` redefined the runtime `Date` the
    /// module imports (`E0255`). `name::TYPES_MODULE_NAMES` must hold every name the module brings
    /// in by `use` and every capitalised type-namespace name it writes bare. The first module's
    /// definitions spell none of those names, so every such use is one the list must reserve; both
    /// date types are used, so the date import is emitted.
    #[test]
    fn types_module_names_cover_every_name_the_types_module_uses() {
        let used = types_module_names(&[
            ("Day", AliasKind::Date),
            ("Moment", AliasKind::DateTime),
            ("Texts", AliasKind::List),
        ]);
        for date in ["Date", "DateTime"] {
            assert!(
                used.imported.iter().any(|name| name == date),
                "the fixture must make the `types` module import `{date}`: {:?}",
                used.imported
            );
        }
        for bare in ["Box", "Option", "Result", "String", "Vec"] {
            assert!(
                used.written.iter().any(|name| name == bare),
                "the walk must see the bare `{bare}` the fixture emits: {:?}",
                used.written
            );
        }
        // Enum variants of the prelude live in the value namespace, which no emitted model occupies.
        let prelude_variants = ["Some", "None", "Ok", "Err", "Self"];
        for name in used
            .imported
            .iter()
            .chain(used.written.iter().filter(|name| {
                !prelude_variants.contains(&name.as_str()) && !used.defined.contains(*name)
            }))
        {
            assert!(
                crate::name::TYPES_MODULE_NAMES.contains(&name.as_str()),
                "the `types` module uses `{name}`, which `name::TYPES_MODULE_NAMES` does not reserve"
            );
        }
    }

    /// A definition whose hint spells a name the `types` module uses yields it, whichever of the
    /// shapes the definition has, and the module still imports the date types it needs.
    #[test]
    fn a_definition_spelling_a_types_module_name_yields_it() {
        let aliases: Vec<(&str, AliasKind)> = crate::name::TYPES_MODULE_NAMES
            .iter()
            .map(|name| {
                let kind = match *name {
                    "Date" => AliasKind::Date,
                    "DateTime" => AliasKind::DateTime,
                    _ => AliasKind::List,
                };
                (*name, kind)
            })
            .collect();
        let used = types_module_names(&aliases);
        assert_eq!(
            used.defined.len(),
            aliases.len() + 1,
            "every alias and `Text` must be defined once: {:?}",
            used.defined
        );
        for name in crate::name::TYPES_MODULE_NAMES {
            assert!(
                !used.defined.contains(*name),
                "a definition kept the bare spelling `{name}` the `types` module already uses"
            );
        }
        for date in ["Date", "DateTime"] {
            assert!(used.imported.iter().any(|name| name == date));
        }
    }
}
