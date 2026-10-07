//! Oracles over a run's report and emitted module that hold for every input, shared by the
//! fixture suite (`frontend.rs`), the fuzz harness (`fuzz_frontend.rs`) and the lowering
//! properties (`lowering_props.rs`), so each holds every run it makes to the same bar (#475):
//!
//! * [`location_violations`]: every diagnostic names a real location (#454);
//! * [`indistinguishable_variants`]: every union's variants can be told apart (#402).
//!
//! Each oracle reports the defects it finds that an open issue already tracks as known, carrying
//! the issue number, so a caller can let them through while the issue is open. The fixtures in
//! `frontend.rs` that pin each known entry fail once the issue's fix lands, and the entry goes with
//! them, so the set of known gaps can only shrink.

use std::collections::BTreeMap;

use spargen::{Code, Diagnostic, Report};

/// One defect an oracle found.
#[derive(Debug)]
pub struct Violation {
    /// What is wrong, and where.
    pub reason: String,
    /// The open issue that tracks this defect, when it is a known one.
    pub known: Option<u32>,
}

/// The diagnostics whose pointer is still the document root although the construct they report has
/// a pointer of its own, each with the open issue that tracks it. [`location_violations`] reports
/// their empty pointer as known; every other rule still applies to them.
pub const KNOWN_ROOT_POINTERS: &[(Code, u32)] = &[
    (Code::DuplicateObjectKey, ISSUE_DUPLICATE_KEY_POINTER),
    (Code::AbsoluteRefUnsupported, ISSUE_REMOTE_REF_POINTER),
    (Code::VendoredRefDrift, ISSUE_REMOTE_REF_POINTER),
];

/// The issue (#533) tracking `E022`'s root pointer: the YAML and JSON parsers raise it before any
/// pointer is tracked, with only the duplicate key's span.
pub const ISSUE_DUPLICATE_KEY_POINTER: u32 = 533;

/// The issue (#534) tracking `E003`'s and `E021`'s root pointer and whole-document span: the
/// bundle raises them against the referring file's root value rather than the `$ref` that names
/// the remote document.
pub const ISSUE_REMOTE_REF_POINTER: u32 = 534;

/// The issue tracking structurally equal nominal `oneOf` variants: two inline structs, or two
/// string enums, with the same definition are not merged (#492).
pub const ISSUE_EQUAL_NOMINAL_VARIANTS: u32 = 492;

/// The issue (#535) tracking union members that lower to `serde_json::Value` with no diagnostic.
pub const ISSUE_UNTYPED_UNION_MEMBER: u32 = 535;

/// Whether `diagnostic` is about the document as a whole, so the root is its true location: an
/// unsupported `openapi` version (`E001`), and an invalid input document (`E011`) the validator
/// could place nowhere below the root.
fn is_document_level(diagnostic: &Diagnostic) -> bool {
    diagnostic.code == Code::UnsupportedOpenApiVersion
        || (diagnostic.code == Code::InvalidInput && diagnostic.pointer.as_str().is_empty())
}

