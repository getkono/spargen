use std::borrow::Cow;

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, Provenance};
use crate::source::{rewrite_refs_absolute, InputBundle};

use super::{deserialize::parse_schema, Document, Schema};

/// Resolves `$ref`s within a [`Document`] and its input bundle.
#[derive(Debug)]
pub struct Resolver<'doc> {
    document: &'doc Document,
    bundle: &'doc InputBundle,
}

/// A resolved reference target. Component refs borrow the target schema from the document; a remote
/// ref yields a schema parsed on the fly from its vendored copy (owned).
#[derive(Debug)]
pub struct Resolved<'doc> {
    /// The target schema.
    pub schema: Cow<'doc, Schema>,
}

impl<'doc> Resolver<'doc> {
    /// Build a resolver over a document and its bundle.
    pub fn new(document: &'doc Document, bundle: &'doc InputBundle) -> Self {
        Self { document, bundle }
    }

    /// Resolve a `$ref` string that appears at `at`, reporting an unresolved/unpinned ref through
    /// `diags`. Remote (`http`/`https`) refs are resolved hermetically from the vendored, hash-
    /// pinned copy already loaded into the bundle — no network access.
    pub fn resolve(
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
