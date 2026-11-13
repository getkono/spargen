//! A property round-trip of generated union and `allOf` types.

use super::*;

/// GENERATED-CODE property round-trip. Generate a module in a fixture crate carrying
/// the representative union/allOf types (a discriminated union, a structurally-disjoint
/// string-vs-array union, a required-key-disjoint closed-object union, a nullable-variant union, and
/// an `allOf`-merged struct), then add `proptest` as a dev-dependency OF THE HARNESS-SCAFFOLDED
/// CRATE ONLY and drive a generated `tests/roundtrip.rs` that round-trips MANY random values through
/// the real emitted serde code. Each value decodes back equal to itself (the generated `PartialEq`),
/// and serialize→deserialize→serialize is a fixed point on the `serde_json::Value` — this catches a
/// disjoint union misrouting a value to the wrong variant and `allOf` field loss. A stronger
/// per-variant assertion proves a value built as variant K deserializes back onto variant K (not
/// misrouted), and the trial-matched unions decode a payload only into a variant of its JSON
/// category (#1340).
///
/// Confirms, before appending, that the fixture manifest carries no `proptest`: the dependency is
/// scoped strictly to this throwaway test crate.
#[test]
fn union_and_allof_roundtrip_under_proptest() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", ROUNDTRIP_SPEC);
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "roundtrip_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    // The property-test dependency is exclusively scaffolding for this test harness's crate.
    let mut manifest = std::fs::read_to_string(out.join("Cargo.toml")).unwrap();
    assert!(
        !manifest.contains("proptest"),
        "fixture manifest must not carry proptest: {manifest}"
    );
    manifest.push_str("\n[dev-dependencies]\nproptest = \"1\"\n");
    std::fs::write(out.join("Cargo.toml"), manifest).unwrap();

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(out.join("tests/roundtrip.rs"), ROUNDTRIP_TEST).unwrap();

    let status = fixture_cargo(&out)
        .args(["test", "--test", "roundtrip"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the generated union/allOf types must survive the proptest JSON round-trip"
    );
    // The union serde these types exercise, the `defaultMapping` trial check among it, lints
    // clean.
    let status = fixture_cargo(&out)
        .args(["clippy", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(status.success(), "the round-trip fixture must lint clean");
}

/// The generated `tests/roundtrip.rs` for [`union_and_allof_roundtrip_under_proptest`]. Hand-written
/// proptest strategies span each type's value space; each type asserts the serialize→deserialize→
/// serialize `serde_json::Value` fixed point over 64 random cases, and the two disjoint unions plus
/// the discriminated union additionally assert a value built as variant K is not misrouted on decode.
const ROUNDTRIP_TEST: &str = r####"
use proptest::prelude::*;
use roundtrip_client::types;

/// A value decodes back equal to itself, and serialize → deserialize → serialize is a fixed point on
/// the JSON value. A misrouted disjoint-union value or a dropped `allOf` field fails both.
fn roundtrip_stable<T>(value: &T) -> Result<(), TestCaseError>
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let first = serde_json::to_value(value).expect("serialize");
    let back: T = serde_json::from_value(first.clone()).expect("deserialize");
    prop_assert_eq!(&back, value);
    let second = serde_json::to_value(&back).expect("re-serialize");
    prop_assert_eq!(first, second);
    Ok(())
}

// Discriminated union (Cat DECLARES the `petType` tag, Dog does not). The tag value is one of the
// values that select Cat (its mapping keys `cat` and `kitty`, and its component name), so the
// payload routes back to the variant it was built as and is written back unchanged; `name`/`bark`
// are random.
fn pet_strategy() -> impl Strategy<Value = types::Pet> {
    prop_oneof![
        (prop::sample::select(vec!["cat", "kitty", "Cat"]), "[a-zA-Z0-9 ]{0,16}").prop_map(
            |(tag, name)| types::Pet::Cat(Box::new(types::Cat {
                pet_type: tag.to_owned(),
                name,
            }))
        ),
        any::<bool>().prop_map(|bark| types::Pet::Dog(Box::new(types::Dog { bark }))),
    ]
}

// Structurally-disjoint union: a bare string vs an array of strings (distinct JSON categories).
fn string_or_list_strategy() -> impl Strategy<Value = types::StringOrList> {
    prop_oneof![
        "[a-zA-Z0-9 ]{0,16}".prop_map(|value| {
            types::StringOrList::StringOrListVariant0(Box::new(value))
        }),
        proptest::collection::vec("[a-zA-Z0-9 ]{0,8}", 0..5)
            .prop_map(|value| types::StringOrList::StringOrListVariant1(Box::new(value))),
    ]
}

// Nullable-variant union: the string member is nullable, hoisting nullability to the whole union, so
// the field is `Option<StringListOrNull>` — `None` is a bare JSON `null`.
fn notes_strategy() -> impl Strategy<Value = Option<types::StringListOrNull>> {
    prop_oneof![
        Just(None),
        "[a-zA-Z0-9 ]{0,16}"
            .prop_map(|s| Some(types::StringListOrNull::StringListOrNullVariant0(Box::new(s)))),
        proptest::collection::vec("[a-zA-Z0-9 ]{0,8}", 0..5)
            .prop_map(|v| Some(types::StringListOrNull::StringListOrNullVariant1(Box::new(v)))),
    ]
}

// Required-key-disjoint union: two CLOSED objects, each carrying a unique required key.
fn shape_strategy() -> impl Strategy<Value = types::Shape> {
    prop_oneof![
        (-1000.0f64..1000.0)
            .prop_map(|radius| types::Shape::Circle(Box::new(types::Circle { radius }))),
        (-1000.0f64..1000.0)
            .prop_map(|side| types::Shape::Square(Box::new(types::Square { side }))),
    ]
}

// `allOf`-merged struct: a required `$ref` base field, a required inline member field, and an
// optional sibling.
fn account_strategy() -> impl Strategy<Value = types::Account> {
    (
        "[a-zA-Z0-9]{0,12}",
        "[a-zA-Z0-9]{0,12}",
        proptest::option::of("[a-zA-Z0-9]{0,12}"),
    )
        .prop_map(|(id, label, owner)| types::Account { id, label, owner })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn pet_roundtrips(value in pet_strategy()) {
        roundtrip_stable(&value)?;
    }

    #[test]
    fn string_or_list_roundtrips(value in string_or_list_strategy()) {
        roundtrip_stable(&value)?;
    }

    #[test]
    fn notes_roundtrips(value in notes_strategy()) {
        roundtrip_stable(&value)?;
    }

    #[test]
    fn shape_roundtrips(value in shape_strategy()) {
        roundtrip_stable(&value)?;
    }

    #[test]
    fn account_roundtrips(value in account_strategy()) {
        roundtrip_stable(&value)?;
    }

    // Stronger property: a value built as variant K, serialized then deserialized, lands back on
    // variant K — the custom disjoint/discriminated Deserialize never misroutes.
    #[test]
    fn pet_variant_not_misrouted(value in pet_strategy()) {
        let was_cat = matches!(value, types::Pet::Cat(_));
        let json = serde_json::to_value(&value).expect("serialize");
        let back: types::Pet = serde_json::from_value(json).expect("deserialize");
        prop_assert_eq!(was_cat, matches!(back, types::Pet::Cat(_)));
    }

    #[test]
    fn string_or_list_variant_not_misrouted(value in string_or_list_strategy()) {
        let was_string = matches!(value, types::StringOrList::StringOrListVariant0(_));
        let json = serde_json::to_value(&value).expect("serialize");
        let back: types::StringOrList = serde_json::from_value(json).expect("deserialize");
        prop_assert_eq!(
            was_string,
            matches!(back, types::StringOrList::StringOrListVariant0(_))
        );
    }

    #[test]
    fn shape_variant_not_misrouted(value in shape_strategy()) {
        let was_circle = matches!(value, types::Shape::Circle(_));
        let json = serde_json::to_value(&value).expect("serialize");
        let back: types::Shape = serde_json::from_value(json).expect("deserialize");
        prop_assert_eq!(was_circle, matches!(back, types::Shape::Circle(_)));
    }

    // A trial-matched union tries a variant only on a payload of its JSON category: serde_json
    // reads an array into a derived struct field by field, so an unguarded struct attempt takes
    // any array whose items fit its leading fields.
    #[test]
    fn a_trial_one_of_decodes_an_array_only_as_its_array_variant(
        items in proptest::collection::vec("[a-z]{1,8}", 1..4)
    ) {
        let back: types::Mixed = serde_json::from_value(serde_json::json!(items))
            .expect("only the array variant takes an array");
        prop_assert_eq!(back, types::Mixed::Labels(Box::new(items.clone())));
        let written = serde_json::to_value(types::Mixed::Labels(Box::new(items.clone())))
            .expect("the array variant serializes");
        prop_assert_eq!(written, serde_json::json!(items));
    }

    #[test]
    fn a_trial_any_of_decodes_an_array_only_as_its_array_variant(
        items in proptest::collection::vec("[a-z]{1,8}", 1..4)
    ) {
        let back: types::MixedAny = serde_json::from_value(serde_json::json!(items))
            .expect("only the array variant takes an array");
        prop_assert_eq!(back, types::MixedAny::Labels(Box::new(items)));
    }

    #[test]
    fn an_untagged_discriminated_variant_refuses_an_array(
        items in proptest::collection::vec("[a-z]{1,8}", 1..4)
    ) {
        let decoded = serde_json::from_value::<types::Vehicle>(serde_json::json!(items));
        prop_assert!(decoded.is_err(), "an array is no object variant: {:?}", decoded);
    }
}

// A variant struct holding a tag that does not select its variant is a serialization error naming
// the tag and the variant (#1344): written out, `dog` would route the payload to Dog, which our own
// decoder then refuses for lacking `bark`. A tag that selects the variant is kept as it is.
#[test]
fn a_discriminated_variant_holding_another_variants_tag_does_not_serialize() {
    for tag in ["dog", "Dog", "cow", ""] {
        let pet = types::Pet::Cat(Box::new(types::Cat {
            pet_type: tag.to_owned(),
            name: "Tom".to_owned(),
        }));
        let error = serde_json::to_value(&pet).expect_err(tag).to_string();
        assert!(error.contains(&format!("\"{tag}\"")), "{tag}: {error}");
        assert!(error.contains("Pet::Cat"), "{tag}: {error}");
    }
    for tag in ["cat", "kitty", "Cat"] {
        let pet = types::Pet::Cat(Box::new(types::Cat {
            pet_type: tag.to_owned(),
            name: "Tom".to_owned(),
        }));
        let written = serde_json::to_value(&pet).expect(tag);
        assert_eq!(written["petType"], tag);
        let back: types::Pet = serde_json::from_value(written).expect(tag);
        assert_eq!(back, pet);
    }
}

// An untagged variant holding a tag a tagged variant claims would decode as that variant, so it
// is refused too; one holding an unclaimed tag, or none, decodes back as itself.
#[test]
fn an_untagged_discriminated_variant_holding_a_claimed_tag_does_not_serialize() {
    for tag in ["car", "Car"] {
        let vehicle = types::Vehicle::VehicleVariant1(Box::new(types::VehicleVariant1 {
            kind: Some(tag.to_owned()),
            label: Some("x".to_owned()),
        }));
        let error = serde_json::to_value(&vehicle).expect_err(tag).to_string();
        assert!(error.contains(&format!("\"{tag}\"")), "{tag}: {error}");
        assert!(error.contains("Vehicle::VehicleVariant1"), "{tag}: {error}");
    }
    for kind in [Some("bike"), None] {
        let vehicle = types::Vehicle::VehicleVariant1(Box::new(types::VehicleVariant1 {
            kind: kind.map(str::to_owned),
            label: Some("x".to_owned()),
        }));
        let written = serde_json::to_value(&vehicle).expect("an unclaimed tag serializes");
        let back: types::Vehicle = serde_json::from_value(written).expect("decodes back");
        assert_eq!(back, vehicle);
    }
}

// The `defaultMapping` variant holding an unclaimed tag decodes through the untagged variants'
// trial before the fallback, so a payload the untagged variant's schema accepts would come back as
// that variant (#1376): it is refused. One no untagged variant accepts, or holding its own tag,
// decodes back as itself.
#[test]
fn a_default_mapping_payload_an_untagged_variant_claims_does_not_serialize() {
    let claimed = types::Ride::RideOther(Box::new(types::RideOther {
        kind: Some("bus".to_owned()),
        label: Some("x".to_owned()),
        note: None,
    }));
    let error = serde_json::to_value(&claimed).expect_err("claimed by the untagged variant");
    let error = error.to_string();
    assert!(error.contains("Ride::RideOther"), "{error}");
    assert!(error.contains("Ride::RideVariant1"), "{error}");
    for (kind, label) in [(Some("bus"), None), (Some("RideOther"), Some("x"))] {
        let ride = types::Ride::RideOther(Box::new(types::RideOther {
            kind: kind.map(str::to_owned),
            label: label.map(str::to_owned),
            note: Some("n".to_owned()),
        }));
        let written = serde_json::to_value(&ride).expect("unclaimed by any untagged variant");
        let back: types::Ride = serde_json::from_value(written).expect("decodes back");
        assert_eq!(back, ride);
    }
    // Holding no tag, the variant is written with its own, which routes it back by tag.
    let untagged = types::Ride::RideOther(Box::new(types::RideOther {
        kind: None,
        label: Some("x".to_owned()),
        note: None,
    }));
    let written = serde_json::to_value(&untagged).expect("its own tag is inserted");
    assert_eq!(written["kind"], "RideOther", "{written}");
    let back: types::Ride = serde_json::from_value(written).expect("decodes back");
    assert!(matches!(back, types::Ride::RideOther(_)), "{back:?}");
}

// A unit string-enum variant refuses the one-key object serde_json would otherwise read as that
// variant, so the `anyOf` takes only strings.
#[test]
fn a_trial_string_variant_refuses_a_one_key_object() {
    let decoded = serde_json::from_value::<types::Status>(serde_json::json!({ "queued": null }));
    assert!(decoded.is_err(), "an object is no string variant: {decoded:?}");
    let back: types::Status = serde_json::from_value(serde_json::json!("queued")).unwrap();
    assert_eq!(back, types::Status::Phase(Box::new(types::Phase::Queued)));
}
"####;

/// The spec generated for [`union_and_allof_roundtrip_under_proptest`]: a `User` object pulling in a
/// discriminated union (`Pet`), a string-vs-array disjoint union (`StringOrList`), a nullable-variant
/// union (`StringListOrNull`), a required-key-disjoint closed-object union (`Shape`), an
/// `allOf`-merged struct (`Account`), and the trial-matched unions a payload of another JSON
/// category must not satisfy: two open objects and an array as a `oneOf` (`Mixed`) and an `anyOf`
/// (`MixedAny`), a string enum beside a string (`Status`), and a discriminated union with an
/// untagged object member (`Vehicle`), and one whose `defaultMapping` member overlaps an untagged
/// one (`Ride`). One operation references `User` so every type is emitted.
const ROUNDTRIP_SPEC: &str = r##"
openapi: 3.1.0
info: { title: Roundtrip, version: 1.0.0 }
servers:
  - url: https://example.com/api
paths:
  /user:
    get:
      operationId: getUser
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/User"
components:
  schemas:
    User:
      type: object
      required: [id]
      properties:
        id: { type: string }
        pet: { $ref: "#/components/schemas/Pet" }
        alias: { $ref: "#/components/schemas/StringOrList" }
        notes: { $ref: "#/components/schemas/StringListOrNull" }
        shape: { $ref: "#/components/schemas/Shape" }
        account: { $ref: "#/components/schemas/Account" }
        mixed: { $ref: "#/components/schemas/Mixed" }
        mixedAny: { $ref: "#/components/schemas/MixedAny" }
        status: { $ref: "#/components/schemas/Status" }
        vehicle: { $ref: "#/components/schemas/Vehicle" }
        ride: { $ref: "#/components/schemas/Ride" }
    Labelled:
      type: object
      properties:
        id: { type: string }
    Named:
      type: object
      required: [name]
      properties:
        name: { type: string }
    Labels:
      type: array
      items: { type: string }
    Mixed:
      oneOf:
        - $ref: "#/components/schemas/Labelled"
        - $ref: "#/components/schemas/Named"
        - $ref: "#/components/schemas/Labels"
    MixedAny:
      anyOf:
        - $ref: "#/components/schemas/Labelled"
        - $ref: "#/components/schemas/Named"
        - $ref: "#/components/schemas/Labels"
    Phase:
      type: string
      enum: [queued, running]
    Status:
      anyOf:
        - $ref: "#/components/schemas/Phase"
        - type: string
    Car:
      type: object
      required: [kind]
      properties:
        kind: { type: string }
        wheels: { type: integer }
    Vehicle:
      oneOf:
        - $ref: "#/components/schemas/Car"
        - type: object
          properties:
            kind: { type: string }
            label: { type: string }
      discriminator:
        propertyName: kind
        mapping:
          car: "#/components/schemas/Car"
    RideOther:
      type: object
      properties:
        kind: { type: string }
        label: { type: string }
        note: { type: string }
    Ride:
      oneOf:
        - $ref: "#/components/schemas/Car"
        - type: object
          required: [label]
          properties:
            kind: { type: string }
            label: { type: string }
        - $ref: "#/components/schemas/RideOther"
      discriminator:
        propertyName: kind
        mapping:
          car: "#/components/schemas/Car"
        defaultMapping: "#/components/schemas/RideOther"
    Cat:
      type: object
      required: [petType, name]
      properties:
        petType: { type: string }
        name: { type: string }
    Dog:
      type: object
      required: [bark]
      properties:
        bark: { type: boolean }
    Pet:
      oneOf:
        - $ref: "#/components/schemas/Cat"
        - $ref: "#/components/schemas/Dog"
      discriminator:
        propertyName: petType
        mapping:
          cat: "#/components/schemas/Cat"
          kitty: "#/components/schemas/Cat"
          dog: "#/components/schemas/Dog"
    StringOrList:
      oneOf:
        - type: string
        - type: array
          items: { type: string }
    StringListOrNull:
      oneOf:
        - type: [string, "null"]
        - type: array
          items: { type: string }
    Circle:
      type: object
      additionalProperties: false
      required: [radius]
      properties:
        radius: { type: number }
    Square:
      type: object
      additionalProperties: false
      required: [side]
      properties:
        side: { type: number }
    Shape:
      oneOf:
        - $ref: "#/components/schemas/Circle"
        - $ref: "#/components/schemas/Square"
    AccountBase:
      type: object
      required: [id]
      properties:
        id: { type: string }
    Account:
      type: object
      properties:
        owner: { type: string }
      allOf:
        - $ref: "#/components/schemas/AccountBase"
        - type: object
          required: [label]
          properties:
            label: { type: string }
"##;
