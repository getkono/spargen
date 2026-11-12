//! Carried docs and lint hygiene (#1269): a module whose spec prose carries a server framework's
//! intra-doc links, bare URLs and stray tags, `include!`d by a crate that denies every lint the
//! generated items allow, with no lint attribute at the include site. A description with no
//! operation is held to the same consumer.

use super::*;

/// Every place spec prose reaches rustdoc: `info`, a server and its variable, a security scheme,
/// a schema (beside a fenced block and a definition to a path), an operation's summary and
/// description, and a response header.
const CARRIED_DOCS_SPEC: &str = include_str!("specs/carried_docs.yaml");

/// A crate root that denies the lints the generated module answers for, and `include!`s it into a
/// private module with nothing but the macro, re-exporting one item so the rest is unreachable.
const STRICT_ROOT: &str = r#"//! A consumer of the generated client that allows nothing at the include site.
#![deny(
    warnings,
    missing_docs,
    unreachable_pub,
    dead_code,
    clippy::pedantic,
    clippy::nursery
)]

mod rest {
    include!("rest_client.rs");
}

pub use rest::Client;
"#;

/// A description with components and a server but no operation: the `ops` module holds the
/// `Operation` type and no constant, so nothing in the generated module builds an `Operation`.
const NO_OPERATIONS_SPEC: &str = r"openapi: 3.1.0
info:
  title: Schemas only
  version: 1.0.0
servers:
  - url: https://example.com
paths: {}
components:
  schemas:
    Note:
      type: object
      required: [text]
      properties:
        text:
          type: string
";

/// Generate the carried-docs spec under `configure` into a fixture crate named `name` whose root is
/// [`STRICT_ROOT`], and return its directory.
fn strict_fixture(
    temp: &std::path::Path,
    name: &str,
    configure: impl FnOnce(Spec) -> Spec,
) -> std::path::PathBuf {
    strict_fixture_of(temp, CARRIED_DOCS_SPEC, name, configure)
}

/// [`strict_fixture`] over the description `contents`.
fn strict_fixture_of(
    temp: &std::path::Path,
    contents: &str,
    name: &str,
    configure: impl FnOnce(Spec) -> Spec,
) -> std::path::PathBuf {
    let spec = write_spec(temp, "moderation.yaml", contents);
    let out = temp.join(name);
    let report = generate_configured_fixture_crate(&spec, &out, name, "2021", configure);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    std::fs::rename(out.join("src/lib.rs"), out.join("src/rest_client.rs")).unwrap();
    std::fs::write(out.join("src/lib.rs"), STRICT_ROOT).unwrap();
    out
}

/// `cargo doc` of the fixture at `out`, every rustdoc warning denied.
fn doc_denying_warnings(out: &std::path::Path) -> std::process::Output {
    fixture_cargo(out)
        .args(["doc", "--no-deps", "--all-features"])
        .env("RUSTDOCFLAGS", "-D warnings")
        .output()
        .unwrap()
}

#[test]
fn the_include_site_needs_no_lint_attribute() {
    let temp = tempfile::tempdir().unwrap();
    let out = strict_fixture(temp.path(), "carried_docs_client", |spec| spec);

    let generated = std::fs::read_to_string(out.join("src/rest_client.rs")).unwrap();
    for rewritten in [
        "served by `crate::serve`. See <https://example.com/docs>.",
        "routed by Router.",
        "as `Region` lists them.",
        "A token `crate::auth::mint` issues; see <https://example.com/auth>.",
        "Client for Moderation, by `crate::App` v1.0.0.",
        "A report the ModerationStore holds, as a Vec\\<Entry>, shaped as \
         [Vec\\<Note>][Vec<Note>] and\n[Page\\<Report>](https://example.com/page).",
        "[kept] Vec<kept>",
        "List reports, as `ModerationStore::list` does.",
        "Reads the store and returns <b>every</b> report",
        "limit \\</b> set in the config. Details at\n<https://example.com/reports>.",
        "The limit `crate::limits::Rate` applies.",
    ] {
        assert!(
            generated.contains(rewritten),
            "the generated module lacks `{rewritten}`"
        );
    }
    assert!(
        !generated.contains("crate::store::ModerationStore"),
        "a reference definition to a path is dropped"
    );

    let status = fixture_cargo(&out)
        .args(["clippy", "--all-features", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the strict consumer must pass clippy with no allow at the include site"
    );
    let output = doc_denying_warnings(&out);
    assert!(
        output.status.success(),
        "the strict consumer must document with every rustdoc warning denied:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A description with no operation, `include!`d into the same private module: the `ops` module's
/// `Operation` type, which no constant constructs, must not fail the strict consumer's clippy as
/// never constructed.
#[test]
fn a_description_without_operations_passes_the_strict_consumer() {
    let temp = tempfile::tempdir().unwrap();
    let out = strict_fixture_of(
        temp.path(),
        NO_OPERATIONS_SPEC,
        "no_operations_client",
        |spec| spec,
    );

    let generated = std::fs::read_to_string(out.join("src/rest_client.rs")).unwrap();
    assert!(
        generated.contains("pub struct Operation") && !generated.contains(": Operation = "),
        "the module declares `Operation` and no constant of it"
    );

    let output = fixture_cargo(&out)
        .args(["clippy", "--all-features", "--", "-D", "warnings"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "a description with no operation must pass the strict consumer's clippy:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `verbatim_docs(true)` carries the prose as written, so the same consumer's documentation fails
/// on the links the default rewrites: the rewrite, not something else, is what keeps rustdoc quiet.
#[test]
fn verbatim_docs_leaves_the_links_for_rustdoc_to_report() {
    let temp = tempfile::tempdir().unwrap();
    let out = strict_fixture(temp.path(), "verbatim_docs_client", |spec| {
        spec.verbatim_docs(true)
    });

    let generated = std::fs::read_to_string(out.join("src/rest_client.rs")).unwrap();
    assert!(
        generated.contains("served by [`crate::serve`]. See https://example.com/docs."),
        "verbatim prose is carried as written"
    );

    let output = doc_denying_warnings(&out);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    for lint in [
        "unresolved link to `crate::serve`",
        "this URL is not a hyperlink",
        "unopened HTML tag `b`",
    ] {
        assert!(
            stderr.contains(lint),
            "rustdoc must report `{lint}`:\n{stderr}"
        );
    }
}
