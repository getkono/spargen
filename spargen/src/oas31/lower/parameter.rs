//! Parameter Objects: location, style, schema, and the parameter shapes the generated client
//! can serialize.

use std::collections::HashSet;

use crate::diag::{Code, Diagnostic};
use crate::ir::{
    AdditionalProps, Delimiter, Docs, MediaType, ParamLoc, ParamStyle, Parameter, Prim, Ty,
    TypeGraph, TypeId, TypeKind,
};
use crate::oas31::{ParameterObject, RefOr, Schema};

use super::content::lower_media_type;
use super::LowerCtx;

impl<'a, 'doc> LowerCtx<'a, 'doc> {
    /// A parameter is always rendered to a wire string — path/query/header/cookie interpolation or a
    /// serialized content value — and `bytes::Bytes` (from `format: binary` / `contentEncoding:
    /// base64`) is not `Display` and has no faithful string rendering. `format: binary` on a
    /// parameter is conventionally just an opaque string, so a parameter whose type lowered to raw
    /// bytes is represented as a plain `String` instead — keeping the parameter renderable and
    /// matching the pre-`Bytes` behavior. Body/multipart binary lowering is unaffected.
    fn remap_binary_param(&mut self, ty: Ty, hint: &str) -> Ty {
        if matches!(
            self.graph.get(ty.id).map(|def| &def.kind),
            Some(TypeKind::Bytes)
        ) {
            let mut remapped = self.insert_type(
                hint,
                TypeKind::Primitive(Prim::String),
                Docs::default(),
                None,
            );
            remapped.nullable = ty.nullable;
            remapped
        } else {
            ty
        }
    }

