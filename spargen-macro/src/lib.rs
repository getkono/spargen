//! # spargen-macro
//!
//! The proc-macro front-end for [`spargen`](https://docs.rs/spargen): generate a typed OpenAPI
//! 3.1.x/3.2.x client **inline**, with no `build.rs` and no CLI step.
//!
//! ```ignore
//! mod api {
//!     // Path is resolved relative to the consumer crate's Cargo.toml.
//!     spargen_macro::generate_api!("openapi.yaml");
//! }
//! ```
//!
//! Keyed form, with the same generation controls as the `build.rs` API:
//!
//! ```ignore
//! spargen_macro::generate_api!(
//!     spec = "openapi.yaml",
//!     no_uuid,
//!     no_time,
//!     carve,
//!     open_narrowing,
//!     error_body_cap = 65536,
//!     batch_cap = 100,
//!     omit {
//!         operations { post "/legacy"; }
//!         paths { "/internal/**"; }
//!         components { schemas { "LegacyPet"; } }
//!         pointers { "/webhooks"; }
//!         file("shared.yaml") { pointers { "/Legacy"; } }
//!     }
//! );
//! ```
//!
//! ## How it works
//!
//! The macro is a thin shim over spargen's internal in-memory renderer. It resolves the spec,
//! renders the client, and parses the rendered source back into tokens. The generated API is the
//! same as the module written by [`spargen::generate`] from a `build.rs`.
//!
//! A generation failure becomes a `compile_error!` carrying spargen's diagnostics — the same
//! loud, no-silent-degradation contract the generator has. (Warnings are not surfaced: stable proc-macro
//! APIs cannot emit them. Run `spargen check <spec>` to see warnings.)
//!
//! ## Cost & alternatives
//!
//! Inline generation recompiles the whole generator (host-side) as part of your build, and the
//! generated code is not materialized on disk (use `cargo expand` to inspect it). When you want
//! the generated source checked in or reviewable, configure the `build.rs` API to write it there.
//! The macro trades that visibility for a zero-config setup with no build.rs.
//!
//! ## Runtime graph
//!
//! This crate and `spargen` are **host/build-time only** — a proc-macro crate is never linked into
//! your binary. Your runtime dependencies are just what the generated code uses (reqwest, serde,
//! …); no spargen crate appears in `cargo tree -e no-proc-macro`.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{braced, parenthesized, Ident, LitInt, LitStr, Token};

/// Generate a typed OpenAPI 3.1.x/3.2.x client in place. See the [crate docs](crate) for forms and
/// caveats.
#[proc_macro]
pub fn generate_api(input: TokenStream) -> TokenStream {
    let args = syn::parse_macro_input!(input as Args);
    match expand(&args) {
        // Cross a string boundary and let the compiler re-tokenize the generated source on its own
        // thread. The tokens from `expand` were built under proc-macro2's fallback (see there), so
        // this reparse — on the real macro server thread — is what binds real spans to the output.
        Ok(tokens) => match tokens.to_string().parse() {
            Ok(stream) => stream,
            Err(error) => syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("spargen produced code that failed to tokenize: {error}"),
            )
            .to_compile_error()
            .into(),
        },
        Err(error) => error.to_compile_error().into(),
    }
}

/// Forces proc-macro2's thread-safe fallback token implementation for its lifetime, restoring the
/// real compiler bridge on drop (even across a panic).
///
/// spargen builds tokens with proc-macro2 internally, on its own worker thread. Inside a
/// proc-macro, proc-macro2 otherwise routes to the real compiler bridge — whose API panics when
/// touched off the macro server thread ("procedural macro API is used outside of a procedural
/// macro"). The fallback is spargen's normal mode under `build.rs`/CLI, so this changes nothing
/// about the output; it just keeps generation off the bridge.
struct FallbackGuard;

impl FallbackGuard {
    fn force() -> Self {
        proc_macro2::fallback::force();
        FallbackGuard
    }
}

impl Drop for FallbackGuard {
    fn drop(&mut self) {
        proc_macro2::fallback::unforce();
    }
}

/// Parsed macro arguments: a spec path (positional string or `spec = "..."`) plus optional flags.
struct Args {
    spec: LitStr,
    no_uuid: bool,
    no_time: bool,
    carve: bool,
    open_narrowing: bool,
    error_body_cap: Option<usize>,
    batch_cap: Option<usize>,
    omit: spargen::Omit,
}

