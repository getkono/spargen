//! Remote (`http`/`https`) `$ref` primitives: URL math, `$ref` classification, and the ref
//! rewriting that keeps hermetic resolution correct. The networked vendor step lives in
//! [`super::vendor`] (feature `remote-fetch`); these helpers are shared by both the hermetic bundle
//! loader and the vendor walk.
//!
//! ## Hermetic by construction
//!
//! `generate` and `check` never reach the network. A remote `$ref` is resolved only from a locally
//! vendored copy that is hash-pinned in [`spargen.lock`](super::lock). The single place bytes are
//! fetched is [`super::vendor`] — driven exclusively by `spargen lock`. This is also the anti-SSRF
//! boundary: no spec content can trigger a network request during a build.

use super::{Node, SpannedValue};

/// Classification of a `$ref` string relative to the document it appears in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefTarget {
    /// A same-document fragment (`#/…`) or an empty ref — resolved in place, no file/URL to load.
    InDocument,
    /// A relative-file ref (only from a local document); the payload is the path portion.
    LocalRelative(String),
    /// A remote document; the payload is the absolute base URL with any fragment stripped.
    Remote(String),
    /// An absolute non-`http(s)` URI (e.g. `urn:`) that cannot be fetched or vendored.
    UnsupportedRemote(String),
}

/// Classify `reference` given the base of the document it appears in. `remote_base` is `Some(url)`
/// when the current document was itself fetched from a URL (so its relative refs resolve against
/// that URL), and `None` for a local file.
pub(crate) fn classify_ref(reference: &str, remote_base: Option<&str>) -> RefTarget {
    let (path, _fragment) = split_fragment(reference);
    if path.is_empty() {
        return RefTarget::InDocument;
    }
    if is_http_url(path) {
        return RefTarget::Remote(path.to_owned());
    }
    if has_uri_scheme(path) {
        return RefTarget::UnsupportedRemote(path.to_owned());
    }
    match remote_base {
        Some(base) => RefTarget::Remote(join_url_path(base, path)),
        None => RefTarget::LocalRelative(path.to_owned()),
    }
}

/// Whether a `$ref` names a remote or absolute-URI target (i.e. not a relative-file or fragment
/// ref). Used to decide when local-file resolution does not apply.
pub(crate) fn is_absolute_ref(reference: &str) -> bool {
    let (path, _fragment) = split_fragment(reference);
    is_http_url(path) || has_uri_scheme(path)
}

/// Whether `reference` is an `http`/`https` URL.
pub(crate) fn is_http_url(reference: &str) -> bool {
    reference.starts_with("http://") || reference.starts_with("https://")
}

fn has_uri_scheme(reference: &str) -> bool {
    reference.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphabetic())
    })
}

/// Split a `$ref` into `(path, fragment)` on the first `#`; the fragment excludes the `#`.
pub(crate) fn split_fragment(reference: &str) -> (&str, &str) {
    match reference.split_once('#') {
        Some((path, fragment)) => (path, fragment),
        None => (reference, ""),
    }
}

/// Resolve `reference` (relative or absolute, possibly with a fragment) against `base_url` into an
/// absolute URL, preserving the fragment. A fragment-only ref resolves to `base_url` itself.
pub(crate) fn resolve_ref_url(base_url: &str, reference: &str) -> String {
    let (path, fragment) = split_fragment(reference);
    let absolute = if path.is_empty() {
        base_url.to_owned()
    } else if is_http_url(path) {
        path.to_owned()
    } else {
        join_url_path(base_url, path)
    };
    if fragment.is_empty() {
        absolute
    } else {
        format!("{absolute}#{fragment}")
    }
}

fn join_url_path(base: &str, relative: &str) -> String {
    let (scheme_authority, base_path) = split_scheme_authority(base);
    let combined = if relative.starts_with('/') {
        relative.to_owned()
    } else {
        let dir = base_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        format!("{dir}/{relative}")
    };
    format!("{scheme_authority}{}", normalize_path(&combined))
}