/// Each diagnostic in `diagnostics` that names no real location (#454).
///
/// A diagnostic raised against a construct the lowering built for itself, such as the struct an
/// `allOf` meet produces, used to carry the document root's provenance: an empty pointer, a span
/// over the whole file, and an empty type name in the message (`` in `` ``). So every diagnostic
/// that is not [`is_document_level`] must have no empty backtick pair in its message, no span
/// covering the whole root document `root`, and a non-empty pointer unless it lies in a referenced
/// file. A pointer addresses the file its span lies in, and the root of a referenced file is a
/// construct of its own (a whole-file `$ref` target); the root of the root document is not. The
/// root is the first file a bundle loads, `FileId(0)`.
///
/// A span covers the whole root when it starts on the first line and runs to the last
/// non-whitespace byte: the root value's span, which the YAML parser starts at the first value
/// rather than at offset 0.
pub fn location_violations(diagnostics: &[Diagnostic], root: &[u8]) -> Vec<Violation> {
    let content_end = root
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(0, |at| at + 1);
    diagnostics
        .iter()
        .filter(|diagnostic| !is_document_level(diagnostic))
        .filter_map(|diagnostic| {
            let in_root = diagnostic.span.is_none_or(|span| span.file.0 == 0);
            let whole_root = diagnostic.span.is_some_and(|span| {
                span.file.0 == 0
                    && span.start.line == 1
                    && content_end > 0
                    && span.end.offset >= content_end
            });
            let known = KNOWN_ROOT_POINTERS
                .iter()
                .find(|(code, _)| *code == diagnostic.code)
                .map(|(_, issue)| *issue);
            if diagnostic.message.contains("``") {
                Some(Violation {
                    reason: format!("the message names something empty (``): {diagnostic:?}"),
                    known: None,
                })
            } else if in_root && diagnostic.pointer.as_str().is_empty() {
                Some(Violation {
                    reason: format!("the pointer is empty: {diagnostic:?}"),
                    known,
                })
            } else if whole_root {
                Some(Violation {
                    reason: format!("the span covers the whole root document: {diagnostic:?}"),
                    known: None,
                })
            } else {
                None
            }
        })
        .collect()
}

/// Everything from the generated `types` module to the end of the file, or nothing when no module
/// was emitted. The embedded runtime before it is not generated from the document.
fn types_module(code: &str) -> &str {
    code.find("pub mod types {")
        .map_or("", |start| &code[start..])
}

/// The item declarations of a generated module, keyed by name: each `pub type`, `pub struct` and
/// `pub enum`, with what [`shape_of`] reads from it.
#[derive(Default)]
struct Declarations {
    /// `pub type NAME = TARGET;`: the target.
    aliases: BTreeMap<String, String>,
    /// `pub struct NAME { … }` and `pub enum NAME { … }`: the kind, the outer attribute lines and
    /// the body lines, trimmed.
    items: BTreeMap<String, (&'static str, Vec<String>, Vec<String>)>,
}

impl Declarations {
    fn read(code: &str) -> Self {
        let lines: Vec<&str> = code.lines().map(str::trim).collect();
        let mut declarations = Self::default();
        for (at, line) in lines.iter().enumerate() {
            if let Some(rest) = line.strip_prefix("pub type ") {
                if let Some((name, target)) = rest.split_once(" = ") {
                    declarations.aliases.insert(
                        name.to_owned(),
                        target.trim_end_matches(';').trim().to_owned(),
                    );
                }
                continue;
            }
            let (kind, rest) = if let Some(rest) = line.strip_prefix("pub struct ") {
                ("struct", rest)
            } else if let Some(rest) = line.strip_prefix("pub enum ") {
                ("enum", rest)
            } else {
                continue;
            };
            let Some(name) = rest.strip_suffix(" {") else {
                continue;
            };
            let attributes = lines[..at]
                .iter()
                .rev()
                .take_while(|line| line.starts_with("#["))
                .map(|line| (*line).to_owned())
                .collect();
            let body = lines[at + 1..]
                .iter()
                .take_while(|line| !line.starts_with('}'))
                .map(|line| (*line).to_owned())
                .collect();
            declarations
                .items
                .insert(name.to_owned(), (kind, attributes, body));
        }
        declarations
    }

    /// The structure `ty` stands for, with every name the module declares replaced by what it
    /// declares: an alias by its target, a struct or enum by its kind, attributes and body. Two
    /// payloads with the same shape decode the same values, whatever they are named (`Uvariant0a`
    /// and `Uvariant1a` are both `String`). A path segment after `::` is never a module name, so
    /// `serde_json::Value` stays itself. Expansion stops at `depth`, so a recursive type ends.
    fn shape_of(&self, ty: &str, depth: usize) -> String {
        let mut shape = String::new();
        let mut ident = String::new();
        let mut after_path = false;
        for c in ty.chars().chain(std::iter::once(' ')) {
            if c.is_alphanumeric() || c == '_' {
                ident.push(c);
                continue;
            }
            if !ident.is_empty() {
                shape.push_str(&self.expand(&ident, after_path, depth));
                ident.clear();
            }
            after_path = c == ':';
            shape.push(c);
        }
        shape.trim_end().to_owned()
    }

