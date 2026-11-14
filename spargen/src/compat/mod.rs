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
//!   metacharacter is an exact rule and removes exactly the one construct it names. A backslash
//!   escapes a metacharacter, in exact and glob rules alike, as `glob` states; rules that
//!   auto-carve builds from literal document text are escaped. See [`Glob`](glob::Glob) for the
//!   semantics.
//! * **Auto-carve.** [`carve_rules`] maps error diagnostics to the smallest enclosing *omittable*
//!   construct, so the facade can iteratively omit the unsupported islands of a spec and generate
//!   the rest. The fixpoint driver lives in the facade's `pipeline` module (it must re-run the
//!   pipeline); this module supplies only the pure pointer→construct mapping.
//!
//! The rule grammar is in `rule`, the matcher in `glob`, and the auto-carve mapping in `carve`;
//! this module applies a profile to a bundle and checks what survives it.

mod carve;
mod glob;
mod keep;
mod rule;

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashSet;

use crate::diag::{
    fnv1a64, Aborted, Code, Diagnostic, Diagnostics, FileId, JsonPointer, Provenance, Span,
};
use crate::source::{InputBundle, SpannedValue};

pub(crate) use carve::{carve_rules, MAX_CARVE_ROUNDS};
use rule::RuleMatch;
pub use rule::{ComponentKind, KeepRule, OmitMethod, OmitRule, UnknownOmitToken};

/// A compatibility omit profile.
///
/// Build one with [`omit!`](crate::omit), or start from `Omit::default()` and push onto
/// [`rules`](Omit::rules) and [`keep`](Omit::keep).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Omit {
    /// Omit rules (exact or glob).
    pub rules: Vec<OmitRule>,
    /// Keep (allow-list) rules. Empty, they change nothing; otherwise every operation none of
    /// them names is omitted, and so is every root-document component the rest no longer
    /// reaches. See [`KeepRule`].
    pub keep: Vec<KeepRule>,
}

impl Omit {
    /// Whether the profile contains no omit rules and no keep rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty() && self.keep.is_empty()
    }

    /// Apply the profile to the loaded input bundle: resolve the keep rules against the document
    /// as written, apply the omit rules, then narrow what remains to the kept operations.
    ///
    /// The omit rules apply in declaration order, a rule equal to an earlier one only once. A rule
    /// that removes nothing is satisfied, not an `E019`, when everything it matches in the
    /// document as written lies at or beneath a construct an earlier rule removed: an exact rule
    /// under a glob that already took its target, or a glob whose every match went before it.
    pub(crate) fn apply(
        &self,
        bundle: &mut InputBundle,
        diags: &mut Diagnostics,
    ) -> Result<(), Aborted> {
        let kept = if self.keep.is_empty() {
            None
        } else {
            // A keep rule that names nothing is an `E019`, which fails the run below; narrowing
            // to the rules that did resolve would only add an `E020` for an empty `paths`.
            Some(keep::resolve(&self.keep, bundle, diags))
        };
        let mut seen = HashSet::new();
        let rules: Vec<&OmitRule> = self
            .rules
            .iter()
            .filter(|rule| seen.insert(*rule))
            .collect();
        // What each rule after the first matches before any rule removes anything: the first
        // has no earlier rule to be covered by.
        let as_written: Vec<Option<Targets>> = rules
            .iter()
            .enumerate()
            .map(|(index, rule)| (index > 0).then(|| targets_as_written(rule, bundle)))
            .collect();
        let mut removed = HashSet::new();
        for (rule, as_written) in rules.into_iter().zip(as_written) {
            apply_rule(rule, as_written.as_ref(), &mut removed, bundle, diags);
        }
        if let Some(Some(kept)) = &kept {
            keep::retain(kept, bundle, diags);
        }
        validate_remaining(bundle, diags);
        diags.result(())
    }

    /// Stable fingerprint used in generated provenance headers: [`fnv1a64`] over the profile's
    /// canonical rule encoding ([`Omit::canonical_encoding`]), so the value is the same on every
    /// host and toolchain for the same rules.
    pub(crate) fn fingerprint(&self) -> String {
        format!("{:016x}", fnv1a64(&self.canonical_encoding()))
    }

    /// The one canonical byte encoding of the profile, shared by [`Omit::fingerprint`] and the
    /// build cache's input fingerprint so the two cannot drift apart.
    ///
    /// Omit rules encode in declaration order, then keep rules in theirs. Each is a variant tag
    /// byte (`0` path, `1` operation, `2` component, `3` pointer, `4` kept operation) followed by
    /// its fields, every string length-prefixed (`u64`,
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
        for rule in &self.keep {
            match rule {
                KeepRule::Operation { method, path } => {
                    out.push(4);
                    push_str(&mut out, method.as_oas_key());
                    push_str(&mut out, path);
                }
            }
        }
        out
    }
}

