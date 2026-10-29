//! Auto-carve: the pure mapping from error diagnostics to the omit rules that carve the smallest
//! enclosing omittable constructs out of a document. The fixpoint driver lives in the facade.

use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::HashSet;

use super::glob::escape_glob_meta;
use super::{ComponentKind, OmitMethod, OmitRule};
use crate::diag::{escape_token, Diagnostic, FileId, JsonPointer, Severity};
use crate::source::{InputBundle, Node, SpannedValue};

/// The maximum number of carve rounds. Each round adds at least one omit rule (or stops), and a
/// spec has finitely many constructs, so any spec terminates; this cap is a belt-and-suspenders
/// bound that also keeps a pathological ref cascade from re-parsing without end.
pub(crate) const MAX_CARVE_ROUNDS: usize = 64;

/// Map error diagnostics to the smallest enclosing **omittable** construct, returning the omit
/// rules that would carve those constructs out of the document.
///
/// Pointer → construct mapping (per the auto-carve contract):
///
/// * `/paths/<path>/<method>/…` → omit that **operation** (`method` + `path`);
/// * `/paths/<path>/…` (path-item level, not into a method) → omit the **path**;
/// * `/components/<kind>/<name>/…` (for a kind spargen models) → omit that **component**.
///
/// A pointer that encloses no omittable construct (the document root, an unmodelled component kind,
/// a `$ref`-target site outside `paths`/`components`, …) yields no rule — the facade reports it as a
/// residual, un-carvable rejection rather than looping. The returned rules are de-duplicated and
/// sorted deterministically, so the carve set is stable for a given set of diagnostics.
///
/// A diagnostic is read in the file its span lies in (the root document when it has no span),
/// because its pointer addresses that file:
///
/// * in the **root document**, the construct is named by path, operation, or component, as above;
/// * in a **referenced sub-file**, the same constructs are carved as a file-scoped
///   [`OmitRule::Pointer`] (`lib.yaml#/components/schemas/Node`), since a path- or component-named
///   rule is read against the root document and would match nothing there (`E019`). Removing it
///   dangles the `$ref`s that reached it, and the next round carves those;
/// * a sub-file pointer outside `paths`/`components` — a bare schema or path item file — is
///   carved through each `$ref` site in the bundle whose target encloses it. Where that site sits
///   where a Path Item belongs and the pointer lies in one of the target's methods, the file does
///   hold an omittable construct, that operation, and it is carved alone as a file-scoped pointer
///   (`pi.yaml#/get`). Otherwise (a schema file, or a path item's own `parameters`) nothing in the
///   file can be omitted without reshaping it, and the site itself is carved, followed back file
///   by file until each reaches a construct.
///
/// `bundle` is the one the diagnostics were produced from, with the current omit profile already
/// applied, so every rule derived here names a construct that is still present.
pub(crate) fn carve_rules(diagnostics: &[Diagnostic], bundle: &InputBundle) -> Vec<OmitRule> {
    let mut carver = Carver {
        bundle,
        sites: OnceCell::new(),
        visited: HashSet::new(),
        rules: Vec::new(),
    };
    for diagnostic in diagnostics {
        if diagnostic.severity != Severity::Error {
            continue;
        }
        let file = diagnostic
            .span
            .map_or_else(|| bundle.root_id(), |span| span.file);
        carver.carve_at(file, &diagnostic.pointer);
    }
    let mut rules = carver.rules;
    // Deterministic carve set: order is independent of diagnostic order.
    rules.sort_by_key(|rule| rule.describe());
    rules
}

/// The state of one [`carve_rules`] call.
struct Carver<'a> {
    bundle: &'a InputBundle,
    /// Every `$ref` in the bundle, walked only once a sub-file pointer needs its referrers.
    sites: OnceCell<Vec<ReferenceSite>>,
    /// Sub-file pointers already expanded, which is what ends a `$ref` cycle between sub-files.
    visited: HashSet<(FileId, String)>,
    rules: Vec<OmitRule>,
}

