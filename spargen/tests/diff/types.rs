//! Public model types: their presence, generation kind, enum variants, union payloads, field
//! requiredness, and typed `additionalProperties` maps.

use super::*;

const PET_AND_OWNER: &str = "    Pet:
      type: object
      required: [id]
      properties:
        id: { type: integer }
        owner: { $ref: '#/components/schemas/Owner' }
    Owner:
      type: object
      required: [name]
      properties:
        name: { type: string }
";

#[test]
fn adding_a_public_type_is_minor_and_removing_one_is_major() {
    let without = full(&pets_get("listPets", "", PET_REF), PET_WITH_INLINE_OWNER);
    let with = full(&pets_get("listPets", "", PET_REF), PET_AND_OWNER);

    // The field pointing at the type changes with it, so each direction also reports that field.
    let added = diff(&without, &with);
    assert_eq!(
        kinds(&added),
        vec![ChangeKind::FieldTypeChanged, ChangeKind::TypeAdded],
        "{:?}",
        added.changes
    );

    let removed = diff(&with, &without);
    assert_eq!(
        kinds(&removed),
        vec![ChangeKind::TypeRemoved, ChangeKind::FieldTypeChanged],
        "{:?}",
        removed.changes
    );
    assert_eq!(removed.bump, Impact::Major);
}

const STATUS_ENUM_TWO: &str = "    Status:
      type: string
      enum: [active, retired]
";

const STATUS_ENUM_THREE: &str = "    Status:
      type: string
      enum: [active, retired, pending]
";

#[test]
fn changing_a_types_generation_kind_is_major() {
    // The same named type going from `struct` to `enum` breaks every construction and every field
    // access, even though the name is unchanged.
    let report = diff(
        &full(TWO_OPS, &with_status(STATUS_STRUCT)),
        &full(TWO_OPS, &with_status(STATUS_ENUM_TWO)),
    );
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::TypeKindChanged],
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn adding_an_enum_variant_is_minor_and_removing_one_is_major() {
    // The documented additive rule: a new value a consumer may now receive is minor.
    let added = diff(
        &full(TWO_OPS, &with_status(STATUS_ENUM_TWO)),
        &full(TWO_OPS, &with_status(STATUS_ENUM_THREE)),
    );
    assert_eq!(kinds(&added), vec![ChangeKind::VariantAdded]);
    assert_eq!(added.bump, Impact::Minor);

    let removed = diff(
        &full(TWO_OPS, &with_status(STATUS_ENUM_THREE)),
        &full(TWO_OPS, &with_status(STATUS_ENUM_TWO)),
    );
    assert_eq!(kinds(&removed), vec![ChangeKind::VariantRemoved]);
    assert_eq!(removed.bump, Impact::Major);
}

/// An integer set is a checked newtype (#1327): a listed value is an associated `const`, so a new
/// one is the additive rule's variant and a removed one breaks code naming it, while a new width
/// changes what `get` returns and `TryFrom` takes. A set turning into a plain `integer` turns the
/// newtype into an alias, a change of generation kind.
#[test]
fn a_checked_sets_values_and_width_are_its_surface() {
    let level = |schema: &str| full(TWO_OPS, &with_status(&format!("    Status: {schema}\n")));
    let two = level("{ type: integer, format: int32, enum: [1, 2] }");
    let three = level("{ type: integer, format: int32, enum: [1, 2, 3] }");
    let wide = level("{ type: integer, enum: [1, 2] }");
    let plain = level("{ type: integer, format: int32 }");

    let added = diff(&two, &three);
    assert_eq!(
        kinds(&added),
        vec![ChangeKind::VariantAdded],
        "{:?}",
        added.changes
    );
    assert_eq!(added.bump, Impact::Minor);

    let removed = diff(&three, &two);
    assert_eq!(kinds(&removed), vec![ChangeKind::VariantRemoved]);
    assert_eq!(removed.bump, Impact::Major);

    let widened = diff(&two, &wide);
    assert_eq!(
        kinds(&widened),
        vec![ChangeKind::FieldTypeChanged],
        "{:?}",
        widened.changes
    );
    assert_eq!(widened.changes[0].location, "Status.get()");
    assert_eq!(widened.bump, Impact::Major);

    let unchecked = diff(&two, &plain);
    assert!(
        kinds(&unchecked).contains(&ChangeKind::TypeKindChanged),
        "{:?}",
        unchecked.changes
    );
    assert_eq!(unchecked.bump, Impact::Major);
}

