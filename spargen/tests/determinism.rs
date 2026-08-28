//! The headline invariant (CLAUDE.md): same spargen version + spec + config produces
//! byte-identical output. Generating the same spec into two module paths must yield identical code.

use camino::Utf8PathBuf;
use spargen::{CargoIntegration, Outcome, Spec};

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

fn generate_module(spec_path: &Utf8PathBuf, path: &std::path::Path) {
    let report = spargen::generate(
        &Spec::new(spec_path.clone())
            .build(Utf8PathBuf::from_path_buf(path.to_path_buf()).unwrap())
            .cargo(CargoIntegration::Off),
    );
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
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
    generate_module(&spec_path, &a.path().join("api.rs"));
    generate_module(&spec_path, &b.path().join("api.rs"));

    let module_a = std::fs::read(a.path().join("api.rs")).unwrap();
    let module_b = std::fs::read(b.path().join("api.rs")).unwrap();
    assert_eq!(module_a, module_b, "generated module is not deterministic");
}
