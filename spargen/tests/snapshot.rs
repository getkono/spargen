//! Content-level regression guard over the pinned corpus specs (`corpus/manifest.toml`).
//!
//! For every corpus case this snapshots a STABLE, REVIEWABLE summary of what spargen produces:
//! the pipeline OUTCOME plus a diagnostic-code HISTOGRAM (`code: count`, sorted), pinning exactly
//! which diagnostics each real-world spec fires. For the smaller GENERATING cases it additionally
//! snapshots the generated consumer API SURFACE — the sorted operation method names and the sorted
//! public type names — derived by parsing the emitted module with `syn`. The full multi-hundred-KB
//! generated source (dominated by the embedded runtime) is deliberately NOT snapshotted: the
//! histogram + API surface is the reviewable guard.
//!
//! This complements `determinism.rs` (self-referential byte-stability): a change to *what* a corpus
//! spec produces surfaces here as a reviewable `.snap` diff. Everything is deterministic — the
//! `batch_cap` is raised so ALL diagnostics are captured (the default cap of 100 truncates the big
//! specs), sets are sorted, and nothing machine-specific (tempdir paths, timestamps) enters a
//! snapshot: only diagnostic codes/counts and generated identifiers, never absolute paths.

use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;
use spargen::{Config, Outcome, OutputTarget, Report};

/// Absolute path to a corpus spec (relative to the workspace root, one level up from this crate).
fn corpus_path(rel: &str) -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("corpus")
        .join(rel)
}

/// A generation config for a corpus spec writing to a throwaway module, with the diagnostic batch
/// uncapped so the WHOLE diagnostic set is captured (the default cap of 100 truncates the big
/// specs, hiding both the terminal rejection code and the true warning counts).
fn config_for(spec: Utf8PathBuf, out: Utf8PathBuf, carve: bool) -> Config {
    let mut config = Config::new(spec, OutputTarget::Module(out));
    config.batch_cap = usize::MAX;
    config.carve = carve;
    config
}

/// A deterministic, reviewable rendering of a run: the outcome plus a sorted `code: count`
/// histogram over its diagnostics. Codes are `E###`/`W###`, so severity is visible in the prefix.
fn summary(report: &Report) -> String {
    let mut histogram: BTreeMap<&'static str, usize> = BTreeMap::new();
    for diagnostic in &report.diagnostics {
        *histogram.entry(diagnostic.code.as_str()).or_insert(0) += 1;
    }
    let mut out = format!("outcome: {:?}\n", report.outcome);
    out.push_str("diagnostics (code: count):\n");
    if histogram.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for (code, count) in histogram {
            out.push_str(&format!("  {code}: {count}\n"));
        }
    }
    out
}

/// Parse a generated module with `syn` and render its consumer-visible API surface: the sorted set
/// of operation method names (the `pub async fn`s on the crate-root `Client`/`BlockingClient`
/// impls) and the sorted set of public type names (model types in the `types` module, qualified
/// `types::Name`, plus crate-root public type definitions). The embedded `support` runtime module
/// is skipped, and `use` re-exports are not type definitions — so only genuinely generated surface
/// is captured. Derived from the emitted source (the `surface` subsystem is crate-private), it is
/// robust to formatting and deterministic given deterministic codegen.
fn api_surface(source: &str) -> String {
    let file = syn::parse_file(source).expect("generated module is valid Rust");
    let mut operations: BTreeSet<String> = BTreeSet::new();
    let mut types: BTreeSet<String> = BTreeSet::new();

    for item in &file.items {
        match item {
            // The embedded freestanding runtime is not consumer API surface — skip it wholesale.
            // The model types live in the `types` module; collect them qualified.
            syn::Item::Mod(module) => {
                if module.ident == "support" {
                    continue;
                }
                if let Some((_, items)) = &module.content {
                    // The private lint-scope wrapper re-exports BlockingClient at the generated
                    // root, so treat its public definitions as root surface rather than exposing
                    // an implementation-detail module path in the snapshot.
                    let module_name = module.ident.to_string();
                    let prefix = if module.ident == "__spargen_blocking" {
                        ""
                    } else {
                        module_name.as_str()
                    };
                    collect_module_types(items, prefix, &mut types);
                }
            }
            // Crate-root impls are the `Client`/`BlockingClient` inherent impls: their `pub async
            // fn`s are the operation methods (constructors/accessors are non-async `pub fn`). The
            // blocking mirror shares method names, so the set dedupes to one entry per operation.
            syn::Item::Impl(imp) => {
                for impl_item in &imp.items {
                    if let syn::ImplItem::Fn(func) = impl_item {
                        if is_pub(&func.vis) && func.sig.asyncness.is_some() {
                            operations.insert(func.sig.ident.to_string());
                        }
                    }
                }
            }
            syn::Item::Struct(item) if is_pub(&item.vis) => {
                types.insert(item.ident.to_string());
            }
            syn::Item::Enum(item) if is_pub(&item.vis) => {
                types.insert(item.ident.to_string());
            }
            syn::Item::Type(item) if is_pub(&item.vis) => {
                types.insert(item.ident.to_string());
            }
            syn::Item::Trait(item) if is_pub(&item.vis) => {
                types.insert(item.ident.to_string());
            }
            syn::Item::Union(item) if is_pub(&item.vis) => {
                types.insert(item.ident.to_string());
            }
            _ => {}
        }
    }

    let mut out = format!("operations ({}):\n", operations.len());
    for name in &operations {
        out.push_str(&format!("  {name}\n"));
    }
    out.push_str(&format!("types ({}):\n", types.len()));
    for name in &types {
        out.push_str(&format!("  {name}\n"));
    }
    out
}

