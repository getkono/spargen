//! Remote (`http`/`https`) `$ref` primitives: URL math, `$ref` classification, and the ref
//! rewriting that keeps hermetic resolution correct. The networked vendor step lives in
//! [`vendor`](fn@super::vendor) (feature `remote-fetch`); these helpers are shared by both the hermetic bundle
//! loader and the vendor walk.
//!
//! ## Hermetic by construction
//!
//! `generate` and `check` never reach the network. A remote `$ref` is resolved only from a locally
//! vendored copy that is hash-pinned in [`spargen.lock`](super::lock). The single place bytes are
//! fetched is [`vendor`](fn@super::vendor) — driven exclusively by `spargen lock`. This is also the anti-SSRF
//! boundary: no spec content can trigger a network request during a build.

use crate::diag::JsonPointer;

use super::{canonical_pointer, Node, SpannedValue};

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

/// Collect every reference string in a value tree, in document order, outside specification
/// extensions.
///
/// This is `$ref` plus one other place OpenAPI puts a reference without calling it one: OpenAPI
/// 3.2 allows a Security Requirement Object's *key* to be the URI of a Security Scheme Object, so
/// the file it names has to be loaded like any other reference target.
///
/// `value` is read as an OpenAPI object — a document root, or the target of a `$ref` — and the walk
/// does not descend into a specification extension (a `^x-` key of an object whose keys are fixed
/// fields). The specification admits any value there (`patternProperties: { "^x-": true }`), so a
/// `$ref`-shaped object inside one is author data rather than a Reference Object, and nothing it
/// names is loaded. An extension's contents are interpreted only where a reference addresses them;
/// [`enters_extension`] names those, and the caller walks each one's target as a value of its own.
pub(crate) fn collect_refs(value: &SpannedValue) -> Vec<String> {
    let mut refs = Vec::new();
    collect_refs_inner(value, Keys::Fields, &mut refs);
    collect_security_requirement_refs(value, &mut refs);
    refs
}

/// Whether `reference`'s fragment is a JSON Pointer that passes through a key spelled like a
/// specification extension (`false` for no fragment, a non-pointer fragment, or a path of no `x-`
/// token).
///
/// [`collect_refs`] skips extensions, so a target inside one is the only kind a whole-document
/// walk has not already read: a reference that addresses it makes its contents part of the
/// description (`$ref: '#/x-defs/Pet'` is interpreted as whatever its site expects), and their own
/// references must be loaded like any others. A token such as `components/schemas/x-pet`, where
/// `x-pet` is a component *name*, also qualifies; walking an already-read value again is redundant,
/// never wrong.
pub(crate) fn enters_extension(reference: &str) -> bool {
    let (_, fragment) = split_fragment(reference);
    // Decoded first, so `#/%78-defs` is read as the `x-defs` it addresses. A canonical token is
    // `~`-escaped, which never changes whether it starts with `x-`.
    canonical_pointer(&JsonPointer::from(fragment.to_owned()))
        .is_some_and(|pointer| pointer.as_str().split('/').any(is_extension_key))
}

/// Whether `key` is a specification extension key, spelled exactly as the metaschema's `^x-`
/// pattern: case-sensitively.
fn is_extension_key(key: &str) -> bool {
    key.starts_with("x-")
}

/// What the keys of an object in an OpenAPI description are, which decides whether an `x-` key in
/// it is a specification extension.
#[derive(Clone, Copy)]
enum Keys {
    /// The fixed fields of an OpenAPI or JSON Schema object, beside which `^x-` keys are
    /// specification extensions. A Paths, Responses or Callback Object is one too: its other keys
    /// are paths, status codes or runtime expressions, none of which can start with `x-`.
    Fields,
    /// Names the author chooses (a map), where `x-rate-limit` is a header like any other and every
    /// value is an object with fixed fields.
    Names,
    /// The Components Object: fixed fields, every one of which is a map of names.
    Components,
}

/// The fixed fields, in any OpenAPI 3.1 or 3.2 object or JSON Schema, whose value is a map keyed by
/// author-chosen names: every `additionalProperties` map the vendored metaschemas declare
/// (`webhooks`, `variables`, `callbacks`, `content`, `encoding`, `headers`, `links`, `examples`,
/// `additionalOperations`), their `map-of-strings` fields (`parameters` on a Link, `mapping`,
/// `scopes`), and JSON Schema's own (`properties`, `patternProperties`, `$defs`,
/// `dependentSchemas`, and the pre-2019 `definitions` many descriptions still address). The
/// Components Object's maps are [`Keys::Components`] instead, since `responses` and `parameters`
/// mean something else elsewhere. Where one of these names is a JSON Schema array (`examples`) or
/// an Operation's `parameters` list, the value is not an object and the entry has no effect.
const NAME_KEYED_FIELDS: &[&str] = &[
    "webhooks",
    "variables",
    "callbacks",
    "content",
    "encoding",
    "headers",
    "links",
    "examples",
    "additionalOperations",
    "parameters",
    "mapping",
    "scopes",
    "properties",
    "patternProperties",
    "$defs",
    "dependentSchemas",
    "definitions",
];

