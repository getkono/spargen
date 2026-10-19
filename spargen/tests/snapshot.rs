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
use spargen::{Outcome, Report, Spec};

#[path = "shared/generate.rs"]
mod generate;

use generate::generate_to_string;

/// Absolute path to a corpus spec (relative to the workspace root, one level up from this crate).
fn corpus_path(rel: &str) -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("corpus")
        .join(rel)
}

/// A generation config for a corpus spec, with the diagnostic batch uncapped so the WHOLE
/// diagnostic set is captured (the default cap of 100 truncates the big specs, hiding both the
/// terminal rejection code and the true warning counts). [`generate_to_string`] runs it outside
/// any Cargo build: the snapshots record what the spec produced, not what the process happened to
/// be.
fn config_for(spec: Utf8PathBuf, carve: bool) -> Spec {
    Spec::new(spec).batch_cap(usize::MAX).carve(carve)
}

/// A deterministic, reviewable rendering of a run: the outcome plus a sorted `code: count`
/// histogram over its diagnostics. Codes are `E###`/`W###`, so severity is visible in the prefix.
fn summary(report: &Report) -> String {
    let mut histogram: BTreeMap<&'static str, usize> = BTreeMap::new();
    for diagnostic in report.diagnostics() {
        *histogram.entry(diagnostic.code.as_str()).or_insert(0) += 1;
    }
    let mut out = format!("outcome: {:?}\n", report.outcome());
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
/// of operation method names and the sorted set of public type names.
///
/// The operations are the `pub async fn`s of the crate-root impls, which are `Client`'s operation
/// methods: its constructors and accessors are non-async `pub fn`s. The `BlockingClient` mirror is
/// not read — its methods are synchronous, and its impl sits inside the `__spargen_blocking`
/// wrapper module rather than at the root — so each operation counts once, through `Client`.
///
/// The types are every public type definition [`collect_module_types`] finds from the root: root
/// definitions by name, and those inside a module qualified by its path (`types::Name`). The
/// embedded `support` runtime module is skipped, and `use` re-exports are not type definitions —
/// so only genuinely generated surface is captured. Derived from the emitted source (the `surface`
/// subsystem is crate-private), it is robust to formatting and deterministic given deterministic
/// codegen.
fn api_surface(source: &str) -> String {
    let file = syn::parse_file(source).expect("generated module is valid Rust");
    let mut operations: BTreeSet<String> = BTreeSet::new();
    let mut types: BTreeSet<String> = BTreeSet::new();

    for item in &file.items {
        if let syn::Item::Impl(imp) = item {
            for impl_item in &imp.items {
                if let syn::ImplItem::Fn(func) = impl_item {
                    if is_pub(&func.vis) && func.sig.asyncness.is_some() {
                        operations.insert(func.sig.ident.to_string());
                    }
                }
            }
        }
    }
    collect_module_types(&file.items, "", &mut types);

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

/// The name a public type definition declares, or `None` for any other item.
fn public_type(item: &syn::Item) -> Option<&syn::Ident> {
    let (vis, ident) = match item {
        syn::Item::Struct(item) => (&item.vis, &item.ident),
        syn::Item::Enum(item) => (&item.vis, &item.ident),
        syn::Item::Type(item) => (&item.vis, &item.ident),
        syn::Item::Trait(item) => (&item.vis, &item.ident),
        syn::Item::Union(item) => (&item.vis, &item.ident),
        _ => return None,
    };
    is_pub(vis).then_some(ident)
}

/// Recursively collect public type definitions from a module's items, qualifying each name with the
/// module path (e.g. `types::Foo`). The embedded `support` runtime is not consumer API surface and
/// is skipped. The private lint-scope wrapper `__spargen_blocking` is re-exported at the generated
/// root, so its definitions keep the enclosing prefix rather than exposing an implementation-detail
/// module path in the snapshot.
fn collect_module_types(items: &[syn::Item], prefix: &str, types: &mut BTreeSet<String>) {
    for item in items {
        if let Some(ident) = public_type(item) {
            types.insert(if prefix.is_empty() {
                ident.to_string()
            } else {
                format!("{prefix}::{ident}")
            });
        } else if let syn::Item::Mod(module) = item {
            let Some((_, nested)) = module
                .content
                .as_ref()
                .filter(|_| module.ident != "support")
            else {
                continue;
            };
            let nested_prefix = if module.ident == "__spargen_blocking" {
                prefix.to_owned()
            } else if prefix.is_empty() {
                module.ident.to_string()
            } else {
                format!("{prefix}::{}", module.ident)
            };
            collect_module_types(nested, &nested_prefix, types);
        }
    }
}

fn is_pub(vis: &syn::Visibility) -> bool {
    matches!(vis, syn::Visibility::Public(_))
}

/// Drive `generate` on a corpus spec (module output, uncapped diagnostics) and return the report
/// plus the generated module source when generation succeeded.
fn generate_corpus(rel: &str, carve: bool) -> (Report, Option<String>) {
    generate_to_string(config_for(corpus_path(rel), carve))
}

// --- Rejecting corpus cases: outcome + full diagnostic histogram --------------------------------

#[test]
fn github_api_3_0_rejects() {
    // Manifest expectation: `reject:E001` — a 3.0 document is refused before lowering.
    let (report, _) = generate_corpus("github-api-3-0/api.github.com.json", false);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    insta::assert_snapshot!("github_api_3_0", summary(&report));
}

#[test]
fn github_api_3_1_generates() {
    // Manifest expectation: `generate`. Keep the large surface out of the snapshot; the dedicated
    // corpus gate compile-checks every emitted item, while this pins the complete diagnostic set.
    let (report, source) = generate_corpus("github-api-3-1/api.github.com.json", false);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(source.is_some(), "GitHub generation must emit a module");
    insta::assert_snapshot!("github_api_3_1", summary(&report));
}

#[test]
fn stripe_rejects() {
    // Manifest expectation: `reject:E001` — Stripe emits 3.0.0, refused at the version gate.
    let (report, _) = generate_corpus("stripe/spec3.json", false);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    insta::assert_snapshot!("stripe", summary(&report));
}

#[test]
fn twilio_api_2010_rejects() {
    // Manifest expectation: `reject:E001` — Twilio emits 3.0.1.
    let (report, _) = generate_corpus("twilio-api-2010/twilio_api_v2010.json", false);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    insta::assert_snapshot!("twilio_api_2010", summary(&report));
}

#[test]
fn kubernetes_authentication_v1_rejects() {
    // Manifest expectation: `reject:E001` — the Kubernetes OpenAPI v3 tree emits 3.0.0.
    let (report, _) = generate_corpus(
        "kubernetes-authentication-v1/apis__authentication.k8s.io__v1_openapi.json",
        false,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    insta::assert_snapshot!("kubernetes_authentication_v1", summary(&report));
}

#[test]
fn meilisearch_rejects() {
    // Manifest expectation: `reject:E011` — 3.1.0, so it clears the version gate and is refused by
    // strict official-schema validation instead (`description: null` on a Tag's externalDocs).
    let (report, _) = generate_corpus("meilisearch/open-api.json", false);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    insta::assert_snapshot!("meilisearch", summary(&report));
}

#[test]
fn openai_openapi_rejects() {
    // Manifest expectation: `reject`.
    let (report, _) = generate_corpus("openai-openapi/openapi.yaml", false);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    insta::assert_snapshot!("openai_openapi", summary(&report));
}

// --- Generating corpus cases: outcome + histogram AND the generated API surface -----------------

#[test]
fn ollama_generates() {
    // Manifest expectation: `generate` (its disjoint oneOf unions lower via custom deserialize;
    // one W001 for a validation-only keyword).
    let (report, source) = generate_corpus("ollama/openapi.yaml", false);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    insta::assert_snapshot!("ollama", summary(&report));
    let source = source.expect("ollama generates a module");
    insta::assert_snapshot!("ollama_surface", api_surface(&source));
}

#[test]
fn openapi_boilerplate_generates() {
    // Manifest expectation: `generate` (a template document — components/types only, no operations).
    let (report, source) = generate_corpus("openapi-boilerplate/src/openapi.yaml", false);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    insta::assert_snapshot!("openapi_boilerplate", summary(&report));
    let source = source.expect("openapi-boilerplate generates a module");
    insta::assert_snapshot!("openapi_boilerplate_surface", api_surface(&source));
}

#[test]
fn mastodon_openapi_generates() {
    // Manifest expectation: `generate`. The corpus's only real-world recursive-nullable `$ref`
    // (`oneOf: [{$ref: Self}, {type: 'null'}]`) and its only `$ref`s with a shape-bearing sibling.
    // Like GitHub 3.1 it is too large for a surface snapshot, so the histogram pins the diagnostic
    // set, and the idiom's typed answer is asserted directly: "did not reject" would also pass on a
    // field silently degraded to `serde_json::Value` or stripped of its `Option`.
    let (report, source) = generate_corpus("mastodon-openapi/schema.json", false);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    insta::assert_snapshot!("mastodon_openapi", summary(&report));
    let source = source.expect("mastodon-openapi generates a module");
    for field in [
        "pub moved: Option<Box<Account>>",
        "pub reblog: Option<Box<Status>>",
        "pub quoted_status: Option<Box<Status>>",
    ] {
        assert!(
            source.contains(field),
            "the recursive-nullable `$ref` no longer lowers to `{field}`"
        );
    }
}
