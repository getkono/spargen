//! Carried docs and lint hygiene (#1269): a module whose spec prose carries a server framework's
//! intra-doc links, bare URLs and stray tags, `include!`d by a crate that denies every lint the
//! generated items allow, with no lint attribute at the include site. A description with no
//! operation is held to the same consumer.

use super::*;

/// Every place spec prose reaches rustdoc: `info`, a server and its variable, a security scheme,
/// a schema (beside a fenced block and a definition to a path), an operation's summary and
/// description, and a response header; and the code blocks rustdoc would run as doctests (#1300):
/// untagged fences, indented code blocks in and out of a list, a fence nesting a shorter one, and
/// a fence the description leaves open; and the input rustc, rustdoc or clippy rejects in a doc
/// comment (#1301): a lone CR, bidirectional controls, a non-void self-closing tag, an
/// over-indented list continuation (one starting with a list marker or `>` among them), and a
/// fence opened on a list item's line.
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

/// A crate root that `include!`s the generated module into a public module (#1302), so rustdoc
/// documents every item the module makes public, the runtime's re-exports among them, while the
/// embedded `support` module stays private.
const PUBLIC_ROOT: &str = r#"//! A consumer of the generated client that publishes the whole module.
#![deny(warnings, missing_docs)]

/// The API.
pub mod rest {
    include!("rest_client.rs");
}
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
    fixture_with_root(temp, contents, name, STRICT_ROOT, configure)
}

/// Generate `contents` under `configure` into a fixture crate named `name` whose root is `root`,
/// which includes the generated module as `rest_client.rs`, and return its directory.
fn fixture_with_root(
    temp: &std::path::Path,
    contents: &str,
    name: &str,
    root: &str,
    configure: impl FnOnce(Spec) -> Spec,
) -> std::path::PathBuf {
    let spec = write_spec(temp, "moderation.yaml", contents);
    let out = temp.join(name);
    let report = generate_configured_fixture_crate(&spec, &out, name, "2021", configure);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    std::fs::rename(out.join("src/lib.rs"), out.join("src/rest_client.rs")).unwrap();
    std::fs::write(out.join("src/lib.rs"), root).unwrap();
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
        "```text\nfn main() {}\n```",
        "```text\ncurl https://example.com/b\n```",
        "  ```text\n  curl https://example.com/c\n  ```",
        "````text\n```\nVec<T>\n```\n````",
        "```text\nan unclosed block\n```\n\nResponse `204`: exported",
        "First line.\nSecond line, after a lone CR.\nThird line.",
        "The author's name, as written: \\u{202E}reversed\\u{202C} and \\u{2066}isolated\\u{2069}.",
        "Its \\u{202E}name\\u{202C} is regional.",
        "One of \\u{2067}eu\\u{2069}.",
        "It expires hourly.",
        "Each is a \\<T/> page, beside a void <br/>.",
        "- a step\n  over indented\n  \\- not a nested item\n  \\> not a quote\n  1\\. not an ordered item",
        "- ```yaml\n  a:\n      b: 1\n      # c\n  ```",
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
    assert!(
        !generated.contains('\r'),
        "a lone CR in carried prose ends its line"
    );
    assert!(
        !generated.contains(['\u{202A}', '\u{202E}', '\u{2066}', '\u{2067}', '\u{2069}']),
        "every bidirectional control in carried prose is escaped"
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
    let output = fixture_cargo(&out)
        .args(["test", "--doc", "--all-features"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "no carried code block may become a doctest of the strict consumer:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The carried-docs module, and one embedding every conditional runtime module (XML, event
/// streams, date-time), each `include!`d into a public module, document with every rustdoc warning
/// denied: no runtime doc a public item carries links an item of the private `support` module
/// (#1302).
#[test]
fn a_public_include_site_documents_with_warnings_denied() {
    let temp = tempfile::tempdir().unwrap();
    for (name, contents) in [
        ("public_docs_client", CARRIED_DOCS_SPEC),
        ("public_capabilities_client", EVERY_CAPABILITY_SPEC),
    ] {
        let dir = temp.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let out = fixture_with_root(&dir, contents, name, PUBLIC_ROOT, |spec| spec);

        let output = doc_denying_warnings(&out);
        assert!(
            output.status.success(),
            "`{name}` must document from a public include site with every rustdoc warning \
             denied:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
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
/// on the links the default rewrites, and its doctests on the code blocks the default tags: the
/// rewrite, not something else, is what keeps rustdoc quiet.
#[test]
fn verbatim_docs_leaves_the_links_for_rustdoc_to_report() {
    let temp = tempfile::tempdir().unwrap();
    let out = strict_fixture(temp.path(), "verbatim_docs_client", |spec| {
        spec.with_verbatim_docs(spargen::CarriedDocs::Verbatim)
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
        "invalid self-closing HTML tag `T`",
    ] {
        assert!(
            stderr.contains(lint),
            "rustdoc must report `{lint}`:\n{stderr}"
        );
    }

    // The untagged fences stay untagged, so rustdoc runs the JSON payload as a doctest and fails:
    // the `text` tag is what keeps the default's doctests passing.
    let output = fixture_cargo(&out)
        .args(["test", "--doc", "--all-features"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains("export_reports"),
        "the failing doctest is the exported operation's:\n{stdout}"
    );
}
