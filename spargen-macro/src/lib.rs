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
//! The macro trades that visibility for a zero-config, single-dependency setup.
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
                                 no_uuid, no_time, carve, error_body_cap, batch_cap, omit"
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
            error_body_cap,
            batch_cap,
            omit,
        })
    }
}

fn expand(args: &Args) -> syn::Result<proc_macro2::TokenStream> {
    let raw = args.spec.value();
    let spec_path = resolve_spec_path(&raw);

    let mut config = spargen::Config::new(
        spec_path.clone(),
        // Never written — the private preview bridge renders in memory.
        "generated.rs",
    );
    config.features.uuid = !args.no_uuid;
    config.features.time = !args.no_time;
    config.carve = args.carve;
    config.omit = args.omit.clone();
    if let Some(cap) = args.error_body_cap {
        config.error_body_cap = cap;
    }
    if let Some(cap) = args.batch_cap {
        config.batch_cap = cap;
    }

    // Keep spargen's codegen (and the tokenization below) off the compiler bridge; restored on drop.
    let _fallback = FallbackGuard::force();
    let preview = spargen::__private::preview(&config);

    let errors: Vec<&spargen::Diagnostic> = preview
        .report
        .diagnostics
        .iter()
        .filter(|d| d.severity == spargen::Severity::Error)
        .collect();

    if preview.report.outcome != spargen::Outcome::Generated || preview.contents.is_none() {
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
        return Err(syn::Error::new(args.spec.span(), message));
    }

    let source = preview.contents.as_ref().expect("checked above");
    let generated: proc_macro2::TokenStream = source.parse().map_err(|error| {
        syn::Error::new(
            args.spec.span(),
            format!("spargen produced code that failed to tokenize: {error}"),
        )
    })?;

    // Force a rebuild whenever the spec changes: referencing it via `include_bytes!` makes Cargo
    // track the file (proc-macros cannot emit `rerun-if-changed`). The path exists — generation
    // above already read it.
    let tracks = preview
        .source_files
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    Ok(quote! {
        #generated
        #(const _: &[u8] = include_bytes!(#tracks);)*
    })
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
                parse_pointers(&file_body, omit, Some(leak(file.value())))?;
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
        let method = match method.to_string().as_str() {
            "get" => spargen::OmitMethod::Get,
            "put" => spargen::OmitMethod::Put,
            "post" => spargen::OmitMethod::Post,
            "delete" => spargen::OmitMethod::Delete,
            "options" => spargen::OmitMethod::Options,
            "head" => spargen::OmitMethod::Head,
            "patch" => spargen::OmitMethod::Patch,
            "trace" => spargen::OmitMethod::Trace,
            _ => return Err(syn::Error::new(method.span(), "unsupported HTTP method")),
        };
        let path: LitStr = body.parse()?;
        body.parse::<Token![;]>()?;
        omit.rules.push(spargen::OmitRule::Operation {
            method,
            path: leak(path.value()),
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
            path: leak(path.value()),
        });
    }
    Ok(())
}

fn parse_components(input: ParseStream, omit: &mut spargen::Omit) -> syn::Result<()> {
    let body;
    braced!(body in input);
    while !body.is_empty() {
        let kind: Ident = body.parse()?;
        let kind = match kind.to_string().as_str() {
            "schemas" => spargen::ComponentKind::Schemas,
            "responses" => spargen::ComponentKind::Responses,
            "parameters" => spargen::ComponentKind::Parameters,
            "request_bodies" => spargen::ComponentKind::RequestBodies,
            "headers" => spargen::ComponentKind::Headers,
            "security_schemes" => spargen::ComponentKind::SecuritySchemes,
            _ => return Err(syn::Error::new(kind.span(), "unsupported component kind")),
        };
        let names;
        braced!(names in body);
        while !names.is_empty() {
            let name: LitStr = names.parse()?;
            names.parse::<Token![;]>()?;
            omit.rules.push(spargen::OmitRule::Component {
                kind,
                name: leak(name.value()),
            });
        }
    }
    Ok(())
}

fn parse_pointers(
    input: ParseStream,
    omit: &mut spargen::Omit,
    file: Option<&'static str>,
) -> syn::Result<()> {
    let body;
    braced!(body in input);
    while !body.is_empty() {
        let pointer: LitStr = body.parse()?;
        body.parse::<Token![;]>()?;
        omit.rules.push(spargen::OmitRule::Pointer {
            file,
            pointer: leak(pointer.value()),
        });
    }
    Ok(())
}

fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

/// Resolve a spec path relative to the **consumer crate's** manifest directory (as `build.rs` and
/// build scripts do from that crate root), so `generate_api!("openapi.yaml")` finds a spec beside the
/// caller's `Cargo.toml`. Absolute paths pass through unchanged.
fn resolve_spec_path(raw: &str) -> String {
    let path = std::path::Path::new(raw);
    if path.is_absolute() {
        return raw.to_owned();
    }
    match std::env::var("CARGO_MANIFEST_DIR") {
        Ok(dir) => std::path::Path::new(&dir)
            .join(path)
            .to_string_lossy()
            .into_owned(),
        Err(_) => raw.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::Args;
    use spargen::{ComponentKind, OmitMethod, OmitRule};

    #[test]
    fn parses_every_build_configuration_control() {
        let args: Args = syn::parse_str(
            r#"spec = "openapi.yaml", no_uuid, no_time, carve,
               error_body_cap = 4096, batch_cap = 7,
               omit {
                   operations { post "/legacy"; }
                   paths { "/internal/**"; }
                   components { schemas { "Legacy"; } }
                   pointers { "/webhooks"; }
                   file("shared.yaml") { pointers { "/Legacy"; } }
               }"#,
        )
        .unwrap();

        assert!(args.no_uuid);
        assert!(args.no_time);
        assert!(args.carve);
        assert_eq!(args.error_body_cap, Some(4096));
        assert_eq!(args.batch_cap, Some(7));
        assert_eq!(args.omit.rules.len(), 5);
        assert_eq!(
            args.omit.rules[0],
            OmitRule::Operation {
                method: OmitMethod::Post,
                path: "/legacy"
            }
        );
        assert_eq!(
            args.omit.rules[2],
            OmitRule::Component {
                kind: ComponentKind::Schemas,
                name: "Legacy"
            }
        );
        assert_eq!(
            args.omit.rules[4],
            OmitRule::Pointer {
                file: Some("shared.yaml"),
                pointer: "/Legacy"
            }
        );
    }
}
