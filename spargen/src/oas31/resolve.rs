use std::borrow::Cow;

use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, JsonPointer, Provenance};
use crate::source::{is_remote_ref, remote_split_fragment, rewrite_refs_absolute, InputBundle};

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
        at: &JsonPointer,
        diags: &mut Diagnostics,
    ) -> Result<Resolved<'doc>, Aborted> {
        let _ = self.bundle.root_id();
        if is_remote_ref(reference) {
            return self.resolve_remote(reference, at, diags);
        }

        let Some(name) = reference.strip_prefix("#/components/schemas/") else {
            Diagnostic::error(
                Code::UnresolvedRef,
                Provenance::new(at.clone(), self.document.provenance.span),
            )
            .message(format!("unsupported or unresolved $ref `{reference}`"))
            .emit(diags);
            return Err(Aborted);
        };
        let Some(target) = self.document.components.schemas.get(name) else {
            Diagnostic::error(
                Code::UnresolvedRef,
                Provenance::new(at.clone(), self.document.provenance.span),
            )
            .message(format!("unresolved schema component `$ref` `{reference}`"))
            .emit(diags);
            return Err(Aborted);
        };
        let super::RefOr::Item(schema) = target else {
            Diagnostic::error(
                Code::UnresolvedRef,
                Provenance::new(at.clone(), self.document.provenance.span),
            )
            .message(format!(
                "nested component $ref `{reference}` is not resolved yet"
            ))
            .emit(diags);
            return Err(Aborted);
        };
        Ok(Resolved {
            schema: Cow::Borrowed(schema),
        })
    }

    /// Resolve a remote `$ref` against the vendored document already loaded into the bundle. The
    /// fragment (a JSON Pointer) selects a subtree, whose `$ref`s are rewritten to absolute URLs so
    /// nested remote/relative refs resolve against the vendored doc's own URL, then parsed to a
    /// [`Schema`]. If the vendored doc is absent the bundle load already rejected it (`E003`/`E021`)
    /// and aborted; this only re-checks defensively.
    fn resolve_remote(
        &self,
        reference: &str,
        at: &JsonPointer,
        diags: &mut Diagnostics,
    ) -> Result<Resolved<'doc>, Aborted> {
        let (base_url, fragment) = remote_split_fragment(reference);
        let Some(file) = self.bundle.remote_file(base_url) else {
            Diagnostic::error(
                Code::AbsoluteRefUnsupported,
                Provenance::new(at.clone(), self.document.provenance.span),
            )
            .message(format!("remote $ref `{reference}` is not pinned"))
            .remedy("run `spargen lock <spec>` to fetch, vendor, and pin it")
            .emit(diags);
            return Err(Aborted);
        };
        let pointer = JsonPointer::from(fragment.to_owned());
        let Some(node) = self.bundle.value_at(file).pointer(&pointer) else {
            Diagnostic::error(
                Code::UnresolvedRef,
                Provenance::new(at.clone(), self.document.provenance.span),
            )
            .message(format!(
                "remote $ref fragment `#{fragment}` was not found in the vendored document for `{base_url}`"
            ))
            .emit(diags);
            return Err(Aborted);
        };
        // Rewrite refs on a clone so nested refs inside the vendored doc become absolute URLs (they
        // were already loaded during bundle walk), then parse into a typed schema.
        let mut node = node.clone();
        rewrite_refs_absolute(&mut node, base_url);
        let Some(schema) = parse_schema(&node, at, diags) else {
            return Err(Aborted);
        };
        Ok(Resolved {
            schema: Cow::Owned(schema),
        })
    }
}
