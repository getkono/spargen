//! The headline invariant (PRD FR3, CLAUDE.md): same spargen version + spec + config produces
//! byte-identical output. Generating the same spec into two independent crate directories must
//! yield identical `Cargo.toml` and `src/lib.rs`.

use camino::Utf8PathBuf;
use spargen::{Config, Outcome, OutputTarget};

const SPEC: &str = r##"
openapi: 3.1.0
info:
  title: Determinism
  version: 1.0.0
servers:
  - url: https://example.com/api
paths:
  /users/{id}:
    get:
      operationId: getUser
      parameters:
        - name: id
          in: path
          required: true
          schema: { type: string }
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/User" }
components:
  schemas:
    User:
      type: object
      required: [id, name]
      properties:
        id: { type: string }
        name: { type: string }
        age: { type: integer }
"##;

fn generate_crate(spec_path: &Utf8PathBuf, dir: &std::path::Path) {
    let report = spargen::generate(&Config::new(
        spec_path.clone(),
        OutputTarget::Crate {
            dir: Utf8PathBuf::from_path_buf(dir.to_path_buf()).unwrap(),
            name: "det_client".to_owned(),
        },
    ));
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");
}

#[test]
fn two_runs_produce_byte_identical_output() {
    // Same spec path + config, different output dirs: the provenance header (which records the
    // source path) is held constant, isolating the invariant to codegen ordering.
    let src = tempfile::tempdir().unwrap();
    let spec_path = Utf8PathBuf::from_path_buf(src.path().join("openapi.yaml")).unwrap();
    std::fs::write(&spec_path, SPEC).unwrap();

    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    generate_crate(&spec_path, a.path());
    generate_crate(&spec_path, b.path());

    let lib_a = std::fs::read(a.path().join("src/lib.rs")).unwrap();
    let lib_b = std::fs::read(b.path().join("src/lib.rs")).unwrap();
    assert_eq!(lib_a, lib_b, "generated src/lib.rs is not deterministic");

    let manifest_a = std::fs::read(a.path().join("Cargo.toml")).unwrap();
    let manifest_b = std::fs::read(b.path().join("Cargo.toml")).unwrap();
    assert_eq!(
        manifest_a, manifest_b,
        "generated Cargo.toml is not deterministic"
    );
}
