//! # Subsystem: compat
//! layer-deps: source, diag
//!
//! Compatibility preprocessing for explicit API omissions. Omit rules are applied to the parsed
//! source bundle before OpenAPI validation/lowering, so callers can generate a conformant subset
//! without editing vendored upstream schemas.
//!
//! Two extensions to the exact-rule surface:
//!
//! * **Globbing / bulk omits.** An [`OmitRule::Path`], [`OmitRule::Operation`], or
//!   [`OmitRule::Component`] path/name — and an [`OmitRule::Pointer`] string — that contains a glob
//!   metacharacter (`*`, `**`, or `?`) is matched as a glob over the construct's path/name (or the
//!   whole pointer string). A glob rule removes **every** matching construct (bulk); a rule with no
//!   metacharacter is an exact rule and behaves exactly as before. A metacharacter is escapable
//!   with a backslash (`\*`, `\?`), because a URI path may legitimately contain one; rules that
//!   auto-carve builds from literal document text are escaped for exactly that reason. A
//!   backslash escapes whatever character follows it, in exact and glob rules alike (`\b` is `b`,
//!   `\\` is one literal backslash). See
//!   [`glob_match`] for the semantics.
//! * **Auto-carve.** [`carve_rules`] maps error diagnostics to the smallest enclosing *omittable*
//!   construct, so the facade can iteratively omit the unsupported islands of a spec and generate
//!   the rest. The fixpoint driver lives in the facade (it must re-run the pipeline); this module
//!   supplies only the pure pointer→construct mapping.
//!
//! The rule grammar is in `rule`, the matcher in `glob`, and the auto-carve mapping in `carve`;
//! this module applies a profile to a bundle and checks what survives it.

mod carve;
mod glob;
mod rule;

use std::cmp::Ordering;

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, FileId, JsonPointer, Provenance, Span};
use crate::source::{InputBundle, Node, SpannedValue};

pub(crate) use carve::{carve_rules, MAX_CARVE_ROUNDS};
use glob::{glob_match, has_glob_meta, unescape_glob};
pub use rule::{ComponentKind, OmitMethod, OmitRule, UnknownOmitToken};

/// A compatibility omit profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Omit {
    /// Exact omit rules.
    pub rules: Vec<OmitRule>,
}

impl Omit {
    /// Whether the profile contains no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Apply the profile to the loaded input bundle.
    pub(crate) fn apply(
        &self,
        bundle: &mut InputBundle,
        diags: &mut Diagnostics,
    ) -> Result<(), Aborted> {
        for rule in &self.rules {
            self.apply_rule(rule, bundle, diags);
        }
        validate_remaining(bundle, diags);
        diags.result(())
    }

