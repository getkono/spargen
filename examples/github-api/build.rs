fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let mut config = spargen::Config::new(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../corpus/github-api-3-1/api.github.com.json"
        ),
        format!("{out}/github.rs"),
    );
    config.batch_cap = 100_000;
    let report = spargen::generate(&config);
    assert_eq!(
        report.outcome,
        spargen::Outcome::Generated,
        "spargen failed: {report:#?}"
    );
}
