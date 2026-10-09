//! Oracles over a run's report and emitted module that hold for every input, shared by the
//! fixture suite (`frontend/`), the fuzz harness (`fuzz_frontend.rs`) and the lowering
//! properties (`lowering_props.rs`), so each holds every run it makes to the same bar (#475):
//!
//! * [`location_violations`]: every diagnostic names a real location (#454);
//! * [`indistinguishable_variants`]: every union's variants can be told apart (#402);
//! * [`relocate`] and [`shape`]: moving a document's schemas into a referenced file changes no
//!   verdict, code or shape (#446).
//!
//! Each oracle reports the defects it finds that an open issue already tracks as known, carrying
//! the issue number, so a caller can let them through while the issue is open. The fixtures in
//! `frontend/` that pin each known entry fail once the issue's fix lands, and the entry goes with
//! them, so the set of known gaps can only shrink.

// Each test crate that declares this module uses the part of it its runs need.
#![allow(dead_code)]

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use spargen::{Code, Diagnostic, Report};
use yaml_rust2::{Yaml, YamlLoader};

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
/// their empty pointer as known; every other rule still applies to them, each checked on its own.
pub const KNOWN_ROOT_POINTERS: &[(Code, u32)] = &[];

/// The diagnostics whose span still covers the whole root document although the construct they
/// report has a span of its own, each with the open issue that tracks it. [`location_violations`]
/// reports that span as known; every other rule still applies to them, each checked on its own.
pub const KNOWN_WHOLE_ROOT_SPANS: &[(Code, u32)] = &[];

