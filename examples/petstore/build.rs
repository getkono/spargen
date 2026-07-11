//! Generates the petstore client into OUT_DIR at build time. This is spargen's `build.rs` mode;
//! the checked-in-output CLI mode (`spargen generate petstore.yaml --out src/petstore.rs`)
//! produces the same code.

fn main() {
    println!("cargo:rerun-if-changed=petstore.yaml");
    let out = std::env::var("OUT_DIR").unwrap();
    let config = spargen::Config::new(
        concat!(env!("CARGO_MANIFEST_DIR"), "/petstore.yaml"),
        spargen::OutputTarget::Module(format!("{out}/petstore.rs").into()),
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
