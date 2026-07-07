# spargen

A compile-time-correct Rust client generator for OpenAPI 3.1.x. Nothing else.

Spargen consumes an OpenAPI 3.1.x document at generation time and produces idiomatic,
deterministic Rust: typed models, a `Client`, one method per operation, and typed errors.
Generated code compiles or generation fails — with a diagnostic that names the exact spec
construct, its JSON Pointer, and its source location. There is no runtime spec interpretation,
ever. Generated output is *freestanding*: no spargen crate appears in a consumer's runtime
dependency tree. See [`docs/prd.md`](docs/prd.md) for the full product requirements.

> **Status:** early scaffolding. The public API is being laid down subsystem by subsystem;
> implementations follow.

## Prerequisites

- [Rust](https://rustup.rs) (toolchain pinned by `rust-toolchain.toml`)
- [mise](https://github.com/jdx/mise) — dev tool provisioning and task runner
- [hk](https://hk.jdx.dev) — git hooks (installed via mise)
- [convco](https://github.com/convco/convco) — Conventional Commit checking (installed via mise)

## Quick Start

```bash
mise install          # provision hk, convco, cargo-deny
mise run hooks        # install git hooks (hk install --mise)
cargo check --workspace --all-features
```

## Development

| Command          | Description                                    |
| ---------------- | ---------------------------------------------- |
| `mise run check` | Type-check the workspace                       |
| `mise run fmt`   | Format the workspace                           |
| `mise run lint`  | Clippy with warnings denied                    |
| `mise run deny`  | Supply-chain audit (licenses, advisories, bans)|

Commit messages must follow [Conventional Commits](https://www.conventionalcommits.org/)
(`feat:`, `fix:`, `chore:`, …). This is enforced by `convco` on pre-push and in CI on pull
requests; merge commits are exempt.

## Architecture

One published crate, internally partitioned into subsystems with a machine-enforced dependency
DAG (`diag`, `source`, `ir`, `oas31`, `name`, `support`, `codegen`, `emit`, `cli`, plus the
`lib.rs` facade). The emitted runtime lives in the `support-runtime` workspace member
(`publish = false`) and is embedded verbatim into generated output. See PRD §2.3.

## Releases

Releases are automated via [release-plz](https://release-plz.dev): a standing pull request
tracks the next version bump; merging it tags the release and publishes to crates.io. The
generated `CHANGELOG.md` is derived from Conventional Commits. Never bump the version or tag
manually.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
