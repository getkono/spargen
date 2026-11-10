//! Type shapes compiled and decoded in a fixture crate: tuples, alias cycles, deprecated
//! components, an `allOf` beside a union, object structs built through `new` and setters, and
//! closed string enums named, listed and parsed by their wire values.

use super::*;

/// Closed `prefixItems` tuples of one and two positions, named and as an inline property, so the
/// one-position case is emitted both as a component alias and as a field type (issue #415).
const TUPLE_ARITY_SPEC: &str = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /pair:
    get:
      operationId: getPair
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Holder' }
components:
  schemas:
    Single: { type: array, prefixItems: [{ type: integer }], items: false }
    Pair: { type: array, prefixItems: [{ type: integer }, { type: string }], items: false }
    Holder:
      type: object
      required: [single, pair, inline]
      properties:
        single: { $ref: '#/components/schemas/Single' }
        pair: { $ref: '#/components/schemas/Pair' }
        inline: { type: array, prefixItems: [{ type: string }], items: false }
"#;

/// A one-position tuple must be emitted as `(T,)`, a tuple, and not `(T)`, which Rust reads as a
/// parenthesized `T` that decodes from a bare scalar and rejects the one-element array the schema
/// describes (issue #415). The fixture's own test decodes the wire shapes through the generated
/// types and reaches each position by index, which a parenthesized scalar has no field for.
#[test]
fn a_one_position_tuple_is_a_tuple_on_the_wire() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", TUPLE_ARITY_SPEC);
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "tuple_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/tuple.rs"),
        r##"
use tuple_client::types::{Holder, Pair, Single};

#[test]
fn a_one_element_array_decodes_into_a_one_position_tuple() {
    let single: Single = serde_json::from_str("[1]").unwrap();
    assert_eq!(single.0, 1);
    assert_eq!(serde_json::to_string(&single).unwrap(), "[1]");
    assert!(serde_json::from_str::<Single>("1").is_err());
    assert!(serde_json::from_str::<Single>("[1, 2]").is_err());

    let pair: Pair = serde_json::from_str(r#"[1, "a"]"#).unwrap();
    assert_eq!((pair.0, pair.1.as_str()), (1, "a"));

    let holder: Holder =
        serde_json::from_str(r#"{"single": [7], "pair": [2, "b"], "inline": ["c"]}"#).unwrap();
    assert_eq!(holder.single.0, 7);
    assert_eq!(holder.inline.0, "c");
    assert_eq!(
        serde_json::to_value(&holder).unwrap(),
        serde_json::json!({"single": [7], "pair": [2, "b"], "inline": ["c"]})
    );
}
"##,
    )
    .unwrap();

    let status = fixture_cargo(&out).arg("test").status().unwrap();
    assert!(
        status.success(),
        "a one-position tuple must decode from a one-element array"
    );
    let status = fixture_cargo(&out)
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(status.success(), "the tuple fixture must lint clean");
}

/// Arrays and tuples that reach themselves through aliases alone (issue #648): a self-referential
/// array, one whose items are nullable, a self-referential one-position tuple, an array and a
/// tuple that name each other, and an array that only points into a cycle.
const ALIAS_CYCLE_SPEC: &str = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /node:
    get:
      operationId: getNode
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Node' }
        '404':
          description: nf
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Tup' }
components:
  schemas:
    Node: { type: array, items: { $ref: '#/components/schemas/Node' } }
    Maybe: { type: array, items: { oneOf: [{ $ref: '#/components/schemas/Maybe' }, { type: 'null' }] } }
    Tup: { type: array, prefixItems: [{ $ref: '#/components/schemas/Tup' }], items: false }
    Ping: { type: array, items: { $ref: '#/components/schemas/Pong' } }
    Pong: { type: array, prefixItems: [{ $ref: '#/components/schemas/Ping' }], items: false }
    Into: { type: array, items: { $ref: '#/components/schemas/Node' } }
"#;

/// Issue #648: a component closing an alias cycle was emitted as `pub type Node = Vec<Node>;`,
/// which rustc rejects (`E0391`). Each is a transparent newtype instead, so the module compiles and
/// lints clean, and the fixture's own test decodes and re-encodes each wire shape through it.
#[test]
fn an_array_or_tuple_closing_an_alias_cycle_compiles_and_round_trips() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", ALIAS_CYCLE_SPEC);
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "alias_cycle_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/alias_cycle.rs"),
        r##"
use alias_cycle_client::types::{Into, Maybe, Node, Ping, Pong, Tup};

fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(wire: &str) -> T {
    let value: T = serde_json::from_str(wire).unwrap();
    assert_eq!(
        serde_json::to_value(&value).unwrap(),
        serde_json::from_str::<serde_json::Value>(wire).unwrap()
    );
    value
}

#[test]
fn every_cycle_member_decodes_its_wire_shape() {
    let node: Node = round_trip("[[], [[]]]");
    assert_eq!(node.0.len(), 2);
    assert_eq!(node.0[1].0.len(), 1);
    assert!(serde_json::from_str::<Node>("[1]").is_err());

    let maybe: Maybe = round_trip("[null, [null]]");
    assert!(maybe.0[0].is_none());

    assert!(serde_json::from_str::<Tup>("[]").is_err());
    assert!(serde_json::from_str::<Tup>("[[]]").is_err());

    let ping: Ping = round_trip("[[[]]]");
    let Pong((inner,)) = &ping.0[0];
    assert!(inner.0.is_empty());

    let into: Into = round_trip("[[]]");
    assert_eq!(into.len(), 1);
}
"##,
    )
    .unwrap();

    let status = fixture_cargo(&out).arg("test").status().unwrap();
    assert!(
        status.success(),
        "every alias-cycle member must decode its wire shape"
    );
    let status = fixture_cargo(&out)
        .args([
            "clippy",
            "--all-features",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ])
        .status()
        .unwrap();
    assert!(status.success(), "the alias-cycle fixture must lint clean");
}

/// Every item kind a `deprecated: true` component lowers to, each named by an operation, a struct
/// field, another component, or a response enum, so the module names each one internally.
const DEPRECATED_COMPONENTS_SPEC: &str = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /legacy:
    post:
      operationId: postLegacy
      parameters:
        - { name: level, in: query, schema: { $ref: '#/components/schemas/Level' } }
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: '#/components/schemas/Tree' }
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Legacy' }
        '201':
          description: created
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Tags' }
        '404':
          description: nf
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Flag' }
  /mixed/{level}:
    get:
      operationId: getMixed
      parameters:
        - { name: level, in: path, required: true, schema: { $ref: '#/components/schemas/Level' } }
        - { name: tags, in: header, schema: { $ref: '#/components/schemas/Tags' } }
      responses:
        '200':
          description: ok
          headers:
            X-Level: { schema: { $ref: '#/components/schemas/Level' } }
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Mixed' }
        '400':
          description: bad
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Tags' }
        '409':
          description: conflict
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Tree' }
  /form:
    post:
      operationId: postForm
      requestBody:
        required: true
        content:
          application/x-www-form-urlencoded:
            schema: { $ref: '#/components/schemas/Legacy' }
      responses:
        '204': { description: stored }
        '400':
          description: bad
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Legacy' }
components:
  schemas:
    Mixed: { oneOf: [{ $ref: '#/components/schemas/Legacy' }, { $ref: '#/components/schemas/Tags' }] }
    Level: { type: integer, enum: [1, 2], deprecated: true }
    Flag: { type: boolean, enum: [true], deprecated: true }
    Tags: { type: array, items: { type: string }, deprecated: true }
    Tree: { type: array, items: { $ref: '#/components/schemas/Tree' }, deprecated: true }
    Forest: { type: array, items: { $ref: '#/components/schemas/Tree' } }
    Legacy:
      type: object
      deprecated: true
      required: [level, flag, tags, tree, forest]
      properties:
        level: { $ref: '#/components/schemas/Level' }
        flag: { $ref: '#/components/schemas/Flag' }
        tags: { $ref: '#/components/schemas/Tags' }
        tree: { $ref: '#/components/schemas/Tree' }
        forest: { $ref: '#/components/schemas/Forest' }
"#;

/// Issue #731: a deprecated component carries `#[deprecated]` whatever item it lowers to, and the
/// module that names those items itself still compiles and lints clean under `-D warnings`.
#[test]
fn a_module_naming_its_deprecated_components_lints_clean() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", DEPRECATED_COMPONENTS_SPEC);
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "deprecated_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    let status = fixture_cargo(&out)
        .args([
            "clippy",
            "--all-features",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "a module naming its own deprecated components must lint clean"
    );
}

