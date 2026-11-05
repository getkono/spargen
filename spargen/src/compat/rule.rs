//! The omit rule grammar: [`OmitRule`] and the method and component kinds it names, how each is
//! parsed from and displayed as text, what a rule resolves to against a loaded bundle, and the
//! [`omit!`](crate::omit) macro that writes a profile.

use std::borrow::Cow;

use super::glob::{glob_match, has_glob_meta, unescape_glob};
use crate::diag::{FileId, JsonPointer};
use crate::source::{InputBundle, Node, SpannedValue};

/// One compatibility omission (exact or glob).
/// Strings are `Cow` so a rule can be written as a literal in [`omit!`](crate::omit) with no allocation and
/// still be built from data at runtime — the CLI, the config file, and the carve driver all derive
/// rules dynamically.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OmitRule {
    /// Remove a path item and every operation beneath it.
    Path {
        /// OAS path template.
        path: Cow<'static, str>,
    },
    /// Remove a single operation.
    Operation {
        /// HTTP method.
        method: OmitMethod,
        /// OAS path template.
        path: Cow<'static, str>,
    },
    /// Remove a named component.
    Component {
        /// Component map.
        kind: ComponentKind,
        /// Component name.
        name: Cow<'static, str>,
    },
    /// Remove an arbitrary JSON Pointer, optionally file-local.
    Pointer {
        /// Optional file path/suffix in the input bundle.
        file: Option<Cow<'static, str>>,
        /// RFC 6901 pointer.
        pointer: Cow<'static, str>,
    },
}

impl OmitRule {
    /// Remove a path item and every operation beneath it.
    pub fn path(path: impl Into<Cow<'static, str>>) -> Self {
        Self::Path { path: path.into() }
    }

    /// Remove a single operation.
    pub fn operation(method: OmitMethod, path: impl Into<Cow<'static, str>>) -> Self {
        Self::Operation {
            method,
            path: path.into(),
        }
    }

    /// Remove a named component.
    pub fn component(kind: ComponentKind, name: impl Into<Cow<'static, str>>) -> Self {
        Self::Component {
            kind,
            name: name.into(),
        }
    }

    /// Remove an arbitrary JSON Pointer, optionally scoped to one file of the bundle.
    pub fn pointer(file: Option<Cow<'static, str>>, pointer: impl Into<Cow<'static, str>>) -> Self {
        Self::Pointer {
            file,
            pointer: pointer.into(),
        }
    }
}

/// A string that names no known omit component kind or HTTP method. Its message names the
/// rejected token and one canonical spelling of every value the parser accepts, so `omit!`, the
/// command line, and `spargen.toml` answer a mistyped token with the same list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownOmitToken {
    token: String,
    noun: &'static str,
    accepted: &'static [&'static str],
}

impl std::fmt::Display for UnknownOmitToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "unknown {}: `{}`; expected one of {}",
            self.noun,
            self.token,
            self.accepted.join("/")
        )
    }
}

impl std::error::Error for UnknownOmitToken {}

impl std::str::FromStr for ComponentKind {
    type Err = UnknownOmitToken;

    /// Accepts the canonical snake_case plural spelling, plus the singular and the camelCase OAS
    /// key, the set [`omit!`](crate::omit) also accepts, so a rule reads the same whether it was
    /// written in Rust, on the command line, or in `spargen.toml`.
    fn from_str(token: &str) -> Result<Self, Self::Err> {
        Ok(match token {
            "schema" | "schemas" => ComponentKind::Schemas,
            "response" | "responses" => ComponentKind::Responses,
            "parameter" | "parameters" => ComponentKind::Parameters,
            "request_body" | "request_bodies" | "requestBody" | "requestBodies" => {
                ComponentKind::RequestBodies
            }
            "header" | "headers" => ComponentKind::Headers,
            "security_scheme" | "security_schemes" | "securityScheme" | "securitySchemes" => {
                ComponentKind::SecuritySchemes
            }
            "path_item" | "path_items" | "pathItem" | "pathItems" => ComponentKind::PathItems,
            "media_type" | "media_types" | "mediaType" | "mediaTypes" => ComponentKind::MediaTypes,
            _ => {
                return Err(UnknownOmitToken {
                    token: token.to_owned(),
                    noun: "component kind",
                    accepted: ComponentKind::CANONICAL,
                })
            }
        })
    }
}

