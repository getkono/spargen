//! A struct's field, a string enum's variant and a union's variant identifiers, allocated by rank
//! rather than by declaration order (#1422).

use super::{allocate, api, JsonPointer, Prim, Provenance, Ty, TypeDef, TypeKind};

/// The kind of type whose members `member_idents` allocates.
#[derive(Debug, Clone, Copy)]
enum Members {
    /// A struct's fields, by wire property name.
    Fields,
    /// A closed string enum's variants, by wire value.
    EnumVariants,
    /// A trial union's variants, by name hint.
    UnionVariants,
}

/// The identifier `allocate` gives each member of one type of kind `kind` declaring `members` in
/// that order, by wire name (a union variant's name hint).
fn member_idents(members: &[String], kind: Members) -> std::collections::BTreeMap<String, String> {
    use crate::ir::{
        AdditionalProps, Field, Openness, PropertyName, ScalarEnum, ScalarRepr, ScalarValue,
        Struct, Union, UnionMode, UnionStrategy, UnionVariant, VariantDispatch,
    };
    let mut api = api(Vec::new(), false);
    let text = Ty {
        id: api.types.iter().next().expect("the text type").0,
        nullable: false,
        boxed: false,
    };
    let type_kind = match kind {
        Members::EnumVariants => TypeKind::Enum(ScalarEnum {
            repr: ScalarRepr::String,
            variants: members.iter().cloned().map(ScalarValue::String).collect(),
            width: Prim::String,
            openness: Openness::Closed,
        }),
        Members::UnionVariants => TypeKind::Union(Union {
            variants: members
                .iter()
                .map(|hint| UnionVariant {
                    name_hint: hint.clone(),
                    ty: text,
                    dispatch: VariantDispatch::Trial {
                        priority: 0,
                        category: None,
                    },
                })
                .collect(),
            strategy: UnionStrategy::Trial {
                mode: UnionMode::AnyOf,
            },
        }),
        Members::Fields => TypeKind::Struct(Struct {
            fields: members
                .iter()
                .map(|wire| Field {
                    name: PropertyName { wire: wire.clone() },
                    ty: text,
                    required: false,
                    deprecated: false,
                    read_only: false,
                    write_only: false,
                    default: None,
                    xml: Default::default(),
                    undeclared: false,
                })
                .collect(),
            additional: AdditionalProps::Allow,
        }),
    };
    let id = api.types.insert(TypeDef {
        name_hint: "Pet".to_owned(),
        kind: type_kind,
        docs: Default::default(),
        provenance: Provenance::new(
            JsonPointer::root()
                .push("components")
                .push("schemas")
                .push("Pet"),
            None,
        ),
        document: String::new(),
    });
    let names = allocate(&api);
    let table = match kind {
        Members::Fields => &names.fields,
        Members::EnumVariants | Members::UnionVariants => &names.variants,
    };
    members
        .iter()
        .map(|wire| (wire.clone(), table[&(id, wire.clone())].as_str().to_owned()))
        .collect()
}

proptest::proptest! {
    /// A struct's field, an enum's variant and a union's variant identifiers depend on the set of
    /// members and not on the order the spec declares them in, and inserting one more member
    /// anywhere moves only the identifier it outranks an existing member for: the bare spelling
    /// both escape to, held by a member whose wire name (a union variant's name hint) is not that
    /// spelling and sorts after the new one. That is the one rebinding `spargen diff` reports as
    /// major for a field or an enum variant (#1422); every other existing member keeps its
    /// identifier.
    #[test]
    fn an_inserted_member_rebinds_only_the_spelling_it_outranks_a_member_for(
        members in proptest::collection::btree_set("[a-cA-C][-_ ]?[a-cA-C]", 1..8),
        inserted in "[a-cA-C][-_ ]?[a-cA-C]",
        position in 0usize..8,
        rotation in 0usize..8,
        kind in proptest::sample::select(vec![
            Members::Fields,
            Members::EnumVariants,
            Members::UnionVariants,
        ]),
    ) {
        proptest::prop_assume!(!members.contains(&inserted));
        let role = match kind {
            Members::Fields => crate::name::IdentRole::Field,
            Members::EnumVariants | Members::UnionVariants => crate::name::IdentRole::Variant,
        };
        let bare = |wire: &str| crate::name::escape(wire, role).unraw().to_owned();
        let rank = |wire: &str| (bare(wire) != wire, wire.to_owned());

        let sorted: Vec<String> = members.iter().cloned().collect();
        let mut rotated = sorted.clone();
        rotated.rotate_left(rotation % sorted.len());
        let old = member_idents(&sorted, kind);
        proptest::prop_assert_eq!(&member_idents(&rotated, kind), &old);

        let mut grown = rotated.clone();
        grown.insert(position % (grown.len() + 1), inserted.clone());
        let new = member_idents(&grown, kind);
        for (wire, ident) in &old {
            if new[wire] == *ident {
                continue;
            }
            proptest::prop_assert_ne!(&bare(wire), wire, "{:?} is spelled as its identifier", wire);
            proptest::prop_assert_eq!(&bare(&inserted), &bare(wire));
            proptest::prop_assert_eq!(ident, &crate::name::escape(wire, role).as_str().to_owned());
            proptest::prop_assert!(
                rank(&inserted) < rank(wire),
                "{:?} does not outrank {:?}",
                inserted,
                wire
            );
        }
    }
}