/// Collect Security Requirement Object keys that name a scheme by URI rather than component name.
fn collect_security_requirement_refs(value: &SpannedValue, refs: &mut Vec<String>) {
    let Node::Object(root) = &value.node else {
        return;
    };
    let mut visit = |requirements: &SpannedValue| {
        let Node::Array(entries) = &requirements.node else {
            return;
        };
        for entry in entries {
            let Node::Object(map) = &entry.node else {
                continue;
            };
            for (key, _) in map.iter() {
                // A single-segment name is a component name unless `./` forces the URI reading.
                if let Some(rest) = key.name.strip_prefix("./") {
                    refs.push(rest.to_owned());
                } else if key.name.contains('/') || key.name.contains('#') {
                    refs.push(key.name.clone());
                }
            }
        }
    };
    if let Some(security) = root.get("security") {
        visit(security);
    }
    if let Some(Node::Object(paths)) = root.get("paths").map(|paths| &paths.node) {
        for (path, item) in paths.iter() {
            if is_extension_key(&path.name) {
                continue;
            }
            let Node::Object(item) = &item.node else {
                continue;
            };
            for (method, operation) in item.iter() {
                if is_extension_key(&method.name) {
                    continue;
                }
                if let Some(security) = operation.get("security") {
                    visit(security);
                }
            }
        }
    }
}

fn collect_refs_inner(value: &SpannedValue, keys: Keys, refs: &mut Vec<String>) {
    match &value.node {
        Node::Object(map) => {
            // In a map, `$ref` is an entry's name (a property called `$ref`), not a reference.
            if matches!(keys, Keys::Fields) {
                if let Some(reference) = map.get("$ref").and_then(SpannedValue::as_str) {
                    refs.push(reference.to_owned());
                }
            }
            for (key, value) in map.iter() {
                let key = key.name.as_str();
                let child = match keys {
                    Keys::Names => Keys::Fields,
                    Keys::Fields | Keys::Components if is_extension_key(key) => continue,
                    Keys::Components => Keys::Names,
                    Keys::Fields if key == "components" => Keys::Components,
                    Keys::Fields if NAME_KEYED_FIELDS.contains(&key) => Keys::Names,
                    Keys::Fields => Keys::Fields,
                };
                collect_refs_inner(value, child, refs);
            }
        }
        Node::Array(values) => {
            for value in values {
                collect_refs_inner(value, Keys::Fields, refs);
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

    fn refs_in(yaml: &str) -> Vec<String> {
        let value = super::super::parse_yaml(crate::diag::FileId(0), yaml, &mut Default::default())
            .unwrap();
        collect_refs(&value)
    }

    /// The walk skips a specification extension wherever keys are fixed fields — the document
    /// root, the Paths Object (#370), an Operation, a Responses Object, a Schema, the Components
    /// Object — and nowhere keys are names: an `x-` header, property, component, media type or
    /// webhook is an entry like any other (#239). A `$ref` key in a map names an entry.
    #[test]
    fn collects_refs_outside_extensions_and_under_every_x_named_entry() {
        let refs = refs_in(
            "x-root: { $ref: skip-root.yaml }\n\
             paths:\n\
             \x20 x-paths: { $ref: skip-paths.yaml }\n\
             \x20 /p:\n\
             \x20   get:\n\
             \x20     x-op: { $ref: skip-op.yaml }\n\
             \x20     responses:\n\
             \x20       x-note: { $ref: skip-responses.yaml }\n\
             \x20       '200':\n\
             \x20         headers: { x-rate-limit: { $ref: header.yaml } }\n\
             \x20         content:\n\
             \x20           x-custom/json:\n\
             \x20             schema:\n\
             \x20               x-meta: { $ref: skip-schema.yaml }\n\
             \x20               properties:\n\
             \x20                 x-owner: { $ref: property.yaml }\n\
             \x20                 $ref: { type: string }\n\
             \x20               $defs: { x-def: { $ref: def.yaml } }\n\
             webhooks: { x-hook: { $ref: webhook.yaml } }\n\
             components:\n\
             \x20 x-components: { $ref: skip-components.yaml }\n\
             \x20 schemas: { x-pet: { $ref: component.yaml } }\n\
             \x20 responses: { x-gone: { $ref: component-response.yaml } }\n",
        );
        assert_eq!(
            refs,
            [
                "header.yaml",
                "property.yaml",
                "def.yaml",
                "webhook.yaml",
                "component.yaml",
                "component-response.yaml",
            ]
        );
    }

    /// A Security Requirement key naming a scheme by URI is still collected, but not one inside
    /// an extension of a Path Item or of the Paths Object (#370).
    #[test]
    fn security_requirement_refs_skip_extensions() {
        let refs = refs_in(
            "paths:\n\
             \x20 x-paths: { get: { security: [{ ./skip-paths-scheme.yaml: [] }] } }\n\
             \x20 /p:\n\
             \x20   x-item: { security: [{ skip-item.yaml: [] }] }\n\
             \x20   get: { security: [{ ./scheme.yaml: [] }] }\n",
        );
        assert_eq!(refs, ["scheme.yaml"]);
    }

    /// Only a pointer through an `x-` token enters an extension, decoded the way the resolver
    /// decodes it; a component *named* `x-pet` qualifies too, harmlessly.
    #[test]
    fn a_reference_enters_an_extension_only_through_an_x_token() {
        for reference in [
            "#/x-defs/Pet",
            "lib.yaml#/x-defs/Pet",
            "https://h/lib.yaml#/components/x-defs/Pet",
            "#/%78-defs/Pet",
            "#/components/schemas/x-pet",
        ] {
            assert!(enters_extension(reference), "{reference}");
        }
        for reference in [
            "#/components/schemas/Pet",
            "lib.yaml",
            "lib.yaml#",
            "#x-anchor",
            "#/components/schemas/X-pet",
            "#/components/schemas/pet-x-",
        ] {
            assert!(!enters_extension(reference), "{reference}");
        }
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