impl Parse for Args {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut spec: Option<LitStr> = None;
        let mut no_uuid = false;
        let mut no_time = false;
        let mut carve = false;
        let mut open_narrowing = false;
        let mut error_body_cap = None;
        let mut batch_cap = None;
        let mut omit = spargen::Omit::default();

        while !input.is_empty() {
            if input.peek(LitStr) {
                let lit: LitStr = input.parse()?;
                if spec.is_some() {
                    return Err(syn::Error::new(
                        lit.span(),
                        "spec path given more than once",
                    ));
                }
                spec = Some(lit);
            } else {
                let key: Ident = input.parse()?;
                match key.to_string().as_str() {
                    "spec" => {
                        input.parse::<Token![=]>()?;
                        let lit: LitStr = input.parse()?;
                        if spec.is_some() {
                            return Err(syn::Error::new(
                                lit.span(),
                                "spec path given more than once",
                            ));
                        }
                        spec = Some(lit);
                    }
                    "no_uuid" => no_uuid = true,
                    "no_time" => no_time = true,
                    "carve" => carve = true,
                    "open_narrowing" => open_narrowing = true,
                    "error_body_cap" => {
                        input.parse::<Token![=]>()?;
                        error_body_cap = Some(parse_usize(input)?);
                    }
                    "batch_cap" => {
                        input.parse::<Token![=]>()?;
                        batch_cap = Some(parse_usize(input)?);
                    }
                    "omit" => parse_omit(input, &mut omit)?,
                    other => {
                        return Err(syn::Error::new(
                            key.span(),
                            format!(
                                "unknown argument `{other}`; expected a spec path or one of: \
                                 no_uuid, no_time, carve, open_narrowing, error_body_cap, batch_cap, \
                                 omit"
                            ),
                        ));
                    }
                }
            }

            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        let spec = spec.ok_or_else(|| {
            input.error("expected a spec path, e.g. generate_api!(\"openapi.yaml\")")
        })?;
        Ok(Args {
            spec,
            no_uuid,
            no_time,
            carve,
            open_narrowing,
            error_body_cap,
            batch_cap,
            omit,
        })
    }
}

fn expand(args: &Args) -> syn::Result<proc_macro2::TokenStream> {
    let raw = args.spec.value();
    let consumer = Consumer::locate(
        std::env::var_os("CARGO_MANIFEST_PATH"),
        std::env::var_os("CARGO_MANIFEST_DIR"),
    )
    .map_err(|message| syn::Error::new(args.spec.span(), message))?;
    let config = spec_for(args, &consumer)?;

    // Keep spargen's codegen (and the tokenization below) off the compiler bridge; restored on drop.
    let _fallback = FallbackGuard::force();
    let preview = spargen::__private::preview_for_macro(&config, &consumer.manifest);

    if let Some(message) = failure_message(&raw, &preview) {
        return Err(syn::Error::new(args.spec.span(), message));
    }

    let source = preview.contents.as_ref().expect("checked above");
    let generated: proc_macro2::TokenStream = source.parse().map_err(|error| {
        syn::Error::new(
            args.spec.span(),
            format!("spargen produced code that failed to tokenize: {error}"),
        )
    })?;

    let tracks = tracking(&preview);
    Ok(quote! {
        #generated
        #tracks
    })
}

/// The `Spec` `expand` generates from: the spec path resolved against the consumer, the
/// `spargen.toml` beside it applied, then the macro arguments over that — the same precedence the
/// CLI and `build.rs` use. A config file that cannot be read or parsed is an error at the spec
/// literal.
fn spec_for(args: &Args, consumer: &Consumer) -> syn::Result<spargen::Spec> {
    let spec_path = consumer.resolve_spec_path(&args.spec.value());
    let discovered = match spargen::Spec::new(spec_path).discover_config_file() {
        Ok(spec) => spec,
        Err(error) => return Err(syn::Error::new(args.spec.span(), error.to_string())),
    };
    Ok(config(args, discovered))
}

