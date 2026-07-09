//! # Subsystem: compat
//! layer-deps: source, diag
//!
//! Compatibility preprocessing for explicit API omissions. Omit rules are applied to the parsed
//! source bundle before OpenAPI validation/lowering, so callers can generate a conformant subset
//! without editing vendored upstream schemas.

use std::hash::{Hash, Hasher};

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, JsonPointer, Provenance};
use crate::source::InputBundle;

/// A compatibility omit profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Omit {
    /// Exact omit rules.
    pub rules: Vec<OmitRule>,
}

impl Omit {
    /// Construct a profile from exact rules.
    pub fn from_rules(rules: Vec<OmitRule>) -> Self {
        Self { rules }
    }

    /// Whether the profile contains no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Apply the profile to the loaded input bundle.
    pub(crate) fn apply(
        &self,
        bundle: &mut InputBundle,
        diags: &mut Diagnostics,
    ) -> Result<(), Aborted> {
        for rule in &self.rules {
            self.apply_rule(rule, bundle, diags);
        }
        validate_remaining(bundle, diags);
        diags.into_result(())
    }

    /// Stable fingerprint used in generated provenance headers.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Fnv64::default();
        for rule in &self.rules {
            rule.hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
    }

    fn apply_rule(&self, rule: &OmitRule, bundle: &mut InputBundle, diags: &mut Diagnostics) {
        let (file, pointer) = match rule {
            OmitRule::Path { path } => (
                bundle.root_id(),
                JsonPointer::root().push("paths").push(path),
            ),
            OmitRule::Operation { method, path } => (
                bundle.root_id(),
                JsonPointer::root()
                    .push("paths")
                    .push(path)
                    .push(method.as_oas_key()),
            ),
            OmitRule::Component { kind, name } => (
                bundle.root_id(),
                JsonPointer::root()
                    .push("components")
                    .push(kind.as_oas_key())
                    .push(name),
            ),
            OmitRule::Pointer { file, pointer } => {
                if pointer.is_empty() {
                    emit_invalid_rule(
                        rule,
                        bundle,
                        diags,
                        "omit rules cannot remove the document root",
                    );
                    return;
                }
                let Some(file_id) = file
                    .map(|path| bundle.file_id_for_path(path))
                    .unwrap_or_else(|| Some(bundle.root_id()))
                else {
                    emit_invalid_rule(rule, bundle, diags, "omit rule references an unloaded file");
                    return;
                };
                (file_id, JsonPointer::from(*pointer))
            }
        };

        let removed = bundle.value_at_mut(file).remove_pointer(&pointer);
        match removed {
            Some(value) => {
                Diagnostic::warning(
                    Code::OmittedConstruct,
                    Provenance::new(pointer, Some(value.span())),
                )
                .message(format!("omitted construct matched by `{}`", rule.describe()))
                .remedy("the source schema was not modified; remove the omit rule once spargen supports this construct")
                .emit(diags);
            }
            None => emit_invalid_rule(
                rule,
                bundle,
                diags,
                "omit rule did not match any source construct",
            ),
        }
    }
}

/// One exact compatibility omission.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OmitRule {
    /// Remove a path item and every operation beneath it.
    Path { path: &'static str },
    /// Remove a single operation.
    Operation {
        /// HTTP method.
        method: OmitMethod,
        /// OAS path template.
        path: &'static str,
    },
    /// Remove a named component.
    Component {
        /// Component map.
        kind: ComponentKind,
        /// Component name.
        name: &'static str,
    },
    /// Remove an arbitrary JSON Pointer, optionally file-local.
    Pointer {
        /// Optional file path/suffix in the input bundle.
        file: Option<&'static str>,
        /// RFC 6901 pointer.
        pointer: &'static str,
    },
}

impl OmitRule {
    fn describe(&self) -> String {
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
pub enum OmitMethod {
    Get,
    Put,
    Post,
    Delete,
    Options,
    Head,
    Patch,
    Trace,
}

impl OmitMethod {
    fn as_oas_key(self) -> &'static str {
        match self {
            OmitMethod::Get => "get",
            OmitMethod::Put => "put",
            OmitMethod::Post => "post",
            OmitMethod::Delete => "delete",
            OmitMethod::Options => "options",
            OmitMethod::Head => "head",
            OmitMethod::Patch => "patch",
            OmitMethod::Trace => "trace",
        }
    }
}

/// Component map used by compatibility omit rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentKind {
    Schemas,
    Responses,
    Parameters,
    RequestBodies,
    Headers,
    SecuritySchemes,
}

impl ComponentKind {
    fn as_oas_key(self) -> &'static str {
        match self {
            ComponentKind::Schemas => "schemas",
            ComponentKind::Responses => "responses",
            ComponentKind::Parameters => "parameters",
            ComponentKind::RequestBodies => "requestBodies",
            ComponentKind::Headers => "headers",
            ComponentKind::SecuritySchemes => "securitySchemes",
        }
    }
}

