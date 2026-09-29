use std::collections::HashSet;

use crate::diag::JsonPointer;

use super::{Ident, IdentRole};

/// A naming scope that allocates unique identifiers and resolves collisions deterministically.
///
/// On a clash, a stable disambiguator derived from the item's JSON Pointer is applied to the
/// *loser*. Which item loses is a separate question: [`Scope::alloc`] hands the bare spelling to
/// whichever item it is asked about first, so it is order-independent only where the call order
/// is. [`Scope::alloc_ranked`] decides the winner from a caller-supplied rank instead, which is
/// what keeps a name stable when the spec is reordered. Injectivity within a scope is a
/// property-tested invariant.
#[derive(Debug, Default)]
pub(crate) struct Scope {
    used: HashSet<String>,
}

/// One identifier request in a [`Scope::alloc_ranked`] batch.
#[derive(Debug)]
pub(crate) struct RankedRequest<'a, K> {
    /// The preferred spelling, before casing and escaping.
    pub(crate) hint: &'a str,
    /// Seeds the disambiguator when this request loses its spelling.
    pub(crate) provenance: &'a JsonPointer,
    /// Decides who keeps a contested spelling: the lowest rank wins it.
    pub(crate) rank: K,
}

impl Scope {
    /// Mark the escaped spelling of `hint` as occupied without disambiguating it.
    ///
    /// This is used when a binding is already part of an externally-derived surface and later
    /// generator-owned bindings must yield to it.
    pub(crate) fn reserve(&mut self, hint: &str, role: IdentRole) {
        let ident = super::escape(hint, role);
        self.used.insert(ident.as_str().to_owned());
    }

    /// Allocate a unique identifier for `hint` in `role`. If the cased/escaped name is already
    /// taken in this scope, `provenance` seeds a stable disambiguator.
    ///
    /// The first call for a spelling wins it, so a scope whose call order can follow document
    /// order — where a reordered mapping would rename a public item — uses
    /// [`Scope::alloc_ranked`] instead.
    pub(crate) fn alloc(&mut self, hint: &str, role: IdentRole, provenance: &JsonPointer) -> Ident {
        let base = super::escape(hint, role);
        if self.used.insert(base.as_str().to_owned()) {
            return base;
        }
        self.disambiguate(&base, role, provenance)
    }