/// The constructs a rule matches in one file: the file and every matching pointer.
type Targets = (FileId, Vec<JsonPointer>);

/// What `rule` matches in `bundle` as it stands: an exact rule's pointer where a value is there,
/// every pointer a glob matches, and nothing for a rule that does not resolve.
fn targets_as_written(rule: &OmitRule, bundle: &InputBundle) -> Targets {
    match rule.resolve(bundle) {
        Ok(RuleMatch::Exact { file, pointer }) => {
            let present = value_at_pointer(bundle.value_at(file), &pointer).is_some();
            (file, present.then_some(pointer).into_iter().collect())
        }
        Ok(RuleMatch::Glob { file, pointers }) => (file, pointers),
        Err(_) => (bundle.root_id(), Vec::new()),
    }
}

/// The value `pointer` names below `value`, read the way `remove_pointer` reads it: object
/// members by key, array items by index.
fn value_at_pointer<'a>(
    value: &'a SpannedValue,
    pointer: &JsonPointer,
) -> Option<&'a SpannedValue> {
    pointer
        .tokens()
        .iter()
        .try_fold(value, |current, token| match current.as_array() {
            Some(items) => items.get(token.parse::<usize>().ok()?),
            None => current.get(token),
        })
}

/// Whether every one of `targets` (at least one) lies at or beneath a pointer in `removed`.
fn covered(targets: &Targets, removed: &HashSet<(FileId, JsonPointer)>) -> bool {
    let (file, pointers) = targets;
    !pointers.is_empty()
        && pointers.iter().all(|pointer| {
            std::iter::successors(Some(pointer.clone()), JsonPointer::parent)
                .any(|ancestor| removed.contains(&(*file, ancestor)))
        })
}

/// Remove what `rule` matches from `bundle`, recording each removed construct in `removed` and
/// emitting a `W009` for it, or the one `E019` that says why the rule removed nothing. A rule
/// that removes nothing because earlier rules removed everything it matched `as_written`
/// emits nothing.
fn apply_rule(
    rule: &OmitRule,
    as_written: Option<&Targets>,
    removed: &mut HashSet<(FileId, JsonPointer)>,
    bundle: &mut InputBundle,
    diags: &mut Diagnostics,
) {
    let applied = match rule.resolve(bundle) {
        Err(invalid) => Err(invalid),
        Ok(RuleMatch::Exact { file, pointer }) => {
            match bundle.value_at_mut(file).remove_pointer(&pointer) {
                Some(value) => {
                    emit_omitted(diags, &pointer, value.span(), &rule.describe());
                    removed.insert((file, pointer));
                    Ok(())
                }
                None => Err("omit rule did not match any source construct".into()),
            }
        }
        Ok(RuleMatch::Glob { file, mut pointers }) => {
            // Remove deepest-first (and, within one array parent, highest-index-first) so a
            // removal never invalidates a still-pending child/sibling pointer. The order is total,
            // so the result does not depend on how `sort_by` treats ties.
            pointers.sort_by(removal_order);
            let describe = rule.describe();
            let mut removed_any = false;
            for pointer in pointers {
                if let Some(value) = bundle.value_at_mut(file).remove_pointer(&pointer) {
                    removed_any = true;
                    emit_omitted(diags, &pointer, value.span(), &describe);
                    removed.insert((file, pointer));
                }
            }
            if removed_any {
                Ok(())
            } else {
                Err("glob omit rule did not match any source construct".into())
            }
        }
    };
    if let Err(message) = applied {
        if !as_written.is_some_and(|targets| covered(targets, removed)) {
            emit_invalid_rule(rule, bundle, diags, message);
        }
    }
}

