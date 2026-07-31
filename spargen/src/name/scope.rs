use std::collections::HashSet;

use crate::diag::JsonPointer;

use super::{Ident, IdentRole};

/// A naming scope that allocates unique identifiers and resolves collisions deterministically.
///
/// On a clash, a stable disambiguator derived from the item's JSON Pointer is applied — being
/// order-independent, it stays deterministic under spec reordering. Injectivity within a
/// scope is a property-tested invariant.
#[derive(Debug, Default)]
pub struct Scope {
    used: HashSet<String>,
}

impl Scope {
    /// Reserve a fixed identifier in this scope so later allocations are disambiguated away from
    /// it. Returns the escaped identifier to make the reservation independently testable.
    pub(crate) fn reserve(&mut self, hint: &str, role: IdentRole) -> Ident {
        let ident = super::escape(hint, role);
        self.used.insert(ident.as_str().to_owned());
        ident
    }

    /// Allocate a unique identifier for `hint` in `role`. If the cased/escaped name is already
    /// taken in this scope, `provenance` seeds a stable disambiguator.
    pub fn alloc(&mut self, hint: &str, role: IdentRole, provenance: &JsonPointer) -> Ident {
        let base = super::escape(hint, role);
        if self.used.insert(base.as_str().to_owned()) {
            return base;
        }

        let raw_base = base.as_str().trim_start_matches("r#");
        let suffix = stable_suffix(provenance.as_str());
        let mut candidate = super::escape(&format!("{raw_base}_{suffix}"), role);
        let mut counter = 2usize;
        while !self.used.insert(candidate.as_str().to_owned()) {
            candidate = super::escape(&format!("{raw_base}_{suffix}_{counter}"), role);
            counter += 1;
        }
        candidate
    }
}

fn stable_suffix(input: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", hash as u32)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use crate::diag::JsonPointer;

    use super::{IdentRole, Scope};

    #[test]
    fn allocation_disambiguates_names_reserved_by_the_signature() {
        let mut scope = Scope::default();
        assert_eq!(scope.reserve("body", IdentRole::Param).as_str(), "body");

        let allocated = scope.alloc(
            "body",
            IdentRole::Param,
            &JsonPointer::from("/paths/~1files/get/parameters/0"),
        );

        assert_ne!(allocated.as_str(), "body");
        assert!(allocated.as_str().starts_with("body_"));
    }

    proptest! {
        #[test]
        fn allocations_are_injective(hints in proptest::collection::vec("[A-Za-z0-9_ -]{0,24}", 1..64)) {
            let mut scope = Scope::default();
            let mut seen = std::collections::HashSet::new();
            for (index, hint) in hints.iter().enumerate() {
                let pointer = JsonPointer::root().push(&index.to_string());
                let ident = scope.alloc(hint, IdentRole::Field, &pointer);
                prop_assert!(seen.insert(ident.as_str().to_owned()));
            }
        }
    }
}
