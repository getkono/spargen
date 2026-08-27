fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let build = spargen::Spec::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/github-api-3-1/api.github.com.json"
    ))
    // GitHub's spec is large enough that the default 100-diagnostic batch truncates its warnings.
    .batch_cap(100_000)
    .build(format!("{out}/github.rs"))
    .cargo(spargen::CargoIntegration::Required);
    spargen::generate(&build).expect_success();
}
