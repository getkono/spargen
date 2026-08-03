//! Generates the petstore client into OUT_DIR at build time.

fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let config = spargen::Config::new(
        concat!(env!("CARGO_MANIFEST_DIR"), "/petstore.yaml"),
        format!("{out}/petstore.rs"),
    );
    let report = spargen::generate(&config);
    for diagnostic in &report.diagnostics {
        println!(
            "cargo:warning=spargen {}: {}",
            diagnostic.code, diagnostic.message
        );
    }
    assert_eq!(
        report.outcome,
        spargen::Outcome::Generated,
        "spargen failed: {report:#?}"
    );
}
