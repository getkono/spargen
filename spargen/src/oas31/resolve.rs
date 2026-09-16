use std::borrow::Cow;

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, Provenance};
use crate::source::{rewrite_refs_absolute, InputBundle, SpannedValue};

use super::{deserialize::parse_schema, Document, Schema};

/// Resolves `$ref`s within a [`Document`] and its input bundle.
#[derive(Debug)]
pub(crate) struct Resolver<'doc> {
    document: &'doc Document,
    bundle: &'doc InputBundle,
}

/// A resolved reference target. Component refs borrow the target schema from the document; a remote
/// ref yields a schema parsed on the fly from its vendored copy (owned).
#[derive(Debug)]
pub(crate) struct Resolved<'doc> {
    /// The target schema.
    pub(crate) schema: Cow<'doc, Schema>,
}

impl<'doc> Resolver<'doc> {
    /// Build a resolver over a document and its bundle.
    pub(crate) fn new(document: &'doc Document, bundle: &'doc InputBundle) -> Self {
        Self { document, bundle }
    }

    /// The root document's file id, which is the bundle's own authority on the question.
    ///
    /// `lower` needs it to tell a reference written in the root document from one written in a
    /// referenced sub-file, and the two branches that distinction gates both retarget a `$ref`
    /// silently when it is wrong. It re-derived the answer as a hardcoded `FileId(0)`, correct only
    /// because `InputBundle::load` happens to load the root before anything else; a constructor that
    /// ever pre-loads a file — an in-memory bundle, a vendored preload, a test harness — would make
    /// it wrong with no test to notice. The authority is one call away, so ask it.
    pub(super) fn root_id(&self) -> crate::diag::FileId {
        self.bundle.root_id()
    }

    /// The path of `file`, when `file` declares a schema component called `name` of its own.
    ///
    /// A JSON Pointer fragment addresses the document it appears in, so a sub-file's own
    /// `#/components/schemas/<name>` is a reference to that file's declaration. spargen consults the
    /// root document's component map first, which means a name both documents declare resolves to
    /// the root's and the sub-file's is never read. That precedence is deliberate, but it is a
    /// decision about the *document*, and the reader has to be told which of the two declarations
    /// was used. This answers the second half — the path is returned rather than a bare `bool`
    /// because a diagnostic that says only "shadowed" names neither namespace.
    pub(super) fn declares_locally(
        &self,
        file: crate::diag::FileId,
        name: &str,
    ) -> Option<&camino::Utf8Path> {
        let reference = format!("#/components/schemas/{name}");
        let (target, pointer) = self.bundle.reference_target(&reference, file)?;
        // An in-document fragment always resolves to the file it is written in; anything else is
        // not a local declaration and is not what this asks about.
        if target != file {
            return None;
        }
        self.bundle.value_at(file).pointer(&pointer)?;
        Some(self.bundle.file(file)?.path.as_path())
    }

    /// Resolve a `$ref` string that appears at `at`, reporting an unresolved/unpinned ref through
    /// `diags`. Remote (`http`/`https`) refs are resolved hermetically from the vendored, hash-
    /// pinned copy already loaded into the bundle — no network access.
    pub(crate) fn resolve(
        &self,
        reference: &str,
        at: &Provenance,
        diags: &mut Diagnostics,
    ) -> Result<Resolved<'doc>, Aborted> {
        let from = at
            .span
            .map(|span| span.file)
            .unwrap_or_else(|| self.bundle.root_id());

        // Root component refs borrow the already-parsed component so named types and recursion are
        // shared. Every other JSON Pointer (including local relative files) is parsed from its
        // source node with that node's own provenance.
        if from == self.bundle.root_id() {
            if let Some(name) = reference.strip_prefix("#/components/schemas/") {
                if let Some(super::RefOr::Item(schema)) = self.document.components.schemas.get(name)
                {
                    return Ok(Resolved {
                        schema: Cow::Borrowed(schema),
                    });
                }
            }
        }

        self.resolve_bundle(reference, from, at, diags)
    }

    /// Resolve a remote `$ref` against the vendored document already loaded into the bundle. The
    /// fragment (a JSON Pointer) selects a subtree, whose `$ref`s are rewritten to absolute URLs so
    /// nested remote/relative refs resolve against the vendored doc's own URL, then parsed to a
    /// [`Schema`]. If the vendored doc is absent the bundle load already rejected it (`E003`/`E021`)
    /// and aborted; this only re-checks defensively.
    /// Resolve a Path Item `$ref` into a parsed Path Item.
    ///
    /// Path Item references are ordinary bundle references: they may address a
    /// `#/components/pathItems/` entry, a pointer inside any loaded file, or a whole relative
    /// file — the shape the multi-file layouts in the wild actually use.
    pub(crate) fn resolve_path_item(
        &self,
        reference: &str,
        from: crate::diag::FileId,
        at: &Provenance,
        diags: &mut Diagnostics,
    ) -> Option<super::PathItem> {
        let Some((file, pointer)) = self.bundle.reference_target(reference, from) else {
            Diagnostic::error(Code::UnresolvedRef, at.clone())
                .message(format!(
                    "unsupported or unresolved Path Item `$ref` `{reference}`"
                ))
                .emit(diags);
            return None;
        };
        let Some(node) = self.bundle.value_at(file).pointer(&pointer) else {
            Diagnostic::error(Code::UnresolvedRef, at.clone())
                .message(format!(
                    "Path Item `$ref` target `{reference}` was not found in the input bundle"
                ))
                .emit(diags);
            return None;
        };
        super::deserialize::parse_path_item(&node.clone(), &pointer, diags)
    }

    /// Resolve any non-component `$ref` into the parsed object at its target.
    ///
    /// Multi-file API descriptions commonly reference a whole file — `../responses/Error.yaml` —
    /// rather than a `#/components/...` entry, so component aliases fall back to the bundle the
    /// same way schema references already do.
    pub(crate) fn resolve_component<T>(
        &self,
        reference: &str,
        from: crate::diag::FileId,
        parse: impl Fn(&SpannedValue, &crate::diag::JsonPointer, &mut Diagnostics) -> Option<T>,
        diags: &mut Diagnostics,
    ) -> Option<T> {
        let (file, pointer) = self.bundle.reference_target(reference, from)?;
        let node = self.bundle.value_at(file).pointer(&pointer)?;
        parse(&node.clone(), &pointer, diags)
    }

    fn resolve_bundle(
        &self,
        reference: &str,
        from: crate::diag::FileId,
        at: &Provenance,
        diags: &mut Diagnostics,
    ) -> Result<Resolved<'doc>, Aborted> {
        let Some((file, pointer)) = self.bundle.reference_target(reference, from) else {
            Diagnostic::error(Code::UnresolvedRef, at.clone())
                .message(format!("unsupported or unresolved $ref `{reference}`"))
                .emit(diags);
            return Err(Aborted);
        };
        let Some(node) = self.bundle.value_at(file).pointer(&pointer) else {
            Diagnostic::error(Code::UnresolvedRef, at.clone())
                .message(format!(
                    "$ref target `{reference}` was not found in the input bundle"
                ))
                .emit(diags);
            return Err(Aborted);
        };
        let mut node = node.clone();
        if let Some(base_url) = self.bundle.remote_origin(file) {
            rewrite_refs_absolute(&mut node, base_url);
        }
        let Some(schema) = parse_schema(&node, &pointer, diags) else {
            return Err(Aborted);
        };
        Ok(Resolved {
            schema: Cow::Owned(schema),
        })
    }
}