impl Carver<'_> {
    /// Derive the rule(s) that carve `pointer` in `file`; see [`carve_rules`].
    fn carve_at(&mut self, file: FileId, pointer: &JsonPointer) {
        let bundle = self.bundle;
        if file == bundle.root_id() {
            // The root document's un-carvable pointers stay residual: the root is the API itself,
            // so there is nothing above it to carve instead.
            if let Some(rule) = omittable_enclosing(pointer) {
                self.push(rule);
            }
            return;
        }
        if !self.visited.insert((file, pointer.as_str().to_owned())) {
            return;
        }
        let tokens = pointer.tokens();
        if let Some(rule) = file_scoped_enclosing(bundle, file, &tokens) {
            self.push(rule);
            return;
        }
        let referrers: Vec<(FileId, JsonPointer, usize, bool)> = self
            .sites
            .get_or_init(|| reference_sites(bundle))
            .iter()
            .filter(|site| site.target_file == file && is_prefix(&site.target_tokens, &tokens))
            .map(|site| {
                (
                    site.file,
                    site.pointer.clone(),
                    site.target_tokens.len(),
                    is_path_item_position(&site.pointer.tokens()),
                )
            })
            .collect();
        for (referrer, at, target_depth, path_item) in referrers {
            // A `$ref` where a Path Item belongs makes its target a path item, whose methods are
            // operations of their own: carve the rejected one alone, in this file, rather than the
            // whole path the `$ref` sits at.
            if path_item {
                if let Some(depth) = operation_depth(&tokens[target_depth..]) {
                    if let Some(rule) =
                        file_scoped_pointer(bundle, file, &tokens[..target_depth + depth])
                    {
                        self.push(rule);
                        continue;
                    }
                }
            }
            self.carve_at(referrer, &at);
        }
    }

    fn push(&mut self, rule: OmitRule) {
        if !self.rules.contains(&rule) {
            self.rules.push(rule);
        }
    }
}

/// One `$ref` in the bundle: the object that carries it, and the construct its target resolves to.
struct ReferenceSite {
    file: FileId,
    pointer: JsonPointer,
    target_file: FileId,
    target_tokens: Vec<String>,
}

/// Every `$ref` in every loaded document that resolves to a loaded document, in load and document
/// order.
fn reference_sites(bundle: &InputBundle) -> Vec<ReferenceSite> {
    let mut sites = Vec::new();
    for file in bundle.file_ids() {
        collect_reference_sites(
            bundle,
            file,
            bundle.value_at(file),
            &JsonPointer::root(),
            &mut sites,
        );
    }
    sites
}

fn collect_reference_sites(
    bundle: &InputBundle,
    file: FileId,
    value: &SpannedValue,
    at: &JsonPointer,
    sites: &mut Vec<ReferenceSite>,
) {
    match &value.node {
        Node::Object(object) => {
            if let Some(target) = value
                .get("$ref")
                .and_then(SpannedValue::as_str)
                .and_then(|reference| bundle.reference_target(reference, file))
            {
                sites.push(ReferenceSite {
                    file,
                    pointer: at.clone(),
                    target_file: target.0,
                    target_tokens: target.1.tokens(),
                });
            }
            for (key, child) in object.iter() {
                collect_reference_sites(bundle, file, child, &at.push(&key.name), sites);
            }
        }
        Node::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_reference_sites(bundle, file, child, &at.index(index), sites);
            }
        }
        _ => {}
    }
}

/// Whether `prefix` is a token-wise prefix of (or equal to) `tokens`.
fn is_prefix(prefix: &[String], tokens: &[String]) -> bool {
    tokens.len() >= prefix.len() && tokens[..prefix.len()] == *prefix
}

/// An omittable construct a pointer lies in: how many of its leading tokens name the construct,
/// and the root-document rule that omits it.
struct Enclosing {
    depth: usize,
    rule: OmitRule,
}

/// The smallest omittable construct enclosing a root-document `pointer`, as the rule that omits
/// it, or `None` if none is (root / unmodelled).
pub(super) fn omittable_enclosing(pointer: &JsonPointer) -> Option<OmitRule> {
    enclosing(&pointer.tokens()).map(|enclosing| enclosing.rule)
}

/// The same construct as [`omittable_enclosing`], found in the sub-file `file` and named by a
/// file-scoped pointer to it, which is how an omit rule reaches a construct outside the root.
fn file_scoped_enclosing(
    bundle: &InputBundle,
    file: FileId,
    tokens: &[String],
) -> Option<OmitRule> {
    let enclosing = enclosing(tokens)?;
    file_scoped_pointer(bundle, file, &tokens[..enclosing.depth])
}