impl std::fmt::Display for ComponentKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_oas_key())
    }
}

impl std::str::FromStr for OmitMethod {
    type Err = UnknownOmitToken;

    fn from_str(token: &str) -> Result<Self, Self::Err> {
        Ok(match token.to_ascii_lowercase().as_str() {
            "get" => OmitMethod::Get,
            "put" => OmitMethod::Put,
            "post" => OmitMethod::Post,
            "delete" => OmitMethod::Delete,
            "options" => OmitMethod::Options,
            "head" => OmitMethod::Head,
            "patch" => OmitMethod::Patch,
            "trace" => OmitMethod::Trace,
            "query" => OmitMethod::Query,
            _ => {
                return Err(UnknownOmitToken {
                    token: token.to_owned(),
                    noun: "HTTP method",
                    accepted: OmitMethod::CANONICAL,
                })
            }
        })
    }
}

impl std::fmt::Display for OmitMethod {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_oas_key())
    }
}

impl OmitRule {
    pub(super) fn describe(&self) -> String {
        match self {
            OmitRule::Path { path } => format!("path {path}"),
            OmitRule::Operation { method, path } => format!("{} {path}", method.as_oas_key()),
            OmitRule::Component { kind, name } => format!("component {} {name}", kind.as_oas_key()),
            OmitRule::Pointer { file, pointer } => match file {
                Some(file) => format!("pointer {file}#{pointer}"),
                None => format!("pointer {pointer}"),
            },
        }
    }

    /// The concrete construct(s) the rule targets in `bundle`. A rule whose path, name or pointer
    /// carries a glob metacharacter matches many constructs ([`RuleMatch::Glob`]); an exact rule
    /// resolves to a single pointer ([`RuleMatch::Exact`]). A pointer rule that names the
    /// document root or an unloaded file resolves to nothing, and the error says which.
    pub(super) fn resolve(&self, bundle: &InputBundle) -> Result<RuleMatch, &'static str> {
        let root = bundle.root_id();
        Ok(match self {
            OmitRule::Path { path } if has_glob_meta(path) => {
                let base = JsonPointer::root().push("paths");
                let pointers = matching_child_keys(bundle.root().get("paths"), path)
                    .map(|key| base.push(&key))
                    .collect();
                RuleMatch::Glob {
                    file: root,
                    pointers,
                }
            }
            OmitRule::Path { path } => RuleMatch::Exact {
                file: root,
                pointer: JsonPointer::root().push("paths").push(&unescape_glob(path)),
            },
            OmitRule::Operation { method, path } if has_glob_meta(path) => {
                let paths = bundle.root().get("paths");
                let method_key = method.as_oas_key();
                let pointers = matching_child_keys(paths, path)
                    .filter_map(|key| {
                        let item = paths?.get(&key)?;
                        item.get(method_key)?;
                        Some(
                            JsonPointer::root()
                                .push("paths")
                                .push(&key)
                                .push(method_key),
                        )
                    })
                    .collect();
                RuleMatch::Glob {
                    file: root,
                    pointers,
                }
            }
            OmitRule::Operation { method, path } => RuleMatch::Exact {
                file: root,
                pointer: JsonPointer::root()
                    .push("paths")
                    .push(&unescape_glob(path))
                    .push(method.as_oas_key()),
            },
            OmitRule::Component { kind, name } if has_glob_meta(name) => {
                let kind_key = kind.as_oas_key();
                let map = bundle
                    .root()
                    .get("components")
                    .and_then(|components| components.get(kind_key));
                let base = JsonPointer::root().push("components").push(kind_key);
                let pointers = matching_child_keys(map, name)
                    .map(|key| base.push(&key))
                    .collect();
                RuleMatch::Glob {
                    file: root,
                    pointers,
                }
            }
            OmitRule::Component { kind, name } => RuleMatch::Exact {
                file: root,
                pointer: JsonPointer::root()
                    .push("components")
                    .push(kind.as_oas_key())
                    .push(&unescape_glob(name)),
            },
            OmitRule::Pointer { file, pointer } => {
                if pointer.is_empty() {
                    return Err("omit rules cannot remove the document root");
                }
                let file_id = file
                    .as_deref()
                    .map(|path| bundle.file_id_for_path(path))
                    .unwrap_or(Some(root))
                    .ok_or("omit rule references an unloaded file")?;
                if has_glob_meta(pointer) {
                    let mut pointers = Vec::new();
                    collect_pointers(
                        bundle.value_at(file_id),
                        &JsonPointer::root(),
                        &mut pointers,
                    );
                    pointers.retain(|candidate| glob_match(pointer, candidate.as_str()));
                    RuleMatch::Glob {
                        file: file_id,
                        pointers,
                    }
                } else {
                    RuleMatch::Exact {
                        file: file_id,
                        pointer: JsonPointer::from(unescape_glob(pointer)),
                    }
                }
            }
        })
    }
}

