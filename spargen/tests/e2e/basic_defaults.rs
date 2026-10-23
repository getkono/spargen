//! The `tests/defaults.rs` that `generated_module_compiles_in_basic_oas31_crate` writes into the
//! `BASIC_SPEC` fixture crate and runs there.

pub(super) const DEFAULTS_TEST: &str = r##"
/// `converted` — a conversion between a typed value and a `serde_json::Value` — failed as a `Data`
/// error: the type refused the value, which is the only way such a conversion can fail.
#[track_caller]
fn assert_refused<T: std::fmt::Debug>(converted: serde_json::Result<T>, what: &str) {
    match converted {
        Ok(value) => panic!("{what}: the type admits no such value, yet produced {value:?}"),
        Err(error) => assert_eq!(
            error.classify(),
            serde_json::error::Category::Data,
            "{what}: refused for the wrong reason: {error}"
        ),
    }
}

/// `document` is well-formed JSON that `T` refuses, read both as text and as a parsed value. A
/// malformed document would fail to decode too, and prove nothing about `T`; the parsed value
/// cannot fail on syntax, so its refusal is the type's own.
#[track_caller]
fn assert_refuses<T: serde::de::DeserializeOwned + std::fmt::Debug>(document: &str, what: &str) {
    let value: serde_json::Value = serde_json::from_str(document)
        .unwrap_or_else(|error| panic!("{what}: {document} is not JSON: {error}"));
    if let Ok(decoded) = serde_json::from_str::<T>(document) {
        panic!("{what}: the type admits no such value, yet decoded {document} as {decoded:?}");
    }
    assert_refused(serde_json::from_value::<T>(value), what);
}