/// The `compile_error!` text for a preview that produced no client, or `None` when it produced
/// one. `raw` is the spec path as the macro was given it. Each error-severity diagnostic is one
/// line under the heading; warnings are left out.
fn failure_message(raw: &str, preview: &spargen::__private::MacroPreview) -> Option<String> {
    if preview.report.outcome() == spargen::Outcome::Generated && preview.contents.is_some() {
        return None;
    }
    let errors: Vec<&spargen::Diagnostic> = preview
        .report
        .diagnostics()
        .iter()
        .filter(|d| d.severity == spargen::Severity::Error)
        .collect();
    let mut message = format!("spargen could not generate a client from `{raw}`");
    if errors.is_empty() {
        message.push_str(": generation did not succeed");
    } else {
        for diagnostic in &errors {
            message.push_str(&format!(
                "\n  error[{}]: {} (at {})",
                diagnostic.code, diagnostic.message, diagnostic.pointer
            ));
        }
    }
    Some(message)
}

/// One `const _: &[u8] = include_bytes!(..);` item per file in `preview.source_files`, in that
/// order, which is what makes Cargo re-expand the macro when one of them changes (proc-macros
/// cannot emit `rerun-if-changed`). Every path exists: generation already read it.
fn tracking(preview: &spargen::__private::MacroPreview) -> proc_macro2::TokenStream {
    let tracks = preview
        .source_files
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    quote! {
        #(const _: &[u8] = include_bytes!(#tracks);)*
    }
}

/// Apply the macro arguments over `discovered`, the `Spec` the config file beside the spec yields.
/// A flag only ever overrides: an argument the macro was not given leaves the config file's value
/// in place, and omit rules are appended to the file's own.
fn config(args: &Args, discovered: spargen::Spec) -> spargen::Spec {
    let mut config = discovered;
    if args.no_uuid {
        config = config.uuid(false);
    }
    if args.no_time {
        config = config.time(false);
    }
    if args.carve {
        config = config.carve(true);
    }
    if args.open_narrowing {
        config = config.open_narrowing(true);
    }
    for rule in &args.omit.rules {
        config = config.omit_rule(rule.clone());
    }
    if let Some(cap) = args.error_body_cap {
        config = config.error_body_cap(cap);
    }
    if let Some(cap) = args.batch_cap {
        config = config.batch_cap(cap);
    }
    config
}

fn parse_usize(input: ParseStream) -> syn::Result<usize> {
    let value: LitInt = input.parse()?;
    value.base10_parse()
}

fn parse_omit(input: ParseStream, omit: &mut spargen::Omit) -> syn::Result<()> {
    let body;
    braced!(body in input);
    while !body.is_empty() {
        let section: Ident = body.parse()?;
        match section.to_string().as_str() {
            "operations" => parse_operations(&body, omit)?,
            "paths" => parse_paths(&body, omit)?,
            "components" => parse_components(&body, omit)?,
            "pointers" => parse_pointers(&body, omit, None)?,
            "file" => {
                let argument;
                parenthesized!(argument in body);
                let file: LitStr = argument.parse()?;
                let file_body;
                braced!(file_body in body);
                let pointers: Ident = file_body.parse()?;
                if pointers != "pointers" {
                    return Err(syn::Error::new(pointers.span(), "expected `pointers`"));
                }
                parse_pointers(&file_body, omit, Some(file.value().into()))?;
            }
            other => {
                return Err(syn::Error::new(
                    section.span(),
                    format!("unknown omit section `{other}`"),
                ));
            }
        }
    }
    Ok(())
}

fn parse_operations(input: ParseStream, omit: &mut spargen::Omit) -> syn::Result<()> {
    let body;
    braced!(body in input);
    while !body.is_empty() {
        let method: Ident = body.parse()?;
        // Delegated to `OmitMethod`'s own `FromStr` rather than matched here, so the macro cannot
        // fall behind the method set the generator supports.
        let method = method
            .to_string()
            .parse::<spargen::OmitMethod>()
            .map_err(|error| syn::Error::new(method.span(), format!("{error}")))?;
        let path: LitStr = body.parse()?;
        body.parse::<Token![;]>()?;
        omit.rules.push(spargen::OmitRule::Operation {
            method,
            path: path.value().into(),
        });
    }
    Ok(())
}

fn parse_paths(input: ParseStream, omit: &mut spargen::Omit) -> syn::Result<()> {
    let body;
    braced!(body in input);
    while !body.is_empty() {
        let path: LitStr = body.parse()?;
        body.parse::<Token![;]>()?;
        omit.rules.push(spargen::OmitRule::Path {
            path: path.value().into(),
        });
    }
    Ok(())
}