/// Whether a `$ref` at `tokens` sits where a Path Item belongs — a `paths` entry, a
/// `components.pathItems` entry, or a webhook — so that whatever it resolves to is read as one.
fn is_path_item_position(tokens: &[String]) -> bool {
    match tokens {
        [collection, _] => collection == "paths" || collection == "webhooks",
        [components, kind, _] => components == "components" && kind == "pathItems",
        _ => false,
    }
}

/// How many leading tokens of a pointer relative to a Path Item name one of its operations — a
/// fixed-field method, or an OpenAPI 3.2 `additionalOperations` method — mirroring [`enclosing`]'s
/// mapping below `/paths/<path>`; `None` for a path-item-level pointer.
fn operation_depth(tokens: &[String]) -> Option<usize> {
    match tokens {
        [additional, _, ..] if additional == "additionalOperations" => Some(2),
        [method, ..] if method.parse::<OmitMethod>().is_ok() => Some(1),
        _ => None,
    }
}

/// The file-scoped pointer rule that omits exactly the construct `tokens` names in `file`.
fn file_scoped_pointer(bundle: &InputBundle, file: FileId, tokens: &[String]) -> Option<OmitRule> {
    let path = bundle.root_relative_path(file)?;
    let pointer: String = tokens
        .iter()
        .map(|token| format!("/{}", escape_glob_meta(&escape_token(token))))
        .collect();
    // The file is matched as a path, never as a glob, so it is carried verbatim; only the pointer
    // is glob-escaped.
    Some(OmitRule::pointer(
        Some(Cow::Owned(path.to_owned())),
        pointer,
    ))
}

