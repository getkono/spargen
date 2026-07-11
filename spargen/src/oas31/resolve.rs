use crate::diag::{Aborted, Code, Diagnostic, Diagnostics, JsonPointer, Provenance};
use crate::source::InputBundle;

use super::{Document, Schema};

/// Resolves `$ref`s within a [`Document`] and its input bundle.
#[derive(Debug)]
pub struct Resolver<'doc> {
    document: &'doc Document,
    bundle: &'doc InputBundle,
}

/// A resolved reference target.
#[derive(Debug)]
pub struct Resolved<'doc> {
    /// The target schema.
    pub schema: &'doc Schema,
}

impl<'doc> Resolver<'doc> {
    /// Build a resolver over a document and its bundle.
    pub fn new(document: &'doc Document, bundle: &'doc InputBundle) -> Self {
        Self { document, bundle }
    }

    /// Resolve a `$ref` string that appears at `at`, reporting an unresolved/absolute ref through
    /// `diags`.
    pub fn resolve(
        &self,
        reference: &str,
        at: &JsonPointer,
        diags: &mut Diagnostics,
    ) -> Result<Resolved<'doc>, Aborted> {
        let _ = self.bundle.root_id();
        if is_absolute_ref(reference) {
            Diagnostic::error(
                Code::AbsoluteRefUnsupported,
                Provenance::new(at.clone(), self.document.provenance.span),
            )
            .message(format!("absolute $ref `{reference}` is not supported"))
            .emit(diags);
            return Err(Aborted);
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
        Ok(Resolved { schema })
    }
}

fn is_absolute_ref(reference: &str) -> bool {
    reference.starts_with("http://")
        || reference.starts_with("https://")
        || reference
            .split_once(':')
            .is_some_and(|(scheme, _)| scheme.chars().all(|ch| ch.is_ascii_alphabetic()))
}
