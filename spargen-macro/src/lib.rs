//! # spargen-macro
//!
//! The proc-macro front-end for [`spargen`](https://docs.rs/spargen): generate a typed OpenAPI
//! 3.1.x client **inline**, with no `build.rs` and no CLI step.
//!
//! ```ignore
//! mod api {
//!     // Path is resolved relative to the consumer crate's Cargo.toml.
//!     spargen_macro::generate_api!("openapi.yaml");
//! }
//! ```
//!
//! Keyed form, with the same feature toggles as the CLI/`build.rs` API:
//!
//! ```ignore
//! spargen_macro::generate_api!(spec = "openapi.yaml", no_uuid, no_time, carve);
//! ```
//!
//! ## How it works
//!
//! The macro is a thin shim over [`spargen::preview`] — the same in-memory renderer behind
//! `spargen generate --out -`. It resolves the spec, renders the client, and parses the rendered
//! source back into tokens. Because the output is byte-identical to what `spargen generate` /
//! `build.rs` produce, all three paths are deterministic and interchangeable.
//!
//! A generation failure becomes a `compile_error!` carrying spargen's diagnostics — the same
//! loud, no-silent-degradation contract the CLI has. (Warnings are not surfaced: stable proc-macro
//! APIs cannot emit them. Run `spargen check <spec>` to see warnings.)
//!
//! ## Cost & alternatives
//!
//! Inline generation recompiles the whole generator (host-side) as part of your build, and the
//! generated code is not materialized on disk (use `cargo expand`, or `spargen generate --out -`,
//! to inspect it). When you want the generated source checked in or reviewable, prefer the
//! `build.rs` API or the `spargen` CLI. The macro trades that visibility for a zero-config,
//! single-dependency setup.
//!
//! ## Runtime graph
//!
//! This crate and `spargen` are **host/build-time only** — a proc-macro crate is never linked into
//! your binary. Your runtime dependencies are just what the generated code uses (reqwest, serde,
//! …); no spargen crate appears in `cargo tree -e no-proc-macro`.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Ident, LitStr, Token};

/// Generate a typed OpenAPI 3.1.x client in place. See the [crate docs](crate) for forms and
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
}

impl Parse for Args {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut spec: Option<LitStr> = None;
        let mut no_uuid = false;
        let mut no_time = false;
        let mut carve = false;

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
                    other => {
                        return Err(syn::Error::new(
                            key.span(),
                            format!(
                                "unknown argument `{other}`; expected a spec path or one of: \
                                 no_uuid, no_time, carve"
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
        })
    }
}

fn expand(args: &Args) -> syn::Result<proc_macro2::TokenStream> {
    let raw = args.spec.value();
    let spec_path = resolve_spec_path(&raw);

    let mut config = spargen::Config::new(
        spec_path.clone(),
        // Never written — `preview` renders in memory. A module layout yields a single file.
        spargen::OutputTarget::Module("generated.rs".into()),
    );
    config.features.uuid = !args.no_uuid;
    config.features.time = !args.no_time;
    config.carve = args.carve;

    // Keep spargen's codegen (and the tokenization below) off the compiler bridge; restored on drop.
    let _fallback = FallbackGuard::force();
    let preview = spargen::preview(&config);

    let errors: Vec<&spargen::Diagnostic> = preview
        .report
        .diagnostics
        .iter()
        .filter(|d| d.severity == spargen::Severity::Error)
        .collect();

    if preview.report.outcome != spargen::Outcome::Generated || preview.files.is_empty() {
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

    let source = &preview.files[0].contents;
    let generated: proc_macro2::TokenStream = source.parse().map_err(|error| {
        syn::Error::new(
            args.spec.span(),
            format!("spargen produced code that failed to tokenize: {error}"),
        )
    })?;

    // Force a rebuild whenever the spec changes: referencing it via `include_bytes!` makes Cargo
    // track the file (proc-macros cannot emit `rerun-if-changed`). The path exists — generation
    // above already read it.
    let track = &spec_path;
    Ok(quote! {
        #generated
        const _: &[u8] = include_bytes!(#track);
    })
}

/// Resolve a spec path relative to the **consumer crate's** manifest directory (as `build.rs` and
/// the CLI do from that crate root), so `generate_api!("openapi.yaml")` finds a spec beside the
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