const ALL_OF_BESIDE_UNION_SPEC: &str = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /holder:
    get:
      operationId: getHolder
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Holder' }
components:
  schemas:
    Cat: { type: object, required: [kind], properties: { kind: { type: string } } }
    Member:
      allOf: [{ required: [kind] }]
      oneOf: [{ type: string }, { $ref: '#/components/schemas/Cat' }]
    Sibling:
      required: [kind]
      allOf: [{ description: a pet }]
      oneOf: [{ type: string }, { $ref: '#/components/schemas/Cat' }]
    Untyped:
      allOf: [{ properties: { name: { type: string } } }]
      oneOf: [{ $ref: '#/components/schemas/Cat' }, { type: 'null' }]
    Typed:
      allOf: [{ type: [object, 'null'], properties: { name: { type: string } } }]
      oneOf: [{ $ref: '#/components/schemas/Cat' }, { type: 'null' }]
    Holder:
      type: object
      required: [member, sibling, untyped, typed]
      properties:
        member: { $ref: '#/components/schemas/Member' }
        sibling: { $ref: '#/components/schemas/Sibling' }
        untyped: { $ref: '#/components/schemas/Untyped' }
        typed: { $ref: '#/components/schemas/Typed' }
"#;

/// Issue #419: an `allOf` beside the schema's own `oneOf`/`anyOf` keeps every branch it does not
/// constrain away. An untyped `allOf` member and the schema's own keywords refine only the object
/// branch, so a string still decodes; where the union has a `null` branch and the `allOf` admits
/// `null` (an untyped member says nothing of it, a typed one lists it), `null` still decodes. The
/// fixture's own test decodes each wire shape through the generated types.
#[test]
fn an_all_of_beside_a_union_decodes_every_value_both_accept() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", ALL_OF_BESIDE_UNION_SPEC);
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "all_of_union_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/all_of_union.rs"),
        r##"
use all_of_union_client::types::Holder;

#[test]
fn strings_and_nulls_decode_beside_the_refined_object_branch() {
    let scalars = serde_json::json!({
        "member": "plain", "sibling": "plain", "untyped": null, "typed": null
    });
    let holder: Holder = serde_json::from_value(scalars.clone()).unwrap();
    assert!(holder.untyped.is_none());
    assert!(holder.typed.is_none());
    assert_eq!(serde_json::to_value(&holder).unwrap(), scalars);

    let objects = serde_json::json!({
        "member": { "kind": "cat" },
        "sibling": { "kind": "cat" },
        "untyped": { "kind": "cat", "name": "a" },
        "typed": { "kind": "cat", "name": "b" }
    });
    let holder: Holder = serde_json::from_value(objects.clone()).unwrap();
    assert!(holder.untyped.is_some());
    assert!(holder.typed.is_some());
    assert_eq!(serde_json::to_value(&holder).unwrap(), objects);

    // The object branch is still `Cat`: its required `kind` is enforced.
    let kindless = serde_json::json!({
        "member": {}, "sibling": "plain", "untyped": null, "typed": null
    });
    assert!(serde_json::from_value::<Holder>(kindless).is_err());
}
"##,
    )
    .unwrap();

    let status = fixture_cargo(&out).arg("test").status().unwrap();
    assert!(
        status.success(),
        "every value both the `allOf` and the union accept must decode"
    );
    let status = fixture_cargo(&out)
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "the allOf-beside-union fixture must lint clean"
    );
}

/// Object structs of every member shape `new` and the `with_*` setters handle: required members
/// (one nullable), optional members (one nullable, one keyword-named, one with a schema `default`,
/// one a recursive reference, one a union), a typed overflow map, a struct with no required member,
/// one with more required members than clippy's argument threshold (issue #1267), and one whose
/// required member is named like the provider of its defaulted one.
const CONSTRUCTORS_SPEC: &str = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /envelope:
    post:
      operationId: postEnvelope
      requestBody:
        content:
          application/json:
            schema: { $ref: '#/components/schemas/Envelope' }
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Settings' }
        '201':
          description: wide
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Wide' }
components:
  schemas:
    Envelope:
      type: object
      required: [id, count, parent]
      properties:
        id: { type: string }
        count: { type: integer }
        parent: { type: [string, 'null'] }
        note: { type: string }
        label: { type: [string, 'null'] }
        type: { type: string }
        mode: { type: string, enum: [fast, slow], default: fast }
        ratio: { type: number }
        child: { $ref: '#/components/schemas/Envelope' }
        pet: { oneOf: [{ type: string }, { type: array, items: { type: integer } }] }
      additionalProperties: { type: integer }
    Settings:
      type: object
      properties:
        verbose: { type: boolean }
        level: { type: integer }
    Tuned:
      type: object
      properties:
        retries: { type: integer, default: 3 }
        tags: { type: array, items: { $ref: '#/components/schemas/Settings' } }
    Wide:
      type: object
      required: [a, b, c, d, e, f, g, h]
      properties:
        a: { type: string }
        b: { type: string }
        c: { type: string }
        d: { type: string }
        e: { type: string }
        f: { type: string }
        g: { type: string }
        h: { type: string }
    Shadowed:
      type: object
      required: [default_29_speed]
      properties:
        default_29_speed: { type: string }
        speed: { type: string, default: fast }