const UNION_STRING_OR_INT: &str = "    Status:
      oneOf:
        - { type: string }
        - { type: integer }
";

const UNION_STRING_OR_BOOL: &str = "    Status:
      oneOf:
        - { type: string }
        - { type: boolean }
";

#[test]
fn changing_a_union_variant_payload_type_is_major() {
    let report = diff(
        &full(TWO_OPS, &with_status(UNION_STRING_OR_INT)),
        &full(TWO_OPS, &with_status(UNION_STRING_OR_BOOL)),
    );
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::VariantTypeChanged],
        "a union payload change is one changed variant, not a remove and an add: {:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn flipping_a_field_between_required_and_optional_is_major_both_ways() {
    // `T` ↔ `Option<T>` on a public struct field.
    let required_name = spec("", "id, name", PET_PROPS, "");
    let optional_name = base();

    for (old, new) in [
        (&optional_name, &required_name),
        (&required_name, &optional_name),
    ] {
        let report = diff(old, new);
        assert_eq!(
            kinds(&report),
            vec![ChangeKind::FieldRequirednessChanged],
            "{:?}",
            report.changes
        );
        assert_eq!(report.bump, Impact::Major);
    }
}

#[test]
fn a_required_field_gaining_read_only_leaves_new_and_is_major() {
    // A `required` `readOnly` member is an `Option<T>` `new` no longer takes (#1326), so gaining
    // the keyword is the requiredness flip a consumer's constructor call sees.
    let plain = base();
    let read_only = spec(
        "",
        "id",
        "        id: { type: integer, readOnly: true }\n        name: { type: string }\n",
        "",
    );

    for (old, new) in [(&plain, &read_only), (&read_only, &plain)] {
        let report = diff(old, new);
        assert_eq!(
            kinds(&report),
            vec![ChangeKind::FieldRequirednessChanged],
            "{:?}",
            report.changes
        );
        assert_eq!(report.bump, Impact::Major);
    }
}

#[test]
fn a_required_field_gaining_write_only_changes_its_type_and_is_major() {
    // A `required` `writeOnly` member stays a `new` argument but is read back as an `Option<T>`.
    let plain = base();
    let write_only = spec(
        "",
        "id",
        "        id: { type: integer, writeOnly: true }\n        name: { type: string }\n",
        "",
    );

    for (old, new) in [(&plain, &write_only), (&write_only, &plain)] {
        let report = diff(old, new);
        assert_eq!(
            kinds(&report),
            vec![ChangeKind::FieldTypeChanged],
            "{:?}",
            report.changes
        );
        assert_eq!(report.bump, Impact::Major);
    }
}

#[test]
fn an_optional_field_becoming_nullable_or_no_longer_nullable_is_major_both_ways() {
    // `Option<T>` ↔ `Nullable<T>` on a public struct field.
    let optional_name = base();
    let nullable_name = spec(
        "",
        "id",
        "        id: { type: integer }\n        name: { type: [string, 'null'] }\n",
        "",
    );

    for (old, new) in [
        (&optional_name, &nullable_name),
        (&nullable_name, &optional_name),
    ] {
        let report = diff(old, new);
        assert_eq!(
            kinds(&report),
            vec![ChangeKind::FieldTypeChanged],
            "{:?}",
            report.changes
        );
        assert_eq!(report.bump, Impact::Major);
    }
}

