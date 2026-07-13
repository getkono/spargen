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
//!   metacharacter is an exact rule and behaves exactly as before. See [`glob_match`] for the
//!   semantics.
//! * **Auto-carve.** [`carve_rules`] maps error diagnostics to the smallest enclosing *omittable*
//!   construct, so the facade can iteratively omit the unsupported islands of a spec and generate
//!   the rest. The fixpoint driver lives in the facade (it must re-run the pipeline); this module
//!   supplies only the pure pointer→construct mapping.

use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

use crate::diag::{
    Aborted, Code, Diagnostic, Diagnostics, FileId, JsonPointer, Provenance, Severity, Span,
};
use crate::source::{InputBundle, Node, SpannedValue};

/// A compatibility omit profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Omit {
    /// Exact omit rules.
    pub rules: Vec<OmitRule>,
}

impl Omit {
    /// Construct a profile from exact rules.
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
        diags.into_result(())
    }

    /// Stable fingerprint used in generated provenance headers.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Fnv64::default();
        for rule in &self.rules {
            rule.hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
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
                pointer: JsonPointer::root().push("paths").push(path),
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
                    .push(path)
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
                    .push(name),
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
                        pointer: JsonPointer::from(*pointer),
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

/// Whether `pattern` contains a glob metacharacter (`*` or `?`) and should be matched as a glob
/// rather than compared exactly.
fn has_glob_meta(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

/// One compiled glob token.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GlobToken {
    /// A literal character.
    Lit(char),
    /// `?` — exactly one character other than `/`.
    Any,
    /// `*` — zero or more characters, none of which is `/` (a single path/name segment).
    Star,
    /// `**` — zero or more characters, including `/` (any depth).
    DoubleStar,
}

fn compile_glob(pattern: &str) -> Vec<GlobToken> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut tokens = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if i + 1 < chars.len() && chars[i + 1] == '*' => {
                tokens.push(GlobToken::DoubleStar);
                i += 2;
            }
            '*' => {
                tokens.push(GlobToken::Star);
                i += 1;
            }
            '?' => {
                tokens.push(GlobToken::Any);
                i += 1;
            }
            other => {
                tokens.push(GlobToken::Lit(other));
                i += 1;
            }
        }
    }
    tokens
}

/// Match `text` against a glob `pattern`. `/`-aware: `*` and `?` never cross a `/` segment
/// separator, while `**` matches across any depth. Polynomial (memoized) so pathological patterns
/// cannot blow up. Used for bulk omit rules and (via [`glob_match`]) documented in the module docs.
fn glob_match(pattern: &str, text: &str) -> bool {
    let tokens = compile_glob(pattern);
    let text: Vec<char> = text.chars().collect();
    let mut memo = vec![vec![None; text.len() + 1]; tokens.len() + 1];
    glob_match_at(&tokens, &text, 0, 0, &mut memo)
}

fn glob_match_at(
    tokens: &[GlobToken],
    text: &[char],
    ti: usize,
    pi: usize,
    memo: &mut [Vec<Option<bool>>],
) -> bool {
    if let Some(cached) = memo[pi][ti] {
        return cached;
    }
    let result = match tokens.get(pi) {
        None => ti == text.len(),
        Some(GlobToken::Lit(expected)) => {
            text.get(ti) == Some(expected) && glob_match_at(tokens, text, ti + 1, pi + 1, memo)
        }
        Some(GlobToken::Any) => {
            matches!(text.get(ti), Some(&ch) if ch != '/')
                && glob_match_at(tokens, text, ti + 1, pi + 1, memo)
        }
        Some(GlobToken::Star) => {
            glob_match_at(tokens, text, ti, pi + 1, memo)
                || matches!(text.get(ti), Some(&ch) if ch != '/')
                    && glob_match_at(tokens, text, ti + 1, pi, memo)
        }
        Some(GlobToken::DoubleStar) => {
            glob_match_at(tokens, text, ti, pi + 1, memo)
                || (ti < text.len() && glob_match_at(tokens, text, ti + 1, pi, memo))
        }
    };
    memo[pi][ti] = Some(result);
    result
}

