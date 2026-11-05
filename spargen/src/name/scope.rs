use std::borrow::Cow;
use std::collections::HashSet;

use crate::diag::{fnv1a64, JsonPointer};

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
    /// The first call for a spelling wins it. Model types, operation error types, success enums
    /// and parameters are allocated through [`Scope::alloc_ranked`] instead, so reordering their
    /// requests renames none of them; operation methods, `…Params` structs, URL accessors and
    /// response-header structs are allocated here, in operation order.
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
        let raw_base = base.unraw();
        let suffix = stable_suffix(&suffix_seed(provenance));
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
    format!("{:08x}", fnv1a64(input.as_bytes()) as u32)
}

/// The text [`stable_suffix`] hashes for `provenance`: the pointer with the type and subtype of
/// every media key it passes through ASCII-lowercased.
///
/// Media types are case-insensitive (RFC 9110 § 8.3.1), so a media key in any letter case is the
/// key its lowercase spelling is, and the suffix it seeds must not move with the spelling (#1153).
/// A media key is the reference token after a `content` token that holds an escaped `/` (`~1`);
/// the parameters after its first `;` are hashed as written, since a parameter value may be
/// case-sensitive. Folding happens on the escaped tokens, where `~0` and `~1` lowercase to
/// themselves, and a pointer naming no media key is hashed exactly as it reads.
fn suffix_seed(provenance: &JsonPointer) -> Cow<'_, str> {
    let pointer = provenance.as_str();
    let mut previous: Option<&str> = None;
    let mut folded: Option<String> = None;
    let mut offset = 0;
    for token in pointer.split('/') {
        if previous == Some("content") && token.contains("~1") {
            let essence = token.split(';').next().unwrap_or(token);
            if essence.bytes().any(|byte| byte.is_ascii_uppercase()) {
                let seed = folded.get_or_insert_with(|| pointer.to_owned());
                seed[offset..offset + essence.len()].make_ascii_lowercase();
            }
        }
        previous = Some(token);
        offset += token.len() + 1;
    }
    folded.map_or(Cow::Borrowed(pointer), Cow::Owned)
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
    fn the_stable_suffix_is_the_low_half_of_the_pointers_fnv_1a_hash() {
        // The suffix is part of every disambiguated public name, so its function is pinned, not
        // only its determinism: `a` and `foobar` are FNV-1a 64 reference vectors
        // (`0xaf63dc4c8601ec8c`, `0x85944171f73967e8`), and the empty input is the offset basis.
        assert_eq!(super::stable_suffix(""), "84222325");
        assert_eq!(super::stable_suffix("a"), "8601ec8c");
        assert_eq!(super::stable_suffix("foobar"), "f73967e8");
        assert_eq!(super::stable_suffix("/components/schemas/Pet"), "58d74607");
    }

    #[test]
    fn media_keys_differing_only_in_letter_case_seed_the_same_suffix() {
        // Media types are case-insensitive, so respelling a key's letter case renames no
        // disambiguated type (#1153): both spellings get the suffix the lowercase pointer seeds.
        let lowercase = JsonPointer::from("/paths/~1note/get/responses/200/content/text~1*");
        let respelled = JsonPointer::from("/paths/~1note/get/responses/200/content/tExt~1*");
        let allocate = |pointer: &JsonPointer| {
            let mut scope = Scope::default();
            scope.reserve("ResponseBody", IdentRole::Type);
            scope.alloc("ResponseBody", IdentRole::Type, pointer)
        };

        assert_ne!(allocate(&lowercase).as_str(), "ResponseBody");
        assert_eq!(allocate(&respelled), allocate(&lowercase));
    }

    #[test]
    fn the_suffix_seed_folds_only_a_media_keys_type_and_subtype() {
        let seed = |pointer: &str| super::suffix_seed(&JsonPointer::from(pointer)).into_owned();
        // Every media key a pointer passes through is folded, an encoding's nested one included.
        assert_eq!(
            seed("/paths/~1A/post/requestBody/content/Multipart~1Form-Data/encoding/Pet/headers/X/content/Application~1JSON"),
            "/paths/~1A/post/requestBody/content/multipart~1form-data/encoding/Pet/headers/X/content/application~1json",
        );
        // A parameter value may be case-sensitive, so only the essence before `;` is folded.
        assert_eq!(
            seed("/paths/~1a/get/responses/200/content/Text~1Plain;Charset=UTF-8"),
            "/paths/~1a/get/responses/200/content/text~1plain;Charset=UTF-8",
        );
        // A `content` token followed by no media key (a property named `content`) folds nothing.
        assert_eq!(
            seed("/components/schemas/Pet/properties/content/properties/Name"),
            "/components/schemas/Pet/properties/content/properties/Name",
        );
        assert_eq!(seed("/components/schemas/Pet"), "/components/schemas/Pet");
    }

    #[test]
    fn the_counter_after_a_repeated_suffix_counts_up_from_two() {
        // The counter's sequence is spelled into public names, so it is pinned exactly: injectivity
        // alone would also hold for a counter that skipped or reordered its values.
        let pointer = JsonPointer::from("/components/schemas/Pet");
        let mut scope = Scope::default();
        let allocated: Vec<String> = (0..5)
            .map(|_| {
                scope
                    .alloc("id", IdentRole::Field, &pointer)
                    .as_str()
                    .to_owned()
            })
            .collect();

        assert_eq!(
            allocated,
            [
                "id",
                "id_58d74607",
                "id_58d74607_2",
                "id_58d74607_3",
                "id_58d74607_4",
            ]
        );
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

    /// A hint biased to collide: two letters around an optional separator in either case, so
    /// distinct hints often case to one spelling, plus keywords and spellings that escape alike.
    fn hint() -> impl Strategy<Value = String> {
        prop_oneof![
            3 => "[a-bA-B][-_ ]?[a-b]",
            1 => proptest::sample::select(vec![
                "", "-", "1a", "type", "type_", "r#type", "Self", "self", "super", "crate", "_",
            ])
            .prop_map(str::to_owned),
        ]
    }

    fn role() -> impl Strategy<Value = IdentRole> {
        proptest::sample::select(vec![
            IdentRole::Type,
            IdentRole::Field,
            IdentRole::Variant,
            IdentRole::Method,
            IdentRole::Param,
        ])
    }

    /// A scope holding `reserved`, as `allocate` reserves its fixed names before any request.
    fn reserved_scope(reserved: &[String], role: IdentRole) -> Scope {
        let mut scope = Scope::default();
        for hint in reserved {
            scope.reserve(hint, role);
        }
        scope
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

        /// No two requests get one identifier, and none gets a spelling the scope reserved.
        #[test]
        fn allocations_are_injective(
            hints in proptest::collection::vec(hint(), 1..64),
            reserved in proptest::collection::vec(hint(), 0..8),
            role in role(),
        ) {
            let mut scope = reserved_scope(&reserved, role);
            let mut seen: std::collections::HashSet<String> = reserved
                .iter()
                .map(|hint| super::super::escape(hint, role).as_str().to_owned())
                .collect();
            for (index, hint) in hints.iter().enumerate() {
                let pointer = JsonPointer::root().push(&index.to_string());
                let ident = scope.alloc(hint, role, &pointer);
                prop_assert!(seen.insert(ident.as_str().to_owned()), "{ident:?} is taken");
            }
        }

        /// Byte-identical output requires the whole allocation sequence to be reproducible, not
        /// just each name in isolation: the same requests against the same reserved set produce
        /// the same identifiers, the disambiguated ones included, whatever order the set was
        /// reserved in.
        #[test]
        fn allocation_sequences_are_deterministic(
            hints in proptest::collection::vec(hint(), 1..64),
            reserved in proptest::collection::vec(hint(), 0..8),
            role in role(),
        ) {
            let allocate = |reserved: &[String]| {
                let mut scope = reserved_scope(reserved, role);
                hints
                    .iter()
                    .enumerate()
                    .map(|(index, hint)| {
                        let pointer = JsonPointer::root().push(&index.to_string());
                        scope.alloc(hint, role, &pointer).as_str().to_owned()
                    })
                    .collect::<Vec<String>>()
            };
            let mut backward = reserved.clone();
            backward.reverse();
            prop_assert_eq!(allocate(&reserved), allocate(&backward));
        }

        /// Every allocated identifier — the disambiguated ones included — must still be a legal
        /// Rust identifier. The suffix path rebuilds through `escape`, and this is what proves it.
        #[test]
        fn allocated_identifiers_are_always_legal(
            hints in proptest::collection::vec(hint(), 1..32),
            reserved in proptest::collection::vec(hint(), 0..8),
            role in role(),
        ) {
            let mut scope = reserved_scope(&reserved, role);
            for (index, hint) in hints.iter().enumerate() {
                let pointer = JsonPointer::root().push(&index.to_string());
                let ident = scope.alloc(hint, role, &pointer);
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
