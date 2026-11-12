//! # Subsystem: codegen
//! layer-deps: ir, name, support
//!
//! IR + allocated names → Rust tokens: models, client, and the embedded `support` module, with
//! deterministic item ordering and `prettyplease` formatting. Codegen never
//! sees a spec document — it consumes only the IR and the [`crate::name::Names`] table.
//!
//! One module per emitted construct, each producing a deterministically-ordered token fragment
//! that [`generate`] assembles and formats: [`models`] (the `types` module) and [`union`] (its
//! union enums), [`client`] (the `Client`, its servers, and the per-operation items beside it),
//! [`operation`] (one async method with its request builder and decoder, the signature its blocking
//! twin shares, and the `ops` module of operation identities), [`params`],
//! [`body`], and [`dispatch`] (the method's parameter, request-body, and status-dispatch code),
//! [`responses`] (response enums, error types, and header structs), [`blocking`] (the synchronous
//! facade), [`testing`] (the opt-in `testing` module and its typed `MockBackend`), [`runtime`]
//! (the embedded `support` module and the root re-exports), the shared
//! [`docs`] and [`ty`] renderers, and [`idents`] (the status variant names the response and
//! dispatch emitters read).

mod blocking;
mod body;
mod client;
mod dispatch;
mod docs;
mod idents;
mod models;
mod operation;
mod params;
mod responses;
mod runtime;
mod testing;
mod ty;
mod union;

use crate::ir::Api;
use crate::name::Names;
use quote::{format_ident, quote};

/// Options controlling code generation. `feature_uuid` and `datetime` carry the `Spec::uuid` /
/// `Spec::datetime` knobs into the one generated module: `uuid` off, or the `String` backend, falls
/// the corresponding `format` mappings back to `String`.
#[derive(Debug, Clone)]
pub(crate) struct CodegenOptions {
    /// Map `format: uuid` to `uuid::Uuid` (else `String`).
    pub(crate) feature_uuid: bool,
    /// The crate `format: date-time`/`date` map to, or `String`.
    pub(crate) datetime: crate::ir::DateTimeBackend,
    /// Max bytes of a response body retained on error variants; stamped into the
    /// generated client's default configuration.
    pub(crate) error_body_cap: usize,
    /// Render spec-carried prose as written, skipping [`docs::sanitize_carried`]: the
    /// `Spec::verbatim_docs` knob.
    pub(crate) verbatim_docs: bool,
    /// Emit the `testing` module and its typed `MockBackend`: the `Spec::testing` knob.
    pub(crate) testing: bool,
}

/// The default cap on bytes of a response body retained on generated error variants, which
/// [`crate::Spec`] starts from.
pub(crate) const DEFAULT_ERROR_BODY_CAP: usize = 64 * 1024;

/// The options a `Spec` with every knob at its default selects; only tests build them this way.
#[cfg(test)]
impl Default for CodegenOptions {
    fn default() -> Self {
        Self {
            feature_uuid: true,
            datetime: crate::ir::DateTimeBackend::Time,
            error_body_cap: DEFAULT_ERROR_BODY_CAP,
            verbatim_docs: false,
            testing: false,
        }
    }
}

/// The complete generated code for one client (models, client, embedded support): one module.
#[derive(Debug, Clone, Default)]
pub(crate) struct GeneratedCode {
    /// The module's source, already formatted rustfmt-clean via `prettyplease`.
    pub(crate) contents: String,
}

/// The prefix of the message a [`CodegenError`] displays. The frontend and fuzz harnesses
/// (`tests/frontend/main.rs`, `tests/fuzz_frontend.rs`) declare the same literal to recognise a
/// report that carries one, and a test in this module holds the declarations equal.
pub(crate) const CODEGEN_FAILURE: &str = "codegen produced unparseable Rust";

/// The assembled token stream did not parse as a Rust file: a codegen bug, not a spec problem.
/// The pipeline reports it as `E011` and writes and caches nothing.
#[derive(Debug)]
pub(crate) struct CodegenError(syn::Error);

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{CODEGEN_FAILURE} (a spargen bug): {}", self.0)
    }
}