    pub(super) fn lower_parameter(&mut self, parameter: &ParameterObject) -> Option<Parameter> {
        let location = match parameter.location.as_str() {
            "path" => ParamLoc::Path,
            "query" => ParamLoc::Query,
            "header" => ParamLoc::Header,
            "cookie" => ParamLoc::Cookie,
            "querystring" => ParamLoc::QueryString,
            _ => {
                Diagnostic::error(Code::InvalidInput, parameter.provenance.clone())
                    .message(format!(
                        "unsupported parameter location `{}`",
                        parameter.location
                    ))
                    .emit(self.diags);
                return None;
            }
        };
        // `Accept`, `Content-Type`, and `Authorization` header parameters "SHALL be ignored": the
        // protocol layer owns those, and emitting a client argument for one would let a caller
        // silently fight the codec or the auth attachment.
        if location == ParamLoc::Header
            && matches!(
                parameter.name.to_ascii_lowercase().as_str(),
                "accept" | "content-type" | "authorization"
            )
        {
            // W011 case: reserved-header-parameter
            Diagnostic::warning(Code::DeclarationHasNoEffect, parameter.provenance.clone())
                .message(format!(
                    "header parameter `{}` is ignored: the specification reserves `Accept`, \
                     `Content-Type`, and `Authorization` to the protocol layer",
                    parameter.name
                ))
                .remedy(
                    "remove the parameter; content types follow the operation's media types and \
                     credentials are registered with `Client::with_credential`",
                )
                .emit(self.diags);
            return None;
        }
        if location == ParamLoc::QueryString {
            let Some((media_name, object)) = parameter.content.iter().next() else {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message("`in: querystring` requires one content media type with a schema")
                .emit(self.diags);
                return None;
            };
            let object = self.resolve_media_object(object, media_name)?;
            let media = lower_media_type(media_name, &parameter.provenance, self.diags)?;
            if !matches!(media, MediaType::Json | MediaType::FormUrlEncoded) {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message(format!(
                    "querystring media type `{media_name}` is not supported; use JSON or \
                         application/x-www-form-urlencoded"
                ))
                .emit(self.diags);
                return None;
            }
            let Some(schema) = object.schema.as_ref() else {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message("querystring content requires a schema for a typed client argument")
                .emit(self.diags);
                return None;
            };
            let ty = self.lower_schema_ref(schema, &parameter.name)?;
            if media == MediaType::FormUrlEncoded
                && !matches!(
                    self.graph.get(ty.id).map(|definition| &definition.kind),
                    Some(TypeKind::Struct(_))
                )
            {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message("form-urlencoded querystring parameters require an object schema")
                .emit(self.diags);
                return None;
            }
            return Some(self.lowered_parameter(
                parameter,
                location,
                ty,
                ParamStyle::Content(media),
                true,
                object.schema.as_ref(),
            ));
        }
        let style_name = parameter.style.as_deref().unwrap_or(match location {
            ParamLoc::Path | ParamLoc::Header => "simple",
            ParamLoc::Query | ParamLoc::Cookie => "form",
            ParamLoc::QueryString => unreachable!("querystring returned above"),
        });
        // The legal `(style, in)` pairs are enforced by the official document schema before
        // lowering (`E011`), so an unknown pairing here is a generator bug rather than user input.
        // The arm is kept so a future schema relaxation cannot silently mis-serialize.
        let style = match (location, style_name) {
            (ParamLoc::Path | ParamLoc::Header, "simple") => ParamStyle::Simple,
            (ParamLoc::Path, "matrix") => ParamStyle::Matrix,
            (ParamLoc::Path, "label") => ParamStyle::Label,
            (ParamLoc::Query | ParamLoc::Cookie, "form") => ParamStyle::Form,
            (ParamLoc::Query, "spaceDelimited") => ParamStyle::Delimited(Delimiter::Space),
            (ParamLoc::Query, "pipeDelimited") => ParamStyle::Delimited(Delimiter::Pipe),
            (ParamLoc::Query, "deepObject") => ParamStyle::DeepObject,
            (ParamLoc::Cookie, "cookie") => ParamStyle::Cookie,
            _ => {
                Diagnostic::error(
                    Code::UnsupportedParameterStyle,
                    parameter.provenance.clone(),
                )
                .message(format!(
                    "parameter style `{style_name}` is not permitted for `{}` parameters",
                    parameter.location
                ))
                .emit(self.diags);
                return None;
            }
        };
        // `deepObject` ignores `explode` entirely; every other style defaults per the
        // specification (true only for `form` and 3.2's `cookie`).
        let explode = parameter
            .explode
            .unwrap_or(matches!(style, ParamStyle::Form | ParamStyle::Cookie));
        if matches!(style, ParamStyle::Delimited(_)) && parameter.explode == Some(true) {
            Diagnostic::error(
                Code::UnsupportedParameterStyle,
                parameter.provenance.clone(),
            )
            .message(format!(
                "`style: {style_name}` with `explode: true` has no defined serialization"
            ))
            .remedy("set `explode: false`, which is the default for this style")
            .emit(self.diags);
            return None;
        }
        // Deprecated in 3.2, and inert for a typed client: an absent optional parameter is simply
        // not sent, so there is never a case where the client would send an empty string instead.
        if parameter.allow_empty_value {
            // W011 case: allow-empty-value
            Diagnostic::warning(Code::DeclarationHasNoEffect, parameter.provenance.clone())
                .message(
                    "`allowEmptyValue` has no effect: an optional parameter the caller omits is \
                     not sent at all",
                )
                .remedy("remove `allowEmptyValue`; it is deprecated in OpenAPI 3.2")
                .emit(self.diags);
        }
        // `allowReserved` only means anything where the location percent-encodes at all.
        if parameter.allow_reserved
            && (location == ParamLoc::Header || matches!(style, ParamStyle::Cookie))
        {
            // W011 case: allow-reserved-parameter
            Diagnostic::warning(Code::DeclarationHasNoEffect, parameter.provenance.clone())
                .message(
                    "`allowReserved` has no effect here: this parameter is sent without \
                     percent-encoding",
                )
                .remedy("remove `allowReserved`, or use `style: form` if encoding is wanted")
                .emit(self.diags);
        }
        let ty = if let Some(schema) = &parameter.schema {
            let ty = self.lower_schema_ref(schema, &parameter.name)?;
            self.remap_binary_param(ty, &parameter.name)
        } else if let Some((media, object)) = parameter.content.iter().next() {
            let object = self.resolve_media_object(object, media)?;
            let media_name = media.clone();
            let media = lower_media_type(media, &parameter.provenance, self.diags)?;
            // A `content` parameter is rendered by its media codec. Only JSON and raw text have a
            // codec that produces a single parameter token; anything else would fall through to
            // `simple` serialization and be sent in the wrong format.
            if !matches!(media, MediaType::Json | MediaType::Text) {
                Diagnostic::error(Code::UnsupportedMediaType, parameter.provenance.clone())
                    .message(format!(
                        "`content` parameter media type `{media_name}` has no single-token \
                         serialization"
                    ))
                    .remedy(
                        "use `application/json` or a `text/*` media type, or describe the \
                         parameter with `schema` and a serialization style",
                    )
                    .emit(self.diags);
                return None;
            }
            let ty = object
                .schema
                .as_ref()
                .and_then(|schema| self.lower_schema_ref(schema, &parameter.name))?;
            let ty = self.remap_binary_param(ty, &parameter.name);
            return Some(self.lowered_parameter(
                parameter,
                location,
                ty,
                ParamStyle::Content(media),
                false,
                object.schema.as_ref(),
            ));
        } else {
            self.insert_type(
                &parameter.name,
                TypeKind::Any,
                Docs::default(),
                Some(parameter.provenance.clone()),
            )
        };
        if let Some((path, kind)) = uninhabited_parameter_part(&self.graph, ty) {
            // `false`, or an `allOf` whose members meet empty, admits no value at all (#407).
            // Nothing is nested, so "nested arrays or objects" would describe nothing the author
            // wrote; name the schema that admits nothing instead. A union with such a member
            // still admits its other members' values, so only the member is called uninhabited.
            let at = format!("{}{path}", parameter.name);
            let (message, remedy) = match kind {
                Uninhabited::Whole => (
                    format!(
                        "parameter schema `{at}` is uninhabited: no value satisfies it (`false`, \
                         or an `allOf` whose members conflict), so simple/form/deepObject \
                         serialization has no token for it"
                    ),
                    format!("give `{at}` a schema some value satisfies, or remove it"),
                ),
                Uninhabited::Member => (
                    format!(
                        "parameter schema `{at}` has a `oneOf`/`anyOf` member that is \
                         uninhabited: no value satisfies that member (`false`, or an `allOf` \
                         whose members conflict), so simple/form/deepObject serialization has no \
                         token for it"
                    ),
                    format!(
                        "remove the uninhabited member from `{at}`, or give it a schema some \
                         value satisfies"
                    ),
                ),
            };
            Diagnostic::error(
                Code::UnsupportedParameterStyle,
                parameter.provenance.clone(),
            )
            .message(message)
            .remedy(remedy)
            .emit(self.diags);
            return None;
        }
        if let Some(property) = unconstrained_parameter_property(&self.graph, ty) {
            // Most often a `required` name no `properties` entry declares, which is a required
            // field typed by `additionalProperties` and unconstrained without one (#140). An
            // arbitrary JSON value has no `key=value` token, and "nested arrays or objects"
            // would describe nothing the author wrote.
            Diagnostic::error(
                Code::UnsupportedParameterStyle,
                parameter.provenance.clone(),
            )
            .message(format!(
                "parameter property `{property}` is unconstrained: no schema constrains its \
                 value, so simple/form/deepObject serialization has no scalar token for it"
            ))
            .remedy(format!(
                "declare `{property}` under `properties` with a scalar schema, or give the object \
                 a scalar `additionalProperties` schema"
            ))
            .emit(self.diags);
            return None;
        }
        if !parameter_shape_supported(&self.graph, ty) {
            Diagnostic::error(
                Code::UnsupportedParameterStyle,
                parameter.provenance.clone(),
            )
            .message(
                "simple/form parameter serialization does not support nested arrays or objects",
            )
            .emit(self.diags);
            return None;
        }
        Some(self.lowered_parameter(
            parameter,
            location,
            ty,
            style,
            explode,
            parameter.schema.as_ref(),
        ))
    }