    /// Allocate a whole batch of identifiers so that the outcome depends on the *set* of requests
    /// and not on the order the batch lists them in. The result is in request order.
    ///
    /// [`Scope::alloc`] gives a contested spelling to whichever request arrives first, and the
    /// provenance-seeded suffix only tells the losers apart; seeding it differently moves the
    /// suffix and leaves *who* is suffixed untouched. Here, every request contesting one spelling
    /// is ordered by `rank`, the lowest takes the bare spelling (unless the scope already holds it),
    /// and the rest are disambiguated in rank order after every winner is placed, so a suffixed
    /// spelling can never take a bare one another request wanted. Requests of equal rank fall back
    /// to request order, so a rank should be unique wherever order is not meaningful.
    pub(crate) fn alloc_ranked<K: Ord>(
        &mut self,
        requests: &[RankedRequest<'_, K>],
        role: IdentRole,
    ) -> Vec<Ident> {
        let bases: Vec<Ident> = requests
            .iter()
            .map(|request| super::escape(request.hint, role))
            .collect();
        let mut order: Vec<usize> = (0..requests.len()).collect();
        order.sort_by(|&left, &right| {
            bases[left]
                .as_str()
                .cmp(bases[right].as_str())
                .then_with(|| requests[left].rank.cmp(&requests[right].rank))
        });

        let mut allocated: Vec<Option<Ident>> = vec![None; requests.len()];
        let mut previous: Option<&str> = None;
        for &index in &order {
            let base = bases[index].as_str();
            let leads_its_spelling = previous != Some(base);
            previous = Some(base);
            if leads_its_spelling && self.used.insert(base.to_owned()) {
                allocated[index] = Some(bases[index].clone());
            }
        }
        for &index in &order {
            if allocated[index].is_none() {
                allocated[index] =
                    Some(self.disambiguate(&bases[index], role, requests[index].provenance));
            }
        }
        allocated
            .into_iter()
            .map(|ident| ident.expect("every request is allocated by one of the two passes"))
            .collect()
    }

    /// The first free pointer-seeded spelling of `base`, which is already taken.
    fn disambiguate(&mut self, base: &Ident, role: IdentRole, provenance: &JsonPointer) -> Ident {
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

    use super::{IdentRole, RankedRequest, Scope};

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

    #[test]
    fn a_ranked_contest_goes_to_the_lowest_rank_whatever_the_request_order() {
        let low = JsonPointer::from("/components/schemas/Shape");
        let high = JsonPointer::from("/paths/~1a/get");
        let request = |provenance, rank| RankedRequest {
            hint: "Shape",
            provenance,
            rank,
        };

        let forward =
            Scope::default().alloc_ranked(&[request(&low, 0), request(&high, 1)], IdentRole::Type);
        let backward =
            Scope::default().alloc_ranked(&[request(&high, 1), request(&low, 0)], IdentRole::Type);

        assert_eq!(forward[0].as_str(), "Shape");
        assert_eq!(backward[1].as_str(), "Shape");
        assert_eq!(forward[1], backward[0]);
        assert!(forward[1].as_str().starts_with("Shape"));
        assert_ne!(forward[1].as_str(), "Shape");
    }

    #[test]
    fn a_ranked_batch_still_yields_to_a_reserved_spelling() {
        let pointer = JsonPointer::from("/components/schemas/Shape");
        let mut scope = Scope::default();
        scope.reserve("Shape", IdentRole::Type);

        let allocated = scope.alloc_ranked(
            &[RankedRequest {
                hint: "Shape",
                provenance: &pointer,
                rank: 0,
            }],
            IdentRole::Type,
        );

        assert_ne!(allocated[0].as_str(), "Shape");
    }

    #[test]
    fn a_suffixed_loser_never_takes_a_bare_spelling_another_request_wanted() {
        // The loser's pointer-seeded spelling is precomputed here and then requested bare by a
        // third item ranked *after* both. Placing every winner before any loser is what keeps the
        // third item's bare spelling from depending on whether it was listed before the loser.
        let pointer = JsonPointer::from("/b");
        let mut probe = Scope::default();
        probe.reserve("Shape", IdentRole::Type);
        let loser_spelling = probe.alloc("Shape", IdentRole::Type, &pointer);
        let other = JsonPointer::from("/c");
        let requests = [
            RankedRequest {
                hint: "Shape",
                provenance: &JsonPointer::from("/a"),
                rank: 0,
            },
            RankedRequest {
                hint: "Shape",
                provenance: &pointer,
                rank: 1,
            },
            RankedRequest {
                hint: loser_spelling.as_str(),
                provenance: &other,
                rank: 2,
            },
        ];

        let allocated = Scope::default().alloc_ranked(&requests, IdentRole::Type);

        assert_eq!(allocated[2], loser_spelling);
        assert_ne!(allocated[1], loser_spelling);
    }

    proptest! {
        /// The property `alloc_ranked` exists for: with distinct ranks, permuting the batch permutes
        /// the result and changes nothing else.
        #[test]
        fn ranked_allocation_is_independent_of_request_order(
            hints in proptest::collection::vec("[A-Ca-c_ ]{0,3}", 1..24),
            rotation in 0usize..24,
        ) {
            let pointers: Vec<JsonPointer> = (0..hints.len())
                .map(|index| JsonPointer::root().push(&index.to_string()))
                .collect();
            let requests: Vec<RankedRequest<'_, usize>> = hints
                .iter()
                .zip(&pointers)
                .enumerate()
                .map(|(rank, (hint, provenance))| RankedRequest { hint, provenance, rank })
                .collect();
            let forward = Scope::default().alloc_ranked(&requests, IdentRole::Type);

            let mut order: Vec<usize> = (0..requests.len()).collect();
            order.rotate_left(rotation % requests.len());
            order.reverse();
            let permuted: Vec<RankedRequest<'_, usize>> = order
                .iter()
                .map(|&index| RankedRequest {
                    hint: requests[index].hint,
                    provenance: requests[index].provenance,
                    rank: requests[index].rank,
                })
                .collect();
            let backward = Scope::default().alloc_ranked(&permuted, IdentRole::Type);

            for (position, &index) in order.iter().enumerate() {
                prop_assert_eq!(&backward[position], &forward[index]);
            }
            let unique: std::collections::HashSet<&str> =
                forward.iter().map(|ident| ident.as_str()).collect();
            prop_assert_eq!(unique.len(), forward.len());
        }

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