#[test]
fn a_nullable_field_flipping_between_required_and_optional_changes_its_type_too() {
    // `Option<T>` (required, maybe `null`) ↔ `Nullable<T>` (optional and nullable).
    let props = "        id: { type: integer }\n        name: { type: [string, 'null'] }\n";
    let required_name = spec("", "id, name", props, "");
    let optional_name = spec("", "id", props, "");

    for (old, new) in [
        (&optional_name, &required_name),
        (&required_name, &optional_name),
    ] {
        let report = diff(old, new);
        assert_eq!(
            kinds(&report),
            vec![
                ChangeKind::FieldRequirednessChanged,
                ChangeKind::FieldTypeChanged
            ],
            "{:?}",
            report.changes
        );
        assert_eq!(report.bump, Impact::Major);
    }
}

/// The `/pets` spec whose `Pet` carries `additional` (an `additionalProperties:` line at 6-space
/// indent, or `""` for none).
fn with_additional(additional: &str) -> String {
    full(
        &pets_get("listPets", "", PET_REF),
        &format!("{PET_SCHEMA}{additional}"),
    )
}

const ADDITIONAL_STRING: &str = "      additionalProperties: { type: string }\n";

const ADDITIONAL_INTEGER: &str = "      additionalProperties: { type: integer }\n";

#[test]
fn changing_a_typed_additional_properties_map_is_major() {
    // The map is a public `BTreeMap<String, T>` field, so its value type is surface (#726).
    let report = diff(
        &with_additional(ADDITIONAL_STRING),
        &with_additional(ADDITIONAL_INTEGER),
    );
    assert_eq!(
        kinds(&report),
        vec![ChangeKind::AdditionalPropertiesTypeChanged],
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}

#[test]
fn adding_a_typed_additional_properties_map_is_minor_and_removing_one_is_major() {
    let added = diff(&with_additional(""), &with_additional(ADDITIONAL_STRING));
    assert_eq!(
        kinds(&added),
        vec![ChangeKind::AdditionalPropertiesAdded],
        "{:?}",
        added.changes
    );
    assert_eq!(added.bump, Impact::Minor);

    let removed = diff(&with_additional(ADDITIONAL_STRING), &with_additional(""));
    assert_eq!(
        kinds(&removed),
        vec![ChangeKind::AdditionalPropertiesRemoved],
        "{:?}",
        removed.changes
    );
    assert_eq!(removed.bump, Impact::Major);

    // `false` changes what a decode accepts and declares no field, so it is no surface change.
    let denied = diff(
        &with_additional(""),
        &with_additional("      additionalProperties: false\n"),
    );
    assert_eq!(kinds(&denied), vec![], "{:?}", denied.changes);
}

#[test]
fn a_typed_additional_properties_map_moved_to_another_field_is_removed_and_added() {
    // A declared `additional` property takes the overflow field's spelling, so the map moves to
    // another field name: the old field is gone and a new one appears (#726).
    let with_property = format!(
        "{}{ADDITIONAL_STRING}",
        PET_SCHEMA.replace(
            "        name: { type: string }\n",
            "        name: { type: string }\n        additional: { type: string }\n",
        )
    );
    let report = diff(
        &with_additional(ADDITIONAL_STRING),
        &full(&pets_get("listPets", "", PET_REF), &with_property),
    );
    let mut moved: Vec<_> = report
        .changes
        .iter()
        .filter(|change| {
            matches!(
                change.kind,
                ChangeKind::AdditionalPropertiesRemoved | ChangeKind::AdditionalPropertiesAdded
            )
        })
        .map(|change| (change.kind, change.location.as_str()))
        .collect();
    moved.sort_by_key(|(_, location)| *location);
    assert_eq!(moved.len(), 2, "{:?}", report.changes);
    assert_eq!(
        moved[0],
        (ChangeKind::AdditionalPropertiesRemoved, "Pet.additional"),
        "{:?}",
        report.changes
    );
    assert_eq!(moved[1].0, ChangeKind::AdditionalPropertiesAdded);
    assert!(
        moved[1].1.starts_with("Pet.additional") && moved[1].1 != "Pet.additional",
        "{:?}",
        report.changes
    );
    assert!(
        !kinds(&report).contains(&ChangeKind::AdditionalPropertiesTypeChanged),
        "{:?}",
        report.changes
    );
    assert_eq!(report.bump, Impact::Major);
}
