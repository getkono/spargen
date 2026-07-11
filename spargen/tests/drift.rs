//! `generate --check` is a CI contract gate: it re-plans output and compares it against what is
//! checked in, reporting clean / drifted / missing without writing (PRD FR6). These tests drive the
//! `check_only` config path against a real on-disk module.

use camino::Utf8PathBuf;
use spargen::{Code, Config, Outcome, OutputTarget, Report};

const SPEC: &str = r##"
openapi: 3.1.0
info:
  title: Drift
  version: 1.0.0
paths:
  /ping:
    get:
      operationId: ping
      responses:
        "204": { description: No Content }
"##;

fn config(spec: &Utf8PathBuf, out: &Utf8PathBuf) -> Config {
    Config::new(spec.clone(), OutputTarget::Module(out.clone()))
}

fn has_code(report: &Report, code: Code) -> bool {
    report.diagnostics.iter().any(|d| d.code == code)
}

#[test]
fn check_only_reports_clean_drifted_and_missing() {
    let temp = tempfile::tempdir().unwrap();
    let spec = Utf8PathBuf::from_path_buf(temp.path().join("openapi.yaml")).unwrap();
    std::fs::write(&spec, SPEC).unwrap();
    let out = Utf8PathBuf::from_path_buf(temp.path().join("client.rs")).unwrap();

    // Generate once for real so there is checked-in output to compare against.
    let report = spargen::generate(&config(&spec, &out));
    assert_eq!(report.outcome, Outcome::Generated, "{report:#?}");

    // Unmodified output is clean: no drift diagnostic.
    let mut check = config(&spec, &out);
    check.check_only = true;
    let report = spargen::generate(&check);
    assert_eq!(report.outcome, Outcome::Clean, "{report:#?}");
    assert!(!has_code(&report, Code::OutputDrifted));

    // A modified file drifts and surfaces W004.
    let mut modified = std::fs::read_to_string(out.as_std_path()).unwrap();
    modified.push_str("\n// hand edit\n");
    std::fs::write(out.as_std_path(), modified).unwrap();
    let report = spargen::generate(&check);
    assert_eq!(report.outcome, Outcome::Drifted, "{report:#?}");
    assert!(has_code(&report, Code::OutputDrifted));

    // A missing file also drifts, and the diagnostic says so.
    std::fs::remove_file(out.as_std_path()).unwrap();
    let report = spargen::generate(&check);
    assert_eq!(report.outcome, Outcome::Drifted, "{report:#?}");
    let drift = report
        .diagnostics
        .iter()
        .find(|d| d.code == Code::OutputDrifted)
        .expect("expected a W004 diagnostic");
    assert!(
        drift.message.contains("missing"),
        "expected the drift message to mention the file is missing: {drift:#?}"
    );
}