/// One exact compatibility omission.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OmitRule {
    /// Remove a path item and every operation beneath it.
    Path { path: &'static str },
    /// Remove a single operation.
    Operation {
        /// HTTP method.
        method: OmitMethod,
        /// OAS path template.
        path: &'static str,
    },
    /// Remove a named component.
    Component {
        /// Component map.
        kind: ComponentKind,
        /// Component name.
        name: &'static str,
    },
    /// Remove an arbitrary JSON Pointer, optionally file-local.
    Pointer {
        /// Optional file path/suffix in the input bundle.
        file: Option<&'static str>,
        /// RFC 6901 pointer.
        pointer: &'static str,
    },
}

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
pub(crate) fn carve_rules(diagnostics: &[Diagnostic]) -> Vec<OmitRule> {
    let mut rules: Vec<OmitRule> = Vec::new();
    for diagnostic in diagnostics {
        if diagnostic.severity != Severity::Error {
            continue;
        }
        if let Some(rule) = omittable_enclosing(&diagnostic.pointer) {
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        }
    }
    // Deterministic carve set: order is independent of diagnostic order.
    rules.sort_by_key(|rule| rule.describe());
    rules
}

/// The smallest omittable construct enclosing `pointer`, or `None` if none is (root / unmodelled).
fn omittable_enclosing(pointer: &JsonPointer) -> Option<OmitRule> {
    let tokens = pointer_tokens(pointer);
    match tokens.first()?.as_str() {
        "paths" => {
            let path = leak(tokens.get(1)?.clone());
            match tokens.get(2).and_then(|token| parse_method(token)) {
                Some(method) => Some(OmitRule::Operation { method, path }),
                None => Some(OmitRule::Path { path }),
            }
        }
        "components" => {
            let kind = parse_component_kind(tokens.get(1)?)?;
            let name = leak(tokens.get(2)?.clone());
            Some(OmitRule::Component { kind, name })
        }
        _ => None,
    }
}

/// Split a JSON Pointer into its unescaped reference tokens (`~1`→`/`, `~0`→`~`).
fn pointer_tokens(pointer: &JsonPointer) -> Vec<String> {
    let raw = pointer.as_str();
    if raw.is_empty() {
        return Vec::new();
    }
    raw.strip_prefix('/')
        .unwrap_or(raw)
        .split('/')
        .map(|token| token.replace("~1", "/").replace("~0", "~"))
        .collect()
}

fn parse_method(token: &str) -> Option<OmitMethod> {
    Some(match token {
        "get" => OmitMethod::Get,
        "put" => OmitMethod::Put,
        "post" => OmitMethod::Post,
        "delete" => OmitMethod::Delete,
        "options" => OmitMethod::Options,
        "head" => OmitMethod::Head,
        "patch" => OmitMethod::Patch,
        "trace" => OmitMethod::Trace,
        _ => return None,
    })
}

fn parse_component_kind(token: &str) -> Option<ComponentKind> {
    Some(match token {
        "schemas" => ComponentKind::Schemas,
        "responses" => ComponentKind::Responses,
        "parameters" => ComponentKind::Parameters,
        "requestBodies" => ComponentKind::RequestBodies,
        "headers" => ComponentKind::Headers,
        "securitySchemes" => ComponentKind::SecuritySchemes,
        _ => return None,
    })
}

/// Leak an owned string to `&'static str`. Carve derives omit rules dynamically from diagnostics,
/// but [`OmitRule`] borrows `'static` (it is designed for the compile-time [`omit!`] macro). A run
/// carves a bounded number of constructs and the process is short-lived, so leaking these small,
/// bounded strings for the duration of the run is acceptable — the same tactic the CLI uses for
/// config-derived rules.
fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

impl OmitRule {
    fn describe(&self) -> String {
        match self {
            OmitRule::Path { path } => format!("path {path}"),
            OmitRule::Operation { method, path } => format!("{} {path}", method.as_oas_key()),
            OmitRule::Component { kind, name } => format!("component {} {name}", kind.as_oas_key()),
            OmitRule::Pointer { file, pointer } => match file {
                Some(file) => format!("pointer {file}#{pointer}"),
                None => format!("pointer {pointer}"),
            },
        }
    }
}

/// HTTP method used by compatibility omit rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OmitMethod {
    Get,
    Put,
    Post,
    Delete,
    Options,
    Head,
    Patch,
    Trace,
}

