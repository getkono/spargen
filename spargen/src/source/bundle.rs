use std::collections::VecDeque;
use std::sync::Arc;

use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexMap;

use crate::diag::{
    Aborted, Code, Diagnostic, Diagnostics, FileId, JsonPointer, Provenance, SourceSnippets,
};

use super::{parse_json, parse_yaml, Node, SpannedValue};

/// A single loaded source file: its path and full text. The text is shared (`Arc<str>`) so
/// rustc-style snippet rendering can borrow lines cheaply.
#[derive(Debug, Clone)]
pub struct SourceFile {
    /// Path as loaded (relative to the bundle root).
    pub path: Utf8PathBuf,
    /// Full file contents.
    pub text: Arc<str>,
}

impl SourceFile {
    /// The text of the 1-based `line`, if present.
    pub fn line_text(&self, line: u32) -> Option<&str> {
        self.text.lines().nth(line.checked_sub(1)? as usize)
    }
}

/// An input bundle: the root document plus every local file reachable through relative-file
/// `$ref`s. Absolute-URL `$ref`s are rejected; network fetches never happen —
/// builds are hermetic.
#[derive(Debug, Default)]
pub struct InputBundle {
    root: Option<FileId>,
    files: IndexMap<FileId, SourceFile>,
    values: IndexMap<FileId, SpannedValue>,
}

impl InputBundle {
    /// Load the root document at `root`, pulling in referenced local files on demand. The parse
    /// format is chosen by extension (`.json` vs `.yaml`/`.yml`). Diagnostics flow through `diags`.
    pub fn load(root: &Utf8Path, diags: &mut Diagnostics) -> Result<InputBundle, Aborted> {
        let mut bundle = InputBundle::default();
        let root = bundle.load_file(root.to_path_buf(), diags)?;
        bundle.root = Some(root);

        let mut queue = VecDeque::from([root]);
        while let Some(file) = queue.pop_front() {
            let refs = collect_refs(bundle.value_at(file));
            for reference in refs {
                if is_absolute_ref(&reference) {
                    Diagnostic::error(
                        Code::AbsoluteRefUnsupported,
                        Provenance::new(JsonPointer::root(), Some(bundle.value_at(file).span())),
                    )
                    .message(format!("absolute $ref `{reference}` is not supported"))
                    .remedy("vendor the referenced document locally and use a relative $ref")
                    .emit(diags);
                    continue;
                }
                let Some((target_path, _)) = split_ref(&reference) else {
                    continue;
                };
                if target_path.is_empty() {
                    continue;
                }
                let path = bundle.resolve_path(file, target_path);
                if bundle.file_id_by_path(&path).is_none() {
                    let loaded = bundle.load_file(path, diags)?;
                    queue.push_back(loaded);
                }
            }
        }

        diags.into_result(bundle)
    }

    /// The root document's value tree.
    pub fn root(&self) -> &SpannedValue {
        self.value_at(self.root.expect("input bundle root is loaded"))
    }

    /// The value tree of a loaded `file`.
    pub fn value_at(&self, file: FileId) -> &SpannedValue {
        self.values
            .get(&file)
            .expect("file id came from this input bundle")
    }

    /// Mutable value tree of a loaded `file`.
    pub(crate) fn value_at_mut(&mut self, file: FileId) -> &mut SpannedValue {
        self.values
            .get_mut(&file)
            .expect("file id came from this input bundle")
    }

    /// The loaded record for `file`, if present.
    pub fn file(&self, file: FileId) -> Option<&SourceFile> {
        self.files.get(&file)
    }

    /// The root document id.
    pub(crate) fn root_id(&self) -> FileId {
        self.root.expect("input bundle root is loaded")
    }

    /// Find a loaded file by its stored path, exact first and suffix second for ergonomic
    /// file-local omit rules.
    pub(crate) fn file_id_for_path(&self, path: &str) -> Option<FileId> {
        self.files
            .iter()
            .find_map(|(id, file)| (file.path.as_str() == path).then_some(*id))
            .or_else(|| {
                self.files
                    .iter()
                    .find_map(|(id, file)| file.path.as_str().ends_with(path).then_some(*id))
            })
    }
}

impl SourceSnippets for InputBundle {
    fn line_text(&self, file: FileId, line: u32) -> Option<&str> {
        self.file(file)?.line_text(line)
    }

    fn path(&self, file: FileId) -> Option<&Utf8Path> {
        self.file(file).map(|file| file.path.as_path())
    }
}

impl InputBundle {
    fn load_file(&mut self, path: Utf8PathBuf, diags: &mut Diagnostics) -> Result<FileId, Aborted> {
        if let Some(id) = self.file_id_by_path(&path) {
            return Ok(id);
        }
        let text = std::fs::read_to_string(&path).map_err(|error| {
            Diagnostic::error(
                Code::InvalidInput,
                Provenance::new(JsonPointer::root(), None),
            )
            .message(format!("failed to read `{path}`: {error}"))
            .emit(diags);
            Aborted
        })?;
        let id = FileId(self.files.len() as u32);
        let parsed = match path.extension() {
            Some("json") => parse_json(id, &text, diags)?,
            Some("yaml" | "yml") => parse_yaml(id, &text, diags)?,
            _ => parse_yaml(id, &text, diags).or_else(|_| parse_json(id, &text, diags))?,
        };
        self.files.insert(
            id,
            SourceFile {
                path,
                text: Arc::<str>::from(text),
            },
        );
        self.values.insert(id, parsed);
        Ok(id)
    }

    fn file_id_by_path(&self, path: &Utf8Path) -> Option<FileId> {
        self.files
            .iter()
            .find_map(|(id, file)| (file.path == path).then_some(*id))
    }

    fn resolve_path(&self, base: FileId, path: &str) -> Utf8PathBuf {
        let base_path = &self.file(base).expect("base file exists").path;
        let parent = base_path.parent().unwrap_or_else(|| Utf8Path::new(""));
        parent.join(path)
    }
}

fn collect_refs(value: &SpannedValue) -> Vec<String> {
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

fn split_ref(reference: &str) -> Option<(&str, &str)> {
    if let Some((path, fragment)) = reference.split_once('#') {
        if fragment.is_empty() {
            Some((path, ""))
        } else {
            Some((path, fragment))
        }
    } else {
        Some((reference, ""))
    }
}

fn is_absolute_ref(reference: &str) -> bool {
    reference.starts_with("http://")
        || reference.starts_with("https://")
        || reference
            .split_once(':')
            .is_some_and(|(scheme, _)| scheme.chars().all(|ch| ch.is_ascii_alphabetic()))
}
