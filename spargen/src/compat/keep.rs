//! Keep (allow-list) rules: which operations a profile's [`KeepRule`]s name, and narrowing the
//! document to them — every other operation, every webhook, and every root-document component the
//! remaining document no longer reaches is removed, each reported as a `W009`.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::glob::{has_glob_meta, unescape_glob, Glob};
use super::{KeepRule, OmitMethod};
use crate::diag::{Code, Diagnostic, Diagnostics, FileId, JsonPointer, Provenance};
use crate::source::{InputBundle, Node, SpannedValue};

/// The operations the keep rules name: each `paths` key of the root document, with the method keys
/// kept under it.
pub(super) type Kept = BTreeMap<String, BTreeSet<&'static str>>;

/// Resolve every keep rule against `bundle`, before any omit rule applies, so a rule naming an
/// operation an omit rule also removes is not a stale rule. A rule that names no declared
/// operation is an `E019`; `None` when any rule was.
pub(super) fn resolve(
    rules: &[KeepRule],
    bundle: &InputBundle,
    diags: &mut Diagnostics,
) -> Option<Kept> {
    let mut kept = Kept::new();
    let mut complete = true;
    let paths = bundle.root().get("paths").and_then(SpannedValue::as_object);
    for rule in rules {
        let KeepRule::Operation { method, path } = rule;
        let method_key = method.as_oas_key();
        let candidates: Vec<String> = match paths {
            None => Vec::new(),
            Some(paths) if has_glob_meta(path) => {
                let glob = Glob::compile(path);
                paths
                    .iter()
                    .map(|(key, _)| key.name.clone())
                    .filter(|key| glob.matches(key))
                    .collect()
            }
            Some(_) => vec![unescape_glob(path)],
        };
        let mut matched = false;
        for key in candidates {
            let declares = paths
                .and_then(|paths| paths.get(&key))
                .and_then(|item| path_item_target(bundle, item))
                .is_some_and(|item| item.get(method_key).is_some());
            if declares {
                matched = true;
                kept.entry(key).or_default().insert(method_key);
            }
        }
        if !matched {
            complete = false;
            let message = if has_glob_meta(path) {
                "glob keep rule did not match any declared operation"
            } else {
                "keep rule did not name a declared operation"
            };
            Diagnostic::error(
                Code::InvalidOmitRule,
                Provenance::new(JsonPointer::root(), Some(bundle.root().span())),
            )
            .message(format!("{message}: {}", rule.describe()))
            .remedy(
                "fix the rule so it names an operation the document declares under `paths`, or \
                 remove it",
            )
            .emit(diags);
        }
    }
    complete.then_some(kept)
}

/// The Path Item a `paths` entry stands for: the entry itself, or the item its `$ref` names.
fn path_item_target<'a>(
    bundle: &'a InputBundle,
    item: &'a SpannedValue,
) -> Option<&'a SpannedValue> {
    match item.get("$ref").and_then(SpannedValue::as_str) {
        Some(reference) => {
            let (file, pointer) = bundle.reference_target(reference, bundle.root_id())?;
            bundle.value_at(file).pointer(&pointer)
        }
        None => Some(item),
    }
}

