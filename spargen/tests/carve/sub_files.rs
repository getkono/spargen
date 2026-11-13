//! Carve across a multi-file description: a rejection whose provenance lies in a referenced
//! sub-file is carved in the file it is written in.

use super::*;

/// The W009 messages of a report, which name the rule that removed each construct.
fn w009_messages(report: &Report) -> Vec<&str> {
    report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == Code::OmittedConstruct)
        .map(|diagnostic| diagnostic.message.as_str())
        .collect()
}

/// A rejection whose provenance lies in a referenced sub-file is carved in the file it is written
/// in. The rule used to be derived from the pointer alone, so a dangling `$ref` inside `lib.yaml`'s
/// `Node` yielded `component schemas Node`, which was read against the root document, matched
/// nothing there, and ended the run with `E019`, taking the healthy operations with it. Now the
/// derived rule is a file-scoped pointer (`lib.yaml#/components/schemas/Node`); the next round
/// finds the root's `$ref` to it dangling and carves the operation that holds it.
///
/// `xlib.yaml` is loaded first and declares a healthy `Node` of its own, so a file-scoped rule
/// resolved by bare suffix (`lib.yaml` is a suffix of `xlib.yaml`) would remove the wrong one and
/// carve away `getUsesHealthy` too. The root-document counterpart is
/// `a_dangling_ref_in_an_unreferenced_component_rejects_and_omitting_it_recovers`.
#[test]
fn carve_recovers_a_rejection_whose_provenance_lies_in_a_sub_file() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_split_description(temp.path());
    let out = temp.path().join("client.rs");

    // Without carve, the sub-file's dangling reference rejects the document.
    let report = spargen::generate(&config(&spec, &out));
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|d| d.code == Code::UnresolvedRef),
        "the dangling sub-file reference is E004: {report:#?}"
    );

    // With carve, the sub-file's component is removed from the sub-file, the root operation whose
    // `$ref` that leaves dangling is carved on the next round, and the rest generates.
    let report = spargen::generate(&carving(&spec, &out));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(
        !report
            .diagnostics()
            .iter()
            .any(|d| d.code == Code::InvalidOmitRule),
        "no derived rule misses its construct: {report:#?}"
    );
    let mut carved = w009_messages(&report);
    carved.sort_unstable();
    assert_eq!(
        carved,
        [
            "omitted construct matched by `get /uses-node`",
            "omitted construct matched by `pointer lib.yaml#/components/schemas/Node`",
        ],
        "exactly the sub-file component and its one referrer are carved: {report:#?}"
    );
    let generated = std::fs::read_to_string(&out).unwrap();
    // The same-named component in `xlib.yaml` is not the one carved.
    assert_eq!(
        operation_methods(&generated),
        [
            "get_good",
            "get_good_url",
            "get_uses_healthy",
            "get_uses_healthy_url"
        ]
    );

    // The derived rule names the sub-file relative to the root document, so the omit fingerprint
    // stamped into the header does not depend on where the description lives.
    let elsewhere = tempfile::tempdir().unwrap();
    let moved = write_split_description(elsewhere.path());
    let moved_out = elsewhere.path().join("client.rs");
    let report = spargen::generate(&carving(&moved, &moved_out));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let fingerprint = |source: &str| {
        let line = source
            .lines()
            .find(|line| line.starts_with("// source: "))
            .expect("provenance header names its source");
        line[line
            .find(" omit=")
            .expect("a carved build stamps its omit profile")..]
            .to_owned()
    };
    assert_eq!(
        fingerprint(&generated),
        fingerprint(&std::fs::read_to_string(&moved_out).unwrap())
    );
}

/// The split description [`carve_recovers_a_rejection_whose_provenance_lies_in_a_sub_file`]
/// carves, written into `dir`; returns the root document's path.
fn write_split_description(dir: &Path) -> Utf8PathBuf {
    let spec = write_spec(
        dir,
        "openapi.yaml",
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [ { url: https://example.com } ]
paths:
  /good:
    get:
      operationId: getGood
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { type: object, properties: { id: { type: string } } }
  /uses-healthy:
    get:
      operationId: getUsesHealthy
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "./xlib.yaml#/components/schemas/Node" }
  /uses-node:
    get:
      operationId: getUsesNode
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "./lib.yaml#/components/schemas/Node" }
"##,
    );
    write_spec(
        dir,
        "xlib.yaml",
        r##"
components:
  schemas:
    Node: { type: object, properties: { name: { type: string } } }
"##,
    );
    write_spec(
        dir,
        "lib.yaml",
        r##"
components:
  schemas:
    Node:
      type: object
      properties:
        missing: { $ref: "#/components/schemas/Missing" }
"##,
    );
    spec
}

