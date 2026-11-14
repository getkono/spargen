//! Header bindings (`Spec::bind_header`): a bound header parameter leaves every operation's
//! arguments, and a binding no operation's header parameter matches is `E026`.

use super::*;

/// Two operations declaring `X-Api-Version`, one requiring it and one not, beside a query
/// parameter `tenant` and a required header `X-Trace`.
const VERSIONED: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /items:
    get:
      operationId: listItems
      parameters:
        - { name: X-Api-Version, in: header, required: true, schema: { type: string } }
        - { name: X-Trace, in: header, required: true, schema: { type: string } }
        - { name: tenant, in: query, schema: { type: string } }
      responses:
        '204': { description: ok }
  /status:
    get:
      operationId: getStatus
      parameters:
        - { name: x-api-version, in: header, schema: { type: string } }
      responses:
        '204': { description: ok }
"##;

/// The spec written in a fresh directory, for a fixture to set options on.
fn versioned_spec(temp: &tempfile::TempDir) -> Spec {
    Spec::new(write_spec(temp.path(), "openapi.yaml", VERSIONED))
}

/// A bound header is no argument of any operation that declares it, in whatever letter case the
/// binding and the declaration spell it: the required one is sent from the client's bound value
/// or fails as `MissingBoundParameter`, the optional one is sent only when bound, and an
/// operation whose one optional parameter was the bound header takes no parameters bundle. The
/// client gains `with_bound_header`; unbound headers and other locations are untouched.
#[test]
fn a_bound_header_leaves_every_operations_arguments() {
    let temp = tempfile::tempdir().unwrap();
    let (report, code) =
        generate_spec_with_code(versioned_spec(&temp).bind_header("X-API-VERSION"));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(report.diagnostics().is_empty(), "{report:#?}");

    let flat = module_body(&code)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // `listItems` keeps its other required header, in `ListItemsParams::new`, and its query
    // bundle, and loses the bound one.
    assert!(
        flat.contains("pub async fn list_items( &self, params: ListItemsParams, )"),
        "{code}"
    );
    assert!(
        flat.contains("pub fn new(x_trace: impl Into<types::XTrace>) -> Self"),
        "{code}"
    );
    assert!(
        flat.contains(
            "match self.core.bound_header(\"X-Api-Version\") { Some(value) => { request = \
             request.sensitive_header(\"X-Api-Version\", value); } None => {"
        ),
        "{code}"
    );
    assert!(
        flat.contains("support::RequestError::MissingBoundParameter { name: \"X-Api-Version\","),
        "{code}"
    );
    // The header's own inline schema type is named by nothing once the header is bound.
    assert!(!flat.contains("XApiVersion"), "{code}");
    // `getStatus`'s one parameter was the bound header, so it takes no argument at all.
    assert!(flat.contains("pub async fn get_status( &self, )"), "{code}");
    assert!(!flat.contains("GetStatusParams"), "{code}");
    assert!(
        flat.contains(
            "if let Some(value) = self.core.bound_header(\"x-api-version\") { request = \
             request.sensitive_header(\"x-api-version\", value); }"
        ),
        "{code}"
    );
    assert!(
        flat.contains(
            "pub fn with_bound_header( mut self, name: &str, value: impl Into<String>, ) -> \
             Result<Self, support::Error<std::convert::Infallible>>"
        ),
        "{code}"
    );
}

/// Without a binding the same header is an argument, and the client has no `with_bound_header`.
#[test]
fn an_unbound_header_stays_an_argument() {
    let temp = tempfile::tempdir().unwrap();
    let (report, code) = generate_spec_with_code(versioned_spec(&temp));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    assert!(code.contains("x_api_version"), "{code}");
    assert!(!code.contains("pub fn with_bound_header"), "{code}");
}

/// A binding that names no header parameter any operation declares is rejected with `E026`, at
/// the document root, by `check` and `generate` alike — a query parameter of that name does not
/// count — and every unmatched binding is reported, not only the first.
#[test]
fn a_binding_no_operation_declares_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let spec = versioned_spec(&temp)
        .bind_header("tenant")
        .bind_header("X-Api-Version")
        .bind_header("X-Missing");
    let (generated, checked, code) = generate_and_check_spec(spec);
    for report in [&generated, &checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(codes(report), ["E026", "E026"], "{report:#?}");
        assert!(has_code(report, Code::BoundHeaderNotDeclared));
        assert_eq!(
            messages_for(report, Code::BoundHeaderNotDeclared),
            [
                "the header binding `tenant` matches no header parameter any operation declares",
                "the header binding `X-Missing` matches no header parameter any operation \
                 declares",
            ],
            "{report:#?}"
        );
    }
    assert!(code.is_empty(), "nothing is written: {code}");
}

/// A binding is matched after every omit and keep rule applies: `X-Trace` is declared only by
/// `listItems`, so omitting that operation, or keeping only `getStatus`, leaves the binding
/// matching nothing, and it is rejected with `E026` by `check` and `generate` alike. The same
/// binding without either rule generates.
#[test]
fn a_binding_only_an_omitted_or_unkept_operation_declares_is_rejected() {
    let bound = tempfile::tempdir().unwrap();
    let (report, _) = generate_spec_with_code(versioned_spec(&bound).bind_header("X-Trace"));
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");

    // Each rejected run gets a directory of its own, so no earlier run's output is read back.
    let (omitted_dir, unkept_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let omitted = versioned_spec(&omitted_dir)
        .bind_header("X-Trace")
        .omit_rule(spargen::OmitRule::operation(
            spargen::OmitMethod::Get,
            "/items",
        ));
    let unkept = versioned_spec(&unkept_dir)
        .bind_header("X-Trace")
        .keep_operation(spargen::OmitMethod::Get, "/status");
    for spec in [omitted, unkept] {
        let (generated, checked, code) = generate_and_check_spec(spec);
        for report in [&generated, &checked] {
            assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
            assert_eq!(
                messages_for(report, Code::BoundHeaderNotDeclared),
                ["the header binding `X-Trace` matches no header parameter any operation declares"],
                "{report:#?}"
            );
        }
        assert!(code.is_empty(), "nothing is written: {code}");
    }
}

/// A header parameter the specification says to ignore (`W011`) is never sent, so binding it
/// binds nothing and is rejected with `E026` too.
#[test]
fn binding_an_ignored_header_parameter_is_rejected() {
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /items:
    get:
      operationId: listItems
      parameters:
        - { name: Authorization, in: header, required: true, schema: { type: string } }
      responses:
        '204': { description: ok }
"##;
    let temp = tempfile::tempdir().unwrap();
    let (report, _) = generate_spec_with_code(
        Spec::new(write_spec(temp.path(), "openapi.yaml", spec)).bind_header("authorization"),
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(codes(&report), ["E026", "W011"], "{report:#?}");
}