/// The concrete target(s) an [`OmitRule`] resolves to against a loaded bundle.
pub(super) enum RuleMatch {
    /// A single exact pointer (a missing target is an `E019`).
    Exact { file: FileId, pointer: JsonPointer },
    /// Every pointer a glob rule matched (an empty match is an `E019`).
    Glob {
        file: FileId,
        pointers: Vec<JsonPointer>,
    },
}

/// The immediate object-member keys of `value` (if it is an object) that a glob `pattern` matches,
/// in source order.
fn matching_child_keys<'a>(
    value: Option<&'a SpannedValue>,
    pattern: &'a str,
) -> impl Iterator<Item = String> + 'a {
    value
        .and_then(SpannedValue::as_object)
        .into_iter()
        .flat_map(|object| object.iter())
        .filter(move |(key, _)| glob_match(pattern, &key.name))
        .map(|(key, _)| key.name.clone())
}

/// Collect every node pointer reachable from `value` (objects by member, arrays by index),
/// excluding the root, into `out` in document order.
fn collect_pointers(value: &SpannedValue, base: &JsonPointer, out: &mut Vec<JsonPointer>) {
    match &value.node {
        Node::Object(object) => {
            for (key, child) in object.iter() {
                let pointer = base.push(&key.name);
                collect_pointers(child, &pointer, out);
                out.push(pointer);
            }
        }
        Node::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let pointer = base.index(index);
                collect_pointers(child, &pointer, out);
                out.push(pointer);
            }
        }
        _ => {}
    }
}

/// HTTP method used by compatibility omit rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OmitMethod {
    Get,
    Put,
    Post,
    Delete,
    Options,
    Head,
    Patch,
    Trace,
    /// OpenAPI 3.2's `query` Path Item field. A method declared under `additionalOperations` has
    /// no fixed field to name, so it is targeted with an [`OmitRule::Pointer`] instead.
    Query,
}

impl OmitMethod {
    /// One spelling per method, as an [`UnknownOmitToken`] lists them; `FromStr` also accepts any
    /// ASCII case of each.
    pub(crate) const CANONICAL: &'static [&'static str] = &[
        "get", "put", "post", "delete", "options", "head", "patch", "trace", "query",
    ];

    /// The Path Item field this method names in an OpenAPI document.
    pub fn as_oas_key(self) -> &'static str {
        match self {
            OmitMethod::Get => "get",
            OmitMethod::Put => "put",
            OmitMethod::Post => "post",
            OmitMethod::Delete => "delete",
            OmitMethod::Options => "options",
            OmitMethod::Head => "head",
            OmitMethod::Patch => "patch",
            OmitMethod::Trace => "trace",
            OmitMethod::Query => "query",
        }
    }
}

/// Component map used by compatibility omit rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ComponentKind {
    Schemas,
    Responses,
    Parameters,
    RequestBodies,
    Headers,
    SecuritySchemes,
    /// `components.pathItems`, reachable from a Path Item `$ref`.
    PathItems,
    /// OpenAPI 3.2's `components.mediaTypes`, reachable from a Media Type Object `$ref`.
    MediaTypes,
}