fn emit_invalid_rule(
    rule: &OmitRule,
    bundle: &InputBundle,
    diags: &mut Diagnostics,
    message: &'static str,
) {
    Diagnostic::error(
        Code::InvalidOmitRule,
        Provenance::new(JsonPointer::root(), Some(bundle.root().span())),
    )
    .message(format!("{message}: {}", rule.describe()))
    .remedy("use exact paths, operation methods, component names, or RFC 6901 pointers")
    .emit(diags);
}

fn validate_remaining(bundle: &InputBundle, diags: &mut Diagnostics) {
    let root = bundle.root();
    let missing = ["openapi", "info", "paths"]
        .into_iter()
        .filter(|key| root.get(key).is_none())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        Diagnostic::error(
            Code::OmitCreatedInvalidDocument,
            Provenance::new(JsonPointer::root(), Some(root.span())),
        )
        .message(format!(
            "omit profile removed required OpenAPI root fields: {}",
            missing.join(", ")
        ))
        .remedy("do not omit required OpenAPI root fields")
        .emit(diags);
    }
}

#[derive(Default)]
struct Fnv64(u64);

impl Hasher for Fnv64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.0 == 0 {
            self.0 = 0xcbf29ce484222325;
        }
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

/// Build an exact compatibility omit profile.
#[macro_export]
macro_rules! omit {
    () => {
        $crate::Omit::default()
    };
    ($($tokens:tt)*) => {{
        let mut omit = $crate::Omit::default();
        $crate::__spargen_omit_parse!(omit; $($tokens)*);
        omit
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __spargen_omit_parse {
    ($omit:ident;) => {};
    ($omit:ident; operations { $($body:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_operations!($omit; $($body)*);
        $crate::__spargen_omit_parse!($omit; $($rest)*);
    }};
    ($omit:ident; paths { $($body:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_paths!($omit; $($body)*);
        $crate::__spargen_omit_parse!($omit; $($rest)*);
    }};
    ($omit:ident; components { $($body:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_components!($omit; $($body)*);
        $crate::__spargen_omit_parse!($omit; $($rest)*);
    }};
    ($omit:ident; pointers { $($body:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_pointers!($omit; None; $($body)*);
        $crate::__spargen_omit_parse!($omit; $($rest)*);
    }};
    ($omit:ident; file($file:literal) { pointers { $($body:tt)* } } $($rest:tt)*) => {{
        $crate::__spargen_omit_pointers!($omit; Some($file); $($body)*);
        $crate::__spargen_omit_parse!($omit; $($rest)*);
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __spargen_omit_operations {
    ($omit:ident;) => {};
    ($omit:ident; get $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Get, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
    ($omit:ident; put $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Put, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
    ($omit:ident; post $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Post, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
    ($omit:ident; delete $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Delete, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
    ($omit:ident; options $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Options, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
    ($omit:ident; head $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Head, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
    ($omit:ident; patch $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Patch, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
    ($omit:ident; trace $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Trace, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __spargen_omit_paths {
    ($omit:ident;) => {};
    ($omit:ident; $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Path { path: $path });
        $crate::__spargen_omit_paths!($omit; $($rest)*);
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __spargen_omit_components {
    ($omit:ident;) => {};
    ($omit:ident; schemas { $($names:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_component_names!($omit; $crate::ComponentKind::Schemas; $($names)*);
        $crate::__spargen_omit_components!($omit; $($rest)*);
    }};
    ($omit:ident; responses { $($names:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_component_names!($omit; $crate::ComponentKind::Responses; $($names)*);
        $crate::__spargen_omit_components!($omit; $($rest)*);
    }};
    ($omit:ident; parameters { $($names:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_component_names!($omit; $crate::ComponentKind::Parameters; $($names)*);
        $crate::__spargen_omit_components!($omit; $($rest)*);
    }};
    ($omit:ident; request_bodies { $($names:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_component_names!($omit; $crate::ComponentKind::RequestBodies; $($names)*);
        $crate::__spargen_omit_components!($omit; $($rest)*);
    }};
    ($omit:ident; headers { $($names:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_component_names!($omit; $crate::ComponentKind::Headers; $($names)*);
        $crate::__spargen_omit_components!($omit; $($rest)*);
    }};
    ($omit:ident; security_schemes { $($names:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_component_names!($omit; $crate::ComponentKind::SecuritySchemes; $($names)*);
        $crate::__spargen_omit_components!($omit; $($rest)*);
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __spargen_omit_component_names {
    ($omit:ident; $kind:expr;) => {};
    ($omit:ident; $kind:expr; $name:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Component { kind: $kind, name: $name });
        $crate::__spargen_omit_component_names!($omit; $kind; $($rest)*);
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __spargen_omit_pointers {
    ($omit:ident; $file:expr;) => {};
    ($omit:ident; $file:expr; $pointer:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Pointer { file: $file, pointer: $pointer });
        $crate::__spargen_omit_pointers!($omit; $file; $($rest)*);
    }};
}

#[cfg(test)]
mod tests {
    use super::{ComponentKind, OmitMethod, OmitRule};

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
            OmitRule::Operation {
                method: OmitMethod::Get,
                path: "/markdown"
            }
        );
        assert_eq!(
            omit.rules[3],
            OmitRule::Component {
                kind: ComponentKind::Schemas,
                name: "legacy"
            }
        );
        assert!(omit.fingerprint().len() == 16);
    }
}