    /// Stable fingerprint used in generated provenance headers: FNV-1a 64 over
    /// the profile's canonical rule encoding (the crate-private `Omit::canonical_encoding`), not over derived [`Hash`](std::hash::Hash), whose byte
    /// stream std documents as unstable across platforms and compiler releases. The value is
    /// therefore the same on every host and toolchain for the same rules.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Fnv64::new();
        hasher.write(&self.canonical_encoding());
        format!("{:016x}", hasher.finish())
    }

    /// The one canonical byte encoding of the profile, shared by [`Omit::fingerprint`] and the
    /// build cache's input fingerprint so the two cannot drift apart.
    ///
    /// Rules encode in declaration order. Each is a variant tag byte (`0` path, `1` operation,
    /// `2` component, `3` pointer) followed by its fields, every string length-prefixed (`u64`,
    /// big-endian) so no two rule sets share an encoding; methods and component kinds encode as
    /// their OpenAPI key, and a `Pointer` rule's `file` as a presence byte (`0` absent,
    /// `1` present) followed by the string when present.
    pub(crate) fn canonical_encoding(&self) -> Vec<u8> {
        fn push_str(out: &mut Vec<u8>, text: &str) {
            out.extend_from_slice(&(text.len() as u64).to_be_bytes());
            out.extend_from_slice(text.as_bytes());
        }

        let mut out = Vec::new();
        for rule in &self.rules {
            match rule {
                OmitRule::Path { path } => {
                    out.push(0);
                    push_str(&mut out, path);
                }
                OmitRule::Operation { method, path } => {
                    out.push(1);
                    push_str(&mut out, method.as_oas_key());
                    push_str(&mut out, path);
                }
                OmitRule::Component { kind, name } => {
                    out.push(2);
                    push_str(&mut out, kind.as_oas_key());
                    push_str(&mut out, name);
                }
                OmitRule::Pointer { file, pointer } => {
                    out.push(3);
                    match file {
                        Some(file) => {
                            out.push(1);
                            push_str(&mut out, file);
                        }
                        None => out.push(0),
                    }
                    push_str(&mut out, pointer);
                }
            }
        }
        out
    }

    fn apply_rule(&self, rule: &OmitRule, bundle: &mut InputBundle, diags: &mut Diagnostics) {
        match self.resolve_rule(rule, bundle, diags) {
            // An invalid pointer/file rule already emitted its own diagnostic.
            None => {}
            Some(RuleMatch::Exact { file, pointer }) => {
                match bundle.value_at_mut(file).remove_pointer(&pointer) {
                    Some(value) => emit_omitted(diags, &pointer, value.span(), &rule.describe()),
                    None => emit_invalid_rule(
                        rule,
                        bundle,
                        diags,
                        "omit rule did not match any source construct",
                    ),
                }
            }
            Some(RuleMatch::Glob { file, mut pointers }) => {
                if pointers.is_empty() {
                    emit_invalid_rule(
                        rule,
                        bundle,
                        diags,
                        "glob omit rule did not match any source construct",
                    );
                    return;
                }
                // Remove deepest-first (and, within one array parent, highest-index-first) so a
                // parent removal never invalidates a still-pending child/sibling pointer.
                pointers.sort_by(removal_order);
                let describe = rule.describe();
                let mut removed_any = false;
                for pointer in pointers {
                    if let Some(value) = bundle.value_at_mut(file).remove_pointer(&pointer) {
                        removed_any = true;
                        emit_omitted(diags, &pointer, value.span(), &describe);
                    }
                }
                if !removed_any {
                    emit_invalid_rule(
                        rule,
                        bundle,
                        diags,
                        "glob omit rule did not match any source construct",
                    );
                }
            }
        }
    }

    /// Resolve a rule to the concrete construct(s) it targets. A rule whose path/name/pointer
    /// carries a glob metacharacter matches many constructs ([`RuleMatch::Glob`]); an exact rule
    /// resolves to a single pointer ([`RuleMatch::Exact`]). Invalid pointer/file rules emit their
    /// diagnostic here and return `None`.
    fn resolve_rule(
        &self,
        rule: &OmitRule,
        bundle: &InputBundle,
        diags: &mut Diagnostics,
    ) -> Option<RuleMatch> {
        let root = bundle.root_id();
        Some(match rule {
            OmitRule::Path { path } if has_glob_meta(path) => {
                let base = JsonPointer::root().push("paths");
                let pointers = matching_child_keys(bundle.root().get("paths"), path)
                    .map(|key| base.push(&key))
                    .collect();
                RuleMatch::Glob {
                    file: root,
                    pointers,
                }
            }
            OmitRule::Path { path } => RuleMatch::Exact {
                file: root,
                pointer: JsonPointer::root().push("paths").push(&unescape_glob(path)),
            },
            OmitRule::Operation { method, path } if has_glob_meta(path) => {
                let paths = bundle.root().get("paths");
                let method_key = method.as_oas_key();
                let pointers = matching_child_keys(paths, path)
                    .filter_map(|key| {
                        let item = paths?.get(&key)?;
                        item.get(method_key)?;
                        Some(
                            JsonPointer::root()
                                .push("paths")
                                .push(&key)
                                .push(method_key),
                        )
                    })
                    .collect();
                RuleMatch::Glob {
                    file: root,
                    pointers,
                }
            }
            OmitRule::Operation { method, path } => RuleMatch::Exact {
                file: root,
                pointer: JsonPointer::root()
                    .push("paths")
                    .push(&unescape_glob(path))
                    .push(method.as_oas_key()),
            },
            OmitRule::Component { kind, name } if has_glob_meta(name) => {
                let kind_key = kind.as_oas_key();
                let map = bundle
                    .root()
                    .get("components")
                    .and_then(|components| components.get(kind_key));
                let base = JsonPointer::root().push("components").push(kind_key);
                let pointers = matching_child_keys(map, name)
                    .map(|key| base.push(&key))
                    .collect();
                RuleMatch::Glob {
                    file: root,
                    pointers,
                }
            }
            OmitRule::Component { kind, name } => RuleMatch::Exact {
                file: root,
                pointer: JsonPointer::root()
                    .push("components")
                    .push(kind.as_oas_key())
                    .push(&unescape_glob(name)),
            },
            OmitRule::Pointer { file, pointer } => {
                if pointer.is_empty() {
                    emit_invalid_rule(
                        rule,
                        bundle,
                        diags,
                        "omit rules cannot remove the document root",
                    );
                    return None;
                }
                let Some(file_id) = file
                    .as_deref()
                    .map(|path| bundle.file_id_for_path(path))
                    .unwrap_or_else(|| Some(bundle.root_id()))
                else {
                    emit_invalid_rule(rule, bundle, diags, "omit rule references an unloaded file");
                    return None;
                };
                if has_glob_meta(pointer) {
                    let mut pointers = Vec::new();
                    collect_pointers(
                        bundle.value_at(file_id),
                        &JsonPointer::root(),
                        &mut pointers,
                    );
                    pointers.retain(|candidate| glob_match(pointer, candidate.as_str()));
                    RuleMatch::Glob {
                        file: file_id,
                        pointers,
                    }
                } else {
                    RuleMatch::Exact {
                        file: file_id,
                        pointer: JsonPointer::from(unescape_glob(pointer)),
                    }
                }
            }
        })
    }
}

