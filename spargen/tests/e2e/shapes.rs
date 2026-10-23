//! Type shapes compiled and decoded in a fixture crate: tuples, alias cycles, deprecated
//! components, and an `allOf` beside a union.

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
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, TUPLE_ARITY_SPEC).unwrap();
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
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, ALIAS_CYCLE_SPEC).unwrap();
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
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, DEPRECATED_COMPONENTS_SPEC).unwrap();
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
    let spec = temp.path().join("openapi.yaml");
    std::fs::write(&spec, ALL_OF_BESIDE_UNION_SPEC).unwrap();
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