/// Emit the `W009` "construct omitted" warning for one removed construct.
fn emit_omitted(diags: &mut Diagnostics, pointer: &JsonPointer, span: Span, describe: &str) {
    Diagnostic::warning(
        Code::OmittedConstruct,
        Provenance::new(pointer.clone(), Some(span)),
    )
    .message(format!("omitted construct matched by `{describe}`"))
    .remedy(
        "the source schema was not modified; once spargen supports this construct, remove a \
         hand-written omit rule (auto-carve stops omitting it on its own)",
    )
    .emit(diags);
}

/// Deterministic, removal-safe total order: deepest pointers first, then grouped by parent
/// pointer, and within one parent array index leaves highest first, so removing one target never
/// shifts a still-pending sibling or child. Pointers of equal depth under different parents cannot
/// disturb each other, so grouping them by parent only has to be total, not meaningful. Leaves
/// that are not indices follow the index leaves and order by their text, and two leaves naming one
/// index differently (`1`, `01`) by their text too, so no two distinct pointers compare equal:
/// `sort_by` on anything weaker than a total order leaves the result unspecified, and may panic.
fn removal_order(a: &JsonPointer, b: &JsonPointer) -> Ordering {
    let depth = |pointer: &JsonPointer| pointer.as_str().matches('/').count();
    let (parent_a, leaf_a) = split_leaf(a);
    let (parent_b, leaf_b) = split_leaf(b);
    depth(b)
        .cmp(&depth(a))
        .then_with(|| parent_a.cmp(parent_b))
        .then_with(
            || match (leaf_a.parse::<usize>().ok(), leaf_b.parse::<usize>().ok()) {
                // Both leaves are array indices: remove the larger index first.
                (Some(ia), Some(ib)) => ib.cmp(&ia),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
        )
        .then_with(|| leaf_a.cmp(leaf_b))
}

/// The pointer's parent and its last (still escaped) reference token.
fn split_leaf(pointer: &JsonPointer) -> (&str, &str) {
    pointer
        .as_str()
        .rsplit_once('/')
        .unwrap_or(("", pointer.as_str()))
}

fn emit_invalid_rule(
    rule: &OmitRule,
    bundle: &InputBundle,
    diags: &mut Diagnostics,
    message: Cow<'static, str>,
) {
    Diagnostic::error(
        Code::InvalidOmitRule,
        Provenance::new(JsonPointer::root(), Some(bundle.root().span())),
    )
    .message(format!("{message}: {}", rule.describe()))
    .remedy("fix the rule so it names a construct below the root of a loaded file, or remove it")
    .emit(diags);
}

/// Check that what survives an omit profile is still a valid OpenAPI root.
///
/// This mirrors the official schemas, which `require` only `openapi` and `info` and then place
/// `paths`, `components`, and `webhooks` in an `anyOf` — a document carrying any one of the three
/// is valid, so a profile that leaves only `components` or `webhooks` is not `E020`.
fn validate_remaining(bundle: &InputBundle, diags: &mut Diagnostics) {
    let root = bundle.root();
    let reject = |message: String, diags: &mut Diagnostics| {
        // E020 case: required-root-field, no-root-member
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

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::glob::{glob_match, unescape_glob};
    use super::{removal_order, ComponentKind, JsonPointer, Omit, OmitMethod, OmitRule};
    use crate::diag::{Code, Diagnostics};
    use crate::source::InputBundle;
    use proptest::prelude::*;

    /// Exact and glob rules share one escaping rule: a backslash escapes whatever follows it, so
    /// `\b` is the literal `b` in both forms: the exact rule `/a\b` names the path `/ab`, as the
    /// glob `/a\b*` matches `/abc`.
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
            keep: Vec::new(),
        };
        omit.apply(&mut bundle, &mut diags).unwrap();
        assert_eq!(keys_under(&bundle, "paths", None), vec![r"/a\b".to_owned()]);
    }

    proptest! {
        /// `removal_order` is a total order over mixed index and key pointers of shared and
        /// distinct parents: antisymmetric, equal only for one pointer, and transitive.
        #[test]
        fn the_removal_order_is_total(
            [a, b, c] in [(); 3].map(|()| {
                prop::collection::vec(
                    prop::sample::select(&["0", "1", "2", "9", "10", "01", "k", "x"][..]),
                    1..4,
                )
            }),
        ) {
            let pointer = |tokens: &[&str]| {
                tokens.iter().fold(JsonPointer::root(), |pointer, token| pointer.push(token))
            };
            let (a, b, c) = (pointer(&a), pointer(&b), pointer(&c));
            prop_assert_eq!(removal_order(&a, &b), removal_order(&b, &a).reverse());
            prop_assert_eq!(removal_order(&a, &b).is_eq(), a == b);
            if removal_order(&a, &b).is_le() && removal_order(&b, &c).is_le() {
                prop_assert!(removal_order(&a, &c).is_le(), "{a:?} {b:?} {c:?}");
            }
        }
    }

    /// Load an inline YAML spec into an [`InputBundle`] via a tempfile (the loader reads from disk).
    pub(super) fn bundle_of(spec: &str) -> InputBundle {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("openapi.yaml");
        std::fs::write(&path, spec).unwrap();
        let mut diags = Diagnostics::default();
        InputBundle::load(camino::Utf8Path::from_path(&path).unwrap(), &mut diags).unwrap()
    }

    pub(super) const MULTI_SPEC: &str = r#"
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
            keep: Vec::new(),
        };
        omit.apply(&mut bundle, &mut diags).unwrap();
        let paths = keys_under(&bundle, "paths", None);
        assert_eq!(
            paths,
            vec!["/public/health".to_owned()],
            "both admin paths gone"
        );
        // One W009 per removed construct, none silent.
        assert_eq!(omitted_count(&diags), 2, "one W009 per removed path");
    }

    #[test]
    fn glob_component_rule_removes_multiple_components() {
        let mut bundle = bundle_of(MULTI_SPEC);
        let mut diags = Diagnostics::default();
        let omit = Omit {
            rules: vec![OmitRule::component(ComponentKind::Schemas, "Legacy*")],
            keep: Vec::new(),
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
            keep: Vec::new(),
        };
        omit.apply(&mut bundle, &mut diags).unwrap();
        let paths = keys_under(&bundle, "paths", None);
        assert_eq!(
            paths,
            vec!["/admin/users/{id}".to_owned(), "/public/health".to_owned()],
            "only the exact path is gone"
        );
        assert_eq!(omitted_count(&diags), 1);
    }

    #[test]
    fn glob_operation_rule_removes_matching_operations_only() {
        let mut bundle = bundle_of(MULTI_SPEC);
        let mut diags = Diagnostics::default();
        let omit = Omit {
            rules: vec![OmitRule::operation(OmitMethod::Get, "/admin/**")],
            keep: Vec::new(),
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
            keep: Vec::new(),
        };
        assert!(omit.apply(&mut bundle, &mut diags).is_err());
        assert!(!invalid_rule_messages(&diags).is_empty());
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
            keep: Vec::new(),
        };
        assert!(omit.apply(&mut bundle, &mut diags).is_err());
        let invalid = invalid_rule_messages(&diags);
        assert_eq!(invalid.len(), 1, "{invalid:#?}");
        assert!(
            invalid[0].starts_with("omit rule did not match"),
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
                keep: Vec::new(),
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

    /// A rule repeated verbatim, from two sources or twice from one, removes its construct once
    /// and is not an `E019` the second time.
    #[test]
    fn a_duplicate_rule_is_applied_once() {
        for rule in [
            OmitRule::path("/admin/users"),
            OmitRule::path("/admin/**"),
            OmitRule::operation(OmitMethod::Get, "/public/health"),
            pointer_rule(None, "/components/schemas/Legacy*"),
        ] {
            let mut bundle = bundle_of(MULTI_SPEC);
            let mut diags = Diagnostics::default();
            let once = {
                let mut bundle = bundle_of(MULTI_SPEC);
                let mut diags = Diagnostics::default();
                omit_of(vec![rule.clone()])
                    .apply(&mut bundle, &mut diags)
                    .unwrap();
                omitted_count(&diags)
            };
            omit_of(vec![rule.clone(), rule.clone()])
                .apply(&mut bundle, &mut diags)
                .unwrap_or_else(|_| panic!("{rule:?}: {:#?}", diags.items()));
            assert!(invalid_rule_messages(&diags).is_empty(), "{rule:?}");
            assert_eq!(omitted_count(&diags), once, "{rule:?}");
        }
    }

    /// An exact rule naming a construct a glob before it already removed, or one beneath it, is
    /// satisfied: it removes nothing further, emits nothing, and is not an `E019`.
    #[test]
    fn an_exact_rule_a_glob_before_it_covered_is_satisfied() {
        for covered in [
            OmitRule::operation(OmitMethod::Get, "/admin/users"),
            OmitRule::path("/admin/users/{id}"),
            pointer_rule(None, "/paths/~1admin~1users/get/responses"),
        ] {
            let mut bundle = bundle_of(MULTI_SPEC);
            let mut diags = Diagnostics::default();
            omit_of(vec![OmitRule::path("/admin/**"), covered.clone()])
                .apply(&mut bundle, &mut diags)
                .unwrap_or_else(|_| panic!("{covered:?}: {:#?}", diags.items()));
            assert!(invalid_rule_messages(&diags).is_empty(), "{covered:?}");
            assert_eq!(omitted_count(&diags), 2, "{covered:?}");
            assert_eq!(keys_under(&bundle, "paths", None), ["/public/health"]);
        }
    }

    /// A glob whose every match an earlier rule removed (an exact rule for each, or a broader
    /// glob) is satisfied, not an `E019`.
    #[test]
    fn a_glob_whose_matches_earlier_rules_removed_is_satisfied() {
        for (earlier, glob, removed) in [
            (
                vec![
                    OmitRule::path("/admin/users"),
                    OmitRule::path("/admin/users/{id}"),
                ],
                OmitRule::path("/admin/**"),
                2,
            ),
            (
                vec![OmitRule::path("/admin/**")],
                OmitRule::operation(OmitMethod::Get, "/admin/**"),
                2,
            ),
            (
                vec![OmitRule::component(ComponentKind::Schemas, "*")],
                OmitRule::component(ComponentKind::Schemas, "Legacy*"),
                3,
            ),
        ] {
            let mut rules = earlier.clone();
            rules.push(glob.clone());
            let mut bundle = bundle_of(MULTI_SPEC);
            let mut diags = Diagnostics::default();
            omit_of(rules)
                .apply(&mut bundle, &mut diags)
                .unwrap_or_else(|_| panic!("{glob:?}: {:#?}", diags.items()));
            assert!(invalid_rule_messages(&diags).is_empty(), "{glob:?}");
            assert_eq!(omitted_count(&diags), removed, "{glob:?}");
        }
    }

    /// Coverage is judged against the document as written: a rule naming a construct the
    /// document never declared stays an `E019` however much earlier rules removed, and so does a
    /// glob that matched nothing to begin with.
    #[test]
    fn a_rule_naming_nothing_as_written_is_still_e019_after_other_rules() {
        for missing in [
            OmitRule::path("/admin/nope"),
            OmitRule::operation(OmitMethod::Post, "/admin/users"),
            OmitRule::path("/nope/**"),
        ] {
            let mut bundle = bundle_of(MULTI_SPEC);
            let mut diags = Diagnostics::default();
            assert!(
                omit_of(vec![OmitRule::path("/admin/**"), missing.clone()])
                    .apply(&mut bundle, &mut diags)
                    .is_err(),
                "{missing:?}"
            );
            assert_eq!(invalid_rule_messages(&diags).len(), 1, "{missing:?}");
        }
    }

    /// The messages of every `E019` in `diags`, in emission order.
    fn invalid_rule_messages(diags: &Diagnostics) -> Vec<&str> {
        diags
            .items()
            .iter()
            .filter(|d| d.code == Code::InvalidOmitRule)
            .map(|d| d.message.as_str())
            .collect()
    }

    /// How many `W009`s — removed constructs — `diags` carries.
    fn omitted_count(diags: &Diagnostics) -> usize {
        diags
            .items()
            .iter()
            .filter(|d| d.code == Code::OmittedConstruct)
            .count()
    }

    /// A pointer rule with a glob metacharacter resolves through its own arm (every node pointer
    /// of the file, filtered by the pattern), not the path/operation/component ones above.
    #[test]
    fn glob_pointer_rule_removes_every_matching_node() {
        let mut bundle = bundle_of(MULTI_SPEC);
        let mut diags = Diagnostics::default();
        let omit = omit_of(vec![pointer_rule(None, "/components/schemas/Legacy*")]);
        omit.apply(&mut bundle, &mut diags).unwrap();
        assert_eq!(
            keys_under(&bundle, "components", Some("schemas")),
            vec!["Pet".to_owned()],
            "both Legacy* schemas gone"
        );
        assert_eq!(omitted_count(&diags), 2, "{:#?}", diags.items());
        assert!(invalid_rule_messages(&diags).is_empty());
    }

    /// A glob pointer whose matches nest — each schema and the `type` member inside it — removes
    /// every one of them, children before parents, with one `W009` each. This is why the glob
    /// arm's `!removed_any` E019 cannot fire once the match list is non-empty: the first pointer in
    /// removal order is a deepest one, nothing has been removed before it, and it was read off the
    /// very value it is removed from. The empty-match E019 above it is the reachable one
    /// (`glob_rule_matching_nothing_is_e019`).
    #[test]
    fn a_glob_pointer_matching_nested_nodes_removes_each_once() {
        let mut bundle = bundle_of(MULTI_SPEC);
        let mut diags = Diagnostics::default();
        let omit = omit_of(vec![pointer_rule(None, "/components/schemas/**")]);
        omit.apply(&mut bundle, &mut diags).unwrap();
        assert!(keys_under(&bundle, "components", Some("schemas")).is_empty());
        // Three schemas and their three `type` members.
        assert_eq!(omitted_count(&diags), 6, "{:#?}", diags.items());
        assert!(invalid_rule_messages(&diags).is_empty());
    }

    /// A spec whose root carries two 25-item arrays around a 25-key object: more same-depth glob
    /// matches than the standard library's small-sort handles, so the removal order the glob arm
    /// sorts them into is the comparator's alone.
    fn long_arrays_spec() -> String {
        let items = |prefix: &str| {
            (0..25)
                .map(|index| format!("{prefix}{index}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let object = (0..25)
            .map(|index| format!("k{index}: v{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "openapi: 3.1.0\ninfo: {{ title: t, version: 1.0.0 }}\npaths: {{}}\n\
             x-list-a: [{}]\nx-obj: {{ {object} }}\nx-list-b: [{}]\n",
            items("a"),
            items("b"),
        )
    }

    /// The string items of the root's `key` array, in order.
    fn root_strings(bundle: &InputBundle, key: &str) -> Vec<String> {
        bundle
            .root()
            .get(key)
            .and_then(|value| value.as_array())
            .unwrap_or_else(|| panic!("`{key}` is not an array"))
            .iter()
            .map(|item| item.as_str().unwrap().to_owned())
            .collect()
    }

    /// Every element a glob pointer matches across arrays longer than the small-sort threshold is
    /// removed, each with its own `W009` naming it, and nothing else is.
    #[test]
    fn a_glob_pointer_over_long_arrays_removes_exactly_its_matches() {
        let mut bundle = bundle_of(&long_arrays_spec());
        let mut diags = Diagnostics::default();
        omit_of(vec![pointer_rule(None, "/x-*/*")])
            .apply(&mut bundle, &mut diags)
            .unwrap();
        assert_eq!(omitted_count(&diags), 75, "{:#?}", diags.items());
        assert!(invalid_rule_messages(&diags).is_empty());
        assert!(root_strings(&bundle, "x-list-a").is_empty());
        assert!(root_strings(&bundle, "x-list-b").is_empty());
        assert!(keys_under(&bundle, "x-obj", None).is_empty());

        // A pattern matching a subset: indices `10`..`19` and keys `k10`..`k19` only. The survivors
        // are exactly the unmatched elements, in their original order, and each `W009` names a
        // pointer the rule matched.
        let mut bundle = bundle_of(&long_arrays_spec());
        let mut diags = Diagnostics::default();
        omit_of(vec![pointer_rule(None, "/x-*/*1?")])
            .apply(&mut bundle, &mut diags)
            .unwrap();
        let surviving = |prefix: &str| {
            (0..25)
                .filter(|index| !(10..20).contains(index))
                .map(|index| format!("{prefix}{index}"))
                .collect::<Vec<_>>()
        };
        assert_eq!(root_strings(&bundle, "x-list-a"), surviving("a"));
        assert_eq!(root_strings(&bundle, "x-list-b"), surviving("b"));
        assert_eq!(keys_under(&bundle, "x-obj", None), surviving("k"));
        let mut named = diags
            .items()
            .iter()
            .filter(|d| d.code == Code::OmittedConstruct)
            .map(|d| d.pointer.as_str().to_owned())
            .collect::<Vec<_>>();
        named.sort();
        let mut expected = ["/x-list-a/", "/x-obj/k", "/x-list-b/"]
            .iter()
            .flat_map(|base| (10..20).map(move |index| format!("{base}{index}")))
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(named, expected);
        assert!(invalid_rule_messages(&diags).is_empty());
    }

    /// A rule's pointer is looked up as a plain RFC 6901 pointer, decoded once: a path holding a
    /// literal `%` is removed by an exact path rule, and a pointer rule still reads a
    /// percent-encoded token (`%20`) as the character it encodes.
    #[test]
    fn a_literal_percent_and_a_percent_encoded_pointer_token_each_remove_their_node() {
        let spec = r#"
openapi: 3.1.0
info: { title: t, version: 1.0.0 }
paths:
  /100%:
    get: { responses: { "200": { description: ok } } }
  /a b:
    get: { responses: { "200": { description: ok } } }
  /kept:
    get: { responses: { "200": { description: ok } } }
"#;
        let mut bundle = bundle_of(spec);
        let mut diags = Diagnostics::default();
        let omit = omit_of(vec![
            OmitRule::path("/100%"),
            pointer_rule(None, "/paths/~1a%20b"),
        ]);
        omit.apply(&mut bundle, &mut diags).unwrap();
        assert_eq!(keys_under(&bundle, "paths", None), vec!["/kept".to_owned()]);
        assert_eq!(omitted_count(&diags), 2, "{:#?}", diags.items());
    }

    /// The empty pointer names the whole document, which no rule may remove.
    #[test]
    fn an_empty_pointer_rule_is_e019() {
        let mut bundle = bundle_of(MULTI_SPEC);
        let mut diags = Diagnostics::default();
        let omit = omit_of(vec![pointer_rule(None, "")]);
        assert!(omit.apply(&mut bundle, &mut diags).is_err());
        assert_eq!(
            invalid_rule_messages(&diags),
            ["omit rules cannot remove the document root: pointer "],
            "{:#?}",
            diags.items()
        );
        assert_eq!(omitted_count(&diags), 0);
        assert!(bundle.root().get("openapi").is_some());
    }

    /// A file-scoped pointer rule naming a file the bundle never loaded is E019 rather than a
    /// silent no-op, and the bundle is left as it was.
    #[test]
    fn a_pointer_rule_into_an_unloaded_file_is_e019() {
        let mut bundle = bundle_of(MULTI_SPEC);
        let mut diags = Diagnostics::default();
        let omit = omit_of(vec![pointer_rule(
            Some("missing.yaml"),
            "/components/schemas/Pet",
        )]);
        assert!(omit.apply(&mut bundle, &mut diags).is_err());
        assert_eq!(
            invalid_rule_messages(&diags),
            ["omit rule references an unloaded file: pointer missing.yaml#/components/schemas/Pet"],
            "{:#?}",
            diags.items()
        );
        assert_eq!(omitted_count(&diags), 0);
        assert_eq!(
            keys_under(&bundle, "components", Some("schemas")),
            ["LegacyPet", "LegacyOwner", "Pet"]
        );
    }

    fn omit_of(rules: Vec<OmitRule>) -> Omit {
        Omit {
            rules,
            keep: Vec::new(),
        }
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