/// The concrete target(s) an [`OmitRule`] resolves to against a loaded bundle.
enum RuleMatch {
    /// A single exact pointer (a missing target is an `E019`).
    Exact { file: FileId, pointer: JsonPointer },
    /// Every pointer a glob rule matched (an empty match is an `E019`).
    Glob {
        file: FileId,
        pointers: Vec<JsonPointer>,
    },
}

/// Emit the `W009` "construct omitted" warning for one removed construct.
fn emit_omitted(diags: &mut Diagnostics, pointer: &JsonPointer, span: Span, describe: &str) {
    Diagnostic::warning(
        Code::OmittedConstruct,
        Provenance::new(pointer.clone(), Some(span)),
    )
    .message(format!("omitted construct matched by `{describe}`"))
    .remedy("the source schema was not modified; remove the omit rule once spargen supports this construct")
    .emit(diags);
}

/// The immediate object-member keys of `value` (if it is an object) that a glob `pattern` matches,
/// in source order.
fn matching_child_keys<'a>(
    value: Option<&'a SpannedValue>,
    pattern: &'a str,
) -> impl Iterator<Item = String> + 'a {
    value
        .and_then(SpannedValue::as_object)
        .into_iter()
        .flat_map(|object| object.iter())
        .filter(move |(key, _)| glob_match(pattern, &key.name))
        .map(|(key, _)| key.name.clone())
}

/// Collect every node pointer reachable from `value` (objects by member, arrays by index),
/// excluding the root, into `out` in document order.
fn collect_pointers(value: &SpannedValue, base: &JsonPointer, out: &mut Vec<JsonPointer>) {
    match &value.node {
        Node::Object(object) => {
            for (key, child) in object.iter() {
                let pointer = base.push(&key.name);
                collect_pointers(child, &pointer, out);
                out.push(pointer);
            }
        }
        Node::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let pointer = base.index(index);
                collect_pointers(child, &pointer, out);
                out.push(pointer);
            }
        }
        _ => {}
    }
}

/// Deterministic, removal-safe ordering: deepest pointers first, and — within one array parent —
/// highest index first, so removing one target never shifts a still-pending sibling/child.
fn removal_order(a: &JsonPointer, b: &JsonPointer) -> Ordering {
    let depth = |pointer: &JsonPointer| pointer.as_str().matches('/').count();
    depth(b).cmp(&depth(a)).then_with(|| {
        match (last_segment_index(a), last_segment_index(b)) {
            // Both leaves are array indices: remove the larger index first.
            (Some(ia), Some(ib)) => ib.cmp(&ia),
            // Otherwise keep the caller's (source/document) order via a stable sort.
            _ => Ordering::Equal,
        }
    })
}

fn last_segment_index(pointer: &JsonPointer) -> Option<usize> {
    pointer.as_str().rsplit('/').next()?.parse::<usize>().ok()
}

fn emit_invalid_rule(
    rule: &OmitRule,
    bundle: &InputBundle,
    diags: &mut Diagnostics,
    message: &'static str,
) {
    Diagnostic::error(
        Code::InvalidOmitRule,
        Provenance::new(JsonPointer::root(), Some(bundle.root().span())),
    )
    .message(format!("{message}: {}", rule.describe()))
    .remedy("use exact paths, operation methods, component names, or RFC 6901 pointers")
    .emit(diags);
}

/// Check that what survives an omit profile is still a valid OpenAPI root.
///
/// This mirrors the official schemas, which `require` only `openapi` and `info` and then place
/// `paths`, `components`, and `webhooks` in an `anyOf` — a document carrying any one of the three
/// is valid. Demanding `paths` outright reported a document the specification accepts as
/// `E020` "omit profile created an invalid document".
fn validate_remaining(bundle: &InputBundle, diags: &mut Diagnostics) {
    let root = bundle.root();
    let reject = |message: String, diags: &mut Diagnostics| {
        Diagnostic::error(
            Code::OmitCreatedInvalidDocument,
            Provenance::new(JsonPointer::root(), Some(root.span())),
        )
        .message(message)
        .remedy("do not omit required OpenAPI root fields")
        .emit(diags);
    };

    let missing = ["openapi", "info"]
        .into_iter()
        .filter(|key| root.get(key).is_none())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        reject(
            format!(
                "omit profile removed required OpenAPI root fields: {}",
                missing.join(", ")
            ),
            diags,
        );
        return;
    }

    if !["paths", "components", "webhooks"]
        .into_iter()
        .any(|key| root.get(key).is_some())
    {
        reject(
            "omit profile left no `paths`, `components`, or `webhooks`; an OpenAPI root requires \
             at least one of the three"
                .to_owned(),
            diags,
        );
    }
}

/// FNV-1a 64 over explicitly written bytes. Deliberately not a [`std::hash::Hasher`], so derived
/// `Hash` (whose byte stream is platform- and toolchain-dependent) cannot be fed into it.
struct Fnv64(u64);

impl Fnv64 {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::carve::omittable_enclosing;
    use super::glob::escape_glob_meta;
    use super::{
        carve_rules, glob_match, has_glob_meta, unescape_glob, ComponentKind, Diagnostic,
        JsonPointer, Omit, OmitMethod, OmitRule, Provenance,
    };
    use crate::diag::{Code, Diagnostics};
    use crate::source::InputBundle;
    use proptest::prelude::*;