/// Narrow `bundle` to the `kept` operations: remove every root `paths` entry none of whose
/// operations is kept, every operation of an inline entry that is not kept (and its
/// `additionalOperations`, which no keep rule names), the `webhooks`, and then every root-document
/// component nothing that remains reaches. A `paths` entry written as a `$ref` is kept whole when
/// one of its operations is, since its operations live in a target other uses may share.
pub(super) fn retain(kept: &Kept, bundle: &mut InputBundle, diags: &mut Diagnostics) {
    let root = bundle.root_id();
    let paths_pointer = JsonPointer::root().push("paths");
    let mut removals: Vec<JsonPointer> = Vec::new();
    if let Some(paths) = bundle.root().get("paths").and_then(SpannedValue::as_object) {
        for (key, item) in paths.iter() {
            let item_pointer = paths_pointer.push(&key.name);
            let Some(methods) = kept.get(&key.name) else {
                removals.push(item_pointer);
                continue;
            };
            if item.get("$ref").is_some() {
                continue;
            }
            for method in OmitMethod::CANONICAL {
                if item.get(method).is_some() && !methods.contains(method) {
                    removals.push(item_pointer.push(method));
                }
            }
            if item.get("additionalOperations").is_some() {
                removals.push(item_pointer.push("additionalOperations"));
            }
        }
    }
    if bundle.root().get("webhooks").is_some() {
        removals.push(JsonPointer::root().push("webhooks"));
    }
    remove_all(bundle, root, removals, diags);

    let reached = Reach::from_root(bundle);
    let mut unreached = Vec::new();
    if let Some(components) = bundle
        .root()
        .get("components")
        .and_then(SpannedValue::as_object)
    {
        for (kind, map) in components.iter() {
            let Some(map) = map.as_object() else { continue };
            for (name, _) in map.iter() {
                if !reached
                    .components
                    .contains(&(kind.name.clone(), name.name.clone()))
                {
                    unreached.push(
                        JsonPointer::root()
                            .push("components")
                            .push(&kind.name)
                            .push(&name.name),
                    );
                }
            }
        }
    }
    remove_all(bundle, root, unreached, diags);
}

/// Remove each pointer (none an ancestor of another) from `file`, in the order given, with a
/// `W009` for each.
fn remove_all(
    bundle: &mut InputBundle,
    file: FileId,
    pointers: Vec<JsonPointer>,
    diags: &mut Diagnostics,
) {
    for pointer in pointers {
        if let Some(value) = bundle.value_at_mut(file).remove_pointer(&pointer) {
            Diagnostic::warning(
                Code::OmittedConstruct,
                Provenance::new(pointer.clone(), Some(value.span())),
            )
            .message("omitted construct that no keep rule names or reaches")
            .remedy(
                "the source schema was not modified; add a keep rule for an operation to \
                 generate it and what it reaches",
            )
            .emit(diags);
        }
    }
}

/// The root-document components the document reaches from outside `components`: through `$ref`s
/// (followed across files), `discriminator` mapping values, and the scheme names of security
/// requirements, transitively through every reached component.
struct Reach<'a> {
    bundle: &'a InputBundle,
    /// `(kind, name)` of every reached root-document component.
    components: HashSet<(String, String)>,
    /// Every `(file, pointer)` already walked.
    walked: HashSet<(FileId, String)>,
    /// Nodes still to walk.
    queue: Vec<(FileId, JsonPointer)>,
}

impl<'a> Reach<'a> {
    fn from_root(bundle: &'a InputBundle) -> Self {
        let mut reach = Reach {
            bundle,
            components: HashSet::new(),
            walked: HashSet::new(),
            queue: Vec::new(),
        };
        let root = bundle.root_id();
        if let Some(object) = bundle.root().as_object() {
            for (key, value) in object.iter() {
                if key.name != "components" {
                    reach.walk(root, value);
                }
            }
        }
        while let Some((file, pointer)) = reach.queue.pop() {
            if let Some(value) = bundle.value_at(file).pointer(&pointer) {
                reach.walk(file, value);
            }
        }
        reach
    }

