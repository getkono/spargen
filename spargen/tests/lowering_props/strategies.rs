//! The union and `allOf` values `lowering_props.rs` generates, and the strategies that generate
//! them. `determinism.rs` declares this file too (`#[path = "lowering_props/strategies.rs"]`), so
//! the documents it holds to byte-identical output are drawn from the same shapes. Each file
//! renders the values into a document itself: these are the shapes, not their YAML.

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;

/// A JSON primitive category to place in a union variant. `Integer` and `Number` deliberately share
/// the numeric wire category — the lowering must never treat them as disjoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    String,
    Integer,
    Number,
    Boolean,
    Array,
}

pub fn category_strategy() -> impl Strategy<Value = Category> {
    prop_oneof![
        Just(Category::String),
        Just(Category::Integer),
        Just(Category::Number),
        Just(Category::Boolean),
        Just(Category::Array),
    ]
}

/// The candidate property names for the closed-object variants and the `allOf` members. Each is a
/// single lowercase letter so its wire name equals its Rust field ident.
pub const KEYS: [&str; 4] = ["a", "b", "c", "d"];

/// The required keys of one closed-object variant, as indices into [`KEYS`].
pub fn key_set_strategy() -> impl Strategy<Value = BTreeSet<usize>> {
    proptest::collection::btree_set(0usize..KEYS.len(), 1..=KEYS.len())
}

/// The type of one `allOf` member property. `string` and `integer` share no value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PropType {
    String,
    Integer,
}

/// One `allOf` member: property name (an index into [`KEYS`]) → (type, required).
pub type Member = BTreeMap<usize, (PropType, bool)>;

pub fn member_strategy() -> impl Strategy<Value = Member> {
    proptest::collection::btree_map(
        0usize..KEYS.len(),
        (
            prop_oneof![Just(PropType::String), Just(PropType::Integer)],
            any::<bool>(),
        ),
        1..=KEYS.len(),
    )
}