fn parse_components(input: ParseStream, omit: &mut spargen::Omit) -> syn::Result<()> {
    let body;
    braced!(body in input);
    while !body.is_empty() {
        let kind: Ident = body.parse()?;
        // One shared parser, so `omit!`, the CLI, and `spargen.toml` accept the same spellings.
        let kind = kind
            .to_string()
            .parse::<spargen::ComponentKind>()
            .map_err(|error| {
                syn::Error::new(
                    kind.span(),
                    format!(
                        "{error}; expected one of \
                         schemas/responses/parameters/request_bodies/headers/security_schemes/\
                         path_items/media_types"
                    ),
                )
            })?;
        let names;
        braced!(names in body);
        while !names.is_empty() {
            let name: LitStr = names.parse()?;
            names.parse::<Token![;]>()?;
            omit.rules
                .push(spargen::OmitRule::component(kind, name.value()));
        }
    }
    Ok(())
}

fn parse_pointers(
    input: ParseStream,
    omit: &mut spargen::Omit,
    file: Option<std::borrow::Cow<'static, str>>,
) -> syn::Result<()> {
    let body;
    braced!(body in input);
    while !body.is_empty() {
        let pointer: LitStr = body.parse()?;
        body.parse::<Token![;]>()?;
        omit.rules.push(spargen::OmitRule::Pointer {
            file: file.clone(),
            pointer: pointer.value().into(),
        });
    }
    Ok(())
}

/// The crate `generate_api!` is expanding for, as the build driver names it.
///
/// Both the spec path and the manifest the runtime-dependency audit (`E023`) reads come from here,
/// and neither ever falls back to the working directory. Under a Cargo workspace build the working
/// directory is the **workspace root**, whose `Cargo.toml` is usually virtual: auditing it reports
/// every runtime dependency missing at spargen's floor version, a confident, specific, wrong
/// diagnostic about a file the consumer never pointed at. Not knowing the crate is an error.
#[derive(Debug, PartialEq, Eq)]
struct Consumer {
    /// The directory a relative spec path resolves against.
    dir: String,
    /// The manifest the audit reads.
    manifest: String,
}

