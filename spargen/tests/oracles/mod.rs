//! Oracles over a run's report and emitted module that hold for every input, shared by the
//! fixture suite (`frontend/`), the fuzz harness (`fuzz_frontend.rs`) and the lowering
//! properties (`lowering_props.rs`), so each holds every run it makes to the same bar (#475):
//!
//! * [`location_violations`]: every diagnostic names a real location (#454);
//! * [`indistinguishable_variants`]: every union's variants can be told apart (#402), unless a
//!   `W001` at the union says why ([`unexplained_variants`]);
//! * [`unreferenced_copies`]: no second lowering of a schema is emitted where nothing names it
//!   (#926), a narrowing of [`unreferenced_types`];
//! * [`relocate`] and [`shape`]: moving a document's schemas into a referenced file changes no
//!   verdict, code or shape (#446).
//!
//! Each oracle returns the defects it finds, one reason each; a caller fails on any of them.

// Each test crate that declares this module uses the part of it its runs need.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use spargen::{Code, Diagnostic, Report};
use yaml_rust2::{Yaml, YamlLoader};

/// Whether `diagnostic` is about the root document as a whole, so the root is its true location:
/// an unsupported `openapi` version (`E001`), and an invalid input document (`E011`) with an empty
/// pointer that the frontend could place nowhere smaller than the root document itself. That is an
/// `E011` in the root document with no span (the root document could not be read, or the pipeline
/// itself failed), with a span at a single point (a syntax error the parser places only by line
/// and column), with a span over the whole root, or that reports the root as a whole: the
/// validator's verdict on the root value (an `OpenAPI structure violation` whose instance is the
/// root, which the YAML parser spans only to the start of a flow collection's closing bracket), or
/// a YAML stream that is not one document, or a mapping key that is not a string where the root
/// mapping itself holds one: such a key is addressed by the mapping that holds it (#923), which
/// there is the root. Any other `E011` with an empty pointer, such as a non-string key in a
/// mapping below the root, reports a construct below the root with a pointer of its own, and is
/// held to every rule.
fn is_document_level(diagnostic: &Diagnostic, root: &[u8], content_end: usize) -> bool {
    if diagnostic.code == Code::UnsupportedOpenApiVersion {
        return true;
    }
    if diagnostic.code != Code::InvalidInput || !diagnostic.pointer.as_str().is_empty() {
        return false;
    }
    let Some(span) = diagnostic.span else {
        return true;
    };
    span.file.0 == 0
        && (span.start.offset == span.end.offset
            || (span.start.line == 1 && span.end.offset >= content_end)
            || diagnostic
                .message
                .starts_with("OpenAPI structure violation: ")
            || diagnostic.message == "YAML input must contain exactly one document"
            || (diagnostic.message == "YAML object keys must be strings"
                && root_mapping_holds_a_non_string_key(root)))
}

/// Whether `root` is one YAML document whose top-level mapping has a key that is not a string.
fn root_mapping_holds_a_non_string_key(root: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(root) else {
        return false;
    };
    let Ok(documents) = YamlLoader::load_from_str(text) else {
        return false;
    };
    match documents.as_slice() {
        [Yaml::Hash(mapping)] => mapping.keys().any(|key| !matches!(key, Yaml::String(_))),
        _ => false,
    }
}