    /// Follow every reference written anywhere in `value`, which lies in `file`.
    fn walk(&mut self, file: FileId, value: &'a SpannedValue) {
        match &value.node {
            Node::Object(object) => {
                if let Some(reference) = value.get("$ref").and_then(SpannedValue::as_str) {
                    self.follow(reference, file);
                }
                if let Some(discriminator) = value.get("discriminator") {
                    let mapped = discriminator
                        .get("mapping")
                        .and_then(SpannedValue::as_object)
                        .into_iter()
                        .flat_map(|mapping| mapping.iter().map(|(_, target)| target))
                        .chain(discriminator.get("defaultMapping"))
                        .filter_map(SpannedValue::as_str);
                    for target in mapped {
                        // A value with no `#` or `/` may be a schema component's name rather than
                        // a reference; reach both readings.
                        if !target.contains('#') && !target.contains('/') {
                            self.reach_root_component("schemas", target);
                        }
                        self.follow(target, file);
                    }
                }
                if let Some(requirements) = value.get("security").and_then(SpannedValue::as_array) {
                    for requirement in requirements.iter().filter_map(SpannedValue::as_object) {
                        for (scheme, _) in requirement.iter() {
                            if scheme.name.contains('#') {
                                self.follow(&scheme.name, file);
                            } else {
                                self.reach_root_component("securitySchemes", &scheme.name);
                            }
                        }
                    }
                }
                for (_, child) in object.iter() {
                    self.walk(file, child);
                }
            }
            Node::Array(items) => {
                for item in items {
                    self.walk(file, item);
                }
            }
            _ => {}
        }
    }

    /// Reach what `reference`, written in `from`, resolves to. A target inside a component reaches
    /// the whole component, since a component is kept or removed whole. A schema component named
    /// from a sub-file also reaches the root's schema of that name, which a sub-file's
    /// `#/components/schemas/<name>` reads first.
    fn follow(&mut self, reference: &str, from: FileId) {
        let Some((file, pointer)) = self.bundle.reference_target(reference, from) else {
            return;
        };
        let tokens = pointer.tokens();
        match tokens.as_slice() {
            [components, kind, name, ..] if components == "components" => {
                if file == self.bundle.root_id() {
                    self.reach_root_component(kind, name);
                } else {
                    if kind == "schemas" {
                        self.reach_root_component(kind, name);
                    }
                    self.enqueue(
                        file,
                        JsonPointer::root().push(components).push(kind).push(name),
                    );
                }
            }
            _ => self.enqueue(file, pointer),
        }
    }

    fn reach_root_component(&mut self, kind: &str, name: &str) {
        if self.components.insert((kind.to_owned(), name.to_owned())) {
            let pointer = JsonPointer::root().push("components").push(kind).push(name);
            self.enqueue(self.bundle.root_id(), pointer);
        }
    }