impl Consumer {
    /// Locate the consumer from `CARGO_MANIFEST_PATH` and `CARGO_MANIFEST_DIR`, passed in rather
    /// than read here so the decision is testable without mutating the process environment.
    ///
    /// `CARGO_MANIFEST_PATH` names the manifest when set; otherwise it is `Cargo.toml` inside
    /// `CARGO_MANIFEST_DIR`. The spec directory is `CARGO_MANIFEST_DIR`, or the manifest's parent
    /// when only the path is set. An empty value is treated as unset, since joining onto it would
    /// reproduce the working-directory guess. Paths must be UTF-8: a lossy conversion would name
    /// a different file.
    fn locate(
        manifest_path: Option<std::ffi::OsString>,
        manifest_dir: Option<std::ffi::OsString>,
    ) -> Result<Self, String> {
        let utf8 =
            |name: &str, value: Option<std::ffi::OsString>| -> Result<Option<String>, String> {
                match value {
                    None => Ok(None),
                    Some(value) if value.is_empty() => Ok(None),
                    Some(value) => value.into_string().map(Some).map_err(|value| {
                        format!(
                        "spargen cannot locate the consuming crate: `{name}` is not valid UTF-8 \
                         ({})",
                        std::path::Path::new(&value).display()
                    )
                    }),
                }
            };
        let manifest_path = utf8("CARGO_MANIFEST_PATH", manifest_path)?;
        let manifest_dir = utf8("CARGO_MANIFEST_DIR", manifest_dir)?;
        match (manifest_path, manifest_dir) {
            (Some(manifest), Some(dir)) => Ok(Consumer { dir, manifest }),
            (Some(manifest), None) => {
                let dir = match std::path::Path::new(&manifest).parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => {
                        parent.to_string_lossy().into_owned()
                    }
                    _ => ".".to_owned(),
                };
                Ok(Consumer { dir, manifest })
            }
            (None, Some(dir)) => {
                let manifest = std::path::Path::new(&dir)
                    .join("Cargo.toml")
                    .to_string_lossy()
                    .into_owned();
                Ok(Consumer { dir, manifest })
            }
            (None, None) => Err("spargen cannot locate the consuming crate: neither \
                 `CARGO_MANIFEST_PATH` nor `CARGO_MANIFEST_DIR` is set, so there is no \
                 `Cargo.toml` to resolve the spec against or to audit runtime dependencies in. \
                 Expand `generate_api!` under Cargo, or have the build driver set \
                 `CARGO_MANIFEST_DIR` to the consuming crate's directory."
                .to_owned()),
        }
    }

    /// Resolve a spec path relative to the consumer crate's directory (as `build.rs` does from
    /// that crate root), so `generate_api!("openapi.yaml")` finds a spec beside the caller's
    /// `Cargo.toml`. Absolute paths pass through unchanged.
    fn resolve_spec_path(&self, raw: &str) -> String {
        let path = std::path::Path::new(raw);
        if path.is_absolute() {
            return raw.to_owned();
        }
        std::path::Path::new(&self.dir)
            .join(path)
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{config, failure_message, spec_for, tracking, Args, Consumer};
    use spargen::{ComponentKind, OmitMethod, OmitRule, Spec};

    fn os(value: &str) -> Option<std::ffi::OsString> {
        Some(value.into())
    }

    #[test]
    fn an_unnamed_consumer_is_an_error_not_the_working_directory() {
        // #195: with neither variable set, the macro used to audit `./Cargo.toml` — under a
        // workspace build, the (usually virtual) workspace root — and report every runtime
        // dependency missing at spargen's floor version. Not knowing the crate must be an error.
        let error = Consumer::locate(None, None).unwrap_err();
        assert!(
            error.contains("CARGO_MANIFEST_PATH") && error.contains("CARGO_MANIFEST_DIR"),
            "{error}"
        );
        // An empty value joined onto would reproduce the same working-directory guess.
        assert_eq!(Consumer::locate(os(""), os("")).unwrap_err(), error);
    }

    #[test]
    fn the_consumer_comes_from_whichever_variable_cargo_set() {
        let dir = "/work/member";
        let manifest = "/work/member/Cargo.toml";
        let expected = Consumer {
            dir: dir.to_owned(),
            manifest: manifest.to_owned(),
        };
        assert_eq!(Consumer::locate(os(manifest), os(dir)).unwrap(), expected);
        assert_eq!(Consumer::locate(None, os(dir)).unwrap(), expected);
        assert_eq!(Consumer::locate(os(manifest), None).unwrap(), expected);
        // An empty variable beside a set one is ignored, not joined onto.
        assert_eq!(Consumer::locate(os(""), os(dir)).unwrap(), expected);
        assert_eq!(Consumer::locate(os(manifest), os("")).unwrap(), expected);

        let consumer = Consumer::locate(None, os(dir)).unwrap();
        assert_eq!(
            consumer.resolve_spec_path("openapi.yaml"),
            std::path::Path::new(dir)
                .join("openapi.yaml")
                .to_string_lossy()
        );
        assert_eq!(consumer.resolve_spec_path(manifest), manifest);

        // A bare relative manifest path has an empty parent; the spec directory is then the one
        // that path is relative to, spelled `.`, never an empty prefix.
        let bare = Consumer::locate(os("Cargo.toml"), None).unwrap();
        assert_eq!(
            bare,
            Consumer {
                dir: ".".to_owned(),
                manifest: "Cargo.toml".to_owned(),
            }
        );
        assert_eq!(
            bare.resolve_spec_path("openapi.yaml"),
            std::path::Path::new(".")
                .join("openapi.yaml")
                .to_string_lossy()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_location_is_an_error_not_a_lossy_path() {
        use std::os::unix::ffi::OsStringExt;
        let invalid = std::ffi::OsString::from_vec(b"/work/\xFF/Cargo.toml".to_vec());
        let error = Consumer::locate(Some(invalid), None).unwrap_err();
        assert!(
            error.contains("`CARGO_MANIFEST_PATH` is not valid UTF-8"),
            "{error}"
        );
    }

    /// Every argument the keyed form accepts, each set away from its default.
    const EVERY_CONTROL: &str = r#"spec = "openapi.yaml", no_uuid, no_time, carve, open_narrowing,
               error_body_cap = 4096, batch_cap = 7,
               omit {
                   operations { post "/legacy"; }
                   paths { "/internal/**"; }
                   components { schemas { "Legacy"; } }
                   pointers { "/webhooks"; }
                   file("shared.yaml") { pointers { "/Legacy"; } }
               }"#;

    /// The rules [`EVERY_CONTROL`]'s `omit` block names, in the order it names them.
    fn every_omit_rule() -> Vec<OmitRule> {
        vec![
            OmitRule::operation(OmitMethod::Post, "/legacy"),
            OmitRule::path("/internal/**"),
            OmitRule::component(ComponentKind::Schemas, "Legacy"),
            OmitRule::pointer(None, "/webhooks"),
            OmitRule::pointer(Some("shared.yaml".into()), "/Legacy"),
        ]
    }

    #[test]
    fn parses_every_build_configuration_control() {
        let args: Args = syn::parse_str(EVERY_CONTROL).unwrap();

        assert_eq!(args.spec.value(), "openapi.yaml");
        assert!(args.no_uuid);
        assert!(args.no_time);
        assert!(args.carve);
        assert!(args.open_narrowing);
        assert_eq!(args.error_body_cap, Some(4096));
        assert_eq!(args.batch_cap, Some(7));
        assert_eq!(args.omit.rules, every_omit_rule());
    }

    #[test]
    fn a_bare_spec_path_sets_no_control() {
        let args: Args = syn::parse_str(r#""openapi.yaml""#).unwrap();

        assert_eq!(args.spec.value(), "openapi.yaml");
        assert!(!args.no_uuid);
        assert!(!args.no_time);
        assert!(!args.carve);
        assert!(!args.open_narrowing);
        assert_eq!(args.error_body_cap, None);
        assert_eq!(args.batch_cap, None);
        assert_eq!(args.omit.rules, Vec::new());
    }

    /// Each argument reaches the `Spec` field it names, and nothing else changes.
    #[test]
    fn every_argument_maps_onto_its_spec_field() {
        let args: Args = syn::parse_str(EVERY_CONTROL).unwrap();
        let mut expected = Spec::new("openapi.yaml")
            .uuid(false)
            .time(false)
            .carve(true)
            .open_narrowing(true)
            .error_body_cap(4096)
            .batch_cap(7);
        for rule in every_omit_rule() {
            expected = expected.omit_rule(rule);
        }
        assert_eq!(config(&args, Spec::new("openapi.yaml")), expected);
    }

    /// The macro overrides the discovered config file only where it was given an argument: an
    /// absent flag keeps the file's value rather than resetting it, and omit rules append.
    #[test]
    fn an_absent_argument_keeps_the_config_files_value() {
        let discovered = Spec::new("openapi.yaml")
            .uuid(false)
            .time(false)
            .carve(true)
            .open_narrowing(true)
            .error_body_cap(1024)
            .batch_cap(3)
            .omit_rule(OmitRule::path("/from-file"));

        let bare: Args = syn::parse_str(r#""openapi.yaml""#).unwrap();
        assert_eq!(config(&bare, discovered.clone()), discovered);

        let capped: Args =
            syn::parse_str(r#""openapi.yaml", batch_cap = 9, omit { paths { "/from-macro"; } }"#)
                .unwrap();
        assert_eq!(
            config(&capped, discovered.clone()),
            discovered
                .batch_cap(9)
                .omit_rule(OmitRule::path("/from-macro"))
        );
    }

    /// A directory under the system temporary directory, removed on drop.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("spargen-macro-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        /// The UTF-8 path of `relative` inside this directory.
        fn path(&self, relative: &str) -> String {
            self.0.join(relative).to_str().unwrap().to_owned()
        }

        /// Write `text` to `relative`, creating its parent, and return its path.
        fn write(&self, relative: &str, text: &str) -> String {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            self.path(relative)
        }

        /// The consumer whose crate directory this is, with its `Cargo.toml` as the manifest.
        fn consumer(&self) -> Consumer {
            Consumer {
                dir: self.path(""),
                manifest: self.path("Cargo.toml"),
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A spec that generates, its one schema reached through a relative-file `$ref`.
    const SPLIT_SPEC: &str = r#"openapi: 3.1.0
info: { title: Split, version: "1" }
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "schemas.yaml#/Pet" }
"#;

    const SPLIT_SCHEMAS: &str = "Pet:\n  type: object\n  properties:\n    name: { type: string }\n";

    fn args(source: &str) -> Args {
        match syn::parse_str(source) {
            Ok(args) => args,
            Err(error) => panic!("`{source}` did not parse: {error}"),
        }
    }

    fn parse_error(source: &str) -> String {
        match syn::parse_str::<Args>(source) {
            Ok(_) => panic!("`{source}` parsed"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn no_config_file_leaves_the_spec_at_its_defaults() {
        let scratch = Scratch::new("no-config");
        let spec = spec_for(&args(r#""openapi.yaml""#), &scratch.consumer()).unwrap();
        assert_eq!(spec, Spec::new(scratch.path("openapi.yaml")));
    }

    /// The `spargen.toml` read is the one beside the spec, not the one beside the consumer's
    /// `Cargo.toml`, and the macro arguments override it.
    #[test]
    fn the_config_file_beside_the_spec_is_applied_under_the_arguments() {
        let scratch = Scratch::new("config-beside-spec");
        scratch.write("spargen.toml", "open_narrowing = true\n");
        scratch.write("api/spargen.toml", "carve = true\nbatch_cap = 3\n");

        let spec = spec_for(&args(r#""api/openapi.yaml""#), &scratch.consumer()).unwrap();
        assert_eq!(
            spec,
            Spec::new(scratch.path("api/openapi.yaml"))
                .carve(true)
                .batch_cap(3)
        );

        let spec = spec_for(
            &args(r#""api/openapi.yaml", batch_cap = 9"#),
            &scratch.consumer(),
        )
        .unwrap();
        assert_eq!(
            spec,
            Spec::new(scratch.path("api/openapi.yaml"))
                .carve(true)
                .batch_cap(9)
        );
    }

    #[test]
    fn an_invalid_config_file_is_an_error_naming_it() {
        let scratch = Scratch::new("invalid-config");
        let config = scratch.write("spargen.toml", "[features]\ncarve = true\n");
        let error = spec_for(&args(r#""openapi.yaml""#), &scratch.consumer()).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "invalid config file `{config}`: the `[features]` table was removed; move its \
                 keys (`carve`, `batch_cap`, …) to the top level of the file"
            )
        );
    }

    #[test]
    fn a_generated_preview_is_no_failure() {
        let scratch = Scratch::new("generated");
        let spec = scratch.write("openapi.yaml", SPLIT_SPEC);
        scratch.write("schemas.yaml", SPLIT_SCHEMAS);
        let preview = spargen::__private::preview(&Spec::new(spec));
        assert_eq!(failure_message("openapi.yaml", &preview), None);
    }

    /// The `compile_error!` text: the spec as the macro was given it, then one line per
    /// error-severity diagnostic with its code, message and pointer.
    #[test]
    fn a_rejected_spec_renders_each_error_with_its_code_and_pointer() {
        let scratch = Scratch::new("rejected");
        let spec = scratch.write(
            "openapi.yaml",
            "openapi: 3.0.3\ninfo: { title: Old, version: \"1\" }\npaths: {}\n",
        );
        let preview = spargen::__private::preview(&Spec::new(spec));
        // A root-level diagnostic's pointer is empty, and renders as `(at )`.
        assert_eq!(
            failure_message("specs/old.yaml", &preview).unwrap(),
            "spargen could not generate a client from `specs/old.yaml`\n  \
             error[E001]: unsupported OpenAPI version `3.0.3`; spargen implements 3.1.x and \
             3.2.x (at )"
        );
    }

    /// Several errors are one line each, in the report's order; a warning beside them is not
    /// rendered.
    #[test]
    fn every_error_is_its_own_line_and_warnings_are_left_out() {
        let scratch = Scratch::new("several");
        let spec = scratch.write(
            "openapi.yaml",
            r##"openapi: 3.1.0
info: { title: Dangling, version: "1" }
paths:
  /a:
    get:
      operationId: getA
      parameters:
        - { name: q, in: query, schema: { type: string, pattern: "^a" } }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Missing" }
  /b:
    get:
      operationId: getB
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/AlsoMissing" }
"##,
        );
        let preview = spargen::__private::preview(&Spec::new(spec));
        assert!(
            preview
                .report
                .diagnostics()
                .iter()
                .any(|d| d.severity == spargen::Severity::Warning),
            "the fixture must carry a warning for its omission to be observed"
        );
        assert_eq!(
            failure_message("openapi.yaml", &preview).unwrap(),
            "spargen could not generate a client from `openapi.yaml`\n  \
             error[E004]: unresolved schema reference `#/components/schemas/Missing` \
             (at /paths/~1a/get/responses/200/content/application~1json/schema)\n  \
             error[E004]: unresolved schema reference `#/components/schemas/AlsoMissing` \
             (at /paths/~1b/get/responses/200/content/application~1json/schema)"
        );
    }

    /// The files a successful expansion `include_bytes!`s, and so the files whose edit makes
    /// Cargo re-expand it: the spec, every file its `$ref`s load, the manifest the audit read, and
    /// a `spargen.lock` beside the spec. The `spargen.toml` beside the spec is not among them.
    #[test]
    fn tracking_includes_the_spec_its_refs_the_manifest_and_the_lock() {
        let scratch = Scratch::new("tracking");
        let spec = scratch.write("openapi.yaml", SPLIT_SPEC);
        let schemas = scratch.write("schemas.yaml", SPLIT_SCHEMAS);
        let lock = scratch.write("spargen.lock", "version = 1\n");
        scratch.write("spargen.toml", "carve = false\n");
        let requirements = spargen::requirements(&Spec::new(spec.clone())).unwrap();
        // `[workspace]` makes the fixture its own workspace root, so the audit's walk upward for
        // one stops here instead of reading an ancestor manifest above the temporary directory.
        let manifest = scratch.write(
            "Cargo.toml",
            &format!(
                "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
                 [workspace]\n\n{}",
                requirements.manifest_block()
            ),
        );

        let preview = spargen::__private::preview_for_macro(&Spec::new(spec.clone()), &manifest);
        assert_eq!(failure_message("openapi.yaml", &preview), None);
        let mut expected = vec![manifest, spec, schemas, lock];
        expected.sort();
        assert_eq!(
            tracking(&preview).to_string(),
            quote::quote! { #(const _: &[u8] = include_bytes!(#expected);)* }.to_string()
        );
    }

    #[test]
    fn every_argument_parse_error_names_what_was_wrong() {
        let cases: &[(&str, &str)] = &[
            (r#""a.yaml", "b.yaml""#, "spec path given more than once"),
            (
                r#""a.yaml", spec = "b.yaml""#,
                "spec path given more than once",
            ),
            (r#"spec "a.yaml""#, "expected `=`"),
            (r#"spec = 1"#, "expected string literal"),
            (r#"42"#, "expected identifier"),
            (
                r#""a.yaml", uuid"#,
                "unknown argument `uuid`; expected a spec path or one of: no_uuid, no_time, \
                 carve, open_narrowing, error_body_cap, batch_cap, omit",
            ),
            (r#""a.yaml" no_uuid"#, "expected `,`"),
            (
                "no_uuid",
                "unexpected end of input, expected a spec path, e.g. \
                 generate_api!(\"openapi.yaml\")",
            ),
            (
                r#""a.yaml", error_body_cap = "big""#,
                "expected integer literal",
            ),
            (
                r#""a.yaml", error_body_cap = 99999999999999999999999"#,
                "number too large to fit in target type",
            ),
            (r#""a.yaml", batch_cap 3"#, "expected `=`"),
            (
                r#""a.yaml", batch_cap = -1"#,
                "invalid digit found in string",
            ),
            (r#""a.yaml", omit ( )"#, "expected curly braces"),
            (
                r#""a.yaml", omit { schemas { } }"#,
                "unknown omit section `schemas`",
            ),
            (
                r#""a.yaml", omit { operations { fetch "/a"; } }"#,
                "unknown HTTP method: `fetch`",
            ),
            (
                r#""a.yaml", omit { operations { get "/a" } }"#,
                "expected `;`",
            ),
            (r#""a.yaml", omit { paths { "/a" } }"#, "expected `;`"),
            (
                r#""a.yaml", omit { components { models { "A"; } } }"#,
                "unknown component kind: `models`; expected one of \
                 schemas/responses/parameters/request_bodies/headers/security_schemes/\
                 path_items/media_types",
            ),
            (r#""a.yaml", omit { pointers { "/a" } }"#, "expected `;`"),
            (
                r#""a.yaml", omit { file("b.yaml") { paths { "/a"; } } }"#,
                "expected `pointers`",
            ),
            (
                r#""a.yaml", omit { file { pointers { "/a"; } } }"#,
                "expected parentheses",
            ),
        ];
        let mut mismatches = Vec::new();
        for (source, expected) in cases {
            let actual = parse_error(source);
            if actual != *expected {
                mismatches.push(format!(
                    "{source}\n  expected: {expected}\n  actual:   {actual}"
                ));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