/// Generate the Rust source for a client from the IR and allocated names.
///
/// Output is deterministic: item ordering does not depend on input map ordering, so checked-in code
/// produces stable diffs. Every spec construct is decided during lowering, so codegen takes no
/// `Diagnostics`. Its one failure is a token stream that does not parse as a Rust file, returned
/// as a [`CodegenError`] for the caller to report.
pub(crate) fn generate(
    api: &Api,
    names: &Names,
    options: &CodegenOptions,
) -> Result<GeneratedCode, CodegenError> {
    let features = api.runtime_features(options.datetime);
    let support = runtime::emit_support(features);
    let models = models::emit_models(api, names, options, features.datetime.is_some());
    let signatures = operation::signatures(api, names, options);
    let client = client::emit_client(api, &signatures, names, options);
    let ops = operation::emit_ops(api, names);
    // The synchronous facade is always emitted; `blocking::blocking_cfg` compiles its items out.
    let blocking = blocking::emit_blocking_client(api, &signatures, names, options);
    let testing = options.testing.then(|| testing::emit_testing(api, names));
    // Attributes ride on items rather than the file (`#![…]`): inner attributes would make the
    // output unusable via `include!` from OUT_DIR, the build.rs consumption path.
    // The root surface is emitted from the lists `name` owns, whose root scope reserves them before
    // allocating the operation types declared beside them.
    let root_reexport = |names: &[&str]| {
        let idents = names.iter().map(|name| format_ident!("{}", name));
        quote! {
            #[allow(unused_imports)]
            pub use support::{ #(#idents),* };
        }
    };
    let root_exports = root_reexport(crate::name::ROOT_REEXPORTS);
    let stream_exports = features
        .streams
        .then(|| root_reexport(crate::name::STREAM_ROOT_REEXPORTS));
    let datetime_exports = features
        .datetime
        .is_some()
        .then(|| root_reexport(crate::name::DATETIME_ROOT_REEXPORTS));
    let tokens = quote! {
        #root_exports
        #stream_exports
        #datetime_exports

        #support
        #models
        #ops
        #client
        #blocking
        #testing
    };
    render(tokens)
}

/// Format the assembled module into the one generated file, deterministic and rustfmt-clean via
/// `prettyplease`, or the [`CodegenError`] its tokens raise when they are not a parseable Rust
/// file.
fn render(tokens: proc_macro2::TokenStream) -> Result<GeneratedCode, CodegenError> {
    let mut file: syn::File = syn::parse2(tokens).map_err(CodegenError)?;
    for item in &mut file.items {
        if let Some(attrs) = item_attrs(item) {
            allow_unsatisfiable_lints(attrs);
        }
    }
    Ok(GeneratedCode {
        contents: prettyplease::unparse(&file),
    })
}

/// The lints every top-level item of the generated module allows, so the `include!` site needs no
/// lint attribute of its own: they judge what the spec chose — its names, its undocumented
/// members, the operations a consumer never calls — or where the consumer puts the module, which
/// no generated text can satisfy for every spec and every include site. They ride on each item
/// rather than in an inner attribute on the file, which `include!` refuses; an item's lint levels
/// reach every item nested inside it.
const UNSATISFIABLE_LINTS: &[&str] = &[
    "missing_docs",
    "unreachable_pub",
    "dead_code",
    "clippy::pedantic",
    "clippy::nursery",
];

/// Allow [`UNSATISFIABLE_LINTS`] in `attrs`: appended to the item's own `#[allow(…)]` where it has
/// one, less the lints it already names (Clippy's `duplicated_attributes` refuses a repeat), and
/// as a new `#[allow(…)]` otherwise.
fn allow_unsatisfiable_lints(attrs: &mut Vec<syn::Attribute>) {
    type Lints = syn::punctuated::Punctuated<syn::Path, syn::Token![,]>;
    let spelled = |path: &syn::Path| quote!(#path).to_string().replace(' ', "");
    let existing = attrs.iter().position(|attr| attr.path().is_ident("allow"));
    let mut lints = match existing {
        Some(index) => attrs[index]
            .parse_args_with(Lints::parse_terminated)
            .expect("codegen's own `#[allow]` names lint paths"),
        None => Lints::new(),
    };
    for lint in UNSATISFIABLE_LINTS {
        if !lints.iter().any(|path| spelled(path) == *lint) {
            lints.push(syn::parse_str(lint).expect("a lint path"));
        }
    }
    let attr: syn::Attribute = syn::parse_quote! { #[allow(#lints)] };
    match existing {
        Some(index) => attrs[index] = attr,
        None => attrs.push(attr),
    }
}

/// The attribute list of a top-level item, for every item kind codegen emits.
fn item_attrs(item: &mut syn::Item) -> Option<&mut Vec<syn::Attribute>> {
    Some(match item {
        syn::Item::Const(item) => &mut item.attrs,
        syn::Item::Enum(item) => &mut item.attrs,
        syn::Item::Fn(item) => &mut item.attrs,
        syn::Item::Impl(item) => &mut item.attrs,
        syn::Item::Macro(item) => &mut item.attrs,
        syn::Item::Mod(item) => &mut item.attrs,
        syn::Item::Static(item) => &mut item.attrs,
        syn::Item::Struct(item) => &mut item.attrs,
        syn::Item::Trait(item) => &mut item.attrs,
        syn::Item::Type(item) => &mut item.attrs,
        syn::Item::Use(item) => &mut item.attrs,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{render, CODEGEN_FAILURE};
    use quote::quote;

    /// No known spec reaches it, so the failure is pinned here: a token stream that is not a Rust
    /// file comes back as an error carrying [`CODEGEN_FAILURE`], not as a module.
    #[test]
    fn an_unparseable_token_stream_is_an_error_not_a_module() {
        let error = render(quote! { fn }).expect_err("`fn` alone is not a Rust file");
        assert!(error.to_string().starts_with(CODEGEN_FAILURE), "{error}");
        assert!(render(quote! { fn f() {} }).is_ok());
    }

    /// Every top-level item leaves `render` allowing the unsatisfiable lints exactly once: merged
    /// into an `#[allow]` it already carries, without repeating a lint that one names, and as an
    /// `#[allow]` of its own otherwise. Items nested inside are reached through their parent.
    #[test]
    fn every_top_level_item_allows_the_unsatisfiable_lints_once() {
        let rendered = render(quote! {
            #[allow(dead_code, unused_imports)]
            mod support { pub struct Inner; }
            #[derive(Debug)]
            pub struct Client;
            impl Client { pub fn new() -> Self { Self } }
            pub use support::Inner;
        })
        .expect("a Rust file")
        .contents;
        let merged =
            "#[allow(\n    dead_code,\n    unused_imports,\n    missing_docs,\n    unreachable_pub,\n    clippy::pedantic,\n    clippy::nursery\n)]\nmod support";
        let own =
            "#[allow(missing_docs, unreachable_pub, dead_code, clippy::pedantic, clippy::nursery)]";
        assert!(rendered.contains(merged), "{rendered}");
        assert_eq!(rendered.matches(own).count(), 3, "{rendered}");
        assert_eq!(rendered.matches("#[allow").count(), 4, "{rendered}");
        assert!(rendered.contains("pub struct Inner;"), "{rendered}");
    }

    /// The shared test oracles match this very literal, and every harness that runs `generate`
    /// holds its runs to them, so each no-codegen-failure oracle sees each one: renaming the prefix
    /// here without the oracles, or a harness that stops calling them, fails this test rather than
    /// silently blinding them.
    #[test]
    fn the_harnesses_match_the_codegen_failure_prefix() {
        let read = |path: &str| {
            std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
                .unwrap_or_else(|error| panic!("{path} is unreadable: {error}"))
        };
        let declared = format!("pub const CODEGEN_FAILURE: &str = \"{CODEGEN_FAILURE}\";");
        assert!(
            read("tests/oracles/mod.rs").contains(&declared),
            "tests/oracles/mod.rs must declare `{declared}`"
        );
        for harness in [
            "tests/frontend/main.rs",
            "tests/fuzz_frontend.rs",
            "tests/lowering_props.rs",
        ] {
            assert!(
                read(harness).contains("oracles::run_violations("),
                "{harness} must hold its runs to `oracles::run_violations`"
            );
        }
    }
}