/// Classify the construct `tokens` lies in; see [`omittable_enclosing`].
///
/// Every path and component name here is literal text lifted out of the document, so its glob
/// metacharacters are escaped: a path such as `/files/*` is a legal URI path, and an unescaped rule
/// for it would be reinterpreted as a bulk pattern and carve away its supported siblings too.
fn enclosing(tokens: &[String]) -> Option<Enclosing> {
    match tokens.first()?.as_str() {
        "paths" => {
            let path = escape_glob_meta(tokens.get(1)?);
            // An OpenAPI 3.2 `additionalOperations` method is not a Path Item fixed field, so no
            // `OmitMethod` names it. Carving it as a pointer keeps the blast radius at the one
            // operation; falling through to the path rule would take its supported siblings too.
            if tokens
                .get(2)
                .is_some_and(|token| token == "additionalOperations")
            {
                let method = tokens.get(3)?;
                return Some(Enclosing {
                    depth: 4,
                    rule: OmitRule::pointer(
                        None,
                        format!(
                            "/paths/{}/additionalOperations/{}",
                            escape_token(&path),
                            escape_glob_meta(&escape_token(method))
                        ),
                    ),
                });
            }
            Some(
                match tokens
                    .get(2)
                    .and_then(|token| token.parse::<OmitMethod>().ok())
                {
                    Some(method) => Enclosing {
                        depth: 3,
                        rule: OmitRule::operation(method, path),
                    },
                    None => Enclosing {
                        depth: 2,
                        rule: OmitRule::path(path),
                    },
                },
            )
        }
        "components" => {
            let kind = tokens.get(1)?.parse::<ComponentKind>().ok()?;
            Some(Enclosing {
                depth: 3,
                rule: OmitRule::component(kind, escape_glob_meta(tokens.get(2)?)),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{carve_rules, is_path_item_position, is_prefix, operation_depth};
    use crate::compat::tests::{bundle_of_files, error_in};
    use crate::compat::{OmitMethod, OmitRule};

    fn tokens(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|&token| token.to_owned()).collect()
    }

    /// A root document whose `paths` block is `paths`, indented as a mapping under it.
    fn root_with_paths(paths: &str) -> String {
        format!("openapi: 3.1.0\ninfo: {{ title: t, version: 1.0.0 }}\npaths:\n{paths}")
    }

    /// An operation whose `200` response body is the schema `$ref`.
    fn get_returning(reference: &str) -> String {
        format!(
            "    get:\n      responses:\n        \"200\":\n          description: ok\n          \
             content:\n            application/json: {{ schema: {{ $ref: '{reference}' }} }}\n"
        )
    }

    #[test]
    fn a_prefix_is_token_wise_and_never_longer_than_the_pointer() {
        let pointer = tokens(&["$defs", "One", "type"]);
        assert!(is_prefix(&[], &pointer));
        assert!(is_prefix(&tokens(&["$defs", "One"]), &pointer));
        assert!(is_prefix(&pointer, &pointer));
        // Same length as a true prefix, different token.
        assert!(!is_prefix(&tokens(&["$defs", "Two"]), &pointer));
        // Longer than the pointer, whose own tokens it starts with.
        assert!(!is_prefix(
            &tokens(&["$defs", "One", "type", "x"]),
            &pointer
        ));
    }

    #[test]
    fn a_path_item_position_is_a_paths_webhooks_or_path_items_entry() {
        assert!(is_path_item_position(&tokens(&["paths", "/x"])));
        assert!(is_path_item_position(&tokens(&["webhooks", "hook"])));
        assert!(is_path_item_position(&tokens(&[
            "components",
            "pathItems",
            "P"
        ])));
        assert!(!is_path_item_position(&tokens(&["paths"])));
        assert!(!is_path_item_position(&tokens(&["paths", "/x", "get"])));
        assert!(!is_path_item_position(&tokens(&["callbacks", "/x"])));
        assert!(!is_path_item_position(&tokens(&[
            "components",
            "schemas",
            "P"
        ])));
        assert!(!is_path_item_position(&tokens(&[
            "x-paths",
            "pathItems",
            "P"
        ])));
    }

    #[test]
    fn an_operation_depth_counts_the_tokens_naming_one_operation() {
        assert_eq!(
            operation_depth(&tokens(&["additionalOperations", "COPY", "responses"])),
            Some(2)
        );
        assert_eq!(operation_depth(&tokens(&["get", "responses"])), Some(1));
        assert_eq!(operation_depth(&tokens(&["get"])), Some(1));
        // `additionalOperations` with no method under it names no operation.
        assert_eq!(operation_depth(&tokens(&["additionalOperations"])), None);
        assert_eq!(operation_depth(&tokens(&["parameters", "0"])), None);
        assert_eq!(operation_depth(&[]), None);
    }

    /// Only a `$ref` into the rejected file, whose target encloses the rejected pointer, is
    /// followed back: neither the operation reaching another file's whole document nor the one
    /// reaching a sibling definition in the same file is carved.
    #[test]
    fn only_referrers_whose_target_encloses_the_pointer_are_carved() {
        let defs = "$defs:\n  One: { type: object }\n  Two: { type: object }\n";
        let other = "type: object\n";
        let root = root_with_paths(&format!(
            "  /one:\n{}  /two:\n{}  /other:\n{}",
            get_returning("./defs.yaml#/$defs/One"),
            get_returning("./defs.yaml#/$defs/Two"),
            get_returning("./other.yaml"),
        ));
        let bundle = bundle_of_files(&root, &[("defs.yaml", defs), ("other.yaml", other)]);
        let rules = carve_rules(
            &[error_in(&bundle, "defs.yaml", "/$defs/One/type")],
            &bundle,
        );
        assert_eq!(rules, vec![OmitRule::operation(OmitMethod::Get, "/one")]);
    }

    /// A `$ref` held in an array element is a referrer like any other.
    #[test]
    fn a_referrer_inside_an_array_is_followed_back() {
        let parameter = "name: q\nin: query\nschema: { type: string }\n";
        let root = root_with_paths(
            "  /x:\n    get:\n      parameters:\n        - $ref: './parameter.yaml'\n      \
             responses: { \"200\": { description: ok } }\n",
        );
        let bundle = bundle_of_files(&root, &[("parameter.yaml", parameter)]);
        let rules = carve_rules(&[error_in(&bundle, "parameter.yaml", "/schema")], &bundle);
        assert_eq!(rules, vec![OmitRule::operation(OmitMethod::Get, "/x")]);
    }

    /// A path item reached below its file's root is carved at the rejected operation: the
    /// pointer keeps the target's own tokens and then the method's.
    #[test]
    fn a_nested_path_item_target_carves_its_operation_below_the_target() {
        let items =
            "defs:\n  Item:\n    get: { responses: { \"200\": { description: ok } } }\n    \
                     post: { responses: { \"200\": { description: ok } } }\n";
        let root = root_with_paths("  /x: { $ref: './items.yaml#/defs/Item' }\n");
        let bundle = bundle_of_files(&root, &[("items.yaml", items)]);
        let rules = carve_rules(
            &[error_in(&bundle, "items.yaml", "/defs/Item/get/responses")],
            &bundle,
        );
        assert_eq!(
            rules,
            vec![OmitRule::pointer(
                Some("items.yaml".into()),
                "/defs/Item/get"
            )]
        );
    }
}