    /// Assemble the lowered [`Parameter`] once its location, type, style and `explode` are
    /// settled. A path parameter is always required, whatever `required` says. `allowReserved`
    /// is carried only for a styled parameter: a `content` or `querystring` parameter is
    /// rendered by its media codec, which does not consult it. `schema` is the Schema Object
    /// whose `default` the parameter documents — the parameter's own, or its media's.
    fn lowered_parameter(
        &self,
        parameter: &ParameterObject,
        location: ParamLoc,
        ty: Ty,
        style: ParamStyle,
        explode: bool,
        schema: Option<&RefOr<Schema>>,
    ) -> Parameter {
        Parameter {
            name: parameter.name.clone(),
            location,
            ty,
            required: parameter.required || location == ParamLoc::Path,
            allow_reserved: parameter.allow_reserved && !matches!(style, ParamStyle::Content(_)),
            style,
            explode,
            deprecated: parameter.deprecated,
            default_display: self.param_default_display(schema, ty),
        }
    }
}

/// The wire name of the first field of an object parameter whose value is unconstrained
/// ([`TypeKind::Any`]), which [`parameter_shape_supported`] refuses because an arbitrary JSON value
/// has no single serialized token. A `oneOf`/`anyOf` parameter schema serializes as whichever
/// member the value is, so the fields of each object member are searched too, through nested
/// unions, as [`uninhabited_parameter_part`] searches them (#435). `None` for a parameter that is
/// neither an object nor a union, or has no such field.
fn unconstrained_parameter_property(graph: &TypeGraph, ty: Ty) -> Option<String> {
    unconstrained_parameter_property_inner(graph, ty, &mut HashSet::new())
}