/// A sub-file that is a bare schema declares no `paths` or `components` of
/// its own, so no construct inside it can be omitted without editing the schema's shape. The
/// rejection is instead carved at every construct that references it — here transitively, through
/// `schemas/wrapper.yaml` referencing `schemas/node.yaml` — ending at the root operations that
/// reach it. `getAlsoGood` references the healthy half of `schemas/wrapper.yaml` only, and survives.
#[test]
fn carve_follows_a_bare_sub_file_rejection_back_to_the_root_constructs_that_reach_it() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [ { url: https://example.com } ]
paths:
  /good:
    get:
      operationId: getGood
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { type: object, properties: { id: { type: string } } }
  /also-good:
    get:
      operationId: getAlsoGood
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "./schemas/wrapper.yaml#/$defs/Healthy" }
  /uses-wrapper:
    get:
      operationId: getUsesWrapper
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "./schemas/wrapper.yaml" }
"##,
    );
    std::fs::create_dir(temp.path().join("schemas")).unwrap();
    write_spec(
        temp.path(),
        "schemas/wrapper.yaml",
        r##"
type: object
properties:
  node: { $ref: "./node.yaml" }
$defs:
  Healthy: { type: object, properties: { name: { type: string } } }
"##,
    );
    write_spec(
        temp.path(),
        "schemas/node.yaml",
        r##"
type: object
properties:
  missing: { $ref: "#/$defs/Missing" }
"##,
    );
    let out = temp.path().join("client.rs");

    let report = spargen::generate(&config(&spec, &out));
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");

    let report = spargen::generate(&carving(&spec, &out));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert_eq!(
        w009_messages(&report),
        ["omitted construct matched by `get /uses-wrapper`"],
        "only the root operation that reaches the bad file is carved: {report:#?}"
    );
    let generated = std::fs::read_to_string(&out).unwrap();
    assert_eq!(
        operation_methods(&generated),
        [
            "get_also_good",
            "get_also_good_url",
            "get_good",
            "get_good_url"
        ]
    );
}

/// A sub-file that declares `paths` of its own is carved at the same three depths the root is: a
/// rejected method as that one operation, a rejected `additionalOperations` method as that one
/// method, and a path-item-level rejection as the whole path item — whose root `$ref` is then left
/// dangling and carved as the root path on the next round. The healthy sibling method in each
/// path item survives.
#[test]
fn carve_scopes_a_sub_file_paths_rejection_to_its_operation_method_or_path() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
servers: [ { url: https://example.com } ]
paths:
  /good:
    get:
      operationId: getGood
      responses: { "204": { description: OK } }
  /x: { $ref: "./lib.yaml#/paths/~1x" }
  /y: { $ref: "./lib.yaml#/paths/~1y" }
  /z: { $ref: "./lib.yaml#/paths/~1z" }
"##,
    );
    write_spec(
        temp.path(),
        "lib.yaml",
        r##"
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/sdp: { schema: { type: string } }
    post:
      operationId: postX
      responses: { "204": { description: OK } }
  /y:
    get:
      operationId: getY
      responses: { "204": { description: OK } }
    additionalOperations:
      PURGE:
        operationId: purgeY
        responses:
          "200":
            description: OK
            content:
              application/sdp: { schema: { type: string } }
  /z:
    parameters: [ { $ref: "#/components/parameters/Missing" } ]
    get:
      operationId: getZ
      responses: { "204": { description: OK } }
"##,
    );
    let out = temp.path().join("client.rs");

    let report = spargen::generate(&carving(&spec, &out));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let mut carved = w009_messages(&report);
    carved.sort_unstable();
    assert_eq!(
        carved,
        [
            "omitted construct matched by `path /z`",
            "omitted construct matched by `pointer lib.yaml#/paths/~1x/get`",
            "omitted construct matched by `pointer lib.yaml#/paths/~1y/additionalOperations/PURGE`",
            "omitted construct matched by `pointer lib.yaml#/paths/~1z`",
        ],
        "each sub-file rejection is carved at its own depth: {report:#?}"
    );
    let generated = std::fs::read_to_string(&out).unwrap();
    // `get_x`, `purge_y` and `get_z` are carved.
    assert_eq!(
        operation_methods(&generated),
        [
            "get_good",
            "get_good_url",
            "get_y",
            "get_y_url",
            "post_x",
            "post_x_url"
        ]
    );
}