    /// [`Self::shape_of`] for one identifier.
    fn expand(&self, ident: &str, after_path: bool, depth: usize) -> String {
        if after_path || depth == 0 {
            return ident.to_owned();
        }
        if let Some(target) = self.aliases.get(ident) {
            return self.shape_of(target, depth - 1);
        }
        let Some((kind, attributes, body)) = self.items.get(ident) else {
            return ident.to_owned();
        };
        let mut shape = format!("{kind} {attributes:?} {{");
        for line in body {
            match line
                .strip_prefix("pub ")
                .and_then(|rest| rest.split_once(": "))
            {
                Some((field, ty)) => shape.push_str(&format!(
                    "pub {field}: {};",
                    self.shape_of(ty.trim_end_matches(','), depth - 1)
                )),
                None => shape.push_str(line),
            }
        }
        shape.push('}');
        shape
    }
}

/// Each union in a generated module whose variants cannot be told apart (#402): two variants of a
/// `oneOf` (one whose decode requires exactly one variant to match) whose payloads have the same
/// shape, so every value matches both or neither; and any union variant whose payload is
/// `serde_json::Value`, a typed member silently degraded.
///
/// A union is an enum with its own `Deserialize` impl, of one-field tuple variants. A discriminated
/// union tells equal payloads apart by its tag and a JSON-category dispatch by the category, so
/// only the trial-matched `oneOf`, whose decode error says "must match exactly one typed variant",
/// is held to distinct payloads.
pub fn indistinguishable_variants(code: &str) -> Vec<Violation> {
    let types = types_module(code);
    let declarations = Declarations::read(types);
    let mut violations = Vec::new();
    for (name, (kind, _, body)) in &declarations.items {
        if *kind != "enum" || !types.contains(&format!("serde::Deserialize<'de> for {name} {{")) {
            continue;
        }
        let payloads: Vec<(&str, String)> = body
            .iter()
            .filter(|line| !line.starts_with("#["))
            .filter_map(|line| {
                let (variant, rest) = line.split_once('(')?;
                let payload = rest.strip_suffix("),")?;
                let payload = payload
                    .strip_prefix("Box<")
                    .and_then(|inner| inner.strip_suffix('>'))
                    .unwrap_or(payload);
                Some((variant, declarations.shape_of(payload, 8)))
            })
            .collect();
        for (variant, shape) in &payloads {
            if shape == "serde_json::Value" {
                violations.push(Violation {
                    reason: format!("union `{name}` variant `{variant}` is `serde_json::Value`"),
                    known: Some(ISSUE_UNTYPED_UNION_MEMBER),
                });
            }
        }
        if !types.contains(&format!(
            "must match exactly one typed variant of union {name}\""
        )) {
            continue;
        }
        for (at, (first, shape)) in payloads.iter().enumerate() {
            for (second, other) in &payloads[at + 1..] {
                if shape == other {
                    let nominal = shape.starts_with("struct ") || shape.starts_with("enum ");
                    violations.push(Violation {
                        reason: format!(
                            "`oneOf` `{name}` variants `{first}` and `{second}` have one shape: \
                             {shape}"
                        ),
                        known: nominal.then_some(ISSUE_EQUAL_NOMINAL_VARIANTS),
                    });
                }
            }
        }
    }
    violations
}

/// The reason of each violation among `violations` that no open issue tracks.
pub fn unknown(violations: Vec<Violation>) -> Vec<String> {
    violations
        .into_iter()
        .filter(|violation| violation.known.is_none())
        .map(|violation| violation.reason)
        .collect()
}

/// The [`indistinguishable_variants`] of `code` a `Generated` run must not have emitted: none,
/// when the run reported a warning that can say why; otherwise every one no open issue tracks.
pub fn unexplained_variants(report: &Report, code: &str) -> Vec<String> {
    if report.warnings().next().is_some() {
        return Vec::new();
    }
    unknown(indistinguishable_variants(code))
}