impl OmitMethod {
    fn as_oas_key(self) -> &'static str {
        match self {
            OmitMethod::Get => "get",
            OmitMethod::Put => "put",
            OmitMethod::Post => "post",
            OmitMethod::Delete => "delete",
            OmitMethod::Options => "options",
            OmitMethod::Head => "head",
            OmitMethod::Patch => "patch",
            OmitMethod::Trace => "trace",
        }
    }
}

/// Component map used by compatibility omit rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentKind {
    Schemas,
    Responses,
    Parameters,
    RequestBodies,
    Headers,
    SecuritySchemes,
}

impl ComponentKind {
    fn as_oas_key(self) -> &'static str {
        match self {
            ComponentKind::Schemas => "schemas",
            ComponentKind::Responses => "responses",
            ComponentKind::Parameters => "parameters",
            ComponentKind::RequestBodies => "requestBodies",
            ComponentKind::Headers => "headers",
            ComponentKind::SecuritySchemes => "securitySchemes",
        }
    }
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

fn validate_remaining(bundle: &InputBundle, diags: &mut Diagnostics) {
    let root = bundle.root();
    let missing = ["openapi", "info", "paths"]
        .into_iter()
        .filter(|key| root.get(key).is_none())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        Diagnostic::error(
            Code::OmitCreatedInvalidDocument,
            Provenance::new(JsonPointer::root(), Some(root.span())),
        )
        .message(format!(
            "omit profile removed required OpenAPI root fields: {}",
            missing.join(", ")
        ))
        .remedy("do not omit required OpenAPI root fields")
        .emit(diags);
    }
}

#[derive(Default)]
struct Fnv64(u64);

impl Hasher for Fnv64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.0 == 0 {
            self.0 = 0xcbf29ce484222325;
        }
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

