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
    /// Mark the escaped spelling of `hint` as occupied without disambiguating it.
    ///
    /// This is used when a binding is already part of an externally-derived surface and later
    /// generator-owned bindings must yield to it.
    pub fn reserve(&mut self, hint: &str, role: IdentRole) {
        let ident = super::escape(hint, role);
        self.used.insert(ident.as_str().to_owned());
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
    fn allocation_yields_to_reserved_identifier() {
        let pointer = JsonPointer::from("/paths/~1files/get");
        let mut scope = Scope::default();
        scope.reserve("path", IdentRole::Param);

        let allocated = scope.alloc("path", IdentRole::Param, &pointer);

        assert_ne!(allocated.as_str(), "path");
        assert!(allocated.as_str().starts_with("path_"));
    }

    #[test]
    fn a_collision_is_disambiguated_from_the_pointer_not_from_arrival_order() {
        // The disambiguator is seeded by the item's JSON Pointer precisely so that reordering the
        // spec does not rename anything. Two scopes fed the same hints in *opposite* orders must
        // therefore agree on which identifier each pointer received.
        let first = JsonPointer::from("/components/schemas/Pet");
        let second = JsonPointer::from("/components/schemas/Order");

        let mut forward = Scope::default();
        let forward_first = forward.alloc("id", IdentRole::Field, &first);
        let forward_second = forward.alloc("id", IdentRole::Field, &second);

        let mut backward = Scope::default();
        let backward_second = backward.alloc("id", IdentRole::Field, &second);
        let backward_first = backward.alloc("id", IdentRole::Field, &first);

        // The winner of the bare name is order-dependent by construction, but the *loser* must be
        // named from its own pointer rather than from a counter.
        assert_eq!(forward_first.as_str(), "id");
        assert_eq!(backward_second.as_str(), "id");
        assert!(forward_second.as_str().starts_with("id_"));
        assert!(backward_first.as_str().starts_with("id_"));
        assert_ne!(forward_second.as_str(), backward_first.as_str());
    }

    #[test]
    fn a_repeated_pointer_still_terminates_with_a_distinct_identifier() {
        // Same hint, same pointer, three times: the stable suffix collides with itself, so the
        // counter path is the only thing that keeps allocation injective.
        let pointer = JsonPointer::from("/components/schemas/Pet");
        let mut scope = Scope::default();
        let allocated: Vec<String> = (0..3)
            .map(|_| {
                scope
                    .alloc("id", IdentRole::Field, &pointer)
                    .as_str()
                    .to_owned()
            })
            .collect();

        let unique: std::collections::HashSet<&String> = allocated.iter().collect();
        assert_eq!(unique.len(), 3, "{allocated:?}");
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

        /// Byte-identical output requires the whole allocation sequence to be reproducible, not
        /// just each name in isolation: two scopes fed the same hints and pointers must produce
        /// the same identifiers, including the disambiguated ones.
        #[test]
        fn allocation_sequences_are_deterministic(
            hints in proptest::collection::vec("[A-Za-z0-9_ -]{0,24}", 1..64)
        ) {
            let allocate = || {
                let mut scope = Scope::default();
                hints
                    .iter()
                    .enumerate()
                    .map(|(index, hint)| {
                        let pointer = JsonPointer::root().push(&index.to_string());
                        scope.alloc(hint, IdentRole::Field, &pointer).as_str().to_owned()
                    })
                    .collect::<Vec<String>>()
            };
            prop_assert_eq!(allocate(), allocate());
        }

        /// Every allocated identifier — the disambiguated ones included — must still be a legal
        /// Rust identifier. The suffix path rebuilds through `escape`, and this is what proves it.
        #[test]
        fn allocated_identifiers_are_always_legal(
            hints in proptest::collection::vec("[A-Za-z0-9_ -]{0,24}", 1..32)
        ) {
            let mut scope = Scope::default();
            for (index, hint) in hints.iter().enumerate() {
                let pointer = JsonPointer::root().push(&index.to_string());
                let ident = scope.alloc(hint, IdentRole::Field, &pointer);
                let text = ident.as_str();
                let parsed = text.parse::<proc_macro2::TokenStream>();
                prop_assert!(parsed.is_ok(), "{text:?} does not lex");
                let mut tokens = parsed.expect("checked").into_iter();
                prop_assert!(matches!(tokens.next(), Some(proc_macro2::TokenTree::Ident(_))));
                prop_assert!(tokens.next().is_none(), "{text:?} is more than one token");
            }
        }
    }
}
