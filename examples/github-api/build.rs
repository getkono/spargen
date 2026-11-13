fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let build = spargen::Spec::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/github-api-3-1/api.github.com.json"
    ))
    // GitHub's spec is large enough that the default 100-diagnostic batch truncates its warnings.
    .with_batch_cap(100_000)
    .build(format!("{out}/github.rs"))
    .with_cargo(spargen::CargoIntegration::Required);
    let report = spargen::generate(&build);
    report.emit_cargo_diagnostics();
    report.expect_success();
}