/// A bare path item file has no `paths` of its own, but each method in it is still an operation
/// that can be removed alone: every `$ref` that reaches the file sits where a Path Item belongs,
/// so the rejected `get` is carved as `pi.yaml#/get` and the healthy `post` beside it survives.
/// Carving the root path the `$ref` sits at instead would take `postZ` with it. A rejection at the
/// path-item level of such a file (`pi-params.yaml`'s dangling parameter) has no smaller construct
/// than the path item, so it is still followed back to the root path that reaches it.
#[test]
fn carve_scopes_a_bare_path_item_file_rejection_to_its_operation() {
    let temp = tempfile::tempdir().unwrap();
    let spec = write_spec(
        temp.path(),
        "openapi.yaml",
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers: [ { url: https://example.com } ]
paths:
  /good:
    get:
      operationId: getGood
      responses: { "204": { description: OK } }
  /z: { $ref: "./pi.yaml" }
  /w: { $ref: "./pi-params.yaml" }
"##,
    );
    write_spec(
        temp.path(),
        "pi.yaml",
        r##"
get:
  operationId: getZ
  responses:
    "200":
      description: OK
      content:
        application/json:
          schema: { $ref: "#/$defs/Missing" }
post:
  operationId: postZ
  responses: { "204": { description: OK } }
"##,
    );
    write_spec(
        temp.path(),
        "pi-params.yaml",
        r##"
parameters: [ { $ref: "#/$defs/Missing" } ]
get:
  operationId: getW
  responses: { "204": { description: OK } }
"##,
    );
    let out = temp.path().join("client.rs");

    let report = spargen::generate(&carving(&spec, &out));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let mut carved = w009_messages(&report);
    carved.sort_unstable();
    assert_eq!(
        carved,
        [
            "omitted construct matched by `path /w`",
            "omitted construct matched by `pointer pi.yaml#/get`",
        ],
        "the bad operation alone is carved from the path item file: {report:#?}"
    );
    let generated = std::fs::read_to_string(&out).unwrap();
    // The healthy sibling `post_z` survives; `get_z` and `get_w` are carved.
    assert_eq!(
        operation_methods(&generated),
        ["get_good", "get_good_url", "post_z", "post_z_url"]
    );
}

/// A sub-file the description names by an absolute path lies outside the root document's
/// directory, so the derived rule names it by that absolute path — the one location the
/// description itself fixes — and the rule still resolves back to that file.
#[test]
fn carve_names_a_sub_file_outside_the_root_directory_by_its_stored_path() {
    let root = tempfile::tempdir().unwrap();
    let shared = tempfile::tempdir().unwrap();
    let lib = write_spec(
        shared.path(),
        "lib.yaml",
        r##"
components:
  schemas:
    Node:
      type: object
      properties:
        missing: { $ref: "#/components/schemas/Missing" }
"##,
    );
    let lib = lib.as_str();
    let spec = write_spec(
        root.path(),
        "openapi.yaml",
        &format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [ {{ url: https://example.com }} ]
paths:
  /good:
    get:
      operationId: getGood
      responses: {{ "204": {{ description: OK }} }}
  /uses-node:
    get:
      operationId: getUsesNode
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: {{ $ref: "{lib}#/components/schemas/Node" }}
"##
        ),
    );
    let out = root.path().join("client.rs");

    let report = spargen::generate(&carving(&spec, &out));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    let mut carved = w009_messages(&report);
    carved.sort_unstable();
    assert_eq!(
        carved,
        [
            "omitted construct matched by `get /uses-node`".to_owned(),
            format!("omitted construct matched by `pointer {lib}#/components/schemas/Node`"),
        ],
        "{report:#?}"
    );
    let generated = std::fs::read_to_string(&out).unwrap();
    assert_eq!(operation_methods(&generated), ["get_good", "get_good_url"]);
}

/// A root in `dir` whose operation `getN` answers with each of `refs` in turn (relative files or
/// vendored remote URLs), so they load in that order after the root. Returns its path.
fn write_root_referring_to(dir: &Path, refs: &[&str]) -> Utf8PathBuf {
    let mut root = String::from(
        "openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\n\
         servers: [ { url: https://example.com } ]\npaths:\n",
    );
    for (index, reference) in refs.iter().enumerate() {
        root.push_str(&format!(
            "  /p{index}:\n    get:\n      operationId: get{index}\n      responses:\n        \
             \"200\":\n          description: OK\n          content:\n            \
             application/json:\n              schema: {{ $ref: \"{reference}\" }}\n"
        ));
    }
    write_spec(dir, "openapi.yaml", &root)
}

/// An object schema declaring `field` beside a `keep` property, written at `dir/relative`.
fn write_schema_file(dir: &Path, relative: &str, field: &str) {
    let path = dir.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!(
            "type: object\nproperties:\n  {field}: {{ type: string }}\n  keep: {{ type: string }}\n"
        ),
    )
    .unwrap();
}

/// The messages of every `E019` in `report`.
fn e019_messages(report: &Report) -> Vec<&str> {
    report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == Code::InvalidOmitRule)
        .map(|diagnostic| diagnostic.message.as_str())
        .collect()
}

/// A rule's `file` names a loaded file by whole path components, never by string suffix: with
/// `schemas/legacy-pet.yaml` loaded before `schemas/pet.yaml`, `pet.yaml` names the second. A
/// string suffix match took `legacy-pet.yaml`, the first in load order, where the pointer missed
/// and the run ended `E019`.
#[test]
fn a_file_scoped_rule_names_its_file_by_whole_path_components() {
    let temp = tempfile::tempdir().unwrap();
    write_schema_file(temp.path(), "schemas/legacy-pet.yaml", "legacy_field");
    write_schema_file(temp.path(), "schemas/pet.yaml", "pet_field");
    let spec = write_root_referring_to(
        temp.path(),
        &["./schemas/legacy-pet.yaml", "./schemas/pet.yaml"],
    );
    let out = temp.path().join("client.rs");
    let rule = spargen::OmitRule::pointer(Some("pet.yaml".into()), "/properties/pet_field");
    let report = spargen::generate(&omitting(&spec, &out, omit_of(vec![rule])));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert_eq!(
        w009_messages(&report),
        ["omitted construct matched by `pointer pet.yaml#/properties/pet_field`"]
    );
    let generated = std::fs::read_to_string(&out).unwrap();
    assert!(!generated.contains("pet_field"), "{generated}");
    assert!(generated.contains("legacy_field"), "{generated}");
}

/// A `file` two loaded files end in — two local files sharing a basename, or a local file and a
/// vendored remote copy under `.spargen/vendor/` — is `E019` naming both, rather than whichever
/// loaded first. The path relative to the root document names exactly one of them.
#[test]
fn a_file_scoped_rule_naming_a_shared_basename_is_e019_naming_the_candidates() {
    use sha2::{Digest, Sha256};

    let url = "https://h.example/v1/pet.yaml";
    let vendored = ".spargen/vendor/h.example/v1/pet.yaml";
    for (label, refs, candidates) in [
        (
            "two local files",
            ["./b/pet.yaml", "./a/pet.yaml"],
            ["b/pet.yaml", "a/pet.yaml"],
        ),
        (
            "a local file and a vendored copy",
            ["./schemas/pet.yaml", url],
            ["schemas/pet.yaml", vendored],
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        for candidate in candidates {
            write_schema_file(temp.path(), candidate, "pet_field");
        }
        if let Ok(bytes) = std::fs::read(temp.path().join(vendored)) {
            std::fs::write(
                temp.path().join("spargen.lock"),
                format!(
                    "version = 1\n\n[[remote]]\nurl = \"{url}\"\nsha256 = \"{:x}\"\n\
                     path = \"h.example/v1/pet.yaml\"\n",
                    Sha256::digest(&bytes)
                ),
            )
            .unwrap();
        }
        let spec = write_root_referring_to(temp.path(), &refs);
        let out = temp.path().join("client.rs");

        let rule = spargen::OmitRule::pointer(Some("pet.yaml".into()), "/properties/pet_field");
        let report = spargen::generate(&omitting(&spec, &out, omit_of(vec![rule])));
        assert_eq!(report.outcome(), Outcome::Rejected, "{label}: {report:#?}");
        assert_eq!(
            e019_messages(&report),
            [format!(
                "omit rule's file matches more than one loaded file ({}, {}); name it by its \
                 path relative to the root document: pointer pet.yaml#/properties/pet_field",
                candidates[0], candidates[1]
            )],
            "{label}"
        );

        for candidate in candidates {
            let rule = spargen::OmitRule::pointer(
                Some(candidate.to_owned().into()),
                "/properties/pet_field",
            );
            let report = spargen::generate(&omitting(&spec, &out, omit_of(vec![rule])));
            assert_eq!(
                report.outcome(),
                Outcome::Generated,
                "{label}, {candidate}: {report:#?}"
            );
            assert_eq!(w009_messages(&report).len(), 1, "{label}, {candidate}");
        }
    }
}