impl ComponentKind {
    /// One spelling per kind, the snake_case plural, as an [`UnknownOmitToken`] lists them;
    /// `FromStr` and [`omit!`](crate::omit) also accept the singular and the camelCase OAS key.
    pub(crate) const CANONICAL: &'static [&'static str] = &[
        "schemas",
        "responses",
        "parameters",
        "request_bodies",
        "headers",
        "security_schemes",
        "path_items",
        "media_types",
    ];

    /// The `components` map key this kind names in an OpenAPI document.
    pub fn as_oas_key(self) -> &'static str {
        match self {
            ComponentKind::Schemas => "schemas",
            ComponentKind::Responses => "responses",
            ComponentKind::Parameters => "parameters",
            ComponentKind::RequestBodies => "requestBodies",
            ComponentKind::Headers => "headers",
            ComponentKind::SecuritySchemes => "securitySchemes",
            ComponentKind::PathItems => "pathItems",
            ComponentKind::MediaTypes => "mediaTypes",
        }
    }
}

/// Build a compatibility omit profile (exact or glob rules).
///
/// A component kind inside `components { … }` takes every spelling [`ComponentKind`]'s `FromStr`
/// accepts — the snake_case plural, the singular, and the camelCase OAS key — so
/// `schema { "Legacy"; }` and `requestBodies { "Legacy"; }` write the same rules as `schemas` and
/// `request_bodies`.
#[macro_export]
macro_rules! omit {
    () => {
        $crate::Omit::default()
    };
    (@parse $omit:ident;) => {};
    (@parse $omit:ident; operations { $($body:tt)* } $($rest:tt)*) => {{
        $crate::omit!(@operations $omit; $($body)*);
        $crate::omit!(@parse $omit; $($rest)*);
    }};
    (@parse $omit:ident; paths { $($body:tt)* } $($rest:tt)*) => {{
        $crate::omit!(@paths $omit; $($body)*);
        $crate::omit!(@parse $omit; $($rest)*);
    }};
    (@parse $omit:ident; components { $($body:tt)* } $($rest:tt)*) => {{
        $crate::omit!(@components $omit; $($body)*);
        $crate::omit!(@parse $omit; $($rest)*);
    }};
    (@parse $omit:ident; pointers { $($body:tt)* } $($rest:tt)*) => {{
        $crate::omit!(@pointers $omit; None; $($body)*);
        $crate::omit!(@parse $omit; $($rest)*);
    }};
    (@parse $omit:ident; file($file:literal) { pointers { $($body:tt)* } } $($rest:tt)*) => {{
        $crate::omit!(@pointers $omit; Some(::std::borrow::Cow::Borrowed($file)); $($body)*);
        $crate::omit!(@parse $omit; $($rest)*);
    }};
    (@operations $omit:ident;) => {};
    (@operations $omit:ident; get $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::operation($crate::OmitMethod::Get, $path));
        $crate::omit!(@operations $omit; $($rest)*);
    }};
    (@operations $omit:ident; put $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::operation($crate::OmitMethod::Put, $path));
        $crate::omit!(@operations $omit; $($rest)*);
    }};
    (@operations $omit:ident; post $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::operation($crate::OmitMethod::Post, $path));
        $crate::omit!(@operations $omit; $($rest)*);
    }};
    (@operations $omit:ident; delete $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::operation($crate::OmitMethod::Delete, $path));
        $crate::omit!(@operations $omit; $($rest)*);
    }};
    (@operations $omit:ident; options $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::operation($crate::OmitMethod::Options, $path));
        $crate::omit!(@operations $omit; $($rest)*);
    }};
    (@operations $omit:ident; head $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::operation($crate::OmitMethod::Head, $path));
        $crate::omit!(@operations $omit; $($rest)*);
    }};
    (@operations $omit:ident; patch $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::operation($crate::OmitMethod::Patch, $path));
        $crate::omit!(@operations $omit; $($rest)*);
    }};
    (@operations $omit:ident; trace $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::operation($crate::OmitMethod::Trace, $path));
        $crate::omit!(@operations $omit; $($rest)*);
    }};
    (@operations $omit:ident; query $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::operation($crate::OmitMethod::Query, $path));
        $crate::omit!(@operations $omit; $($rest)*);
    }};
    (@paths $omit:ident;) => {};
    (@paths $omit:ident; $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::path($path));
        $crate::omit!(@paths $omit; $($rest)*);
    }};
    (@components $omit:ident;) => {};
    (@components $omit:ident; $kind:ident { $($names:tt)* } $($rest:tt)*) => {{
        $crate::omit!(@component_names $omit; $crate::omit!(@kind $kind); $($names)*);
        $crate::omit!(@components $omit; $($rest)*);
    }};
    // The spellings `ComponentKind::from_str` accepts, one arm each.
    (@kind schema) => { $crate::ComponentKind::Schemas };
    (@kind schemas) => { $crate::ComponentKind::Schemas };
    (@kind response) => { $crate::ComponentKind::Responses };
    (@kind responses) => { $crate::ComponentKind::Responses };
    (@kind parameter) => { $crate::ComponentKind::Parameters };
    (@kind parameters) => { $crate::ComponentKind::Parameters };
    (@kind request_body) => { $crate::ComponentKind::RequestBodies };
    (@kind request_bodies) => { $crate::ComponentKind::RequestBodies };
    (@kind requestBody) => { $crate::ComponentKind::RequestBodies };
    (@kind requestBodies) => { $crate::ComponentKind::RequestBodies };
    (@kind header) => { $crate::ComponentKind::Headers };
    (@kind headers) => { $crate::ComponentKind::Headers };
    (@kind security_scheme) => { $crate::ComponentKind::SecuritySchemes };
    (@kind security_schemes) => { $crate::ComponentKind::SecuritySchemes };
    (@kind securityScheme) => { $crate::ComponentKind::SecuritySchemes };
    (@kind securitySchemes) => { $crate::ComponentKind::SecuritySchemes };
    (@kind path_item) => { $crate::ComponentKind::PathItems };
    (@kind path_items) => { $crate::ComponentKind::PathItems };
    (@kind pathItem) => { $crate::ComponentKind::PathItems };
    (@kind pathItems) => { $crate::ComponentKind::PathItems };
    (@kind media_type) => { $crate::ComponentKind::MediaTypes };
    (@kind media_types) => { $crate::ComponentKind::MediaTypes };
    (@kind mediaType) => { $crate::ComponentKind::MediaTypes };
    (@kind mediaTypes) => { $crate::ComponentKind::MediaTypes };
    (@component_names $omit:ident; $kind:expr;) => {};
    (@component_names $omit:ident; $kind:expr; $name:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::component($kind, $name));
        $crate::omit!(@component_names $omit; $kind; $($rest)*);
    }};
    (@pointers $omit:ident; $file:expr;) => {};
    (@pointers $omit:ident; $file:expr; $pointer:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::pointer($file, $pointer));
        $crate::omit!(@pointers $omit; $file; $($rest)*);
    }};
    ($($tokens:tt)*) => {{
        let mut omit = $crate::Omit::default();
        $crate::omit!(@parse omit; $($tokens)*);
        omit
    }};
}

