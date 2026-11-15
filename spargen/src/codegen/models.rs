//! The `types` module: one model per emitted definition — structs, enums, aliases, and (through
//! [`union`](super::union)) union enums — with their serde wiring.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::ir::{
    AdditionalProps, Api, BytesEncoding, Emission, Field, ScalarValue, Ty, TypeDef, TypeGraph,
    TypeKind,
};
use crate::name::Names;

use super::docs::{doc_tokens, normalize_rustdoc};
use super::operation::takes_into;
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
        .map(|(id, def)| emit_type_def(id, def, &api.types, names, options));
    let decode_present = format_ident!("{DECODE_PRESENT}");
    let decode_required = format_ident!("{DECODE_REQUIRED}");
    // The RFC 3339 newtypes live beside `types`, so bring them into scope under the same bare names
    // `prim_tokens` emits; at the generated root the prelude re-export supplies them instead.
    let datetime_import = uses_time.then(|| {
        quote! { use super::{Date, DateTime}; }
    });
    quote! {
        #[forbid(unsafe_code)]
        #[allow(dead_code, deprecated, unused_imports)]
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

            /// The `deserialize_with` of every required, nullable field but an XML attribute, which
            /// writes `null` by omission. It decodes a present value as `Option<T>` does, `null` as
            /// `None`. A field with a `deserialize_with` and no `default` is one serde refuses as
            /// `missing field` when it is absent, where a bare `Option<T>` field would read the
            /// absent member as `None`.
            fn #decode_required<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
            where
                D: serde::Deserializer<'de>,
                T: serde::Deserialize<'de>,
            {
                <Option<T> as serde::Deserialize<'de>>::deserialize(deserializer)
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
/// `listed` are the [`string_variants`] the closed enum declares, and `display_arms` its `Display`
/// arms, which map each unit variant to its value.
fn emit_open_string_enum(
    id: crate::ir::TypeId,
    ident: &crate::name::Ident,
    listed: &[(&String, &crate::name::Ident)],
    names: &Names,
    docs: TokenStream,
    deprecated: Option<TokenStream>,
    display_arms: Vec<TokenStream>,
) -> TokenStream {
    let other = names
        .open_variants
        .get(&id)
        .expect("open variant name allocated");
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

/// Emit a closed string enum: one unit variant per listed value, serde-derived under each value's
/// `rename`. Every variant is a unit, so it is `Copy` and `Hash`; `as_str` is the `const` wire
/// value `Display` writes, `ALL` lists the variants in declaration order, and `FromStr` accepts
/// exactly the listed values, failing with the runtime's `UnknownVariant`. The serde renames, the
/// `as_str` arms and the `FromStr` arms all read `listed`, so the three agree on every value.
///
/// `variants` are the variant declarations and `display_arms` map each variant to its value, as
/// the open enum reads them too.
fn emit_closed_string_enum(
    ident: &crate::name::Ident,
    listed: &[(&String, &crate::name::Ident)],
    docs: TokenStream,
    deprecated: Option<TokenStream>,
    variants: impl Iterator<Item = TokenStream>,
    display_arms: impl Iterator<Item = TokenStream>,
) -> TokenStream {
    let all = listed
        .iter()
        .map(|(_, variant_ident)| quote! { #ident::#variant_ident, });
    let parse_arms = listed.iter().map(|(value, variant_ident)| {
        quote! { #value => Ok(#ident::#variant_ident), }
    });
    let type_name = ident.as_str();
    // Reserved in the variant scope, so no variant shadows it.
    let all_const = format_ident!("{}", crate::name::CLOSED_ENUM_ALL);
    quote! {
        #docs
        #deprecated
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum #ident {
            #(#variants)*
        }

        impl #ident {
            /// Every variant, in the order the description lists its values.
            pub const #all_const: &'static [Self] = &[#(#all)*];

            /// The wire value.
            pub const fn as_str(self) -> &'static str {
                match self {
                    #(#display_arms)*
                }
            }
        }

        impl std::fmt::Display for #ident {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for #ident {
            type Err = super::support::UnknownVariant;

            /// Parse a wire value: exactly one of the listed values, matched byte for byte.
            fn from_str(value: &str) -> Result<Self, super::support::UnknownVariant> {
                match value {
                    #(#parse_arms)*
                    _ => Err(super::support::UnknownVariant::new(#type_name, value)),
                }
            }
        }
    }
}

/// Each listed value of string enum `id` beside the variant identifier `name` allocated for it,
/// in declaration order. An open enum is a string enum too, so both shapes read their variants
/// here.
fn string_variants<'a>(
    id: crate::ir::TypeId,
    enumeration: &'a crate::ir::ScalarEnum,
    names: &'a Names,
) -> Vec<(&'a String, &'a crate::name::Ident)> {
    enumeration
        .variants
        .iter()
        .map(|variant| {
            let ScalarValue::String(value) = variant else {
                unreachable!("a string enum lists only string values");
            };
            let variant_ident = names
                .variants
                .get(&(id, value.clone()))
                .expect("variant name allocated");
            (value, variant_ident)
        })
        .collect()
}

/// Emit the model for one definition: a struct, a string enum (closed or open), an integer or
/// boolean enum's alias, an uninhabited enum, a union enum, an alias of the type it lowered to, or
/// — for an array or tuple that [closes an alias cycle](TypeGraph::closes_alias_cycle) — a
/// transparent newtype over it.
fn emit_type_def(
    id: crate::ir::TypeId,
    def: &TypeDef,
    types: &TypeGraph,
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
    let ident = names.types.get(&id).expect("type name allocated");
    let docs = doc_tokens(&def.docs, options);
    let deprecated = def.docs.deprecated.then(|| quote! { #[deprecated] });
    match types.emission(id, def) {
        Emission::Struct(object) => {
            let deny_unknown = matches!(object.additional, AdditionalProps::Deny)
                .then(|| quote! { #[serde(deny_unknown_fields)] });
            // A root `xml.name`, or a component's inferred one, names the element quick-xml writes
            // for this struct at the root of an XML body. serde hands a struct's (renamed) name to
            // the serializer, and only quick-xml reads it, so this changes no other wire. A name
            // equal to the identifier is the serde name already.
            let xml_root = types
                .xml_root_name(id)
                .filter(|name| *name != ident.as_str())
                .map(|name| quote! { #[serde(rename = #name)] });
            let fields = object
                .fields
                .iter()
                .map(|field| emit_field(id, field, names, options.testing));
            let (additional, serialize_overflow) = match &object.additional {
                AdditionalProps::Typed(ty) => emit_overflow(id, ident, object, **ty, names),
                AdditionalProps::Allow | AdditionalProps::Deny => (quote! {}, quote! {}),
            };
            let StructConstructors {
                derive_default,
                constructors,
            } = emit_struct_constructors(id, object, types, names, deprecated.as_ref());
            quote! {
                #docs
                #deprecated
                #[derive(Debug, Clone, PartialEq, #derive_default Serialize, Deserialize)]
                #xml_root
                #deny_unknown
                pub struct #ident {
                    #(#fields)*
                    #additional
                }
                #constructors
                #serialize_overflow
            }
        }
        Emission::StringEnum(enumeration) => {
            let listed = string_variants(id, enumeration, names);
            let variants = listed.iter().map(|(value, variant_ident)| {
                quote! { #[serde(rename = #value)] #variant_ident, }
            });
            let display_arms = listed.iter().map(|(value, variant_ident)| {
                quote! { #ident::#variant_ident => #value, }
            });
            if enumeration.is_open() {
                return emit_open_string_enum(
                    id,
                    ident,
                    &listed,
                    names,
                    docs,
                    deprecated,
                    display_arms.collect(),
                );
            }
            emit_closed_string_enum(ident, &listed, docs, deprecated, variants, display_arms)
        }
        Emission::Checked(set) => emit_checked_newtype(ident, set, docs, deprecated, options),
        Emission::BoolAlias => quote! { #docs #deprecated pub type #ident = bool; },
        Emission::Never => {
            let error = format!("no JSON value can inhabit schema {}", ident.as_str());
            quote! {
                #docs
                #deprecated
                #[derive(Debug, Clone, PartialEq)]
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
        Emission::Union(union) => emit_union(id, ident, union, types, names, docs, deprecated),
        Emission::Base64 => emit_base64_newtype(ident, docs, deprecated, Base64Codec::Standard),
        Emission::Base64Url => emit_base64_newtype(ident, docs, deprecated, Base64Codec::UrlSafe),
        // An array or tuple that reaches itself again through aliases alone cannot be an alias:
        // rustc expands an alias eagerly, so `pub type Node = Vec<Node>;` is a cycle (`E0391`). A
        // transparent newtype is nominal, which stops the expansion, and keeps the same wire form.
        Emission::Newtype => {
            let ty = type_kind_tokens(&def.kind, names, options);
            quote! {
                #docs
                #deprecated
                #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
                #[serde(transparent)]
                pub struct #ident(pub #ty);
            }
        }
        Emission::Alias => {
            let ty = type_kind_tokens(&def.kind, names, options);
            quote! { #docs #deprecated pub type #ident = #ty; }
        }
    }
}

/// The associated `const` naming one listed value of a checked newtype: `V3` for `3`, `V_MINUS_3`
/// for `-3`, and `TRUE` or `FALSE` for a boolean. Distinct values have distinct names, and none is
/// `ALL`.
fn checked_const_ident(value: &ScalarValue) -> proc_macro2::Ident {
    match value {
        ScalarValue::Int(int) if *int < 0 => format_ident!("V_MINUS_{}", int.unsigned_abs()),
        ScalarValue::Int(int) => format_ident!("V{int}"),
        ScalarValue::Bool(true) => format_ident!("TRUE"),
        ScalarValue::Bool(false) => format_ident!("FALSE"),
        ScalarValue::String(_) => unreachable!("a string set is emitted as a `pub enum`"),
    }
}

/// The Rust literal of one listed value of a checked newtype.
fn checked_literal(value: &ScalarValue) -> TokenStream {
    match value {
        ScalarValue::Int(int) => {
            let literal = proc_macro2::Literal::i64_unsuffixed(*int);
            quote! { #literal }
        }
        ScalarValue::Bool(boolean) => quote! { #boolean },
        ScalarValue::String(_) => unreachable!("a string set is emitted as a `pub enum`"),
    }
}

/// Emit an integer or boolean `enum`/`const` set as a checked newtype over its
/// [width](crate::ir::ScalarEnum::width) (#1327): a private field, an associated `const` per listed
/// value and `ALL` listing them in declaration order, `get`, `Display`, `From` into the width,
/// and a `TryFrom` from it that accepts exactly the listed values, failing with the runtime's
/// `UnknownVariant`. Serde writes the bare value and reads it through `TryFrom`, so a value the set
/// does not list is a decode error; an open set (`open_narrowing`) decodes any value of its width.
fn emit_checked_newtype(
    ident: &crate::name::Ident,
    set: &crate::ir::ScalarEnum,
    docs: TokenStream,
    deprecated: Option<TokenStream>,
    options: &CodegenOptions,
) -> TokenStream {
    let width = prim_tokens(set.width, options);
    let type_name = ident.as_str();
    let listed: Vec<String> = set
        .variants
        .iter()
        .map(|value| format!("`{}`", value.wire()))
        .collect();
    let listing = if set.is_open() {
        format!(
            "Lists {}. Decoding also accepts any other value; `TryFrom` accepts only the listed \
             ones.",
            listed.join(", ")
        )
    } else {
        format!("Holds exactly one of: {}.", listed.join(", "))
    };
    let separator = (!docs.is_empty()).then(|| quote! { #[doc = ""] });
    let consts = set.variants.iter().map(|value| {
        let name = checked_const_ident(value);
        let literal = checked_literal(value);
        let doc = format!("The value `{}`.", value.wire());
        quote! {
            #[doc = #doc]
            pub const #name: Self = Self(#literal);
        }
    });
    let all = set.variants.iter().map(checked_const_ident);
    let decode = if set.is_open() {
        quote! { Ok(Self(value)) }
    } else {
        quote! { Self::try_from(value).map_err(serde::de::Error::custom) }
    };
    quote! {
        #docs
        #separator
        #[doc = #listing]
        #deprecated
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct #ident(#width);

        impl #ident {
            #(#consts)*

            /// Every listed value, in the order the description lists them.
            pub const ALL: &'static [Self] = &[#(Self::#all,)*];

            /// The value.
            pub const fn get(self) -> #width {
                self.0
            }
        }

        impl std::fmt::Display for #ident {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0, f)
            }
        }

        impl From<#ident> for #width {
            fn from(value: #ident) -> Self {
                value.0
            }
        }

        impl TryFrom<#width> for #ident {
            type Error = super::support::UnknownVariant;

            /// Accept exactly one of the listed values.
            fn try_from(value: #width) -> Result<Self, super::support::UnknownVariant> {
                if Self::ALL.iter().any(|listed| listed.0 == value) {
                    Ok(Self(value))
                } else {
                    Err(super::support::UnknownVariant::new(#type_name, value.to_string()))
                }
            }
        }

        impl serde::Serialize for #ident {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serde::Serialize::serialize(&self.0, serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for #ident {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = <#width as serde::Deserialize>::deserialize(deserializer)?;
                #decode
            }
        }
    }
}

/// Which of the embedded runtime's base64 codecs a newtype's serde impls call.
#[derive(Debug, Clone, Copy)]
enum Base64Codec {
    /// `contentEncoding: base64`, RFC 4648 §4.
    Standard,
    /// `contentEncoding: base64url`, RFC 4648 §5.
    UrlSafe,
}

/// Emit a `contentEncoding: base64` or `base64url` schema: a newtype over `bytes::Bytes` whose
/// serde form is the text the embedded runtime's `codec` writes and reads (#1230, #1417), so the
/// value is that text wherever a serde-encoded body carries it. It `Deref`s to the bytes and
/// converts with `From` in both directions; a multipart part and a raw body take the bytes
/// themselves.
fn emit_base64_newtype(
    ident: &crate::name::Ident,
    docs: TokenStream,
    deprecated: Option<TokenStream>,
    codec: Base64Codec,
) -> TokenStream {
    let (serialize, deserialize) = match codec {
        Base64Codec::Standard => (
            format_ident!("serialize_base64"),
            format_ident!("deserialize_base64"),
        ),
        Base64Codec::UrlSafe => (
            format_ident!("serialize_base64url"),
            format_ident!("deserialize_base64url"),
        ),
    };
    quote! {
        #docs
        #deprecated
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
        pub struct #ident(pub bytes::Bytes);

        impl std::ops::Deref for #ident {
            type Target = bytes::Bytes;

            fn deref(&self) -> &bytes::Bytes {
                &self.0
            }
        }

        impl From<bytes::Bytes> for #ident {
            fn from(bytes: bytes::Bytes) -> Self {
                #ident(bytes)
            }
        }

        impl From<#ident> for bytes::Bytes {
            fn from(value: #ident) -> Self {
                value.0
            }
        }

        impl serde::Serialize for #ident {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                super::support::#serialize(&self.0, serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for #ident {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                super::support::#deserialize(deserializer).map(#ident)
            }
        }
    }
}

/// The name of the private associated function a struct with declared members and a typed
/// overflow map carries: the overflow's `serialize_with`. The struct's other methods are `new`,
/// the `with_` setters and the `_or_default` accessors, and this name is none of them.
const SERIALIZE_OVERFLOW: &str = "serialize_overflow";

/// A struct's typed `additionalProperties` field, flattened into the struct's object, and the
/// `impl` block that carries its `serialize_with`, if it has one. An overflow entry whose key is a
/// declared member's name is not written, whether that member is set or not, so the object never
/// carries a key twice and the overflow never stands in for a declared member, the rule the form
/// and multipart encoders follow. A declared name is the one serde writes the member under: the
/// property's name, or the name an XML hint gives it, as [`emit_field`] renames it. Decoding never
/// puts a key serde reads as a declared member into the map, so a decoded value writes back what
/// it read.
fn emit_overflow(
    id: crate::ir::TypeId,
    ident: &crate::name::Ident,
    object: &crate::ir::Struct,
    value_ty: Ty,
    names: &Names,
) -> (TokenStream, TokenStream) {
    let ty = ty_tokens(value_ty, names, false);
    let overflow = names
        .struct_overflow
        .get(&id)
        .expect("overflow field name allocated");
    let declared = overflow_skipped_keys(object);
    if declared.is_empty() {
        return (
            quote! { #[serde(flatten)] pub #overflow: BTreeMap<String, #ty>, },
            quote! {},
        );
    }
    let function = format_ident!("{SERIALIZE_OVERFLOW}");
    let path = format!("{ident}::{SERIALIZE_OVERFLOW}");
    let field = quote! {
        #[serde(flatten, serialize_with = #path)]
        pub #overflow: BTreeMap<String, #ty>,
    };
    let serialize = quote! {
        impl #ident {
            /// The overflow map's `serialize_with`: every entry but one whose key is a declared
            /// member's name, which only the member itself is written under.
            fn #function<S>(overflow: &BTreeMap<String, #ty>, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.collect_map(
                    overflow
                        .iter()
                        .filter(|(key, _)| !matches!(key.as_str(), #(#declared)|*)),
                )
            }
        }
    };
    (field, serialize)
}

/// The overflow keys struct `object`'s serde encoding leaves out: each declared member's serde
/// name, once, in declaration order. Empty when the struct declares no member.
fn overflow_skipped_keys(object: &crate::ir::Struct) -> Vec<String> {
    let mut declared: Vec<String> = Vec::new();
    for field in &object.fields {
        let name = serde_wire(field);
        if !declared.contains(&name) {
            declared.push(name);
        }
    }
    declared
}

/// The name serde writes and reads struct field `field` under. An `xml.name`/`xml.attribute` hint
/// overrides it for XML bodies (an attribute uses quick-xml's `@name` convention); otherwise it is
/// the plain property wire name. The Rust identifier and the names-table key stay keyed off the
/// original property name, so only the wire string changes.
fn serde_wire(field: &Field) -> String {
    field
        .xml
        .wire_override(&field.name.wire)
        .unwrap_or_else(|| field.name.wire.clone())
}

/// One struct field: its rustdoc notes, its serde attribute (wire name, default, the skip a
/// required nullable XML attribute's `null` takes, and the present-value decode an optional
/// non-nullable field takes), and its declaration. `testing` is
/// whether the `testing` module is emitted, whose typed replies serialize a `readOnly` or
/// `writeOnly` member in the response direction.
fn emit_field(id: crate::ir::TypeId, field: &Field, names: &Names, testing: bool) -> TokenStream {
    let ident = names
        .fields
        .get(&(id, field.name.wire.clone()))
        .expect("field name allocated");
    let wire = serde_wire(field);
    let ty = field_ty_tokens(field, names);
    // An optional field always deserializes an absent value as the type's `Default`: `None` for an
    // `Option`, and `Nullable::Absent` for a nullable member, a schema `default` included, which
    // only its `{field}_or_default` accessor reads. The absent state is skipped on the wire, so a
    // decoded value re-sends no member the payload left out.
    let skip = if field.ty.nullable {
        NULLABLE_IS_ABSENT
    } else {
        "Option::is_none"
    };
    // A request leaves a `readOnly` member out, `required` or not. Under `testing`, a typed reply
    // encodes in the response direction, which writes a present `readOnly` member and leaves a
    // `writeOnly` one out, so each such member asks the `testing` module which direction it is
    // serializing in. A member marked both is never written in either.
    let nullable = field.ty.nullable;
    let skip = match (field.read_only, field.write_only) {
        (true, true) => quote! { skip_serializing, },
        (true, false) if testing => {
            let path = if nullable {
                "super::testing::skip_read_only_nullable"
            } else {
                "super::testing::skip_read_only"
            };
            quote! { skip_serializing_if = #path, }
        }
        (true, false) => quote! { skip_serializing, },
        (false, true) if testing => {
            let path = if nullable {
                "super::testing::skip_write_only_nullable"
            } else {
                "super::testing::skip_write_only"
            };
            quote! { skip_serializing_if = #path, }
        }
        _ => quote! { skip_serializing_if = #skip, },
    };
    // A required nullable XML attribute writes its `null` by leaving the attribute out (OAS
    // §Handling `null` Values), never as an empty `name=""` no reader recovers as `null`; decoding
    // reads that absent attribute back as `None`, below.
    let serde_default = if field.decodes_optional() {
        quote! { default, #skip }
    } else if field.ty.nullable && field.xml.attribute {
        quote! { skip_serializing_if = "Option::is_none", }
    } else {
        quote! {}
    };
    // The `Option` wrapping an optional non-nullable field stands for absence only, yet serde's
    // `Option<T>` maps a JSON `null` to `None` without ever calling `T::deserialize`, so
    // `{"x": null}` would decode as absent (and re-serialise as `{}`) although the schema admits no
    // `null` there. Route every *present* value, `null` included, to `T`'s own deserializer, which
    // rejects it wherever `T` does (an uninhabited `T` rejects every value); an absent field still
    // takes `default`. A nullable one is a `Nullable`, whose own decode reads `null` as `Null`, an
    // admitted value there.
    //
    // A required nullable field is an `Option<T>` too, its `None` a present `null`. serde-derive
    // reads a missing `Option` field as `None` unless the field has a `deserialize_with`, so an
    // absent member would decode as an explicit `null`. `decode_required` decodes exactly as
    // `Option<T>` does and, with no `default` beside it, makes the absent member `missing field`.
    // An XML attribute is the exception: OpenAPI writes a `null` attribute by omitting it
    // (§Handling `null` Values), so an absent required nullable attribute is that `null`, and
    // keeps serde's absent-`Option`-is-`None` reading.
    let decode_with = match (field.decodes_optional(), field.ty.nullable) {
        (true, false) => Some(DECODE_PRESENT),
        (false, true) if !field.xml.attribute => Some(DECODE_REQUIRED),
        _ => None,
    }
    .map(|path| quote! { deserialize_with = #path, });
    let notes = field_notes(field);
    let notes = notes
        .iter()
        .map(|note| normalize_rustdoc(note))
        .map(|note| quote! { #[doc = #note] });
    quote! {
        #(#notes)*
        #[serde(rename = #wire, #serde_default #decode_with)]
        pub #ident: #ty,
    }
}

/// The rustdoc notes struct field `field` carries, on the struct and on its `…Required` struct
/// alike: its deprecation, `readOnly`, `writeOnly` and schema `default`.
fn field_notes(field: &Field) -> Vec<String> {
    let mut notes: Vec<String> = Vec::new();
    if field.deprecated {
        notes.push("Deprecated per the spec.".to_owned());
    }
    if field.read_only {
        notes.push("Read-only: the server sets it, so it is never sent in a request.".to_owned());
    }
    if field.write_only {
        notes.push(
            "Write-only: the server accepts it in requests and omits it from responses, so it \
             decodes as absent."
                .to_owned(),
        );
    }
    if let Some(default) = &field.default {
        notes.push(default.doc_note.clone());
    }
    notes
}

/// `ty` with its nullability dropped.
fn non_null(ty: Ty) -> Ty {
    Ty {
        nullable: false,
        ..ty
    }
}

/// A struct field's declared Rust type.A required field is the type `ty_tokens` names, which
/// wraps a nullable one (`"null"` in the type array) in `Option`: present, but maybe `null`. An
/// optional non-nullable field is `Option<T>`, its `None` the absent member. An optional nullable
/// field has three wire states (absent, `null`, a value), so it is `support::Nullable<T>` over the
/// non-nullable type. A `required` field marked `readOnly` or `writeOnly` is optional here
/// ([`Field::decodes_optional`]).
fn field_ty_tokens(field: &Field, names: &Names) -> TokenStream {
    if !field.decodes_optional() {
        ty_tokens(field.ty, names, false)
    } else if field.ty.nullable {
        nullable_tokens(non_null_ty_tokens(field.ty, names))
    } else {
        let ty = ty_tokens(field.ty, names, false);
        quote! { Option<#ty> }
    }
}

/// The type `ty` names inside `types` with its nullability dropped: the `T` an optional member's
/// setter takes, and the payload of a `Nullable` field.
fn non_null_ty_tokens(ty: Ty, names: &Names) -> TokenStream {
    ty_tokens(
        Ty {
            nullable: false,
            ..ty
        },
        names,
        false,
    )
}

/// `support::Nullable<inner>`, spelled from inside `types`.
fn nullable_tokens(inner: TokenStream) -> TokenStream {
    quote! { super::support::Nullable<#inner> }
}

/// The `skip_serializing_if` path of an optional nullable field: an absent member is left off the
/// wire, while an explicit `null` is written.
const NULLABLE_IS_ABSENT: &str = "super::support::Nullable::is_absent";

/// What [`emit_struct_constructors`] adds to an object struct: `Default` for its derive list, when
/// the derive builds what `new` does, and the items following the struct.
struct StructConstructors {
    derive_default: Option<TokenStream>,
    constructors: TokenStream,
}

/// The inherent `impl` of object struct `id`: `new`, taking every [constructed] member by value in
/// declaration order (every required member but a `readOnly` one; a `writeOnly` one is stored as
/// `Some` of it) and setting every other member as decoding an absent one does (`None` or
/// `Nullable::Absent`, a schema `default` included, and the overflow map empty), then one
/// consuming `with_<field>` setter per member `new` does not take and for the overflow map, in
/// field order, then one `{field}_or_default` accessor per member with an
/// [applied default](Field::applied_default) ([`default_accessor`]). A non-nullable member's
/// setter takes the bare `T` by value, as the `…Params` setters take theirs, and stores `Some` of
/// it. A nullable member's takes
/// `impl Into<Nullable<T>>`, so a value or `Nullable::Null` fits, and stores what it converts to;
/// `Nullable` has no `From<Option<T>>`, so a `None` meaning "unset" never becomes a `null` that
/// clears the member. Its `with_<field>_null()` setter stores that explicit `null`. The overflow
/// map's takes the whole map.
///
/// So `new` with the constructed members equals what decoding an object of just those members
/// yields, and so does serializing it: a member left unset is left off the wire. With no
/// constructed member, `Default` is derived, and builds what `new()` does. Setter names are
/// `with_` before the field identifier, or the null setter's stem ([`Names::null_setters`]),
/// unraw, so they are distinct exactly when those identifiers are, which allocation makes them
/// within one struct; the accessors are allocated after them ([`Names::default_accessors`]).
///
/// [constructed]: Field::constructed
fn emit_struct_constructors(
    id: crate::ir::TypeId,
    object: &crate::ir::Struct,
    types: &TypeGraph,
    names: &Names,
    deprecated: Option<&TokenStream>,
) -> StructConstructors {
    let ident = names.types.get(&id).expect("type name allocated");
    let field_ident = |field: &Field| {
        names
            .fields
            .get(&(id, field.name.wire.clone()))
            .expect("field name allocated")
    };
    // A constructed member is held as the type a required field has (`T`, or `Option<T>` when
    // nullable); a `writeOnly` one is stored as its optional member's value.
    let required_fields = object
        .fields
        .iter()
        .filter(|field| field.constructed())
        .map(|field| {
            let field_ident = field_ident(field);
            let ty = ty_tokens(field.ty, names, false);
            let doc = format!("The `{}` member.", field.name.wire);
            let notes = field_notes(field)
                .into_iter()
                .map(|note| normalize_rustdoc(&note))
                .map(|note| quote! { #[doc = #note] });
            quote! {
                #[doc = #doc]
                #(#notes)*
                pub #field_ident: #ty,
            }
        });
    let inits = object.fields.iter().map(|field| {
        let field_ident = field_ident(field);
        if field.constructed() && !field.decodes_optional() {
            quote! { #field_ident: required.#field_ident, }
        } else if field.constructed() && field.ty.nullable {
            // The member is a required one's `Option<T>`, its `None` the `null` it may be.
            quote! {
                #field_ident: match required.#field_ident {
                    Some(value) => super::support::Nullable::Value(value),
                    None => super::support::Nullable::Null,
                },
            }
        } else if field.constructed() {
            quote! { #field_ident: Some(required.#field_ident), }
        } else if field.ty.nullable {
            quote! { #field_ident: super::support::Nullable::Absent, }
        } else {
            quote! { #field_ident: None, }
        }
    });
    let mut setters: Vec<TokenStream> = object
        .fields
        .iter()
        .filter(|field| !field.constructed())
        .map(|field| {
            let field_ident = field_ident(field);
            let setter = format_ident!("with_{}", field_ident.unraw());
            let inner = non_null_ty_tokens(field.ty, names);
            if field.ty.nullable {
                let doc = format!(
                    "Set the `{}` member: a value, or `Nullable::Null` for an explicit `null`.",
                    field.name.wire
                );
                let null_doc = format!(
                    "Set the `{}` member to an explicit `null`.",
                    field.name.wire
                );
                let stem = names
                    .null_setters
                    .get(&(id, field.name.wire.clone()))
                    .expect("null setter allocated");
                let null_setter = format_ident!("with_{}", stem.unraw());
                let nullable = nullable_tokens(inner);
                // Spelled by path: `name::TYPES_MODULE_NAMES` does not reserve `Into`, so a schema
                // may take that name in this module.
                quote! {
                    #[doc = #doc]
                    #[must_use]
                    pub fn #setter(mut self, value: impl std::convert::Into<#nullable>) -> Self {
                        self.#field_ident = value.into();
                        self
                    }

                    #[doc = #null_doc]
                    #[must_use]
                    pub fn #null_setter(mut self) -> Self {
                        self.#field_ident = super::support::Nullable::Null;
                        self
                    }
                }
            } else if takes_into(types, non_null(field.ty)) {
                let doc = format!("Set the `{}` member.", field.name.wire);
                quote! {
                    #[doc = #doc]
                    #[must_use]
                    pub fn #setter(mut self, value: impl std::convert::Into<#inner>) -> Self {
                        self.#field_ident = Some(value.into());
                        self
                    }
                }
            } else {
                let doc = format!("Set the `{}` member.", field.name.wire);
                quote! {
                    #[doc = #doc]
                    #[must_use]
                    pub fn #setter(mut self, value: #inner) -> Self {
                        self.#field_ident = Some(value);
                        self
                    }
                }
            }
        })
        .collect();
    let overflow_init = match &object.additional {
        AdditionalProps::Typed(ty) => {
            let ty = ty_tokens(**ty, names, false);
            let overflow = names
                .struct_overflow
                .get(&id)
                .expect("overflow field name allocated");
            let setter = format_ident!("with_{}", overflow.unraw());
            let doc = if object.fields.is_empty() {
                "Set the members no property declares, replacing any set before."
            } else {
                "Set the members no property declares, replacing any set before. An entry whose \
                 key is a declared member's name is not sent."
            };
            setters.push(quote! {
                #[doc = #doc]
                #[must_use]
                pub fn #setter(mut self, value: BTreeMap<String, #ty>) -> Self {
                    self.#overflow = value;
                    self
                }
            });
            quote! { #overflow: BTreeMap::new(), }
        }
        AdditionalProps::Allow | AdditionalProps::Deny => quote! {},
    };
    let required = object
        .fields
        .iter()
        .filter(|field| field.constructed())
        .count();
    let optional_doc = "every optional member unset, a member with a schema `default` included.";
    let accessors: Vec<TokenStream> = object
        .fields
        .iter()
        .filter_map(|field| default_accessor(id, field, types, names))
        .collect();
    let has_methods = !setters.is_empty() || !accessors.is_empty();
    let methods = quote! {
        #(#setters)*

        #(#accessors)*
    };
    if required == 0 {
        // With no constructed member, `new` sets every member to its type's `Default`, so the
        // derive builds what `new` does.
        let new_doc = format!("A `{ident}` with {optional_doc} `Default` builds the same value.");
        return StructConstructors {
            derive_default: Some(quote! { Default, }),
            constructors: quote! {
                impl #ident {
                    #[doc = #new_doc]
                    #[must_use]
                    pub fn new() -> Self {
                        Self {
                            #(#inits)*
                            #overflow_init
                        }
                    }

                    #methods
                }
            },
        };
    }
    let required_ident = names
        .required_structs
        .get(&id)
        .expect("`…Required` struct name allocated");
    let required_doc = format!(
        "The required members of `{ident}`, which converts from this with every other member \
         unset."
    );
    let from_doc = if setters.is_empty() {
        format!("A `{ident}` holding the members `{required_ident}` holds.")
    } else {
        format!("A `{ident}` holding the members `{required_ident}` holds, and {optional_doc}")
    };
    // A struct whose every member is required has no setter and, with no optional member, no
    // `_or_default` accessor, so no inherent method at all.
    let methods = has_methods.then(|| quote! { impl #ident { #methods } });
    // Spelled by path: `name::TYPES_MODULE_NAMES` does not reserve `From`, so a schema may take
    // that name in this module.
    let constructors = quote! {
        #[doc = #required_doc]
        #deprecated
        #[derive(Debug, Clone, PartialEq)]
        pub struct #required_ident {
            #(#required_fields)*
        }

        impl std::convert::From<#required_ident> for #ident {
            #[doc = #from_doc]
            fn from(required: #required_ident) -> Self {
                Self {
                    #(#inits)*
                    #overflow_init
                }
            }
        }

        #methods
    };
    StructConstructors {
        derive_default: None,
        constructors,
    }
}

/// The name of the private function `types` carries for every optional, non-nullable field: a
/// `deserialize_with` target that decodes a present value, `null` included, as the field's own
/// type. Snake case, so it cannot collide with a `PascalCase` type.
const DECODE_PRESENT: &str = "decode_present";

/// The name of the private function `types` carries for every required, nullable field that is
/// not an XML attribute: a `deserialize_with` target that decodes as `Option<T>` does, so an absent
/// member is serde's `missing field` rather than `None`. Snake case, so it cannot collide with a
/// `PascalCase` type.
const DECODE_REQUIRED: &str = "decode_required";

/// The `{field}_or_default` accessor of struct `id`'s `field`, when it has an
/// [applied default](Field::applied_default): `pub fn …(&self) -> T`, the member's value where it
/// is set, else the schema default. Lowering applies a default only to a non-nullable member, so
/// the member is an `Option<T>`. A `Copy` value is returned as it is, and any other is cloned: a
/// boolean, a number, a checked newtype and a closed string enum are `Copy`; a string and an open
/// string enum are not. Only a string's default is built lazily, through `unwrap_or_else`; an
/// enum's is a unit variant, passed to `unwrap_or`.
fn default_accessor(
    id: crate::ir::TypeId,
    field: &Field,
    types: &TypeGraph,
    names: &Names,
) -> Option<TokenStream> {
    use crate::ir::DefaultValue;
    let applied = field.applied_default()?;
    let key = (id, field.name.wire.clone());
    let field_ident = names.fields.get(&key).expect("field name allocated");
    let accessor = names
        .default_accessors
        .get(&key)
        .expect("default accessor allocated");
    let ty = ty_tokens(field.ty, names, false);
    let value = default_value_tokens(applied, field.ty, types, names);
    let copy = match applied {
        DefaultValue::Bool(_) | DefaultValue::Int(_) | DefaultValue::Float(_) => true,
        DefaultValue::Str(_) => false,
        DefaultValue::EnumVariant(_) => types.get(field.ty.id).is_some_and(|def| {
            matches!(
                types.emission(field.ty.id, def),
                Emission::StringEnum(enumeration) if !enumeration.is_open()
            )
        }),
    };
    // A unit variant is as cheap to build as the closure that would defer it, so only a string's
    // `.to_owned()` is deferred: `unwrap_or_else` over a variant sets off clippy's
    // `unnecessary_lazy_evaluations` in the consumer's crate.
    let body = match (copy, applied) {
        (true, _) => quote! { self.#field_ident.unwrap_or(#value) },
        (false, DefaultValue::Str(_)) => {
            quote! { self.#field_ident.clone().unwrap_or_else(|| #value) }
        }
        (false, _) => quote! { self.#field_ident.clone().unwrap_or(#value) },
    };
    let doc = format!(
        "The `{}` member where it is set, else its schema default.",
        field.name.wire
    );
    let note = normalize_rustdoc(
        &field
            .default
            .as_ref()
            .expect("an applied default is a declared one")
            .doc_note,
    );
    Some(quote! {
        #[doc = #doc]
        #[doc = ""]
        #[doc = #note]
        #[must_use]
        pub fn #accessor(&self) -> #ty {
            #body
        }
    })
}

/// Render a representable default as a Rust literal (or generated enum variant) for the field's
/// Rust type. An integer or boolean default of a checked newtype is the newtype over the literal,
/// which `types` may build through the private field: lowering applies only a listed value.
fn default_value_tokens(
    value: &crate::ir::DefaultValue,
    ty: Ty,
    types: &TypeGraph,
    names: &Names,
) -> TokenStream {
    use crate::ir::DefaultValue;
    let checked = types
        .get(ty.id)
        .is_some_and(|def| matches!(types.emission(ty.id, def), Emission::Checked(_)));
    if checked {
        let newtype = names.types.get(&ty.id).expect("type name allocated");
        let literal = match value {
            DefaultValue::Int(int) => checked_literal(&ScalarValue::Int(*int)),
            DefaultValue::Bool(boolean) => checked_literal(&ScalarValue::Bool(*boolean)),
            DefaultValue::Float(_) | DefaultValue::Str(_) | DefaultValue::EnumVariant(_) => {
                unreachable!("an integer or boolean set's default is an integer or a boolean")
            }
        };
        return quote! { #newtype(#literal) };
    }
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
        TypeKind::Bytes(BytesEncoding::Raw) => quote! { bytes::Bytes },
        TypeKind::Null => quote! { () },
        TypeKind::Any => quote! { serde_json::Value },
        // Unreachable on a checked `Api`; see `TypeKind::Reserved`.
        TypeKind::Reserved => {
            unreachable!(
                "a reservation reached codegen; `check_invariants` should have rejected it"
            )
        }
        TypeKind::Struct(_)
        | TypeKind::Enum(_)
        | TypeKind::Never
        | TypeKind::Union(_)
        | TypeKind::Bytes(BytesEncoding::Base64 | BytesEncoding::Base64Url) => {
            unreachable!("named definitions emitted separately")
        }
    }
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::CodegenOptions;
    use crate::diag::{JsonPointer, Provenance};
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
        let names = crate::name::allocate(&api);
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
    /// the prelude types it writes bare, so a schema hinted `Date` would redefine the runtime
    /// `Date` the module imports (`E0255`) were it not reserved. `name::TYPES_MODULE_NAMES` must
    /// hold every name the module brings
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

/// The emitted model of each definition shape, read from what an inline spec generates.
#[cfg(test)]
mod emitted_tests;