/// Each diagnostic in `diagnostics` that names no real location (#454).
///
/// A diagnostic raised against a construct the lowering built for itself, such as the struct an
/// `allOf` meet produces, must not carry the document root's provenance: an empty pointer, a span
/// over the whole file, and an empty type name in the message (`` in `` ``). So every diagnostic
/// that is not [`is_document_level`] must have no empty backtick pair in its message (unless the
/// root document itself holds one, which a message may quote), no span
/// covering the whole root document `root`, and a non-empty pointer unless it lies in a referenced
/// file. A pointer addresses the file its span lies in, and the root of a referenced file is a
/// construct of its own (a whole-file `$ref` target); the root of the root document is not. The
/// root is the first file a bundle loads, `FileId(0)`.
///
/// A span covers the whole root when it starts on the first line and runs to the last
/// non-whitespace byte: the root value's span, which the YAML parser starts at the first value
/// rather than at offset 0.
pub fn location_violations(diagnostics: &[Diagnostic], root: &[u8]) -> Vec<String> {
    let content_end = root
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(0, |at| at + 1);
    let quotes_empty_pair = root.windows(2).any(|pair| pair == b"``");
    let mut violations = Vec::new();
    for diagnostic in diagnostics
        .iter()
        .filter(|diagnostic| !is_document_level(diagnostic, root, content_end))
    {
        let in_root = diagnostic.span.is_none_or(|span| span.file.0 == 0);
        let whole_root = diagnostic.span.is_some_and(|span| {
            span.file.0 == 0
                && span.start.line == 1
                && content_end > 0
                && span.end.offset >= content_end
        });
        // Each rule is checked on its own, so a diagnostic that breaks two reports both.
        if diagnostic.message.contains("``") && !quotes_empty_pair {
            violations.push(format!(
                "the message names something empty (``): {diagnostic:?}"
            ));
        }
        if in_root && diagnostic.pointer.as_str().is_empty() {
            violations.push(format!("the pointer is empty: {diagnostic:?}"));
        }
        if whole_root {
            violations.push(format!(
                "the span covers the whole root document: {diagnostic:?}"
            ));
        }
    }
    violations
}

/// Everything from the generated `types` module to the end of the file, or nothing when no module
/// was emitted: the provenance header and the embedded runtime before it are stripped, since they
/// are not generated from the document.
///
/// It is **not** bounded at the module's closing brace: the returned text also carries the
/// `Client` impl and the emitted scaffolding after `types`. A `.contains(…)` on it can therefore
/// match client code rather than a lowered type; a check that must pin a lowered type should match
/// a declaration (`pub struct X`, `pub type X =`) rather than a bare type name.
///
/// Two generations of the *same* spec already differ as whole files: the header carries the output
/// path and a per-run `input-sha256`/`content-sha256`. So comparing two generated clients whole
/// proves nothing about what they lowered to. Comparing from `pub mod types {` onward compares
/// what the two documents actually lowered to.
pub fn types_module(code: &str) -> String {
    code.find("pub mod types {")
        .map_or_else(String::new, |start| code[start..].to_owned())
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
pub fn indistinguishable_variants(code: &str) -> Vec<String> {
    union_violations(code)
        .into_iter()
        .map(|(_, reason)| reason)
        .collect()
}

/// [`indistinguishable_variants`], each reason with the name of the union it is about.
fn union_violations(code: &str) -> Vec<(String, String)> {
    let types = types_module(code);
    let declarations = Declarations::read(&types);
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
                violations.push((
                    name.clone(),
                    format!("`oneOf` `{name}` variant `{variant}` is `serde_json::Value`"),
                ));
            }
        }
        for (at, (first, shape)) in payloads.iter().enumerate() {
            for (second, other) in &payloads[at + 1..] {
                if shape == other {
                    // Equal nominal payloads (two structs, or two string enums, of one
                    // definition) are merged with `W001` (#492), so none should be left.
                    violations.push((
                        name.clone(),
                        format!(
                            "`oneOf` `{name}` variants `{first}` and `{second}` have one shape: \
                             {shape}"
                        ),
                    ));
                }
            }
        }
    }
    violations
}