    fn enqueue(&mut self, file: FileId, pointer: JsonPointer) {
        if self.walked.insert((file, pointer.as_str().to_owned())) {
            self.queue.push((file, pointer));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::bundle_of;
    use super::super::{KeepRule, Omit, OmitMethod, OmitRule};
    use crate::diag::{Code, Diagnostics, JsonPointer};
    use crate::source::InputBundle;

    /// Three operations on two paths, a webhook, and components reached by a `$ref` chain, a
    /// discriminator's bare-name and `$ref` mapping values, a security requirement, and a path
    /// item `$ref`, beside components nothing the kept operation names reaches.
    const SPEC: &str = r##"
openapi: 3.1.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    parameters:
      - $ref: "#/components/parameters/Trace"
    get:
      security: [{ key: [] }]
      responses:
        "200":
          description: ok
          content:
            application/json: { schema: { $ref: "#/components/schemas/Pet" } }
    post:
      requestBody: { $ref: "#/components/requestBodies/NewPet" }
      responses: { "204": { description: ok } }
  /owners:
    get:
      responses:
        "200":
          description: ok
          content:
            application/json: { schema: { $ref: "#/components/schemas/Owner" } }
  /shared:
    $ref: "#/components/pathItems/Shared"
webhooks:
  petAdded:
    post: { responses: { "204": { description: ok } } }
components:
  parameters:
    Trace: { name: trace, in: header, schema: { type: string } }
  requestBodies:
    NewPet:
      content:
        application/json: { schema: { $ref: "#/components/schemas/NewPet" } }
  pathItems:
    Shared:
      get: { responses: { "204": { description: ok } } }
      put: { responses: { "204": { description: ok } } }
  securitySchemes:
    key: { type: apiKey, in: header, name: X-Key }
    unused: { type: apiKey, in: header, name: X-Other }
  schemas:
    Pet:
      oneOf: [{ $ref: "#/components/schemas/Cat" }, { $ref: "#/components/schemas/Dog" }]
      discriminator:
        propertyName: kind
        mapping: { cat: Cat, dog: "#/components/schemas/Dog" }
    Cat: { type: object, properties: { tag: { $ref: "#/components/schemas/Tag" } } }
    Dog: { type: object }
    Tag: { type: string }
    Owner: { type: object }
    NewPet: { type: object }
"##;

    fn keys(bundle: &InputBundle, pointer: &str) -> Vec<String> {
        bundle
            .root()
            .pointer(&JsonPointer::from(pointer.to_owned()))
            .and_then(|value| value.as_object())
            .map(|object| object.iter().map(|(key, _)| key.name.clone()).collect())
            .unwrap_or_default()
    }

    fn apply(omit: &Omit) -> (InputBundle, Diagnostics) {
        let mut bundle = bundle_of(SPEC);
        let mut diags = Diagnostics::default();
        let _ = omit.apply(&mut bundle, &mut diags);
        (bundle, diags)
    }

    fn keep(rules: Vec<KeepRule>) -> Omit {
        Omit {
            rules: Vec::new(),
            keep: rules,
        }
    }

    /// The pointer of every `W009`, in emission order.
    fn omitted(diags: &Diagnostics) -> Vec<&str> {
        diags
            .items()
            .iter()
            .filter(|diagnostic| diagnostic.code == Code::OmittedConstruct)
            .map(|diagnostic| diagnostic.pointer.as_str())
            .collect()
    }

    /// Keeping one operation removes every other operation, path and webhook, and exactly the
    /// components nothing that remains reaches, each with its own `W009`.
    #[test]
    fn keeping_one_operation_carves_the_document_to_what_it_reaches() {
        let (bundle, diags) = apply(&keep(vec![KeepRule::operation(OmitMethod::Get, "/pets")]));
        assert!(!diags.has_errors(), "{:?}", diags.items());
        assert_eq!(keys(&bundle, "/paths"), ["/pets"]);
        assert_eq!(keys(&bundle, "/paths/~1pets"), ["parameters", "get"]);
        assert!(bundle.root().get("webhooks").is_none());
        assert_eq!(
            keys(&bundle, "/components/schemas"),
            ["Pet", "Cat", "Dog", "Tag"],
            "the `$ref` chain and both discriminator mapping spellings"
        );
        assert_eq!(keys(&bundle, "/components/parameters"), ["Trace"]);
        assert_eq!(keys(&bundle, "/components/securitySchemes"), ["key"]);
        assert!(keys(&bundle, "/components/requestBodies").is_empty());
        assert!(keys(&bundle, "/components/pathItems").is_empty());
        assert_eq!(
            omitted(&diags),
            [
                "/paths/~1pets/post",
                "/paths/~1owners",
                "/paths/~1shared",
                "/webhooks",
                "/components/requestBodies/NewPet",
                "/components/pathItems/Shared",
                "/components/securitySchemes/unused",
                "/components/schemas/Owner",
                "/components/schemas/NewPet",
            ]
        );
    }

    /// A `paths` entry written as a `$ref` is kept whole once one of its target's operations is
    /// kept, and the target stays reached.
    #[test]
    fn a_kept_operation_of_a_referenced_path_item_keeps_the_item_whole() {
        let (bundle, diags) = apply(&keep(vec![KeepRule::operation(OmitMethod::Put, "/shared")]));
        assert!(!diags.has_errors(), "{:?}", diags.items());
        assert_eq!(keys(&bundle, "/paths"), ["/shared"]);
        assert_eq!(
            keys(&bundle, "/components/pathItems/Shared"),
            ["get", "put"]
        );
        assert!(keys(&bundle, "/components/schemas").is_empty());
    }

    /// Omit rules apply on top of keep rules: a kept operation an omit rule removes is gone, and
    /// the keep rule naming it is not stale.
    #[test]
    fn omit_rules_apply_on_top_of_keep_rules() {
        let omit = Omit {
            rules: vec![OmitRule::operation(OmitMethod::Post, "/pets")],
            keep: vec![
                KeepRule::operation(OmitMethod::Get, "/pets"),
                KeepRule::operation(OmitMethod::Post, "/pets"),
            ],
        };
        let (bundle, diags) = apply(&omit);
        assert!(!diags.has_errors(), "{:?}", diags.items());
        assert_eq!(keys(&bundle, "/paths/~1pets"), ["parameters", "get"]);
        assert!(keys(&bundle, "/components/requestBodies").is_empty());
    }

    /// A glob keep rule keeps every operation its path pattern matches.
    #[test]
    fn a_glob_keep_rule_keeps_every_match() {
        let (bundle, diags) = apply(&keep(vec![KeepRule::operation(OmitMethod::Get, "/*")]));
        assert!(!diags.has_errors(), "{:?}", diags.items());
        assert_eq!(keys(&bundle, "/paths"), ["/pets", "/owners", "/shared"]);
        assert_eq!(keys(&bundle, "/paths/~1pets"), ["parameters", "get"]);
    }

    /// A keep rule naming no declared operation is an `E019` saying so, and the document is left
    /// un-narrowed rather than adding an `E020` for an empty `paths`.
    #[test]
    fn a_keep_rule_naming_nothing_is_e019() {
        for (rule, expected) in [
            (
                KeepRule::operation(OmitMethod::Delete, "/pets"),
                "keep rule did not name a declared operation: keep delete /pets",
            ),
            (
                KeepRule::operation(OmitMethod::Get, "/nope"),
                "keep rule did not name a declared operation: keep get /nope",
            ),
            (
                KeepRule::operation(OmitMethod::Get, "/nope/*"),
                "glob keep rule did not match any declared operation: keep get /nope/*",
            ),
        ] {
            let (bundle, diags) = apply(&keep(vec![rule]));
            let codes: Vec<(Code, &str)> = diags
                .items()
                .iter()
                .map(|diagnostic| (diagnostic.code, diagnostic.message.as_str()))
                .collect();
            assert_eq!(codes, [(Code::InvalidOmitRule, expected)]);
            assert_eq!(keys(&bundle, "/paths").len(), 3, "nothing narrowed");
        }
    }

    /// Keep rules move the profile's fingerprint, while a profile without them encodes as it
    /// did before they existed.
    #[test]
    fn keep_rules_move_the_fingerprint_and_only_when_present() {
        let omit_only = Omit {
            rules: vec![OmitRule::path("/a")],
            keep: Vec::new(),
        };
        let mut expected = vec![0];
        expected.extend_from_slice(&2u64.to_be_bytes());
        expected.extend_from_slice(b"/a");
        assert_eq!(omit_only.canonical_encoding(), expected);

        let kept = Omit {
            keep: vec![KeepRule::operation(OmitMethod::Get, "/a")],
            ..omit_only.clone()
        };
        expected.push(4);
        expected.extend_from_slice(&3u64.to_be_bytes());
        expected.extend_from_slice(b"get");
        expected.extend_from_slice(&2u64.to_be_bytes());
        expected.extend_from_slice(b"/a");
        assert_eq!(kept.canonical_encoding(), expected);
        assert_ne!(kept.fingerprint(), omit_only.fingerprint());
        assert!(!keep(vec![KeepRule::operation(OmitMethod::Get, "/a")]).is_empty());
    }
}