"#;

/// The provider `new` calls to apply `Shadowed.speed`'s default. `Shadowed`'s required member has
/// the same name, so it becomes a `new` parameter that shadows a bare call to the provider.
const SHADOWED_PROVIDER: &str = "fn default_29_speed(";

/// Issue #1267: an object struct derives `PartialEq` (and `Default` when no member is required),
/// and `new` plus one `with_*` setter per optional member build the value a struct literal does, so
/// a test compares a decoded value with an expected one directly.
#[test]
fn object_structs_build_through_new_and_setters_and_compare_equal() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", CONSTRUCTORS_SPEC);
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "constructors_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let module = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert!(
        module.contains(SHADOWED_PROVIDER),
        "`Shadowed`'s required member must share its default provider's name; providers: {:?}",
        module
            .lines()
            .filter(|line| line.contains("fn default_"))
            .collect::<Vec<_>>()
    );

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/constructors.rs"),
        r##"
use std::collections::BTreeMap;

use constructors_client::types::{
    Envelope, EnvelopeMode, EnvelopePet, Settings, Shadowed, Tuned, Wide,
};

#[test]
fn new_and_setters_build_the_decoded_value() {
    let built = Envelope::new("e1".to_owned(), 3, None)
        .with_note("hi".to_owned())
        .with_label("l".to_owned())
        .with_type("kind".to_owned())
        .with_mode(EnvelopeMode::Slow)
        .with_ratio(0.5)
        .with_child(Box::new(Envelope::new("e2".to_owned(), 0, Some("e1".to_owned()))))
        .with_pet(EnvelopePet::EnvelopePetVariant0(Box::new("cat".to_owned())))
        .with_additional(BTreeMap::from([("extra".to_owned(), 7)]));
    let wire = serde_json::json!({
        "id": "e1", "count": 3, "parent": null, "note": "hi", "label": "l", "type": "kind",
        "mode": "slow", "ratio": 0.5,
        "child": { "id": "e2", "count": 0, "parent": "e1", "mode": "fast" },
        "pet": "cat", "extra": 7
    });
    let decoded: Envelope = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(decoded, built);
    assert_eq!(serde_json::to_value(&built).unwrap(), wire);
    assert_ne!(decoded, built.with_note("other".to_owned()));
}

#[test]
fn new_sets_each_optional_member_as_decoding_an_absent_one_does() {
    let envelope = Envelope::new("e".to_owned(), 1, Some("p".to_owned()));
    assert_eq!(envelope.note, None);
    assert_eq!(envelope.label, None);
    assert_eq!(envelope.mode, Some(EnvelopeMode::Fast));
    assert!(envelope.additional.is_empty());
    let decoded: Envelope =
        serde_json::from_value(serde_json::json!({ "id": "e", "count": 1, "parent": "p" }))
            .unwrap();
    assert_eq!(envelope, decoded);
}

#[test]
fn a_struct_with_no_required_member_defaults_to_new() {
    assert_eq!(Settings::default(), Settings::new());
    let settings = Settings::new().with_verbose(true).with_level(2);
    let decoded: Settings =
        serde_json::from_value(serde_json::json!({ "verbose": true, "level": 2 })).unwrap();
    assert_eq!(settings, decoded);

    // A schema `default` makes `Default` an `impl` calling `new`, which applies it.
    assert_eq!(Tuned::default(), Tuned::new());
    assert_eq!(Tuned::default().retries, Some(3));
    let decoded: Tuned = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(Tuned::default(), decoded);
}

#[test]
fn every_required_member_is_a_new_argument() {
    let [a, b, c, d, e, f, g, h] = ["a", "b", "c", "d", "e", "f", "g", "h"].map(str::to_owned);
    let wide = Wide::new(a, b, c, d, e, f, g, h);
    assert_eq!(wide.h, "h");
}

#[test]
fn new_applies_a_default_whose_provider_a_required_member_is_named_after() {
    let shadowed = Shadowed::new("x".to_owned());
    assert!(shadowed.speed.is_some());
    let decoded: Shadowed =
        serde_json::from_value(serde_json::json!({ "default_29_speed": "x" })).unwrap();
    assert_eq!(shadowed, decoded);
}
"##,
    )
    .unwrap();

    let status = fixture_cargo(&out).arg("test").status().unwrap();
    assert!(
        status.success(),
        "`new` and the setters must build the value decoding yields"
    );
    let status = fixture_cargo(&out)
        .args(["clippy", "--all-targets", "--", "-D", "warnings"])
        .status()
        .unwrap();
    assert!(status.success(), "the constructors fixture must lint clean");
}

