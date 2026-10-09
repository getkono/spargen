//! Servers and server variables, including path-item and operation overrides.

use super::*;

#[test]
fn server_variables_generate_a_typed_builder() {
    // Regression: `servers[].variables` was dropped entirely, so a templated URL reached rustdoc
    // with its `{braces}` intact and no way to fill them.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
servers:
  - name: regional
    url: "https://{region}.example.com/{basePath}"
    variables:
      region:
        default: us
        enum: [us, eu]
      basePath:
        default: v2
paths:
  /x:
    get:
      responses:
        "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("pub mod servers"), "{code}");
    assert!(code.contains("pub fn default_url()"), "{code}");
    // The `enum` variable becomes a closed type, so an illegal region cannot be constructed.
    // Its variant set is the claim, so a dropped variant must fail here, not just a dropped type.
    assert_eq!(
        enum_variants(&code, "RegionalRegion"),
        ["Us", "Eu"],
        "{code}"
    );
    assert!(code.contains("with_default_server"), "{code}");
}

#[test]
fn e011_server_variable_default_outside_its_enum() {
    // The default is actually sent, so a default outside its own `enum` would make the
    // no-argument path put an illegal value on the wire.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers:
  - url: "https://{region}.example.com"
    variables:
      region:
        default: apac
        enum: [us, eu]
paths:
  /x:
    get:
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
    }
}

#[test]
fn e011_server_url_references_an_undeclared_variable() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers:
  - url: "https://{region}.example.com"
paths:
  /x:
    get:
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
    }
}

#[test]
fn path_item_and_operation_servers_override_the_base_url() {
    // Regression: `servers` was read only at the document root, so a Path Item or Operation Object
    // that redirects its calls to another host was skipped along with every other non-method key —
    // silently generating a client that called the document's server instead.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers:
  - url: https://api.example.com/v1
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        "204": { description: No Content }
  /upload:
    servers:
      - url: https://files.example.net/store
    post:
      operationId: uploadItem
      responses:
        "204": { description: No Content }
  /reports:
    get:
      operationId: getReport
      servers:
        - url: https://{region}.reports.example.org/v2
          variables:
            region:
              default: eu
              enum: [eu, us]
      responses:
        "204": { description: No Content }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    // No override: the client's base URL stands.
    assert!(
        code.contains("build_url_on(&self.core, None, &path, &query)"),
        "an operation without an override must pass no server: {code}"
    );
    // A path-item override applies to every operation on that path.
    assert!(
        code.contains(r#"Some("https://files.example.net/store")"#),
        "a path-item `servers` override must reach the URL builder: {code}"
    );
    // An operation override wins, and its variables are substituted with their declared defaults.
    assert!(
        code.contains(r#"Some("https://eu.reports.example.org/v2")"#),
        "an operation `servers` override must be rendered with variable defaults: {code}"
    );
}

#[test]
fn w011_server_override_past_the_first_has_no_effect() {
    // The specification defines no rule for a client to choose among several per-operation servers,
    // so the first is used and the rest are acknowledged rather than dropped in silence.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers:
  - url: https://api.example.com
paths:
  /x:
    get:
      servers:
        - url: https://first.example.net
        - url: https://second.example.net
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            has_code(&report, Code::DeclarationHasNoEffect),
            "{report:#?}"
        );
    }
}

#[test]
fn e011_server_override_variables_are_validated_like_the_document_s() {
    // The override goes through the same `lower_server`, so a template naming an undeclared
    // variable is rejected wherever it appears rather than only at the document root.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
servers:
  - url: https://api.example.com
paths:
  /x:
    get:
      servers:
        - url: https://{stage}.example.net
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::InvalidInput), "{report:#?}");
    }
}
