//! The union and `allOf` values `lowering_props.rs` generates, and the strategies that generate
//! them. `determinism.rs` declares this file too (`#[path = "lowering_props/strategies.rs"]`), so
//! the documents it holds to byte-identical output are drawn from the same shapes. Each file
//! renders the values into a document itself: these are the shapes, not their YAML. The nested
//! `allOf` members are `lowering_props.rs`'s alone, so each of their items carries
//! `#[allow(dead_code)]` for `determinism.rs`.

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;

/// A JSON category to place in a union variant. `Integer` and `Number` deliberately share the
/// numeric wire category — the lowering must never treat them as disjoint. `Object` is an object
/// nesting another object, and `Null` is the `null` branch, which admits `null` rather than being a
/// variant of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    String,
    Integer,
    Number,
    Boolean,
    Array,
    Object,
    Null,
}

pub fn category_strategy() -> impl Strategy<Value = Category> {
    prop_oneof![
        Just(Category::String),
        Just(Category::Integer),
        Just(Category::Number),
        Just(Category::Boolean),
        Just(Category::Array),
        Just(Category::Object),
        Just(Category::Null),
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

/// The type of one property of a nested `allOf` member: a scalar, `null`, or an object whose own
/// properties are typed the same way.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Nested {
    Scalar(PropType),
    Null,
    Object(NestedMember),
}

/// One nested `allOf` member, or the properties of a [`Nested::Object`]: property name (an index
/// into [`KEYS`]) → (type, required).
#[allow(dead_code)]
pub type NestedMember = BTreeMap<usize, (Nested, bool)>;

/// How many objects deep a [`Nested`] type may nest below its member.
#[allow(dead_code)]
const NESTING_DEPTH: u32 = 2;

/// The property keys a nested member or object draws from: fewer than [`KEYS`], so two members
/// repeat a property often and the meet has a field to reconcile at every depth.
#[allow(dead_code)]
const NESTED_KEYS: usize = 3;

/// A nested `allOf` member, its objects nested at most [`NESTING_DEPTH`] deep.
#[allow(dead_code)]
pub fn nested_member_strategy() -> impl Strategy<Value = NestedMember> {
    let leaf = prop_oneof![
        Just(Nested::Scalar(PropType::String)),
        Just(Nested::Scalar(PropType::Integer)),
        Just(Nested::Null),
    ];
    let ty = leaf.prop_recursive(NESTING_DEPTH, 16, NESTED_KEYS as u32, |inner| {
        proptest::collection::btree_map(
            0usize..NESTED_KEYS,
            (inner, any::<bool>()),
            1..=NESTED_KEYS,
        )
        .prop_map(Nested::Object)
    });
    proptest::collection::btree_map(0usize..NESTED_KEYS, (ty, any::<bool>()), 1..=NESTED_KEYS)
}