fn unconstrained_parameter_property_inner(
    graph: &TypeGraph,
    ty: Ty,
    members: &mut HashSet<TypeId>,
) -> Option<String> {
    match &graph.get(ty.id)?.kind {
        TypeKind::Struct(object) => object
            .fields
            .iter()
            .find(|field| matches!(graph.get(field.ty.id).map(|d| &d.kind), Some(TypeKind::Any)))
            .map(|field| field.name.wire.clone()),
        // `members` stops a union that reaches itself through a member from being walked again.
        TypeKind::Union(union) if members.insert(ty.id) => {
            let found = union.variants.iter().find_map(|variant| {
                unconstrained_parameter_property_inner(graph, variant.ty, members)
            });
            members.remove(&ty.id);
            found
        }
        // A reservation's shape is unknown, so it has no field to name; it is left to
        // `parameter_shape_supported`, which refuses it.
        TypeKind::Reserved => None,
        _ => None,
    }
}

/// How a parameter position fails to be inhabited, as [`uninhabited_parameter_part`] finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Uninhabited {
    /// The schema at the position admits no value: [`TypeKind::Never`], or a union every one of
    /// whose members admits none.
    Whole,
    /// The schema is a union that admits some value, but at least one of its members admits none
    /// (`oneOf: [{type: string}, false]`). The position itself is inhabited, so only the member
    /// may be called uninhabited.
    Member,
}

/// The path, relative to the parameter, of the first schema at a position simple/form/deepObject
/// serialization would otherwise accept that is uninhabited, or is a union holding an uninhabited
/// ([`TypeKind::Never`]) member, together with which of the two it is. [`parameter_shape_supported`]
/// refuses both, because no value of the uninhabited schema has a token. The path is `""` for the
/// parameter itself, `.name` for an object property, `.*` for its `additionalProperties`, `[]` for
/// an array's items, and `[i]` for a tuple's. A `oneOf`/`anyOf` parameter schema serializes as
/// whichever member the value is, so the parts of each member are searched too, with the same
/// paths (a member adds nothing to it): `f.a` is the property `a` of an object member of `f`
/// (#435). Only those positions are searched, so an uninhabited schema below a nested array or
/// object stays reported as the nesting. `None` when there is no such schema.
fn uninhabited_parameter_part(graph: &TypeGraph, ty: Ty) -> Option<(String, Uninhabited)> {
    uninhabited_parameter_part_inner(graph, ty, &mut HashSet::new())
}