/// `name` with everything but its ASCII letters and digits dropped, lowercased: how a schema name
/// and the Rust type name allocated from it compare (`my_pet` and `MyPet` are both `mypet`).
fn normalized(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The component schema `pointer` lies under, if any: `Pet` for `/components/schemas/Pet/oneOf`.
fn component_of(pointer: &str) -> Option<String> {
    let rest = pointer.strip_prefix("/components/schemas/")?;
    let name = rest.split('/').next()?;
    Some(name.replace("~1", "/").replace("~0", "~"))
}

/// Whether `diagnostic` is a `W001` that explains why a `oneOf`'s variants cannot be told apart:
/// a member or variant that lowers to `serde_json::Value`, which accepts every value, or members
/// or branches that lower to one generated type, or differ only in keywords it does not carry.
fn explains_union(diagnostic: &Diagnostic) -> bool {
    diagnostic.code == Code::ValidationKeywordIgnored
        && [
            "to `serde_json::Value`, which accepts every value",
            "lower to the same generated type",
            "differ only in keywords",
        ]
        .iter()
        .any(|says| diagnostic.message.contains(says))
}

/// The [`indistinguishable_variants`] of `code` a `Generated` run must not have emitted: every one
/// except those of a union a `W001` of `report` explains at the union's own pointer.
///
/// A union is tied to a pointer only through its name: a `W001` that [explains a
/// union](explains_union) at a pointer under the component schema `C` excuses one union whose name
/// begins with `C`'s, as every type lowered under `C` is named (`U`'s property `p` is `Up`). Each
/// such warning excuses one union, so two unions cannot shelter behind one warning, and a warning
/// anywhere else, or of any other kind, excuses none.
pub fn unexplained_variants(report: &Report, code: &str) -> Vec<String> {
    let violations = union_violations(code);
    let mut explanations: Vec<String> = report
        .diagnostics()
        .iter()
        .filter(|diagnostic| explains_union(diagnostic))
        .filter_map(|diagnostic| component_of(diagnostic.pointer.as_str()))
        .map(|component| normalized(&component))
        .collect();
    let unions: BTreeSet<&str> = violations.iter().map(|(name, _)| name.as_str()).collect();
    let mut excused = BTreeSet::new();
    for union in unions {
        let name = normalized(union);
        // The longest matching component first, so `Pet` takes `Pet`'s warning before `P` can.
        let best = explanations
            .iter()
            .enumerate()
            .filter(|(_, component)| name.starts_with(component.as_str()))
            .max_by_key(|(_, component)| component.len())
            .map(|(at, _)| at);
        if let Some(at) = best {
            explanations.remove(at);
            excused.insert(union);
        }
    }
    violations
        .iter()
        .filter(|(name, _)| !excused.contains(name.as_str()))
        .map(|(_, reason)| reason.clone())
        .collect()
}

/// The shape of the type the module `code` declares as `name`: what it stands for with every name
/// it reaches expanded, so two modules that lower one schema under different names agree. `name`
/// itself when the module declares nothing by that name.
pub fn shape(code: &str, name: &str) -> String {
    Declarations::read(&types_module(code)).shape_of(name, 8)
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

/// Whether `text` holds `ident` as a whole identifier, not as part of a longer one.
fn names(text: &str, ident: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(ident).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(is_ident)
            && !text[at + ident.len()..]
                .chars()
                .next()
                .is_some_and(is_ident)
    })
}

/// The type a top-level item of the `types` module belongs to: the one it declares (`pub struct`,
/// `pub enum`, `pub type`) or implements (`impl … for X`, `impl X`). `None` for anything else, such
/// as a helper function or a `use`.
fn owner_of(first_line: &str) -> Option<String> {
    let ident = |text: &str| -> String {
        text.chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect()
    };
    for prefix in ["pub struct ", "pub enum ", "pub type "] {
        if let Some(rest) = first_line.strip_prefix(prefix) {
            return Some(ident(rest));
        }
    }
    let rest = first_line.strip_prefix("impl")?;
    if let Some((_, target)) = rest.rsplit_once(" for ") {
        return Some(ident(target.trim_start()));
    }
    // `impl<T> X<T> {`: skip the generics the impl declares.
    let rest = match rest.strip_prefix('<') {
        Some(generics) => generics.split_once('>').map_or("", |(_, after)| after),
        None => rest,
    };
    Some(ident(rest.trim_start()))
}