/// The issue `table` tracks `code` under, if any.
fn known_in(table: &[(Code, u32)], code: Code) -> Option<u32> {
    table
        .iter()
        .find(|(known, _)| *known == code)
        .map(|(_, issue)| *issue)
}

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
    let mut violations = Vec::new();
    for diagnostic in diagnostics
        .iter()
        .filter(|diagnostic| !is_document_level(diagnostic))
    {
        let in_root = diagnostic.span.is_none_or(|span| span.file.0 == 0);
        let whole_root = diagnostic.span.is_some_and(|span| {
            span.file.0 == 0
                && span.start.line == 1
                && content_end > 0
                && span.end.offset >= content_end
        });
        // Each rule is checked on its own, so a diagnostic that breaks two reports both, and a
        // known gap under one rule never hides a break of another.
        if diagnostic.message.contains("``") {
            violations.push(Violation {
                reason: format!("the message names something empty (``): {diagnostic:?}"),
                known: None,
            });
        }
        if in_root && diagnostic.pointer.as_str().is_empty() {
            violations.push(Violation {
                reason: format!("the pointer is empty: {diagnostic:?}"),
                known: known_in(KNOWN_ROOT_POINTERS, diagnostic.code),
            });
        }
        if whole_root {
            violations.push(Violation {
                reason: format!("the span covers the whole root document: {diagnostic:?}"),
                known: known_in(KNOWN_WHOLE_ROOT_SPANS, diagnostic.code),
            });
        }
    }
    violations
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
/// shape, so every value matches both or neither; and a `oneOf` variant whose payload is
/// `serde_json::Value` (#535), which accepts every value, so every value another variant accepts
/// matches two.
///
/// A union is an enum with its own `Deserialize` impl, of one-field tuple variants. A discriminated
/// union tells equal payloads apart by its tag and a JSON-category dispatch by the category, so
/// only the trial-matched `oneOf`, whose decode error says "must match exactly one typed variant",
/// is held to distinct, typed payloads. An `anyOf` is not: one match decodes it, and its most
/// specific match is selected, so a typed variant still takes every value it accepts, and a
/// `serde_json::Value` variant, the lowering of an untyped member (`{}`, `true`), takes only the
/// rest. That is the faithful lowering of what the document says, so it is not reported.
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
        if !types.contains(&format!(
            "must match exactly one typed variant of union {name}\""
        )) {
            continue;
        }
        for (variant, shape) in &payloads {
            if shape == "serde_json::Value" {
                violations.push(Violation {
                    reason: format!("`oneOf` `{name}` variant `{variant}` is `serde_json::Value`"),
                    known: None,
                });
            }
        }
        for (at, (first, shape)) in payloads.iter().enumerate() {
            for (second, other) in &payloads[at + 1..] {
                if shape == other {
                    // Equal nominal payloads (two structs, or two string enums, of one
                    // definition) are merged with `W001` since #492, so no issue tracks them.
                    violations.push(Violation {
                        reason: format!(
                            "`oneOf` `{name}` variants `{first}` and `{second}` have one shape: \
                             {shape}"
                        ),
                        known: None,
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

/// The shape of the type the module `code` declares as `name`: what it stands for with every name
/// it reaches expanded, so two modules that lower one schema under different names agree. `name`
/// itself when the module declares nothing by that name.
pub fn shape(code: &str, name: &str) -> String {
    Declarations::read(types_module(code)).shape_of(name, 8)
}

/// A YAML string scalar.
fn text(value: &str) -> Yaml {
    Yaml::String(value.to_owned())
}

/// Append `node` to `out` as JSON, keeping every mapping's key order: the order of a schema's
/// `properties` is the order of the fields it lowers to, so a relocation that reordered them would
/// change a shape the document never changed. A key that is not a string is written as its YAML
/// scalar text, as the frontend reads it.
fn write_json(node: &Yaml, out: &mut String) {
    let quoted = |value: &str| serde_json::to_string(value).expect("a string serializes");
    match node {
        Yaml::Real(value) => match value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
        {
            Some(number) => out.push_str(&number.to_string()),
            None => out.push_str(&quoted(value)),
        },
        Yaml::Integer(value) => out.push_str(&value.to_string()),
        Yaml::String(value) => out.push_str(&quoted(value)),
        Yaml::Boolean(value) => out.push_str(&value.to_string()),
        Yaml::Array(items) => {
            out.push('[');
            for (at, item) in items.iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                write_json(item, out);
            }
            out.push(']');
        }
        Yaml::Hash(entries) => {
            out.push('{');
            for (at, (key, value)) in entries.iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                let key = match key {
                    Yaml::String(key) | Yaml::Real(key) => key.clone(),
                    Yaml::Integer(key) => key.to_string(),
                    Yaml::Boolean(key) => key.to_string(),
                    _ => String::from("null"),
                };
                out.push_str(&quoted(&key));
                out.push(':');
                write_json(value, out);
            }
            out.push('}');
        }
        Yaml::Alias(_) | Yaml::Null | Yaml::BadValue => out.push_str("null"),
    }
}

/// The value at `key` of a YAML mapping, mutably.
fn entry<'a>(node: &'a mut Yaml, key: &str) -> Option<&'a mut Yaml> {
    match node {
        Yaml::Hash(entries) => entries.get_mut(&text(key)),
        _ => None,
    }
}

/// The file [`relocate`] moves a document's schemas into, beside the root.
pub const RELOCATED_LIB: &str = "lib.yaml";

/// `spec` with its `components.schemas` moved into [`RELOCATED_LIB`] (#446), as the root and
/// sub-file texts, or `None` when `spec` declares no schema or is not a YAML mapping.
///
/// The sub-file declares the schemas under its own `components.schemas`. A
/// `#/components/schemas/…` reference between them would read the root document's component of
/// that name first (a sub-file's bare component reference does), which is now the `$ref` back into
/// the sub-file, so each is spelled against the sub-file itself (`./lib.yaml#/components/…`). Each
/// root entry becomes a `$ref` to its moved schema, so every schema stays reachable under its name
/// and every reference the root makes still resolves to it. Both files are written as JSON, which
/// YAML reads, with every key in its original order.
pub fn relocate(spec: &str) -> Option<(String, String)> {
    let mut root = YamlLoader::load_from_str(spec).ok()?.into_iter().next()?;
    let schemas = entry(entry(&mut root, "components")?, "schemas")?;
    if schemas.as_hash().is_none_or(|entries| entries.is_empty()) {
        return None;
    }
    let mut moved = std::mem::replace(schemas, Yaml::Hash(Default::default()));
    point_into_lib(&mut moved);
    let Yaml::Hash(moved_entries) = &moved else {
        unreachable!("the moved schemas are a mapping")
    };
    let Yaml::Hash(entries) = schemas else {
        unreachable!("the emptied schemas are a mapping")
    };
    for name in moved_entries.keys() {
        let Yaml::String(name) = name else { continue };
        let target = format!(
            "./{RELOCATED_LIB}#/components/schemas/{}",
            name.replace('~', "~0").replace('/', "~1")
        );
        let mut reference = yaml_rust2::yaml::Hash::new();
        reference.insert(text("$ref"), text(&target));
        entries.insert(text(name), Yaml::Hash(reference));
    }
    let mut components = yaml_rust2::yaml::Hash::new();
    components.insert(text("schemas"), moved);
    let mut lib = yaml_rust2::yaml::Hash::new();
    lib.insert(text("components"), Yaml::Hash(components));
    let (mut root_text, mut lib_text) = (String::new(), String::new());
    write_json(&root, &mut root_text);
    write_json(&Yaml::Hash(lib), &mut lib_text);
    Some((root_text, lib_text))
}

/// Respell every `$ref` under `node` that names a root schema (`#/components/schemas/…`) against
/// [`RELOCATED_LIB`], which now holds the schemas.
fn point_into_lib(node: &mut Yaml) {
    match node {
        Yaml::Hash(entries) => {
            for (key, child) in entries.iter_mut() {
                match child {
                    Yaml::String(target)
                        if *key == text("$ref") && target.starts_with("#/components/schemas/") =>
                    {
                        *target = format!("./{RELOCATED_LIB}{target}");
                    }
                    _ => point_into_lib(child),
                }
            }
        }
        Yaml::Array(items) => items.iter_mut().for_each(point_into_lib),
        _ => {}
    }
}

/// Write [`relocate`]'s two files into `dir`, returning the root's path, or `None` when there is
/// nothing to move.
pub fn write_relocated(dir: &Utf8Path, spec: &str) -> Option<Utf8PathBuf> {
    let (root, lib) = relocate(spec)?;
    let path = dir.join("openapi.yaml");
    std::fs::write(&path, root).unwrap();
    std::fs::write(dir.join(RELOCATED_LIB), lib).unwrap();
    Some(path)
}

/// The names `spec` declares under `components.schemas`, in document order.
pub fn schema_names(spec: &str) -> Vec<String> {
    let Some(mut root) = YamlLoader::load_from_str(spec)
        .ok()
        .and_then(|documents| documents.into_iter().next())
    else {
        return Vec::new();
    };
    match entry(&mut root, "components").and_then(|components| entry(components, "schemas")) {
        Some(Yaml::Hash(entries)) => entries
            .keys()
            .filter_map(|name| name.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}
