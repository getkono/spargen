//! What the runtime requirements derive from: the types the output carries, and none of the
//! definitions lowering withheld from it (#723).

use super::*;

/// The `[dependencies]` block `spargen deps` prints for an inline spec.
fn manifest_block(spec: &str) -> String {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
    std::fs::write(&spec_path, spec).unwrap();
    spargen::requirements(&Spec::new(spec_path))
        .expect("spec lowers")
        .manifest_block()
}

/// A union met with an `allOf`'s other members is lowered on its own first, and the branch the
/// meet drops is withheld from the output (#561). A `format: date` or `contentEncoding` bytes
/// branch that only that withheld lowering holds is no type of the client, so it declares neither
/// the `time` dependency nor the `bytes` `serde` feature, and embeds no RFC 3339 newtype. Each
/// spec's control keeps the branch, so the same spelling is shown to reach the requirement.
#[test]
fn a_withheld_union_branch_adds_no_runtime_requirement() {
    fn spec(branch: &str, other: &str) -> String {
        format!(
            r#"
openapi: 3.1.0
info: {{ title: t, version: '1' }}
paths:
  /x:
    get:
      operationId: getX
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: {{ $ref: '#/components/schemas/U' }}
components:
  schemas:
    U:
      allOf:
        - oneOf: [{{ type: object, properties: {{ v: {branch} }} }}, {{ type: integer }}]
        - {other}
"#
        )
    }
    let date = "{ type: string, format: date }";
    let bytes = "{ type: string, contentEncoding: base64 }";
    for (branch, needle) in [(date, "time ="), (bytes, "\"serde\"")] {
        // The control: met with an object, the object branch survives the meet.
        let kept = spec(branch, "{ type: object }");
        let block = manifest_block(&kept);
        assert!(
            block.contains(needle),
            "the surviving {branch} branch must reach `{needle}`:\n{block}"
        );
        // Met with an integer, only the integer branch survives.
        let withheld = spec(branch, "{ type: integer }");
        let (report, code) = generate_with_code(&withheld);
        assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
        let block = manifest_block(&withheld);
        assert!(
            !block.contains(needle),
            "a withheld {branch} branch must not reach `{needle}`:\n{block}"
        );
        assert!(
            !code.contains("pub struct Date"),
            "a withheld {branch} branch must embed no RFC 3339 newtype"
        );
    }
}
