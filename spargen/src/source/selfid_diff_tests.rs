//! `spargen lock` and the build each keep their own choice of parse format for a document they
//! reach. This table drives both copies over one set of URLs and paths and holds them to the same
//! format row by row. Both read a document's `$self` through the one `remote::classify_self`, and
//! test URLs through the one `remote::is_http_url`, which `remote.rs` pins directly.
//!
//! The lock walk is reachable only under `remote-fetch`, which gates this module as it gates
//! `vendor`.

use crate::diag::{Diagnostics, FileId};

use super::bundle::parse_by_name;
use super::lock::vendor_path_for_url;
use super::vendor::{parse_scratch_local, parse_scratch_remote};

/// The format a copy parses a document as, observed rather than read from its code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Json,
    Yaml,
    /// YAML, then JSON.
    Either,
}

/// YAML that is not JSON.
const YAML_ONLY: &str = "a: 1\n";
/// JSON that YAML refuses: a UTF-16 surrogate pair, where YAML's `\u` escape names one code point.
const JSON_ONLY: &str = r#"{"a": "\ud83d\ude00"}"#;

fn observe(accepts: impl Fn(&str) -> bool) -> Format {
    match (accepts(YAML_ONLY), accepts(JSON_ONLY)) {
        (false, true) => Format::Json,
        (true, false) => Format::Yaml,
        (true, true) => Format::Either,
        (false, false) => panic!("neither probe parsed"),
    }
}

/// The lock walk's choice for the document named `name`: a local path (`parse_scratch_local`), or
/// the URL it was fetched from (`parse_scratch_remote`).
fn lock_format(name: &str, remote: bool) -> Format {
    if remote {
        observe(|text| parse_scratch_remote(name, text).is_some())
    } else {
        observe(|text| parse_scratch_local(name, text).is_some())
    }
}

/// The build's choice (`parse_by_name`): a local document by its path, and a remote one by the
/// vendored path `spargen lock` records for its URL.
fn build_format(name: &str, remote: bool) -> Format {
    let name = if remote {
        vendor_path_for_url(name)
    } else {
        name.to_owned()
    };
    observe(|text| parse_by_name(FileId(0), &name, text, &mut Diagnostics::default()).is_ok())
}

#[test]
fn the_lock_and_the_build_parse_every_document_alike() {
    use Format::{Either, Json, Yaml};
    let local: &[(&str, Format)] = &[
        ("specs/api.json", Json),
        ("specs/api.yaml", Yaml),
        ("specs/api.yml", Yaml),
        ("specs/api", Either),
        ("specs/api.txt", Either),
        ("specs/./lib/../api.json", Json),
        ("specs.json/api", Either),
        // Extension case.
        ("specs/api.JSON", Json),
        ("specs/api.Yaml", Yaml),
        ("specs/api.YML", Yaml),
        // A query string, which a relative `$ref` carries into the local path it names (#916).
        ("specs/api.json?v=1", Either),
        ("specs/api.yaml?v=1", Either),
        ("specs/api.json#x", Either),
        // A name that is only a dot and an extension.
        ("specs/.json", Either),
        ("specs/.yaml", Either),
    ];
    let remote: &[(&str, Format)] = &[
        ("https://h.example/api.json", Json),
        ("https://h.example/api.yaml", Yaml),
        ("https://h.example/api.yml", Yaml),
        ("https://h.example/api", Either),
        ("https://h.example/", Either),
        ("https://h.example/v1/../api.json", Json),
        // Scheme and host case.
        ("HTTPS://H.EXAMPLE/api.json", Json),
        // Query strings and fragments.
        ("https://h.example/api.json?v=1", Json),
        ("https://h.example/api.yaml?format=json", Yaml),
        ("https://h.example/api?format=json", Either),
        ("https://h.example/api.json#/components", Json),
        // Characters the vendored path folds to `_`.
        ("https://h.example/api.json;v=1", Either),
        ("https://h.example/api%2Ejson", Either),
        // Extension case.
        ("https://h.example/api.JSON", Json),
        ("https://h.example/api.Yaml", Yaml),
        // A trailing slash.
        ("https://h.example/api.json/", Json),
        ("https://h.example/api.yaml/", Yaml),
        ("https://h.example/api.json//", Json),
        // A name that is only a dot and an extension.
        ("https://h.example/.json", Either),
    ];
    let rows = local.iter().map(|row| (row, false));
    let rows = rows.chain(remote.iter().map(|row| (row, true)));
    let mut failures = Vec::new();
    for ((name, want), remote) in rows {
        let got = (lock_format(name, remote), build_format(name, remote));
        if got != (*want, *want) {
            failures.push(format!(
                "`{name}`: (lock, build) = {got:?}, want agreement on {want:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
