//! `spargen::check_with`: the consumer-manifest audit and rebuild triggers of a `check`, under each
//! `CargoIntegration` policy, from a real build script and outside one (#1430).

use super::manifest_audit::{build_script_consumer, build_script_output, BUILD_SCRIPT_HELPERS};
use super::*;

/// `check_with(.., CargoIntegration::Off)` from a build script neither audits the build script's
/// own manifest nor prints a rebuild trigger: a gate crate whose client lives elsewhere checks a
/// spec its manifest is not the consumer of, so a manifest missing a runtime dependency still
/// checks clean, with no diagnostic.
#[test]
fn check_with_cargo_integration_off_in_a_build_script_skips_the_audit_and_the_directives() {
    let build_rs = format!(
        r#"fn main() {{
    let spec = spargen::Spec::new(concat!(env!("CARGO_MANIFEST_DIR"), "/openapi.yaml"));
    let off = without_secrecy(|| spargen::check_with(&spec, spargen::CargoIntegration::Off));
    assert_eq!(off.outcome(), spargen::Outcome::Clean, "{{off:#?}}");
    assert!(off.diagnostics().is_empty(), "{{off:#?}}");
}}
{BUILD_SCRIPT_HELPERS}"#
    );
    let temp = build_script_consumer("check-off-consumer", &build_rs);

    let output = fixture_cargo(&temp.path().join("consumer"))
        .arg("check")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "the build script's assertions failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let recorded = build_script_output("check-off-consumer-");
    assert!(
        !recorded.contains("cargo:rerun-if-changed="),
        "`Off` printed a rebuild trigger:\n{recorded}"
    );
}

/// Outside a build script, `check_with` follows the policy `generate` does: `Required` rejects with
/// `E024` before any audit, while `Off` and `check`'s `Auto` check the spec with no Cargo
/// diagnostic, `W013` being about a generation's rebuild triggers.
#[test]
fn check_with_outside_a_build_script_follows_the_cargo_integration_policy() {
    let temp = tempfile::tempdir().unwrap();
    let spec_path = write_spec(
        temp.path(),
        "openapi.yaml",
        r#"
openapi: 3.1.0
info: { title: t, version: '1' }
paths:
  /ping:
    get: { operationId: ping, responses: { "204": { description: ok } } }
"#,
    );
    let spec = Spec::new(spec_path);

    let required = spargen::check_with(&spec, spargen::CargoIntegration::Required);
    assert_eq!(required.outcome(), Outcome::Rejected, "{required:#?}");
    let codes: Vec<_> = required
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect();
    assert_eq!(codes, ["E024"], "{required:#?}");

    for integration in [
        spargen::CargoIntegration::Off,
        spargen::CargoIntegration::Auto,
    ] {
        let report = spargen::check_with(&spec, integration);
        assert_eq!(
            report.outcome(),
            Outcome::Clean,
            "{integration:?}: {report:#?}"
        );
        assert!(
            report.diagnostics().is_empty(),
            "{integration:?}: {report:#?}"
        );
    }
    assert_eq!(
        format!("{:?}", spargen::check(&spec).diagnostics()),
        format!(
            "{:?}",
            spargen::check_with(&spec, spargen::CargoIntegration::Auto).diagnostics()
        )
    );
}