/// Recursively collect public type definitions from a module's items, qualifying each name with the
/// module path (e.g. `types::Foo`).
fn collect_module_types(items: &[syn::Item], prefix: &str, types: &mut BTreeSet<String>) {
    let qualified = |ident: &syn::Ident| {
        if prefix.is_empty() {
            ident.to_string()
        } else {
            format!("{prefix}::{ident}")
        }
    };
    for item in items {
        match item {
            syn::Item::Struct(item) if is_pub(&item.vis) => {
                types.insert(qualified(&item.ident));
            }
            syn::Item::Enum(item) if is_pub(&item.vis) => {
                types.insert(qualified(&item.ident));
            }
            syn::Item::Type(item) if is_pub(&item.vis) => {
                types.insert(qualified(&item.ident));
            }
            syn::Item::Trait(item) if is_pub(&item.vis) => {
                types.insert(qualified(&item.ident));
            }
            syn::Item::Union(item) if is_pub(&item.vis) => {
                types.insert(qualified(&item.ident));
            }
            syn::Item::Mod(module) if module.ident != "support" => {
                if let Some((_, nested)) = &module.content {
                    let nested_prefix = if prefix.is_empty() {
                        module.ident.to_string()
                    } else {
                        format!("{prefix}::{}", module.ident)
                    };
                    collect_module_types(nested, &nested_prefix, types);
                }
            }
            _ => {}
        }
    }
}

fn is_pub(vis: &syn::Visibility) -> bool {
    matches!(vis, syn::Visibility::Public(_))
}

/// Drive `generate` on a corpus spec (module output, uncapped diagnostics) and return the report
/// plus the generated module source when generation succeeded. The tempdir is kept alive by the
/// caller for the duration of the read.
fn generate_corpus(rel: &str, carve: bool) -> (Report, Option<String>) {
    let temp = tempfile::tempdir().unwrap();
    let out = Utf8PathBuf::from_path_buf(temp.path().join("client.rs")).unwrap();
    let report = spargen::generate(&config_for(corpus_path(rel), out.clone(), carve));
    let source = (report.outcome == Outcome::Generated)
        .then(|| std::fs::read_to_string(&out).expect("generated module written"));
    (report, source)
}

// --- Rejecting corpus cases: outcome + full diagnostic histogram --------------------------------

#[test]
fn github_api_3_0_rejects() {
    // Manifest expectation: `reject:E001` — a 3.0 document is refused before lowering.
    let (report, _) = generate_corpus("github-api-3-0/api.github.com.json", false);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    insta::assert_snapshot!("github_api_3_0", summary(&report));
}

#[test]
fn github_api_3_1_generates() {
    // Manifest expectation: `generate`. Keep the large surface out of the snapshot; the dedicated
    // corpus gate compile-checks every emitted item, while this pins the complete diagnostic set.
    let (report, source) = generate_corpus("github-api-3-1/api.github.com.json", false);
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    assert!(source.is_some(), "GitHub generation must emit a module");
    insta::assert_snapshot!("github_api_3_1", summary(&report));
}

#[test]
fn openai_openapi_rejects() {
    // Manifest expectation: `reject`.
    let (report, _) = generate_corpus("openai-openapi/openapi.yaml", false);
    assert_eq!(report.outcome, Outcome::Rejected, "{report:#?}");
    insta::assert_snapshot!("openai_openapi", summary(&report));
}

// --- Generating corpus cases: outcome + histogram AND the generated API surface -----------------

#[test]
fn ollama_generates() {
    // Manifest expectation: `generate` (its disjoint oneOf unions lower via custom deserialize;
    // one W001 for a validation-only keyword).
    let (report, source) = generate_corpus("ollama/openapi.yaml", false);
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    insta::assert_snapshot!("ollama", summary(&report));
    let source = source.expect("ollama generates a module");
    insta::assert_snapshot!("ollama_surface", api_surface(&source));
}

#[test]
fn openapi_boilerplate_generates() {
    // Manifest expectation: `generate` (a template document — components/types only, no operations).
    let (report, source) = generate_corpus("openapi-boilerplate/src/openapi.yaml", false);
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
    insta::assert_snapshot!("openapi_boilerplate", summary(&report));
    let source = source.expect("openapi-boilerplate generates a module");
    insta::assert_snapshot!("openapi_boilerplate_surface", api_surface(&source));
}