#[test]
fn absent_optional_fields_use_schema_defaults() {
    let settings: basic_client::types::Settings =
        serde_json::from_str(r#"{"retries": 7}"#).unwrap();
    assert_eq!(settings.color.as_deref(), Some("red"));
    assert_eq!(settings.enabled, Some(true));
    assert_eq!(settings.ratio, Some(1.5));
    assert_eq!(settings.retries, 7);
    assert_eq!(settings.mode, Some(basic_client::types::Mode::Auto));
}

#[test]
fn an_uninhabited_optional_field_drops_its_members_default() {
    // Absent, the field stays absent: no default provider fabricates a value no type admits.
    let merged: basic_client::types::ConflictDefault = serde_json::from_str("{}").unwrap();
    assert!(merged.x.is_none());
    let sibling: basic_client::types::ConflictDefaultSibling = serde_json::from_str("{}").unwrap();
    assert!(sibling.x.is_none());
    // Present, no value decodes: neither member's type is the field's.
    assert_refuses::<basic_client::types::ConflictDefault>(r#"{"x": "a"}"#, "ConflictDefault");
    assert_refuses::<basic_client::types::ConflictDefault>(r#"{"x": 1}"#, "ConflictDefault");
    assert_refuses::<basic_client::types::ConflictDefaultSibling>(
        r#"{"x": "a"}"#,
        "ConflictDefaultSibling",
    );
}

#[test]
fn a_narrowing_meet_retypes_or_drops_its_members_default() {
    // Absent, `valid` takes the default as the enum's variant and `ratio` as an integer, on both
    // spellings of the meet; `bad` and `fraction` admit no default of the narrowed type.
    let merged: basic_client::types::NarrowDefault = serde_json::from_str("{}").unwrap();
    let sibling: basic_client::types::NarrowDefaultSibling = serde_json::from_str("{}").unwrap();
    // The two spellings name their enums differently, so each is compared through its wire form.
    for (valid, bad, ratio, fraction) in [
        (
            serde_json::to_value(&merged.valid).unwrap(),
            serde_json::to_value(&merged.bad).unwrap(),
            merged.ratio,
            merged.fraction,
        ),
        (
            serde_json::to_value(&sibling.valid).unwrap(),
            serde_json::to_value(&sibling.bad).unwrap(),
            sibling.ratio,
            sibling.fraction,
        ),
    ] {
        assert_eq!(valid, serde_json::json!("a"));
        assert!(bad.is_null());
        assert_eq!(ratio, Some(3_i64));
        assert!(fraction.is_none());
    }
    // The wider member keeps its own defaults as it wrote them.
    let base: basic_client::types::NarrowDefaultBase = serde_json::from_str("{}").unwrap();
    assert_eq!(base.valid.as_deref(), Some("a"));
    assert_eq!(base.bad.as_deref(), Some("zzz"));
    assert_eq!(base.fraction, Some(2.5));
}

#[test]
fn a_meet_applies_a_default_only_a_later_side_declares() {
    use basic_client::types::{MergedDefault, MergedDefaultClash, MergedDefaultSibling};
    let merged: MergedDefault = serde_json::from_str("{}").unwrap();
    assert_eq!(merged.p.as_deref(), Some("hello"));
    let sibling: MergedDefaultSibling = serde_json::from_str("{}").unwrap();
    assert_eq!(sibling.p.as_deref(), Some("hello"));
    // `world` comes first here, yet the clash keeps `hello`, as it does in the other order.
    let clash: MergedDefaultClash = serde_json::from_str("{}").unwrap();
    assert_eq!(clash.p.as_deref(), Some("hello"));
    // A present value is still the value.
    let present: MergedDefault = serde_json::from_str(r#"{"p": "x"}"#).unwrap();
    assert_eq!(present.p.as_deref(), Some("x"));
}

// An optional uninhabited field is `Option<Never>`, and serde's `Option<T>` maps a JSON `null` to
// `None` without ever calling `T::deserialize` — so without a field-level deserializer the
// uninhabited type is never consulted and `{"x": null}`, which no schema here admits, decodes and
// re-serialises as `{}`. Every spelling that produces such a field is held to the same five rows:
// absent and unknown-only documents decode (and round-trip to `{}`), and any present value —
// `null` included — is rejected.
fn assert_only_absence_decodes<T>(type_name: &str)
where
    T: serde::de::DeserializeOwned + serde::Serialize + std::fmt::Debug,
{
    for valid in ["{}", r#"{"zz": 1}"#] {
        let value: T = serde_json::from_str(valid)
            .unwrap_or_else(|error| panic!("{type_name}: {valid} must decode: {error}"));
        assert_eq!(serde_json::to_string(&value).unwrap(), "{}", "{type_name}: {valid}");
    }
    for invalid in [r#"{"x": 1}"#, r#"{"x": "s"}"#, r#"{"x": null}"#] {
        assert_refuses::<T>(invalid, &format!("{type_name}: a value no type admits"));
    }
}

#[test]
fn an_uninhabited_optional_field_admits_only_absence() {
    assert_only_absence_decodes::<basic_client::types::ConflictDefault>("ConflictDefault");
    assert_only_absence_decodes::<basic_client::types::ConflictDefaultSibling>(
        "ConflictDefaultSibling",
    );
    assert_only_absence_decodes::<basic_client::types::ForbiddenProperty>("ForbiddenProperty");
}

#[test]
fn a_nullable_uninhabited_field_still_admits_null() {
    // `null` is the one value a nullable `false` schema admits, so there the `Option` is the type.
    let present: basic_client::types::NullOnlyProperty =
        serde_json::from_str(r#"{"x": null}"#).unwrap();
    assert!(present.x.is_none());
    assert_refuses::<basic_client::types::NullOnlyProperty>(r#"{"x": 1}"#, "NullOnlyProperty");
}

// serde's `Option<T>` maps a JSON `null` to `None` without calling `T::deserialize`, so an
// optional non-nullable field would decode `{"name": null}` as absent and re-serialise it as `{}`:
// a value the schema does not admit, accepted and then silently rewritten. A present value, `null`
// included, is decoded as the field's own type instead.
#[test]
fn an_optional_non_nullable_field_rejects_a_present_null() {
    use basic_client::types::OptionalFields;
    for field in ["name", "count", "flag", "tags", "mode", "nested", "choice", "colour"] {
        let document = format!(r#"{{"{field}": null}}"#);
        assert_refuses::<OptionalFields>(&document, &format!("`{field}` is not nullable"));
    }
    // Absence is still `None` (or the schema default), and serialises back to absence.
    let absent: OptionalFields = serde_json::from_str("{}").unwrap();
    assert!(absent.name.is_none() && absent.count.is_none() && absent.choice.is_none());
    assert_eq!(absent.colour.as_deref(), Some("red"));
    // A present, well-typed value still decodes.
    let present: OptionalFields = serde_json::from_str(
        r#"{"name": "n", "count": 2, "flag": false, "tags": [], "mode": "manual", "nested": {},
            "choice": 3, "colour": "blue"}"#,
    )
    .unwrap();
    assert_eq!(present.name.as_deref(), Some("n"));
    assert_eq!(present.count, Some(2));
    assert_eq!(present.colour.as_deref(), Some("blue"));
    // An untyped property admits `null` as a value of its own, and keeps it present on the wire.
    let untyped: OptionalFields = serde_json::from_str(r#"{"anything": null}"#).unwrap();
    assert_eq!(untyped.anything, Some(serde_json::Value::Null));
    assert_eq!(
        serde_json::to_value(&untyped).unwrap(),
        serde_json::json!({"anything": null, "colour": "red"})
    );
    // A nullable optional property is where `null` and absence both mean `None`.
    let nullable: OptionalFields = serde_json::from_str(r#"{"maybe": null}"#).unwrap();
    assert!(nullable.maybe.is_none());
}

#[test]
fn pattern_properties_capture_into_typed_overflow_map() {
    // The declared `host` field is typed; every non-declared property is captured by the flatten
    // `BTreeMap<String, String>` overflow that `patternProperties` lowered to.
    let headers: basic_client::types::Headers =
        serde_json::from_str(r#"{"host": "h", "x-a": "1", "x-b": "2"}"#).unwrap();
    assert_eq!(headers.host.as_deref(), Some("h"));
    assert_eq!(headers.additional.get("x-a").map(String::as_str), Some("1"));
    assert_eq!(headers.additional.get("x-b").map(String::as_str), Some("2"));
}

#[test]
fn null_mixed_enum_field_is_option_of_enum() {
    // The null-mixed `Priority` enum lowered to a real Rust enum used behind `Option`: an absent
    // field and an explicit `null` both deserialize to `None`; a string value to the variant.
    let absent: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n"}"#).unwrap();
    assert_eq!(absent.priority, None);

    let explicit_null: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "priority": null}"#).unwrap();
    assert_eq!(explicit_null.priority, None);

    let set: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "priority": "high"}"#).unwrap();
    assert_eq!(set.priority, Some(basic_client::types::Priority::High));
}

#[test]
fn all_of_compatible_constraints_keep_the_narrow_typed_intersection() {
    let json = serde_json::json!({
        "run_id": 7,
        "status": "queued",
        "marker": null,
        "labels": ["linux", "x64"],
        "steps": [{"name": "build"}],
        "empty_only": [],
    });
    let refined: basic_client::types::Refined = serde_json::from_value(json.clone()).unwrap();

    // `number & integer` is emitted as an integer, and exact JSON null is Rust unit.
    assert_eq!(refined.run_id, 7_i64);
    assert_eq!(refined.marker, ());
    assert_eq!(serde_json::to_value(refined).unwrap(), json);

    let invalid = serde_json::json!({
        "run_id": 7,
        "status": "queued",
        "marker": null,
        "labels": ["linux"],
        "steps": [{"name": "build"}],
        "empty_only": [null],
    });
    assert_refused(
        serde_json::from_value::<basic_client::types::Refined>(invalid),
        "Refined with a null in `empty_only`",
    );
}

#[test]
fn overlapping_unions_enforce_one_of_and_canonicalize_any_of() {
    let string: basic_client::types::AnyString =
        serde_json::from_str(r#""special""#).unwrap();
    assert!(matches!(
        string,
        basic_client::types::AnyString::StringLiteral(_)
    ));

    let number: basic_client::types::AnyNumber = serde_json::from_str("7").unwrap();
    assert!(matches!(
        number,
        basic_client::types::AnyNumber::AnyNumberVariant1(_)
    ));

    let owner: basic_client::types::AnyOwner =
        serde_json::from_str(r#"{"id":7}"#).unwrap();
    assert!(matches!(
        owner,
        basic_client::types::AnyOwner::DetailedOwner(_)
    ));

    // Both branches accept `special`, so oneOf rejects it. A manually constructed broad branch is
    // revalidated during serialization and rejected for the same reason.
    assert_refuses::<basic_client::types::OneOverlap>(
        r#""special""#,
        "OneOverlap decoding a value both branches accept",
    );
    let ambiguous = basic_client::types::OneOverlap::OneOverlapVariant0(Box::new(
        "special".to_owned(),
    ));
    assert_refused(
        serde_json::to_value(ambiguous),
        "OneOverlap serializing a value both branches accept",
    );
    // A value only the broad branch accepts decodes as that branch, carrying the value.
    let other: basic_client::types::OneOverlap = serde_json::from_str(r#""other""#).unwrap();
    assert!(
        matches!(
            &other,
            basic_client::types::OneOverlap::OneOverlapVariant0(inner) if inner.as_str() == "other"
        ),
        "{other:?}"
    );
}

#[test]
fn mixed_discriminator_dispatches_arrays_by_category_and_objects_by_tag() {
    let directory: basic_client::types::MixedContent =
        serde_json::from_str(r#"["README.md"]"#).unwrap();
    assert!(matches!(
        directory,
        basic_client::types::MixedContent::MixedContentVariant0(_)
    ));

    let file: basic_client::types::MixedContent =
        serde_json::from_str(r#"{"type":"file","content":"hello"}"#).unwrap();
    assert!(matches!(
        file,
        basic_client::types::MixedContent::ContentFile(_)
    ));
    assert_eq!(
        serde_json::to_value(file).unwrap(),
        serde_json::json!({"type": "file", "content": "hello"})
    );
}

#[test]
fn component_nullability_propagates_through_ref() {
    // A REQUIRED field referencing the nullable `Priority` component is `Option<Priority>`: the key
    // must be present, but `null` deserializes to `None` and a string to the variant. This only
    // holds if the component's nullability propagated to the `$ref` use site.
    let null_priority: basic_client::types::Ticket =
        serde_json::from_str(r#"{"priority": null, "history": []}"#).unwrap();
    assert_eq!(null_priority.priority, None);

    // An array of the nullable component is `Vec<Option<Priority>>`: a `null` element is accepted.
    let set: basic_client::types::Ticket =
        serde_json::from_str(r#"{"priority": "high", "history": ["low", null]}"#).unwrap();
    assert_eq!(set.priority, Some(basic_client::types::Priority::High));
    assert_eq!(
        set.history,
        vec![Some(basic_client::types::Priority::Low), None]
    );
}

#[test]
fn all_of_merged_struct_carries_every_member_field() {
    // `Account` merged a `$ref` base (id, required), an inline member (label, required) and a
    // sibling property (owner, optional). Required fields are plain, the optional is `Option`, and a
    // payload carrying all three deserializes into the single flattened struct.
    let account: basic_client::types::Account =
        serde_json::from_str(r#"{"id": "a1", "label": "L", "owner": "o"}"#).unwrap();
    assert_eq!(account.id, "a1");
    assert_eq!(account.label, "L");
    assert_eq!(account.owner.as_deref(), Some("o"));
}

#[test]
fn discriminated_union_round_trips_with_tag() {
    // Cat DECLARES `petType` as a required property — the shape that broke serde internal tagging
    // ("missing field petType"). The custom buffer-to-Value Deserialize hands the WHOLE value to the
    // variant, so Cat's own `pet_type` field is filled, and re-serialization keeps the tag.
    let pet: basic_client::types::Pet =
        serde_json::from_str(r#"{"petType": "cat", "name": "Whiskers"}"#).unwrap();
    match &pet {
        basic_client::types::Pet::Cat(cat) => {
            assert_eq!(cat.name, "Whiskers");
            assert_eq!(cat.pet_type, "cat");
        }
        other => panic!("expected Cat variant, got {other:?}"),
    }
    let json = serde_json::to_value(&pet).unwrap();
    assert_eq!(json["petType"], "cat");
    assert_eq!(json["name"], "Whiskers");

    // Dog does NOT declare `petType`; the custom Serialize re-inserts the tag it would otherwise
    // lack, and deserialization still routes by the tag.
    let dog: basic_client::types::Pet =
        serde_json::from_str(r#"{"petType": "dog", "bark": true}"#).unwrap();
    assert!(matches!(dog, basic_client::types::Pet::Dog(_)));
    let json = serde_json::to_value(&dog).unwrap();
    assert_eq!(json["petType"], "dog");
    assert_eq!(json["bark"], true);

    // Every value that names a member selects it (#263): the second mapping key `kitty`, and each
    // component name, which no mapping key claims. Cat keeps the tag it was decoded with in its own
    // field; Dog re-serializes with its first mapping key, `dog`.
    for tag in ["kitty", "Cat"] {
        let pet: basic_client::types::Pet = serde_json::from_value(
            serde_json::json!({"petType": tag, "name": "Whiskers"}),
        )
        .unwrap();
        match &pet {
            basic_client::types::Pet::Cat(cat) => assert_eq!(cat.pet_type, tag),
            other => panic!("{tag}: expected Cat variant, got {other:?}"),
        }
        assert_eq!(serde_json::to_value(&pet).unwrap()["petType"], tag);
    }
    let dog: basic_client::types::Pet =
        serde_json::from_str(r#"{"petType": "Dog", "bark": false}"#).unwrap();
    assert!(matches!(dog, basic_client::types::Pet::Dog(_)));
    assert_eq!(serde_json::to_value(&dog).unwrap()["petType"], "dog");
    assert_refuses::<basic_client::types::Pet>(
        r#"{"petType": "cow", "bark": true}"#,
        "Pet with an unmapped tag",
    );
}

#[test]
fn an_untagged_discriminated_member_leaves_the_tagged_dispatch_in_place() {
    use basic_client::types::LooseAnimal;
    // `Cat` (two required fields) outranks `Dog` in a trial, and accepts this payload too; the tag
    // names `Dog`, so `Dog` it is.
    let dog: LooseAnimal =
        serde_json::from_str(r#"{"petType": "Dog", "name": "Rex", "bark": true}"#).unwrap();
    assert!(matches!(dog, LooseAnimal::Dog(_)), "{dog:?}");
    // `Dog` declares no `petType`, so serialization re-inserts its tag.
    assert_eq!(serde_json::to_value(&dog).unwrap()["petType"], "Dog");
    let cat: LooseAnimal =
        serde_json::from_str(r#"{"petType": "Cat", "name": "Tom"}"#).unwrap();
    assert!(matches!(cat, LooseAnimal::Cat(_)), "{cat:?}");
    // A tag naming a tagged member never falls through to the untagged one.
    assert_refuses::<LooseAnimal>(
        r#"{"petType": "Dog", "fins": 3}"#,
        "LooseAnimal tagged Dog without Dog's fields",
    );
    // An unrecognized or absent tag tries the untagged member by its schema, and it writes no tag
    // of its own beyond the field it holds.
    let fish: LooseAnimal =
        serde_json::from_str(r#"{"petType": "Shark", "fins": 3}"#).unwrap();
    assert!(matches!(fish, LooseAnimal::LooseAnimalVariant2(_)), "{fish:?}");
    assert_eq!(
        serde_json::to_value(&fish).unwrap(),
        serde_json::json!({"petType": "Shark", "fins": 3})
    );
    assert_refuses::<LooseAnimal>(r#"{"bark": true}"#, "LooseAnimal untagged, matching no member");
}

#[test]
fn nullable_variant_union_resolves_null_at_option() {
    // A `null` payload resolves at the outer `Option` (variant nullability hoisted to the union),
    // and non-null string/array content routes to the right disjoint variant and re-serializes as a
    // bare value.
    let null: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "notes": null}"#).unwrap();
    assert!(null.notes.is_none());

    let text: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "notes": "hi"}"#).unwrap();
    assert_eq!(
        serde_json::to_value(&text.notes).unwrap(),
        serde_json::json!("hi")
    );

    let list: basic_client::types::User =
        serde_json::from_str(r#"{"id": "u", "name": "n", "notes": ["a", "b"]}"#).unwrap();
    assert_eq!(
        serde_json::to_value(&list.notes).unwrap(),
        serde_json::json!(["a", "b"])
    );
}

#[test]
fn disjoint_union_round_trips_without_wrapper() {
    // A `string` payload deserializes to the string variant and re-serializes as a BARE string —
    // no tag, no wrapper (strategy B custom Serialize).
    let text: basic_client::types::StringOrList =
        serde_json::from_str(r#""hello""#).unwrap();
    assert_eq!(serde_json::to_string(&text).unwrap(), r#""hello""#);

    // An `array` payload deserializes to the array variant and re-serializes as a bare array.
    let list: basic_client::types::StringOrList =
        serde_json::from_str(r#"["a","b"]"#).unwrap();
    assert_eq!(serde_json::to_string(&list).unwrap(), r#"["a","b"]"#);
}

#[test]
fn multi_status_response_enums_carry_typed_variants() {
    // the two success statuses lowered to a `GetMultiResponse` enum and the two error
    // statuses to a `GetMultiError` enum, each variant carrying that status's typed body. The
    // variants deserialize their bodies (the same `serde_json::from_slice` the generated dispatch
    // runs after selecting by HTTP status), proving the types are real and payload-carrying — not
    // `serde_json::Value`.
    let ok: basic_client::types::MultiOk = serde_json::from_str(r#"{"ok":"yes"}"#).unwrap();
    match basic_client::GetMultiResponse::Status200(Box::new(ok)) {
        basic_client::GetMultiResponse::Status200(body) => assert_eq!(body.ok, "yes"),
        other => panic!("expected Status200, got {other:?}"),
    }
    let created: basic_client::types::MultiCreated =
        serde_json::from_str(r#"{"id":7}"#).unwrap();
    match basic_client::GetMultiResponse::Status201(Box::new(created)) {
        basic_client::GetMultiResponse::Status201(body) => assert_eq!(body.id, 7),
        other => panic!("expected Status201, got {other:?}"),
    }
    // The documented bodyless 204 is a payload-free unit variant (carries no body).
    assert!(matches!(
        basic_client::GetMultiResponse::Status204,
        basic_client::GetMultiResponse::Status204
    ));

    let not_found: basic_client::types::NotFoundError =
        serde_json::from_str(r#"{"reason":"gone"}"#).unwrap();
    match basic_client::GetMultiError::Status404(Box::new(not_found)) {
        basic_client::GetMultiError::Status404(body) => assert_eq!(body.reason, "gone"),
        other => panic!("expected Status404, got {other:?}"),
    }
    let conflict: basic_client::types::ConflictError =
        serde_json::from_str(r#"{"detail":"dup"}"#).unwrap();
    match basic_client::GetMultiError::Status409(Box::new(conflict)) {
        basic_client::GetMultiError::Status409(body) => assert_eq!(body.detail, "dup"),
        other => panic!("expected Status409, got {other:?}"),
    }
}

#[test]
fn multipart_body_struct_has_typed_form_part_fields() {
    // the multipart/form-data body lowered to a typed struct whose fields are the form
    // parts. The binary `file` part is `bytes::Bytes` (its `serde` impls compile only because the
    // synthesized Cargo.toml enabled bytes' `serde` feature), `caption` a required `String`, and the
    // optional `count`/`tags` are `Option`. Constructing the value proves the field types; the
    // generated `upload_file` method (compiled here) builds the `reqwest::multipart::Form` from it,
    // which compiles only with reqwest's `multipart` feature enabled.
    let body = basic_client::types::RequestBodyE12d70b5 {
        file: bytes::Bytes::from_static(b"hello"),
        caption: "a caption".to_owned(),
        count: Some(3),
        tags: Some(vec!["x".to_owned(), "y".to_owned()]),
    };
    assert_eq!(&body.file[..], b"hello");
    assert_eq!(body.caption, "a caption");
    assert_eq!(body.count, Some(3));
    assert_eq!(body.tags.as_deref(), Some(&["x".to_owned(), "y".to_owned()][..]));
}

#[test]
fn streaming_op_item_type_is_typed_not_json_value() {
    // the SSE `/chat/stream` response schema lowered to a real `ChatChunk` type — the
    // streamed item of the `EventStream<ChatChunk>` the `stream_chat` method returns (that signature
    // and the embedded runtime `EventStream` are compile-verified by this crate's build). The item
    // type is a typed struct, never `serde_json::Value`; deserializing a frame the way the runtime's
    // `next` does proves it.
    let chunk: basic_client::types::ChatChunk =
        serde_json::from_str(r#"{"delta": "hi"}"#).unwrap();
    assert_eq!(chunk.delta, "hi");
}

#[test]
fn xml_body_types_carry_attribute_and_rename() {
    // the XML request/response bodies lowered to typed structs whose serde wire names
    // honor the `xml` hints — `XmlOrder.id` is an attribute (`xml.attribute` → serde `@id`), `sku` a
    // child element, and `XmlReceipt.code` is renamed via `xml.name` to `ReceiptCode`. The generated
    // crate depends on quick-xml (proving the conditional `xml` feature was enabled in its
    // Cargo.toml), so this exercises the same codec the `submit_order` method's `to_xml`/decode use.
    let order = basic_client::types::XmlOrder {
        id: 42,
        sku: "ABC".to_owned(),
    };
    let xml = quick_xml::se::to_string(&order).unwrap();
    assert!(xml.contains("id=\"42\""), "{xml}");
    assert!(xml.contains("<sku>ABC</sku>"), "{xml}");
    // The root `xml.name` names the body's root element, in place of the Rust type's name.
    assert!(xml.starts_with("<order "), "{xml}");
    assert!(xml.ends_with("</order>"), "{xml}");

    let receipt: basic_client::types::XmlReceipt =
        quick_xml::de::from_str("<XmlReceipt><ReceiptCode>OK</ReceiptCode></XmlReceipt>").unwrap();
    assert_eq!(receipt.code, "OK");
    assert_eq!(receipt.note, None);
    // Without a root `xml.name`, the component name `xml-receipt` names the root element, as
    // OpenAPI infers it, in place of the Rust type's name.
    let xml = quick_xml::se::to_string(&receipt).unwrap();
    assert_eq!(xml, "<xml-receipt><ReceiptCode>OK</ReceiptCode></xml-receipt>");
    // A present optional element is decoded as the field's own type through its present-value
    // deserializer, not through quick-xml's `Option` handling.
    let noted: basic_client::types::XmlReceipt = quick_xml::de::from_str(
        "<XmlReceipt><ReceiptCode>OK</ReceiptCode><note>late</note></XmlReceipt>",
    )
    .unwrap();
    assert_eq!(noted.note.as_deref(), Some("late"));
}

#[test]
fn json_only_schema_with_xml_metadata_keeps_original_json_names() {
    // regression guard: `JsonMeta` carries `xml.attribute`/`xml.name` hints but is used
    // only by a JSON operation, so the format-agnostic serde rename is SUPPRESSED. JSON must use the
    // original `id`/`sku` names — deserializing a normal server payload succeeds and re-serializing
    // produces the same names (never `@id`/`ProductSku`), proving JSON is uncorrupted.
    let parsed: basic_client::types::JsonMeta =
        serde_json::from_str(r#"{"id": 5, "sku": "Z9"}"#).unwrap();
    assert_eq!(parsed.id, 5);
    assert_eq!(parsed.sku, "Z9");
    let back = serde_json::to_string(&parsed).unwrap();
    assert!(back.contains(r#""id":5"#), "{back}");
    assert!(back.contains(r#""sku":"Z9""#), "{back}");
    assert!(!back.contains("@id"), "{back}");
    assert!(!back.contains("ProductSku"), "{back}");
    // `JsonMeta`'s root `xml.name` is a container rename, which serde_json never reads.
    assert_eq!(back, r#"{"id":5,"sku":"Z9"}"#);
}

#[test]
fn optional_params_construct_via_fluent_setters() {
    // each optional param on a `…Params` struct gets a `#[must_use]` consuming setter
    // named after its field, taking the field's inner `T` (never `Option<T>`) and storing `Some`.
    // `getUser` has an ordinary optional query param (`page` → `Option<i64>`) and a NULLABLE optional
    // one (`filter`, `type: [integer, "null"]` → `Option<i64>`); the setter for the nullable param
    // must still take the bare `i64`. Building via `default().setter(x)` must compile and set fields.
    let params = basic_client::GetUserParams::default()
        .page(2)
        .filter(7);
    assert_eq!(params.page, Some(2));
    assert_eq!(params.filter, Some(7));

    // Back-compat: the struct still derives `Default` and keeps public fields, so the pre-existing
    // struct-literal form is unchanged.
    let literal = basic_client::GetUserParams {
        page: Some(2),
        ..Default::default()
    };
    assert_eq!(literal.filter, None);
}

#[test]
fn generated_support_module_exposes_link_paginator() {
    // the generic Link-header paginator is a runtime helper re-exported at the crate root
    // (`basic_client::LinkPaginator` / `basic_client::next_link`), so a generated client can drive
    // Link/RFC-8288 pagination with no per-operation codegen. Constructing one via
    // `client.core().paginate_links::<T>(url)` compiles under clippy -D warnings, proving the
    // embedded `support::paginate` module is present and wired.
    let client = basic_client::Client::new("https://api.example.com").unwrap();
    let first = reqwest::Url::parse("https://api.example.com/items?page=1").unwrap();
    let pages: basic_client::LinkPaginator<Vec<i64>> = client.core().paginate_links(first);
    assert!(pages.has_next());

    // The pure `rel="next"` header helper is exposed too: no `Link` header → no next page.
    let mut headers = reqwest::header::HeaderMap::new();
    assert!(basic_client::next_link(&headers).is_none());
    headers.insert(
        reqwest::header::LINK,
        r#"<https://api.example.com/items?page=2>; rel="next""#
            .parse()
            .unwrap(),
    );
    assert_eq!(
        basic_client::next_link(&headers).unwrap().as_str(),
        "https://api.example.com/items?page=2"
    );
}

#[test]
fn custom_http_backend_plugs_into_non_generic_client() {
    // the transport seam is re-exported at the crate root
    // (`basic_client::HttpBackend` / `ExecuteFuture` / `ReqwestBackend`), so a consumer can
    // implement their own transport and plug it via `Client::with_backend` WITHOUT `Client`
    // becoming generic. A trivial backend compiles (under clippy -D warnings) and constructs a
    // client. This test only exercises construction, so the transport is never polled — the runtime
    // crate's own tests prove that dispatch actually routes through the installed backend.
    #[derive(Debug)]
    struct TestBackend;
    impl basic_client::HttpBackend for TestBackend {
        fn execute(&self, _request: reqwest::Request) -> basic_client::ExecuteFuture<'_> {
            Box::pin(async { unreachable!("transport is never exercised in this construction test") })
        }
    }

    let backend: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(TestBackend);
    let _client = basic_client::Client::with_backend(backend, "https://api.example.com").unwrap();

    // Back-compat: the pre-existing `new` / `with_client` constructors still work and install the
    // default reqwest-backed transport.
    let _default = basic_client::Client::new("https://api.example.com").unwrap();
    let _byo = basic_client::Client::with_client(
        reqwest::Client::new(),
        "https://api.example.com",
    )
    .unwrap();

    // The default backend type is nameable and usable as an `HttpBackend` too.
    let _reqwest_backend: std::sync::Arc<dyn basic_client::HttpBackend> =
        std::sync::Arc::new(basic_client::ReqwestBackend::new(reqwest::Client::new()));
}

#[test]
fn retry_backend_wraps_an_inner_backend() {
    // the retry adapter is re-exported at the crate root (`basic_client::RetryBackend`
    // / `RetryPolicy` / `RetryOutcome` / `exponential_backoff`). A consumer implements a policy
    // that decides retry AND supplies the wait (bring-your-own timing — no tokio in the runtime),
    // wraps their backend in a `RetryBackend`, and installs it via `Client::with_backend`, all
    // without `Client` becoming generic. This construction test compiles under clippy -D warnings;
    // the runtime crate's own tests prove the retry loop actually retries.
    use std::future::Future;
    use std::pin::Pin;
    use std::time::Duration;

    #[derive(Debug)]
    struct TrivialBackend;
    impl basic_client::HttpBackend for TrivialBackend {
        fn execute(&self, _request: reqwest::Request) -> basic_client::ExecuteFuture<'_> {
            Box::pin(async { unreachable!("transport is never exercised in this construction test") })
        }
    }

    struct BackoffPolicy;
    impl basic_client::RetryPolicy for BackoffPolicy {
        fn retry<'a>(
            &'a self,
            attempt: u32,
            outcome: &basic_client::RetryOutcome<'_>,
        ) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'a>>> {
            if attempt < 3 && outcome.is_transient() {
                // A real policy would await the caller's timer here (e.g. tokio::time::sleep); a
                // ready future keeps this construction test runtime-free.
                let _wait = basic_client::exponential_backoff(
                    attempt,
                    Duration::from_millis(50),
                    Duration::from_secs(2),
                );
                Some(Box::pin(std::future::ready(())))
            } else {
                None
            }
        }
    }

    let inner: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(TrivialBackend);
    let retry = basic_client::RetryBackend::new(inner, std::sync::Arc::new(BackoffPolicy));
    let backend: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(retry);
    let _client = basic_client::Client::with_backend(backend, "https://api.example.com").unwrap();
}

#[test]
fn middleware_backend_wraps_an_inner_backend() {
    // the interceptor middleware is re-exported at the crate root
    // (`basic_client::Middleware` / `Next` / `MiddlewareBackend`). A consumer implements a trivial
    // header-injecting middleware, layers it onto a `MiddlewareBackend`, and installs the whole
    // chain via `Client::with_backend` — all without `Client` becoming generic. This construction
    // test compiles under clippy -D warnings; the runtime crate's own tests prove the chain
    // actually observes/modifies/short-circuits and composes in order.
    #[derive(Debug)]
    struct TrivialBackend;
    impl basic_client::HttpBackend for TrivialBackend {
        fn execute(&self, _request: reqwest::Request) -> basic_client::ExecuteFuture<'_> {
            Box::pin(async { unreachable!("transport is never exercised in this construction test") })
        }
    }

    // A middleware that inserts a header on the way in, then proceeds to the rest of the chain via
    // `Next::run`. Modifying the request before `run` and returning `run`'s future directly is the
    // simplest shape; the trait's `'a` ties the borrow of `self`, the `Next`, and the boxed future.
    #[derive(Debug)]
    struct InjectHeader;
    impl basic_client::Middleware for InjectHeader {
        fn handle<'a>(
            &'a self,
            mut request: reqwest::Request,
            next: basic_client::Next<'a>,
        ) -> basic_client::ExecuteFuture<'a> {
            request.headers_mut().insert(
                reqwest::header::HeaderName::from_static("x-generated-mw"),
                reqwest::header::HeaderValue::from_static("on"),
            );
            next.run(request)
        }
    }

    let inner: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(TrivialBackend);
    let middleware = basic_client::MiddlewareBackend::new(inner).layer(std::sync::Arc::new(InjectHeader));
    let backend: std::sync::Arc<dyn basic_client::HttpBackend> = std::sync::Arc::new(middleware);
    let _client = basic_client::Client::with_backend(backend, "https://api.example.com").unwrap();
}
"##;