/// Closed string enums reached as a component, a struct field, a query parameter and a response
/// body, beside a deprecated one. `ALL` is listed as a value so the variant it names sits next to
/// the `ALL` constant, `a_l_l` cases to the constant's own spelling, so its variant must take
/// another, and `a b` and `derivative_manifest` are values whose spelling the variant identifier
/// does not keep.
const CLOSED_ENUMS_SPEC: &str = r#"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /blobs:
    get:
      operationId: getBlobs
      parameters:
        - { name: role, in: query, schema: { $ref: '#/components/schemas/WireBlobRole' } }
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Blob' }
        '409':
          description: legacy
          content:
            application/json:
              schema: { $ref: '#/components/schemas/Legacy' }
components:
  schemas:
    WireBlobRole: { type: string, enum: [original, derivative_manifest, ALL, 'a b', a_l_l] }
    Legacy: { type: string, enum: [old], deprecated: true }
    Blob:
      type: object
      required: [role]
      properties:
        role: { $ref: '#/components/schemas/WireBlobRole' }
"#;

/// Issue #1266: a closed string enum is `Copy` and `Hash`, names its wire value through a `const`
/// `as_str` that `Display` writes, lists its variants in `ALL`, and parses with `FromStr`, which
/// accepts exactly the values serde decodes and fails with the root-nameable `UnknownVariant`.
#[test]
fn closed_string_enums_name_list_and_parse_their_wire_values() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(temp.path(), "openapi.yaml", CLOSED_ENUMS_SPEC);
    let out = temp.path().join("client");

    let report = generate_fixture_crate(&spec, &out, "closed_enums_client");
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    std::fs::create_dir_all(out.join("tests")).unwrap();
    std::fs::write(
        out.join("tests/closed_enums.rs"),
        r##"
use std::collections::HashSet;

use closed_enums_client::types::{Blob, WireBlobRole};
use closed_enums_client::UnknownVariant;

const MANIFEST: &str = WireBlobRole::DerivativeManifest.as_str();

#[test]
fn as_str_is_the_wire_value_display_writes_and_serde_uses() {
    assert_eq!(MANIFEST, "derivative_manifest");
    assert_eq!(WireBlobRole::AB.as_str(), "a b");
    for role in WireBlobRole::ALL {
        assert_eq!(role.to_string(), role.as_str());
        assert_eq!(serde_json::to_value(role).unwrap(), role.as_str());
    }
}

#[test]
fn all_lists_every_variant_in_declaration_order() {
    // `a_l_l`'s variant takes a disambiguated spelling, so `ALL` still names the constant.
    let spelled_all: WireBlobRole = "a_l_l".parse().unwrap();
    assert_eq!(spelled_all.as_str(), "a_l_l");
    assert_eq!(
        WireBlobRole::ALL,
        &[
            WireBlobRole::Original,
            WireBlobRole::DerivativeManifest,
            WireBlobRole::All,
            WireBlobRole::AB,
            spelled_all,
        ]
    );
}

#[test]
fn from_str_accepts_exactly_what_serde_decodes() {
    for role in WireBlobRole::ALL {
        let parsed: WireBlobRole = role.as_str().parse().unwrap();
        assert_eq!(parsed, *role);
        let decoded: WireBlobRole = serde_json::from_value(role.as_str().into()).unwrap();
        assert_eq!(decoded, parsed);
    }
    for text in ["", "Original", "derivative-manifest", "original ", "All"] {
        let error: UnknownVariant = text.parse::<WireBlobRole>().unwrap_err();
        assert_eq!(error.value(), text);
        assert_eq!(error.type_name(), "WireBlobRole");
        assert!(serde_json::from_value::<WireBlobRole>(text.into()).is_err());
    }
}

#[test]
fn a_field_is_copied_and_hashed() {
    let blob: Blob = serde_json::from_value(serde_json::json!({ "role": "ALL" })).unwrap();
    let borrowed = &blob;
    let role = borrowed.role;
    let roles: HashSet<WireBlobRole> = [role, blob.role, WireBlobRole::Original].into();
    assert_eq!(roles.len(), 2);
    assert!(roles.contains(&WireBlobRole::All));
}
"##,
    )
    .unwrap();

    let status = fixture_cargo(&out).arg("test").status().unwrap();
    assert!(
        status.success(),
        "a closed enum's as_str, ALL and FromStr must agree with its serde form"
    );
    let status = fixture_cargo(&out)
        .args([
            "clippy",
            "--all-features",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ])
        .status()
        .unwrap();
    assert!(status.success(), "the closed enums fixture must lint clean");
}