fn uninhabited_parameter_part_inner(
    graph: &TypeGraph,
    ty: Ty,
    members: &mut HashSet<TypeId>,
) -> Option<(String, Uninhabited)> {
    fn classify(graph: &TypeGraph, ty: Ty, visiting: &mut HashSet<TypeId>) -> Option<Uninhabited> {
        if !visiting.insert(ty.id) {
            return None;
        }
        let found = match graph.get(ty.id).map(|definition| &definition.kind) {
            Some(TypeKind::Never) => Some(Uninhabited::Whole),
            Some(TypeKind::Union(union)) => {
                let members: Vec<_> = union
                    .variants
                    .iter()
                    .map(|variant| classify(graph, variant.ty, visiting))
                    .collect();
                if !members.is_empty()
                    && members
                        .iter()
                        .all(|member| *member == Some(Uninhabited::Whole))
                {
                    Some(Uninhabited::Whole)
                } else if members.iter().any(Option::is_some) {
                    Some(Uninhabited::Member)
                } else {
                    None
                }
            }
            // A reservation's shape is unknown, so nothing proves it admits no value; it is left
            // to `parameter_shape_supported`, which refuses it.
            Some(TypeKind::Reserved) => None,
            _ => None,
        };
        visiting.remove(&ty.id);
        found
    }
    let mut visiting = HashSet::new();
    if let Some(kind) = classify(graph, ty, &mut visiting) {
        return Some((String::new(), kind));
    }
    let mut at = |path: String, ty: Ty| classify(graph, ty, &mut visiting).map(|kind| (path, kind));
    match &graph.get(ty.id)?.kind {
        TypeKind::Array(item) => at("[]".to_owned(), **item),
        TypeKind::Tuple(items) => items
            .iter()
            .enumerate()
            .find_map(|(index, item)| at(format!("[{index}]"), *item)),
        TypeKind::Struct(object) => object
            .fields
            .iter()
            .find_map(|field| at(format!(".{}", field.name.wire), field.ty))
            .or_else(|| match &object.additional {
                AdditionalProps::Typed(value) => at(".*".to_owned(), **value),
                AdditionalProps::Deny | AdditionalProps::Allow => None,
            }),
        // `parameter_shape_supported` gives each member the position the union holds, so a
        // member's parts are parts of the parameter. `members` stops a union that reaches itself
        // through a member from being walked again.
        TypeKind::Union(union) if members.insert(ty.id) => {
            let found = union
                .variants
                .iter()
                .find_map(|variant| uninhabited_parameter_part_inner(graph, variant.ty, members));
            members.remove(&ty.id);
            found
        }
        // The parameter itself was classified above; a reservation has no parts to search.
        TypeKind::Reserved => None,
        _ => None,
    }
}

fn parameter_shape_supported(graph: &TypeGraph, ty: Ty) -> bool {
    parameter_shape_supported_inner(graph, ty, false, &mut HashSet::new())
}

fn parameter_shape_supported_inner(
    graph: &TypeGraph,
    ty: Ty,
    scalar_only: bool,
    visiting: &mut HashSet<TypeId>,
) -> bool {
    if !visiting.insert(ty.id) {
        return false;
    }
    let Some(definition) = graph.get(ty.id) else {
        visiting.remove(&ty.id);
        return false;
    };
    let supported = match &definition.kind {
        TypeKind::Primitive(_) | TypeKind::Enum(_) | TypeKind::Bytes | TypeKind::Null => true,
        TypeKind::Array(item) if !scalar_only => {
            parameter_shape_supported_inner(graph, **item, true, visiting)
        }
        TypeKind::Tuple(items) if !scalar_only => items
            .iter()
            .all(|item| parameter_shape_supported_inner(graph, *item, true, visiting)),
        TypeKind::Struct(object) if !scalar_only => {
            object
                .fields
                .iter()
                .all(|field| parameter_shape_supported_inner(graph, field.ty, true, visiting))
                && match &object.additional {
                    AdditionalProps::Deny | AdditionalProps::Allow => true,
                    AdditionalProps::Typed(value) => {
                        parameter_shape_supported_inner(graph, **value, true, visiting)
                    }
                }
        }
        TypeKind::Union(union) => union.variants.iter().all(|variant| {
            parameter_shape_supported_inner(graph, variant.ty, scalar_only, visiting)
        }),
        // A reservation's shape is unknown, so it cannot be *proved* serialisable as a parameter.
        // This function answers "is this supported", and an unknown must answer no: saying yes
        // would let a recursive schema through as a parameter on the strength of nothing.
        //
        // Parameters are lowered only after every component, and each lazily resolved target
        // fills its reservation before returning, so no reservation is open here. One still
        // survives: a component whose lowering *failed* never fills its reservation, and a
        // component that closed a cycle through it before the failure is cached complete, holding
        // that dangling id. `A: {properties: {bs: {$ref: B}, x: {$ref: Missing}}}` with
        // `B: {type: array, items: {$ref: A}}` leaves `B`'s items `Reserved` for good, and a
        // parameter referencing `B` reaches this arm. The document is already rejected by the
        // failure (`E004` there); answering no adds `E010` for the parameter rather than accepting
        // a shape nobody knows. Pinned by
        // `a_parameter_reaching_a_failed_components_reservation_is_refused`.
        TypeKind::Reserved
        | TypeKind::Struct(_)
        | TypeKind::Array(_)
        | TypeKind::Tuple(_)
        | TypeKind::Never
        | TypeKind::Any => false,
    };
    visiting.remove(&ty.id);
    supported
}