#[cfg(test)]
mod tests {
    use super::{ComponentKind, OmitMethod, OmitRule};

    #[test]
    fn component_kind_displays_as_its_camel_case_oas_key() {
        for (kind, text) in [
            (ComponentKind::Schemas, "schemas"),
            (ComponentKind::Responses, "responses"),
            (ComponentKind::Parameters, "parameters"),
            (ComponentKind::RequestBodies, "requestBodies"),
            (ComponentKind::Headers, "headers"),
            (ComponentKind::SecuritySchemes, "securitySchemes"),
            (ComponentKind::PathItems, "pathItems"),
            (ComponentKind::MediaTypes, "mediaTypes"),
        ] {
            assert_eq!(kind.to_string(), text);
        }
    }

    #[test]
    fn omit_method_displays_as_its_lowercase_path_item_field() {
        for (method, text) in [
            (OmitMethod::Get, "get"),
            (OmitMethod::Put, "put"),
            (OmitMethod::Post, "post"),
            (OmitMethod::Delete, "delete"),
            (OmitMethod::Options, "options"),
            (OmitMethod::Head, "head"),
            (OmitMethod::Patch, "patch"),
            (OmitMethod::Trace, "trace"),
            (OmitMethod::Query, "query"),
        ] {
            assert_eq!(method.to_string(), text);
        }
    }

