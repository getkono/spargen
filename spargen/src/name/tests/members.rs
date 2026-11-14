//! A struct's field and a string enum's variant identifiers, allocated by rank rather than by
//! declaration order (#1422).

use super::{allocate, api, JsonPointer, Prim, Provenance, Ty, TypeDef, TypeKind};

/// The identifier `allocate` gives each member of one struct (`variant` unset) or one closed
/// string enum (`variant` set) declaring `members` in that order, by wire name.
fn member_idents(members: &[String], variant: bool) -> std::collections::BTreeMap<String, String> {
    use crate::ir::{
        AdditionalProps, Field, Openness, PropertyName, ScalarEnum, ScalarRepr, ScalarValue, Struct,
    };
    let mut api = api(Vec::new(), false);
    let text = Ty {
        id: api.types.iter().next().expect("the text type").0,
        nullable: false,
        boxed: false,
    };
    let kind = if variant {
        TypeKind::Enum(ScalarEnum {
            repr: ScalarRepr::String,
            variants: members.iter().cloned().map(ScalarValue::String).collect(),
            width: Prim::String,
            openness: Openness::Closed,
        })
    } else {
        TypeKind::Struct(Struct {
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
        })
    };
    let id = api.types.insert(TypeDef {
        name_hint: "Pet".to_owned(),
        kind,
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
    let table = if variant {
        &names.variants
    } else {
        &names.fields
    };
    members
        .iter()
        .map(|wire| (wire.clone(), table[&(id, wire.clone())].as_str().to_owned()))
        .collect()
}

proptest::proptest! {
    /// A struct's field and an enum's variant identifiers depend on the set of members and not on
    /// the order the spec declares them in, and inserting one more member anywhere moves only the
    /// identifier it outranks an existing member for: the bare spelling both escape to, held by a
    /// member whose wire name is not that spelling and sorts after the new one. That is the one
    /// rebinding `spargen diff` reports as major (#1422); every other existing member keeps its
    /// identifier.
    #[test]
    fn an_inserted_member_rebinds_only_the_spelling_it_outranks_a_member_for(
        members in proptest::collection::btree_set("[a-cA-C][-_ ]?[a-cA-C]", 1..8),
        inserted in "[a-cA-C][-_ ]?[a-cA-C]",
        position in 0usize..8,
        rotation in 0usize..8,
        variant in proptest::bool::ANY,
    ) {
        proptest::prop_assume!(!members.contains(&inserted));
        let role = if variant {
            crate::name::IdentRole::Variant
        } else {
            crate::name::IdentRole::Field
        };
        let bare = |wire: &str| crate::name::escape(wire, role).unraw().to_owned();
        let rank = |wire: &str| (bare(wire) != wire, wire.to_owned());

        let sorted: Vec<String> = members.iter().cloned().collect();
        let mut rotated = sorted.clone();
        rotated.rotate_left(rotation % sorted.len());
        let old = member_idents(&sorted, variant);
        proptest::prop_assert_eq!(&member_idents(&rotated, variant), &old);

        let mut grown = rotated.clone();
        grown.insert(position % (grown.len() + 1), inserted.clone());
        let new = member_idents(&grown, variant);
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
