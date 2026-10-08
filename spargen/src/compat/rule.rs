//! The omit rule grammar: [`OmitRule`] and the method and component kinds it names, how each is
//! parsed from and displayed as text, and the [`omit!`](crate::omit) macro that writes a profile.

use std::borrow::Cow;

/// One exact compatibility omission.
/// Strings are `Cow` so a rule can be written as a literal in [`omit!`](crate::omit) with no allocation and
/// still be built from data at runtime — the CLI, the config file, and the carve driver all derive
/// rules dynamically, and each previously leaked a `String` to fake a `&'static str`.
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

/// A string that names no known omit component kind or HTTP method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownOmitToken {
    token: String,
    expected: &'static str,
}

impl std::fmt::Display for UnknownOmitToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "unknown {}: `{}`", self.expected, self.token)
    }
}

impl std::error::Error for UnknownOmitToken {}

impl std::str::FromStr for ComponentKind {
    type Err = UnknownOmitToken;

    /// Accepts the canonical snake_case plural spelling used by [`omit!`](crate::omit), plus the singular and
    /// the camelCase OAS key, so a rule reads the same whether it was written in Rust, on the
    /// command line, or in `spargen.toml`.
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
                    expected: "component kind",
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
                    expected: "HTTP method",
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

/// Build an exact compatibility omit profile.
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
    (@components $omit:ident; schemas { $($names:tt)* } $($rest:tt)*) => {{
        $crate::omit!(@component_names $omit; $crate::ComponentKind::Schemas; $($names)*);
        $crate::omit!(@components $omit; $($rest)*);
    }};
    (@components $omit:ident; responses { $($names:tt)* } $($rest:tt)*) => {{
        $crate::omit!(@component_names $omit; $crate::ComponentKind::Responses; $($names)*);
        $crate::omit!(@components $omit; $($rest)*);
    }};
    (@components $omit:ident; parameters { $($names:tt)* } $($rest:tt)*) => {{
        $crate::omit!(@component_names $omit; $crate::ComponentKind::Parameters; $($names)*);
        $crate::omit!(@components $omit; $($rest)*);
    }};
    (@components $omit:ident; request_bodies { $($names:tt)* } $($rest:tt)*) => {{
        $crate::omit!(@component_names $omit; $crate::ComponentKind::RequestBodies; $($names)*);
        $crate::omit!(@components $omit; $($rest)*);
    }};
    (@components $omit:ident; headers { $($names:tt)* } $($rest:tt)*) => {{
        $crate::omit!(@component_names $omit; $crate::ComponentKind::Headers; $($names)*);
        $crate::omit!(@components $omit; $($rest)*);
    }};
    (@components $omit:ident; security_schemes { $($names:tt)* } $($rest:tt)*) => {{
        $crate::omit!(@component_names $omit; $crate::ComponentKind::SecuritySchemes; $($names)*);
        $crate::omit!(@components $omit; $($rest)*);
    }};
    (@components $omit:ident; path_items { $($names:tt)* } $($rest:tt)*) => {{
        $crate::omit!(@component_names $omit; $crate::ComponentKind::PathItems; $($names)*);
        $crate::omit!(@components $omit; $($rest)*);
    }};
    (@components $omit:ident; media_types { $($names:tt)* } $($rest:tt)*) => {{
        $crate::omit!(@component_names $omit; $crate::ComponentKind::MediaTypes; $($names)*);
        $crate::omit!(@components $omit; $($rest)*);
    }};
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