/// Each type the generated module `code` declares in its `types` module that nothing else in the
/// module names and that is not one of `spec`'s components (#561, #926): a type the lowering built
/// for itself and then left behind, such as a union a meet replaced or a second lowering of a
/// schema a position did not take, is a public type of the output no field, variant, alias or
/// operation uses.
///
/// A type is named when an item other than its own declaration and impls mentions it, inside
/// `types` or anywhere else in the module (an operation's parameters and bodies are named by the
/// client). A component is the document's own vocabulary, so it is emitted whether or not anything
/// uses it; the Rust name allocated to component `my_pet` is `MyPet`, so names are compared
/// [`normalized`], allowing a numeric suffix a name collision adds, and the hash suffix a
/// component takes when its name collides with a reserved one.
pub fn unreferenced_types(code: &str, spec: &str) -> Vec<String> {
    let Some(start) = code.find("pub mod types {") else {
        return Vec::new();
    };
    let body_start = start + "pub mod types {".len();
    let body_len = code[body_start..]
        .find("\n}")
        .unwrap_or(code.len() - body_start);
    let body = &code[body_start..body_start + body_len];
    // The client after the module; the embedded runtime before it is no use of a generated type.
    let outside = &code[body_start + body_len..];
    // The top-level items of `types`, each with the type it belongs to: an item runs from its
    // first line to the line its braces close on, or that ends it with `;`.
    let mut items: Vec<(Option<String>, String)> = Vec::new();
    let (mut depth, mut current, mut owner): (i64, String, Option<Option<String>>) =
        (0, String::new(), None);
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        current.push_str(line);
        current.push('\n');
        if trimmed.starts_with("//") || (depth == 0 && trimmed.starts_with("#[")) {
            continue;
        }
        if owner.is_none() {
            owner = Some(owner_of(trimmed));
        }
        depth += trimmed.matches('{').count() as i64 - trimmed.matches('}').count() as i64;
        if depth <= 0 && (trimmed.ends_with('}') || trimmed.ends_with(';')) {
            items.push((owner.take().flatten(), std::mem::take(&mut current)));
            depth = 0;
        }
    }
    let declared: BTreeSet<&str> = items
        .iter()
        .filter(|(owner, text)| {
            owner.is_some()
                && text.lines().any(|line| {
                    let line = line.trim();
                    ["pub struct ", "pub enum ", "pub type "]
                        .iter()
                        .any(|prefix| line.starts_with(prefix))
                })
        })
        .filter_map(|(owner, _)| owner.as_deref())
        .collect();
    let components: Vec<String> = component_names(spec)
        .iter()
        .map(|name| normalized(name))
        .collect();
    // A component's own name, or that name with a numeric collision suffix.
    let names_component = |name: &str, component: &str| {
        name.strip_prefix(component)
            .is_some_and(|suffix| suffix.chars().all(|c| c.is_ascii_digit()))
    };
    let is_component = |name: &str| {
        let name = normalized(name);
        components.iter().any(|component| {
            names_component(&name, component)
                // A component whose name collides with one the module reserves (`Box`) is
                // allocated its name with the stable 8-hex-digit suffix instead (`Box00ba9f5f`),
                // but only when no declared type has the component's own name: otherwise the
                // suffixed type is a second lowering of the component's schema (#926).
                || (name.strip_prefix(component.as_str()).is_some_and(|suffix| {
                    suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_hexdigit())
                }) && !declared
                    .iter()
                    .any(|other| names_component(&normalized(other), component)))
        })
    };
    declared
        .iter()
        .copied()
        .filter(|name| !is_component(name))
        .filter(|name| {
            !names(outside, name)
                && !items
                    .iter()
                    .any(|(owner, text)| owner.as_deref() != Some(*name) && names(text, name))
        })
        .map(|name| format!("`{name}` is emitted, but nothing names it and it is no component"))
        .collect()
}

/// The [`unreferenced_types`] that are a second copy of a type: a schema lowered again under the
/// name a first lowering already took is allocated that name with the stable 8-hex-digit suffix
/// of its pointer (`S071def3c4`, `ResponseBody9c134e93`), and where the position takes the first
/// lowering, the copy is elided (#926).
pub fn unreferenced_copies(code: &str, spec: &str) -> Vec<String> {
    unreferenced_types(code, spec)
        .into_iter()
        .filter(|reason| {
            let name = reason.split('`').nth(1).unwrap_or_default();
            let Some(split) = name
                .len()
                .checked_sub(8)
                .filter(|&split| split > 0 && name.is_char_boundary(split))
            else {
                return false;
            };
            let suffix = &name[split..];
            suffix.chars().all(|c| c.is_ascii_hexdigit())
                && suffix.chars().any(|c| c.is_ascii_digit())
        })
        .collect()
}

/// The names `spec` declares under any `components` section (`schemas`, `responses`, …), in
/// document order.
pub fn component_names(spec: &str) -> Vec<String> {
    let Some(mut root) = YamlLoader::load_from_str(spec)
        .ok()
        .and_then(|documents| documents.into_iter().next())
    else {
        return Vec::new();
    };
    let Some(Yaml::Hash(sections)) = entry(&mut root, "components") else {
        return Vec::new();
    };
    sections
        .values()
        .filter_map(Yaml::as_hash)
        .flat_map(|entries| entries.keys())
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect()
}