/// Build an exact compatibility omit profile.
#[macro_export]
macro_rules! omit {
    () => {
        $crate::Omit::default()
    };
    ($($tokens:tt)*) => {{
        let mut omit = $crate::Omit::default();
        $crate::__spargen_omit_parse!(omit; $($tokens)*);
        omit
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __spargen_omit_parse {
    ($omit:ident;) => {};
    ($omit:ident; operations { $($body:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_operations!($omit; $($body)*);
        $crate::__spargen_omit_parse!($omit; $($rest)*);
    }};
    ($omit:ident; paths { $($body:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_paths!($omit; $($body)*);
        $crate::__spargen_omit_parse!($omit; $($rest)*);
    }};
    ($omit:ident; components { $($body:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_components!($omit; $($body)*);
        $crate::__spargen_omit_parse!($omit; $($rest)*);
    }};
    ($omit:ident; pointers { $($body:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_pointers!($omit; None; $($body)*);
        $crate::__spargen_omit_parse!($omit; $($rest)*);
    }};
    ($omit:ident; file($file:literal) { pointers { $($body:tt)* } } $($rest:tt)*) => {{
        $crate::__spargen_omit_pointers!($omit; Some($file); $($body)*);
        $crate::__spargen_omit_parse!($omit; $($rest)*);
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __spargen_omit_operations {
    ($omit:ident;) => {};
    ($omit:ident; get $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Get, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
    ($omit:ident; put $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Put, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
    ($omit:ident; post $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Post, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
    ($omit:ident; delete $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Delete, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
    ($omit:ident; options $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Options, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
    ($omit:ident; head $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Head, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
    ($omit:ident; patch $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Patch, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
    ($omit:ident; trace $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Operation { method: $crate::OmitMethod::Trace, path: $path });
        $crate::__spargen_omit_operations!($omit; $($rest)*);
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __spargen_omit_paths {
    ($omit:ident;) => {};
    ($omit:ident; $path:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Path { path: $path });
        $crate::__spargen_omit_paths!($omit; $($rest)*);
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __spargen_omit_components {
    ($omit:ident;) => {};
    ($omit:ident; schemas { $($names:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_component_names!($omit; $crate::ComponentKind::Schemas; $($names)*);
        $crate::__spargen_omit_components!($omit; $($rest)*);
    }};
    ($omit:ident; responses { $($names:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_component_names!($omit; $crate::ComponentKind::Responses; $($names)*);
        $crate::__spargen_omit_components!($omit; $($rest)*);
    }};
    ($omit:ident; parameters { $($names:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_component_names!($omit; $crate::ComponentKind::Parameters; $($names)*);
        $crate::__spargen_omit_components!($omit; $($rest)*);
    }};
    ($omit:ident; request_bodies { $($names:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_component_names!($omit; $crate::ComponentKind::RequestBodies; $($names)*);
        $crate::__spargen_omit_components!($omit; $($rest)*);
    }};
    ($omit:ident; headers { $($names:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_component_names!($omit; $crate::ComponentKind::Headers; $($names)*);
        $crate::__spargen_omit_components!($omit; $($rest)*);
    }};
    ($omit:ident; security_schemes { $($names:tt)* } $($rest:tt)*) => {{
        $crate::__spargen_omit_component_names!($omit; $crate::ComponentKind::SecuritySchemes; $($names)*);
        $crate::__spargen_omit_components!($omit; $($rest)*);
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __spargen_omit_component_names {
    ($omit:ident; $kind:expr;) => {};
    ($omit:ident; $kind:expr; $name:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Component { kind: $kind, name: $name });
        $crate::__spargen_omit_component_names!($omit; $kind; $($rest)*);
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __spargen_omit_pointers {
    ($omit:ident; $file:expr;) => {};
    ($omit:ident; $file:expr; $pointer:literal; $($rest:tt)*) => {{
        $omit.rules.push($crate::OmitRule::Pointer { file: $file, pointer: $pointer });
        $crate::__spargen_omit_pointers!($omit; $file; $($rest)*);
    }};
}

#[cfg(test)]
mod tests {
    use super::{
        carve_rules, glob_match, has_glob_meta, omittable_enclosing, ComponentKind, Diagnostic,
        JsonPointer, Omit, OmitMethod, OmitRule, Provenance,
    };
    use crate::diag::{Code, Diagnostics};
    use crate::source::InputBundle;

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
            rules: vec![OmitRule::Path { path: "/admin/**" }],
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
            rules: vec![OmitRule::Component {
                kind: ComponentKind::Schemas,
                name: "Legacy*",
            }],
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
            rules: vec![OmitRule::Path {
                path: "/admin/users",
            }],
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
            rules: vec![OmitRule::Operation {
                method: OmitMethod::Get,
                path: "/admin/**",
            }],
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
            rules: vec![OmitRule::Path { path: "/nope/**" }],
        };
        assert!(omit.apply(&mut bundle, &mut diags).is_err());
        assert!(diags
            .items()
            .iter()
            .any(|d| d.code == Code::InvalidOmitRule));
    }

    #[test]
    fn carve_rules_map_pointers_to_enclosing_constructs() {
        // Operation pointer → operation.
        assert_eq!(
            omittable_enclosing(&JsonPointer::from(
                "/paths/~1pets~1{id}/get/responses/200".to_owned()
            )),
            Some(OmitRule::Operation {
                method: OmitMethod::Get,
                path: "/pets/{id}"
            })
        );
        // Path-item-level pointer (not into a method) → path.
        assert_eq!(
            omittable_enclosing(&JsonPointer::from("/paths/~1pets/parameters/0".to_owned())),
            Some(OmitRule::Path { path: "/pets" })
        );
        // Component pointer → component.
        assert_eq!(
            omittable_enclosing(&JsonPointer::from(
                "/components/schemas/Bad/oneOf/0".to_owned()
            )),
            Some(OmitRule::Component {
                kind: ComponentKind::Schemas,
                name: "Bad"
            })
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
                Code::NonDisjointUnion,
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
        let rules = carve_rules(&diagnostics);
        // Warnings ignored; duplicates collapsed; sorted deterministically by description.
        assert_eq!(
            rules,
            vec![
                OmitRule::Operation {
                    method: OmitMethod::Get,
                    path: "/a"
                },
                OmitRule::Operation {
                    method: OmitMethod::Get,
                    path: "/b"
                },
            ]
        );
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
            OmitRule::Operation {
                method: OmitMethod::Get,
                path: "/markdown"
            }
        );
        assert_eq!(
            omit.rules[3],
            OmitRule::Component {
                kind: ComponentKind::Schemas,
                name: "legacy"
            }
        );
        assert!(omit.fingerprint().len() == 16);
    }
}