fn split_scheme_authority(url: &str) -> (String, String) {
    if let Some(idx) = url.find("://") {
        let after = &url[idx + 3..];
        if let Some(slash) = after.find('/') {
            let authority_end = idx + 3 + slash;
            (
                url[..authority_end].to_owned(),
                url[authority_end..].to_owned(),
            )
        } else {
            (url.to_owned(), "/".to_owned())
        }
    } else {
        (String::new(), url.to_owned())
    }
}

fn normalize_path(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    format!("/{}", out.join("/"))
}

/// Rewrite every `$ref` in `value` (a subtree parsed from a remote document fetched from
/// `base_url`) into an absolute URL. This lets hermetic resolution treat nested refs uniformly: a
/// relative ref inside a vendored doc resolves against that doc's URL, and a same-document `#/…`
/// fragment becomes `base_url#/…`, so it is resolved within the same vendored doc rather than being
/// mistaken for a component of the root spec.
pub(crate) fn rewrite_refs_to_absolute(value: &mut SpannedValue, base_url: &str) {
    match &mut value.node {
        Node::Object(map) => {
            if let Some(reference) = map.get_mut("$ref") {
                if let Node::String(text) = &mut reference.node {
                    *text = resolve_ref_url(base_url, text);
                }
            }
            for value in map.values_mut() {
                rewrite_refs_to_absolute(value, base_url);
            }
        }
        Node::Array(values) => {
            for value in values {
                rewrite_refs_to_absolute(value, base_url);
            }
        }
        _ => {}
    }
}

/// Collect every `$ref` string in a value tree, in document order.
pub(crate) fn collect_refs(value: &SpannedValue) -> Vec<String> {
    let mut refs = Vec::new();
    collect_refs_inner(value, &mut refs);
    refs
}

fn collect_refs_inner(value: &SpannedValue, refs: &mut Vec<String>) {
    match &value.node {
        Node::Object(map) => {
            if let Some(reference) = map.get("$ref").and_then(SpannedValue::as_str) {
                refs.push(reference.to_owned());
            }
            for (_, value) in map.iter() {
                collect_refs_inner(value, refs);
            }
        }
        Node::Array(values) => {
            for value in values {
                collect_refs_inner(value, refs);
            }
        }
        Node::Null | Node::Bool(_) | Node::Number(_) | Node::String(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_refs_by_origin() {
        assert_eq!(
            classify_ref("#/components/schemas/A", None),
            RefTarget::InDocument
        );
        assert_eq!(
            classify_ref("./schemas/Pet.yaml", None),
            RefTarget::LocalRelative("./schemas/Pet.yaml".to_owned())
        );
        assert_eq!(
            classify_ref("https://h/x.yaml#/A", None),
            RefTarget::Remote("https://h/x.yaml".to_owned())
        );
        assert_eq!(
            classify_ref("urn:foo:bar", None),
            RefTarget::UnsupportedRemote("urn:foo:bar".to_owned())
        );
        // A relative ref inside a remote doc resolves against that doc's URL.
        assert_eq!(
            classify_ref("../s/y.yaml#/B", Some("https://h/a/x.yaml")),
            RefTarget::Remote("https://h/s/y.yaml".to_owned())
        );
    }

    #[test]
    fn resolves_ref_urls() {
        assert_eq!(
            resolve_ref_url("https://h/a/x.yaml", "y.yaml#/Foo"),
            "https://h/a/y.yaml#/Foo"
        );
        assert_eq!(
            resolve_ref_url("https://h/a/x.yaml", "#/components/schemas/Bar"),
            "https://h/a/x.yaml#/components/schemas/Bar"
        );
        assert_eq!(
            resolve_ref_url("https://h/a/x.yaml", "/root.yaml"),
            "https://h/root.yaml"
        );
        assert_eq!(
            resolve_ref_url("https://h/a/x.yaml", "https://other/z.yaml#/Z"),
            "https://other/z.yaml#/Z"
        );
    }
}