    #[test]
    fn the_accepted_lists_name_each_variant_exactly_once() {
        // Each `match` names every variant's canonical spelling, so a new variant stops this test
        // compiling until its spelling is written here. The assertions then hold `CANONICAL` to
        // those spellings: entry `i` parses to the variant declared `i`th, whose spelling it is,
        // and every variant listed below maps, in declaration order, onto `CANONICAL` exactly.
        fn kind_spelling(kind: ComponentKind) -> &'static str {
            match kind {
                ComponentKind::Schemas => "schemas",
                ComponentKind::Responses => "responses",
                ComponentKind::Parameters => "parameters",
                ComponentKind::RequestBodies => "request_bodies",
                ComponentKind::Headers => "headers",
                ComponentKind::SecuritySchemes => "security_schemes",
                ComponentKind::PathItems => "path_items",
                ComponentKind::MediaTypes => "media_types",
            }
        }
        fn method_spelling(method: OmitMethod) -> &'static str {
            match method {
                OmitMethod::Get => "get",
                OmitMethod::Put => "put",
                OmitMethod::Post => "post",
                OmitMethod::Delete => "delete",
                OmitMethod::Options => "options",
                OmitMethod::Head => "head",
                OmitMethod::Patch => "patch",
                OmitMethod::Trace => "trace",
                OmitMethod::Query => "query",
            }
        }

        for (index, token) in ComponentKind::CANONICAL.iter().enumerate() {
            let kind: ComponentKind = token.parse().unwrap();
            assert_eq!(
                kind as usize, index,
                "`{token}` is out of declaration order"
            );
            assert_eq!(kind_spelling(kind), *token);
        }
        let kinds = [
            ComponentKind::Schemas,
            ComponentKind::Responses,
            ComponentKind::Parameters,
            ComponentKind::RequestBodies,
            ComponentKind::Headers,
            ComponentKind::SecuritySchemes,
            ComponentKind::PathItems,
            ComponentKind::MediaTypes,
        ];
        assert_eq!(kinds.map(kind_spelling), ComponentKind::CANONICAL);

        for (index, token) in OmitMethod::CANONICAL.iter().enumerate() {
            let method: OmitMethod = token.parse().unwrap();
            assert_eq!(
                method as usize, index,
                "`{token}` is out of declaration order"
            );
            assert_eq!(method_spelling(method), *token);
        }
        let methods = [
            OmitMethod::Get,
            OmitMethod::Put,
            OmitMethod::Post,
            OmitMethod::Delete,
            OmitMethod::Options,
            OmitMethod::Head,
            OmitMethod::Patch,
            OmitMethod::Trace,
            OmitMethod::Query,
        ];
        assert_eq!(methods.map(method_spelling), OmitMethod::CANONICAL);

        assert_eq!(
            "nope".parse::<ComponentKind>().unwrap_err().to_string(),
            "unknown component kind: `nope`; expected one of schemas/responses/parameters/\
             request_bodies/headers/security_schemes/path_items/media_types"
        );
        assert_eq!(
            "nope".parse::<OmitMethod>().unwrap_err().to_string(),
            "unknown HTTP method: `nope`; \
             expected one of get/put/post/delete/options/head/patch/trace/query"
        );
    }

    #[test]
    fn omit_macro_expands_to_typed_rules() {
        let omit = crate::omit! {
            operations {
                get "/markdown";
                post "/markdown/raw";
            }
            paths {
                "/octocat";
            }
            components {
                schemas { "legacy"; }
                request_bodies { "legacy-body"; }
            }
            pointers {
                "/paths/~1legacy";
            }
            file("schemas/legacy.yaml") {
                pointers {
                    "/properties/unsupported";
                }
            }
        };

        assert_eq!(omit.rules.len(), 7);
        assert_eq!(
            omit.rules[0],
            OmitRule::operation(OmitMethod::Get, "/markdown")
        );
        assert_eq!(
            omit.rules[3],
            OmitRule::component(ComponentKind::Schemas, "legacy")
        );
        assert!(omit.fingerprint().len() == 16);
    }
}
