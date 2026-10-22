//! A property round-trip of generated union and `allOf` types.

use super::*;

/// GENERATED-CODE property round-trip. Generate a module in a fixture crate carrying
/// the representative union/allOf types (a discriminated union, a structurally-disjoint
/// string-vs-array union, a required-key-disjoint closed-object union, a nullable-variant union, and
/// an `allOf`-merged struct), then add `proptest` as a dev-dependency OF THE HARNESS-SCAFFOLDED
/// CRATE ONLY and drive a generated `tests/roundtrip.rs` that round-trips MANY random values through
/// the real emitted serde code. The types derive `Serialize`/`Deserialize` but NOT `PartialEq`, so
/// stability is asserted via re-serialized `serde_json::Value` equality (serialize→deserialize→
/// serialize is a fixed point) — this catches a disjoint union misrouting a value to the wrong
/// variant (j2 would differ) and `allOf` field loss. A stronger per-variant assertion proves a value
/// built as variant K deserializes back onto variant K (not misrouted).
///
/// Confirms, before appending, that the fixture manifest carries no `proptest`: the dependency is
/// scoped strictly to this throwaway test crate.
#[test]
fn union_and_allof_roundtrip_under_proptest() {
    let temp = tempfile::tempdir().unwrap();
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, ROUNDTRIP_SPEC).unwrap();
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
}

/// The generated `tests/roundtrip.rs` for [`union_and_allof_roundtrip_under_proptest`]. Hand-written
/// proptest strategies span each type's value space; each type asserts the serialize→deserialize→
/// serialize `serde_json::Value` fixed point over 64 random cases, and the two disjoint unions plus
/// the discriminated union additionally assert a value built as variant K is not misrouted on decode.
const ROUNDTRIP_TEST: &str = r####"
use proptest::prelude::*;
use roundtrip_client::types;

/// serialize → deserialize → serialize is a fixed point on the JSON value (the types lack
/// `PartialEq`, so equality is asserted on the re-serialized `serde_json::Value`). A misrouted
/// disjoint-union value or a dropped `allOf` field would make the second serialization differ.
fn roundtrip_stable<T>(value: &T) -> Result<(), TestCaseError>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let first = serde_json::to_value(value).expect("serialize");
    let back: T = serde_json::from_value(first.clone()).expect("deserialize");
    let second = serde_json::to_value(&back).expect("re-serialize");
    prop_assert_eq!(first, second);
    Ok(())
}

// Discriminated union (Cat DECLARES the `petType` tag, Dog does not). The tag value is fixed to the
// variant's mapping key so the payload routes back to the variant it was built as; `name`/`bark` are
// random.
fn pet_strategy() -> impl Strategy<Value = types::Pet> {
    prop_oneof![
        "[a-zA-Z0-9 ]{0,16}".prop_map(|name| types::Pet::Cat(Box::new(types::Cat {
            pet_type: "cat".to_owned(),
            name,
        }))),
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
}
"####;

/// The spec generated for [`union_and_allof_roundtrip_under_proptest`]: a `User` object pulling in a
/// discriminated union (`Pet`), a string-vs-array disjoint union (`StringOrList`), a nullable-variant
/// union (`StringListOrNull`), a required-key-disjoint closed-object union (`Shape`), and an
/// `allOf`-merged struct (`Account`). One operation references `User` so every type is emitted.
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
