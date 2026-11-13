//! Generates the petstore client into OUT_DIR at build time.

fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let build = spargen::Spec::new(concat!(env!("CARGO_MANIFEST_DIR"), "/petstore.yaml"))
        // Each documented problem status narrows the problem `type` to one value; open those sets,
        // so a problem type the server adds later still decodes into `Error::Api`.
        .with_open_narrowing(spargen::Narrowing::Open)
        .build(format!("{out}/petstore.rs"))
        // This IS a build script, and generating without Cargo wired up would mean a silently
        // stale client — so say it must be, and fail loudly if it ever is not.
        .with_cargo(spargen::CargoIntegration::Required);
    let report = spargen::generate(&build);
    report.emit_cargo_diagnostics();
    // Accepts a fresh render and a verified cache hit alike; panics with the full diagnostic list
    // otherwise.
    report.expect_success();
}