    #[test]
    fn glob_matcher_semantics() {
        // Exact literals.
        assert!(glob_match("/pets", "/pets"));
        assert!(!glob_match("/pets", "/pet"));
        // `*` matches within one segment but never crosses `/`.
        assert!(glob_match("/pets/*", "/pets/dog"));
        assert!(!glob_match("/pets/*", "/pets/dog/paw"));
        assert!(glob_match("Legacy*", "LegacyPet"));
        assert!(glob_match("*Pet", "LegacyPet"));
        assert!(glob_match("*", "anything"));
        assert!(!glob_match("*", "a/b"));
        // `**` matches across any depth.
        assert!(glob_match("/admin/**", "/admin/users"));
        assert!(glob_match("/admin/**", "/admin/users/{id}"));
        assert!(!glob_match("/admin/**", "/public/users"));
        // `?` matches exactly one non-`/` character.
        assert!(glob_match("/pet?", "/pets"));
        assert!(!glob_match("/pet?", "/pet"));
        assert!(!glob_match("/pet?", "/pe/s"));
        // No metacharacter ⇒ not treated as a glob.
        assert!(!has_glob_meta("/pets/{id}"));
        assert!(has_glob_meta("/pets/*"));
        assert!(has_glob_meta("/pet?"));
    }

    /// The escape half of the matcher, pinned against mutation testing: a scoped `cargo mutants`
    /// run over this file left every arithmetic and guard mutant in `compile_glob` and the
    /// backslash arm of `has_glob_meta` alive, because the integration tests exercise escaping
    /// only through paths where an escaped and an unescaped rule happen to remove the same
    /// construct. These assert the two functions directly.
    #[test]
    fn escaped_metacharacters_are_literal_and_cannot_run_off_the_pattern() {
        // An escaped metacharacter is not a metacharacter, so the rule stays exact.
        assert!(!has_glob_meta(r"\*"));
        assert!(!has_glob_meta(r"\?"));
        assert!(!has_glob_meta(r"/files/\*"));
        // Escaping is per-character: a second, unescaped one still makes it a glob.
        assert!(has_glob_meta(r"/files/\*/*"));
        // An escaped backslash consumes itself, so the `*` after it is live again.
        assert!(has_glob_meta(r"\\*"));
        // A trailing lone backslash escapes nothing and must not read past the end.
        assert!(!has_glob_meta(r"/files/\"));

        // The escaped character matches itself, and nothing else.
        assert!(glob_match(r"/files/\*", "/files/*"));
        assert!(!glob_match(r"/files/\*", "/files/other"));
        assert!(glob_match(r"/files/\?", "/files/?"));
        assert!(!glob_match(r"/files/\?", "/files/x"));
        // An escaped backslash is one literal backslash.
        assert!(glob_match(r"/files/\\", r"/files/\"));
        // `**` still spans depth when it is not escaped, and its first `*` escapes cleanly.
        assert!(glob_match(r"/a/**", "/a/b/c"));
        assert!(glob_match(r"/a/\**", "/a/*b"));
        // A `*` that is not the final character must stay a single-segment star: reading the
        // lookahead off by one turns every `*` into `**` and silently widens the rule.
        assert!(glob_match("/pets/*/paw", "/pets/dog/paw"));
        assert!(!glob_match("/pets/*/paw", "/pets/dog/left/paw"));
        // `**` must consume both of its characters, so what follows it still has to match.
        assert!(glob_match("/a/**/z", "/a/b/c/z"));
        assert!(!glob_match("/a/**/z", "/a/b/c/y"));
        assert!(glob_match("/a/**x", "/a/b/cx"));
        assert!(!glob_match("/a/**/z", "/a/bz"));
        // A pattern that is nothing but a trailing backslash compiles and terminates.
        assert!(glob_match(r"\", r"\"));
        // A pattern opening with an escape must not index before its start.
        assert!(glob_match(r"\*x", "*x"));
    }

    /// Exact and glob rules share one escaping rule: a backslash escapes whatever follows it, so
    /// `\b` is the literal `b` in both forms. The exact form used to keep the backslash, so the
    /// exact rule `/a\b` named the path `/a\b` while the glob `/a\b*` named `/abc`.
    #[test]
    fn an_escaped_ordinary_character_means_itself_in_exact_and_glob_rules() {
        assert_eq!(unescape_glob(r"/a\b"), "/ab");
        assert!(glob_match(r"/a\b", "/ab"));
        assert!(!glob_match(r"/a\b", r"/a\b"));
        assert!(glob_match(r"/a\b*", "/abc"));
        assert!(!glob_match(r"/a\b*", r"/a\bc"));
        // The metacharacter escapes and a trailing lone backslash are unchanged.
        assert_eq!(unescape_glob(r"/f/\*\?\\"), r"/f/*?\");
        assert_eq!(unescape_glob(r"/f/\"), r"/f/\");

        // An exact path rule applies the same reading: `/a\b` removes `/ab`, not `/a\b`.
        let spec = r#"
openapi: 3.1.0
info: { title: t, version: 1.0.0 }
paths:
  /ab:
    get: { responses: { "200": { description: ok } } }
  '/a\b':
    get: { responses: { "200": { description: ok } } }
"#;
        let mut bundle = bundle_of(spec);
        let mut diags = Diagnostics::default();
        let omit = Omit {
            rules: vec![OmitRule::path(r"/a\b")],
        };
        omit.apply(&mut bundle, &mut diags).unwrap();
        assert_eq!(keys_under(&bundle, "paths", None), vec![r"/a\b".to_owned()]);
    }

    proptest! {
        /// Escaping literal text yields a rule that matches exactly that text.
        #[test]
        fn an_escaped_text_matches_itself(text in r"[a-c/*?\\]{0,12}") {
            let pattern = escape_glob_meta(&text);
            prop_assert!(!has_glob_meta(&pattern), "{pattern:?}");
            prop_assert!(glob_match(&pattern, &text), "{pattern:?} vs {text:?}");
            prop_assert_eq!(unescape_glob(&pattern), text);
        }

        /// A pattern with no unescaped metacharacter matches as a glob exactly the text its
        /// exact-rule reading names, so the two forms can never disagree.
        #[test]
        fn a_meta_free_pattern_matches_exactly_its_unescaped_text(
            // Ordinary characters and escapes of anything, then an optional trailing backslash:
            // every pattern with no unescaped metacharacter, built rather than filtered.
            pattern in r"([a-c/]|\\[a-c/*?\\]){0,8}\\?",
            text in r"[a-c/*?\\]{0,12}",
        ) {
            prop_assert!(!has_glob_meta(&pattern), "{pattern:?}");
            let literal = unescape_glob(&pattern);
            prop_assert!(glob_match(&pattern, &literal), "{pattern:?} vs {literal:?}");
            prop_assert_eq!(glob_match(&pattern, &text), literal == text);
        }
    }

    /// Load an inline YAML spec into an [`InputBundle`] via a tempfile (the loader reads from disk).
    fn bundle_of(spec: &str) -> InputBundle {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("openapi.yaml");
        std::fs::write(&path, spec).unwrap();
        let mut diags = Diagnostics::default();
        InputBundle::load(camino::Utf8Path::from_path(&path).unwrap(), &mut diags).unwrap()
    }

    const MULTI_SPEC: &str = r#"
openapi: 3.1.0
info: { title: t, version: 1.0.0 }
paths:
  /admin/users:
    get: { responses: { "200": { description: ok } } }
  /admin/users/{id}:
    get: { responses: { "200": { description: ok } } }
    delete: { responses: { "204": { description: ok } } }
  /public/health:
    get: { responses: { "200": { description: ok } } }
components:
  schemas:
    LegacyPet: { type: object }
    LegacyOwner: { type: object }
    Pet: { type: object }
"#;

    fn keys_under(bundle: &InputBundle, section: &str, sub: Option<&str>) -> Vec<String> {
        let mut node = bundle.root().get(section);
        if let Some(sub) = sub {
            node = node.and_then(|value| value.get(sub));
        }
        node.and_then(|value| value.as_object())
            .map(|object| object.iter().map(|(key, _)| key.name.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn glob_path_rule_removes_multiple_paths() {
        let mut bundle = bundle_of(MULTI_SPEC);
        let mut diags = Diagnostics::default();
        let omit = Omit {
            rules: vec![OmitRule::path("/admin/**")],
        };
        omit.apply(&mut bundle, &mut diags).unwrap();
        let paths = keys_under(&bundle, "paths", None);
        assert_eq!(
            paths,
            vec!["/public/health".to_owned()],
            "both admin paths gone"
        );
        // One W009 per removed construct, none silent.
        let w009 = diags
            .items()
            .iter()
            .filter(|d| d.code == Code::OmittedConstruct)
            .count();
        assert_eq!(w009, 2, "one W009 per removed path");
    }

    #[test]
    fn glob_component_rule_removes_multiple_components() {
        let mut bundle = bundle_of(MULTI_SPEC);
        let mut diags = Diagnostics::default();
        let omit = Omit {
            rules: vec![OmitRule::component(ComponentKind::Schemas, "Legacy*")],
        };
        omit.apply(&mut bundle, &mut diags).unwrap();
        let schemas = keys_under(&bundle, "components", Some("schemas"));
        assert_eq!(schemas, vec!["Pet".to_owned()], "both Legacy* schemas gone");
    }

    #[test]
    fn exact_rule_is_unchanged_by_glob_support() {
        // An exact rule (no metacharacter) removes exactly its one target and nothing else.
        let mut bundle = bundle_of(MULTI_SPEC);
        let mut diags = Diagnostics::default();
        let omit = Omit {
            rules: vec![OmitRule::path("/admin/users")],
        };
        omit.apply(&mut bundle, &mut diags).unwrap();
        let paths = keys_under(&bundle, "paths", None);
        assert_eq!(
            paths,
            vec!["/admin/users/{id}".to_owned(), "/public/health".to_owned()],
            "only the exact path is gone"
        );
        let w009 = diags
            .items()
            .iter()
            .filter(|d| d.code == Code::OmittedConstruct)
            .count();
        assert_eq!(w009, 1);
    }

    #[test]
    fn glob_operation_rule_removes_matching_operations_only() {
        let mut bundle = bundle_of(MULTI_SPEC);
        let mut diags = Diagnostics::default();
        let omit = Omit {
            rules: vec![OmitRule::operation(OmitMethod::Get, "/admin/**")],
        };
        omit.apply(&mut bundle, &mut diags).unwrap();
        // The two admin `get` operations are gone; the `delete` and the public `get` remain.
        assert!(bundle
            .root()
            .get("paths")
            .and_then(|p| p.get("/admin/users/{id}"))
            .and_then(|item| item.get("get"))
            .is_none());
        assert!(bundle
            .root()
            .get("paths")
            .and_then(|p| p.get("/admin/users/{id}"))
            .and_then(|item| item.get("delete"))
            .is_some());
        assert!(bundle
            .root()
            .get("paths")
            .and_then(|p| p.get("/public/health"))
            .and_then(|item| item.get("get"))
            .is_some());
    }

    #[test]
    fn glob_rule_matching_nothing_is_e019() {
        let mut bundle = bundle_of(MULTI_SPEC);
        let mut diags = Diagnostics::default();
        let omit = Omit {
            rules: vec![OmitRule::path("/nope/**")],
        };
        assert!(omit.apply(&mut bundle, &mut diags).is_err());
        assert!(diags
            .items()
            .iter()
            .any(|d| d.code == Code::InvalidOmitRule));
        // A glob that matched nothing means the *pattern* is wrong; say which kind of rule it was,
        // because the fix differs from an exact rule naming a construct that is not there.
        assert!(
            diags
                .items()
                .iter()
                .any(|d| d.message.starts_with("glob omit rule")),
            "{:#?}",
            diags.items()
        );
    }

    /// The exact-rule counterpart. The two paths are behaviourally identical when the rule has no
    /// metacharacter — a literal glob matches the same single key — so the message is the only
    /// thing that distinguishes them, and it is what tells a user which mistake they made.
    #[test]
    fn an_exact_rule_matching_nothing_is_e019_and_says_it_was_exact() {
        let mut bundle = bundle_of(MULTI_SPEC);
        let mut diags = Diagnostics::default();
        let omit = Omit {
            rules: vec![OmitRule::path("/nope")],
        };
        assert!(omit.apply(&mut bundle, &mut diags).is_err());
        let invalid: Vec<_> = diags
            .items()
            .iter()
            .filter(|d| d.code == Code::InvalidOmitRule)
            .collect();
        assert_eq!(invalid.len(), 1, "{invalid:#?}");
        assert!(
            invalid[0].message.starts_with("omit rule did not match"),
            "{invalid:#?}"
        );

        // The same distinction on the other two rule kinds, which resolve through their own arms.
        for rule in [
            OmitRule::operation(OmitMethod::Get, "/nope"),
            OmitRule::component(ComponentKind::Schemas, "Nope"),
        ] {
            let mut bundle = bundle_of(MULTI_SPEC);
            let mut diags = Diagnostics::default();
            let omit = Omit {
                rules: vec![rule.clone()],
            };
            assert!(omit.apply(&mut bundle, &mut diags).is_err(), "{rule:?}");
            assert!(
                diags
                    .items()
                    .iter()
                    .any(|d| d.message.starts_with("omit rule did not match")),
                "{rule:?}: {:#?}",
                diags.items()
            );
        }
    }

    #[test]
    fn carve_rules_map_pointers_to_enclosing_constructs() {
        // Operation pointer → operation.
        assert_eq!(
            omittable_enclosing(&JsonPointer::from(
                "/paths/~1pets~1{id}/get/responses/200".to_owned()
            )),
            Some(OmitRule::operation(OmitMethod::Get, "/pets/{id}"))
        );
        // Path-item-level pointer (not into a method) → path.
        assert_eq!(
            omittable_enclosing(&JsonPointer::from("/paths/~1pets/parameters/0".to_owned())),
            Some(OmitRule::path("/pets"))
        );
        // Component pointer → component.
        assert_eq!(
            omittable_enclosing(&JsonPointer::from(
                "/components/schemas/Bad/oneOf/0".to_owned()
            )),
            Some(OmitRule::component(ComponentKind::Schemas, "Bad"))
        );
        // Root / unmodelled ⇒ not carvable.
        assert_eq!(omittable_enclosing(&JsonPointer::root()), None);
        assert_eq!(
            omittable_enclosing(&JsonPointer::from("/components/callbacks/X".to_owned())),
            None
        );
    }

    #[test]
    fn carve_rules_are_deduped_sorted_and_error_only() {
        let error = |pointer: &str| {
            Diagnostic::error(
                Code::UnsupportedMediaType,
                Provenance::new(JsonPointer::from(pointer.to_owned()), None),
            )
            .build()
        };
        let warning = Diagnostic::warning(
            Code::OmittedConstruct,
            Provenance::new(JsonPointer::from("/paths/~1w/get".to_owned()), None),
        )
        .build();
        let diagnostics = vec![
            error("/paths/~1b/get/responses/200"),
            error("/paths/~1a/get/responses/200"),
            // Duplicate of the first (same enclosing operation).
            error("/paths/~1b/get/responses/404"),
            warning,
        ];
        // No span, so every diagnostic is read against the root document.
        let rules = carve_rules(&diagnostics, &bundle_of(MULTI_SPEC));
        // Warnings ignored; duplicates collapsed; sorted deterministically by description.
        assert_eq!(
            rules,
            vec![
                OmitRule::operation(OmitMethod::Get, "/a"),
                OmitRule::operation(OmitMethod::Get, "/b"),
            ]
        );
    }

    /// Load a root document plus sub-files, all written beside each other.
    fn bundle_of_files(root: &str, files: &[(&str, &str)]) -> InputBundle {
        let dir = tempfile::tempdir().unwrap();
        for (name, contents) in files {
            std::fs::write(dir.path().join(name), contents).unwrap();
        }
        let path = dir.path().join("openapi.yaml");
        std::fs::write(&path, root).unwrap();
        let mut diags = Diagnostics::default();
        InputBundle::load(camino::Utf8Path::from_path(&path).unwrap(), &mut diags).unwrap()
    }

    /// An error reported at `pointer` in the loaded file named `file`.
    fn error_in(bundle: &InputBundle, file: &str, pointer: &str) -> Diagnostic {
        let id = bundle.file_id_for_path(file).unwrap();
        // Any error code carves the same way; this one is not held to an emission-site marker.
        Diagnostic::error(
            Code::UnsupportedMediaType,
            Provenance::new(
                JsonPointer::from(pointer.to_owned()),
                Some(bundle.value_at(id).span()),
            ),
        )
        .build()
    }

    /// A sub-file construct is carved as a pointer into that file, and its name is literal text: a
    /// `/` is pointer-escaped and a glob metacharacter glob-escaped, so applying the rule removes
    /// exactly that component and not its sibling a glob `A*B` would also match.
    #[test]
    fn a_sub_file_construct_is_carved_by_an_escaped_file_scoped_pointer() {
        let lib = "components:\n  schemas:\n    \"A*B/C\": { type: object }\n    AxB/C: { type: object }\n";
        let root = "openapi: 3.1.0\ninfo: { title: t, version: 1.0.0 }\npaths: {}\n\
                    components: { schemas: { X: { $ref: './lib.yaml#/components/schemas/A*B~1C' } } }\n";
        let mut bundle = bundle_of_files(root, &[("lib.yaml", lib)]);
        let rules = carve_rules(
            &[error_in(
                &bundle,
                "lib.yaml",
                "/components/schemas/A*B~1C/properties/x",
            )],
            &bundle,
        );
        assert_eq!(
            rules,
            vec![OmitRule::pointer(
                Some("lib.yaml".into()),
                r"/components/schemas/A\*B~1C"
            )]
        );

        let mut diags = Diagnostics::default();
        Omit { rules }.apply(&mut bundle, &mut diags).unwrap();
        let lib_id = bundle.file_id_for_path("lib.yaml").unwrap();
        let remaining: Vec<_> = bundle
            .value_at(lib_id)
            .get("components")
            .and_then(|components| components.get("schemas"))
            .and_then(|schemas| schemas.as_object())
            .map(|object| object.iter().map(|(key, _)| key.name.clone()).collect())
            .unwrap();
        assert_eq!(remaining, vec!["AxB/C".to_owned()]);
    }

    /// Two bare sub-files that reference each other form a cycle the referrer walk must leave: it
    /// expands each site once, and still reaches the root operation that pulled the pair in.
    #[test]
    fn the_referrer_walk_terminates_on_a_cycle_between_sub_files() {
        let a = "type: object\nproperties:\n  b: { $ref: './b.yaml' }\n";
        let b = "type: object\nproperties:\n  a: { $ref: './a.yaml' }\n  bad: { $ref: '#/nope' }\n";
        let root = "openapi: 3.1.0\ninfo: { title: t, version: 1.0.0 }\npaths:\n  /x:\n    get:\n      \
                    responses:\n        \"200\":\n          description: ok\n          content:\n            \
                    application/json: { schema: { $ref: './a.yaml' } }\n";
        let bundle = bundle_of_files(root, &[("a.yaml", a), ("b.yaml", b)]);
        let rules = carve_rules(&[error_in(&bundle, "b.yaml", "/properties/bad")], &bundle);
        assert_eq!(rules, vec![OmitRule::operation(OmitMethod::Get, "/x")]);
    }

    #[test]
    fn omit_macro_expands_to_typed_rules() {
        let omit = crate::omit! {
            operations {
                get "/markdown";
                post "/markdown/raw";
            }
            paths {
                "/octocat";
            }
            components {
                schemas { "legacy"; }
                request_bodies { "legacy-body"; }
            }
            pointers {
                "/paths/~1legacy";
            }
            file("schemas/legacy.yaml") {
                pointers {
                    "/properties/unsupported";
                }
            }
        };

        assert_eq!(omit.rules.len(), 7);
        assert_eq!(
            omit.rules[0],
            OmitRule::operation(OmitMethod::Get, "/markdown")
        );
        assert_eq!(
            omit.rules[3],
            OmitRule::component(ComponentKind::Schemas, "legacy")
        );
        assert!(omit.fingerprint().len() == 16);
    }

    fn omit_of(rules: Vec<OmitRule>) -> Omit {
        Omit { rules }
    }

    fn pointer_rule(file: Option<&'static str>, pointer: &'static str) -> OmitRule {
        OmitRule::pointer(file.map(Cow::Borrowed), pointer)
    }

    /// The fingerprint is stamped into every generated file's provenance header, so it is how a
    /// reader tells which omit profile produced a module. The only assertion on it was that it is
    /// 16 characters long — which a constant would also satisfy.
    #[test]
    fn the_fingerprint_distinguishes_profiles_and_repeats_for_the_same_one() {
        let empty = omit_of(Vec::new());
        let by_path = omit_of(vec![OmitRule::path("/a")]);
        let by_other_path = omit_of(vec![OmitRule::path("/b")]);
        let by_operation = omit_of(vec![OmitRule::operation(OmitMethod::Get, "/a")]);
        let by_component = omit_of(vec![OmitRule::component(ComponentKind::Schemas, "/a")]);
        let two_rules = omit_of(vec![OmitRule::path("/a"), OmitRule::path("/b")]);

        // Same rules, computed twice: a fingerprint that moved between calls would make generated
        // output non-deterministic, since it is stamped into the header.
        assert_eq!(
            by_path.fingerprint(),
            omit_of(vec![OmitRule::path("/a")]).fingerprint()
        );

        let distinct = [
            ("empty", empty.fingerprint()),
            ("path /a", by_path.fingerprint()),
            ("path /b", by_other_path.fingerprint()),
            ("get /a", by_operation.fingerprint()),
            ("schema /a", by_component.fingerprint()),
            ("two paths", two_rules.fingerprint()),
        ];
        for (index, (left_name, left)) in distinct.iter().enumerate() {
            assert_eq!(left.len(), 16, "{left_name} is not 16 hex characters");
            for (right_name, right) in &distinct[index + 1..] {
                assert_ne!(
                    left, right,
                    "`{left_name}` and `{right_name}` share a fingerprint"
                );
            }
        }
    }

    /// The rules are hashed in declaration order, so the fingerprint is order-*sensitive*. That is
    /// the behavior, and it is worth stating: two profiles that omit the same things in a
    /// different order stamp different headers even though they generate the same module.
    #[test]
    fn the_fingerprint_follows_rule_order() {
        let forward = omit_of(vec![OmitRule::path("/a"), OmitRule::path("/b")]);
        let reversed = omit_of(vec![OmitRule::path("/b"), OmitRule::path("/a")]);
        assert_ne!(forward.fingerprint(), reversed.fingerprint());
    }

    #[test]
    fn the_fingerprint_is_hexadecimal() {
        // It lands in a `// source:` comment, so anything outside `[0-9a-f]` would be a surprise
        // to whatever reads the header back.
        let fingerprint = omit_of(vec![OmitRule::path("/a")]).fingerprint();
        assert!(
            fingerprint
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
            "{fingerprint}"
        );
    }

    /// The fingerprint is stamped into the provenance header, so the same rules must yield the
    /// same value on every host and toolchain, not merely twice in one process. Pinning a literal
    /// for a set covering every rule kind catches an encoding that drifts (derived `Hash` did, by
    /// platform and rustc release). Changing this literal changes every header that carries an
    /// omit profile, which is a deliberate, reviewable output change.
    #[test]
    fn the_fingerprint_of_a_fixed_profile_is_pinned() {
        let omit = omit_of(vec![
            OmitRule::path("/pets"),
            OmitRule::operation(OmitMethod::Query, "/pets/{id}"),
            OmitRule::component(ComponentKind::MediaTypes, "Legacy"),
            pointer_rule(None, "/components/schemas/Old"),
            pointer_rule(Some("shared.yaml"), "/x"),
        ]);
        assert_eq!(omit.fingerprint(), "e5c02d327e1ebd34");
    }

    /// `describe()` renders `pointer {file}#{pointer}`, which cannot tell a file containing `#`
    /// from a pointer containing it, nor an absent file from an empty one. The canonical encoding
    /// must keep each of those pairs apart.
    #[test]
    fn the_fingerprint_separates_pointer_rules_describe_would_conflate() {
        let pairs = [
            (
                pointer_rule(Some("a#"), "/b"),
                pointer_rule(Some("a"), "#/b"),
            ),
            (pointer_rule(Some(""), "/b"), pointer_rule(None, "/b")),
            (OmitRule::path("/ab"), pointer_rule(None, "/ab")),
        ];
        for (left, right) in pairs {
            assert_ne!(
                omit_of(vec![left.clone()]).fingerprint(),
                omit_of(vec![right.clone()]).fingerprint(),
                "{left:?} and {right:?} share a fingerprint"
            );
        }
    }
}
