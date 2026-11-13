# Security policy

## Reporting a vulnerability

Report a suspected vulnerability privately through GitHub's private vulnerability reporting:
open [a new security advisory](https://github.com/getkono/spargen/security/advisories/new) for
this repository. Do not open a public issue, pull request, or discussion for it.

A useful report names the affected crate (`spargen`, `spargen-macro`, or the runtime embedded in
generated code) and its version, the OpenAPI description or configuration that triggers the
problem (a minimal one where possible), and what an attacker gains. The maintainers acknowledge
the report in the advisory, agree a fix and a disclosure date with you there, and publish the
advisory once a fixed release is on crates.io.

## Supported versions

Fixes are released from `master` as the next `spargen` and `spargen-macro` release on
[crates.io](https://crates.io/crates/spargen); earlier releases do not receive backports.

| Version | Supported |
| --- | --- |
| The latest published release | Yes |
| Any earlier release | No: upgrade to the latest |

Generated code embeds the runtime of the `spargen` release that generated it, so a fix to that
runtime reaches a consumer when they rebuild against a fixed `spargen`.

## Scope

Spargen runs at build time only, and the code it generates runs in the consumer's program. Both
are in scope:

- The generator: anything a crafted OpenAPI description, `spargen.toml`, or `spargen.lock` can
  make `spargen`, `spargen-macro`, or the `spargen` CLI do beyond reporting a diagnostic, such as
  reading or writing outside the paths it was given, or exhausting the build host's memory or
  stack.
- The generated client and its embedded runtime: a request sent somewhere other than the
  operation's URL, a credential attached where its security scheme does not apply or leaked
  through `Debug` or an error, or a response decoded into the wrong type.

### Network fetching

`spargen lock`, behind the `remote-fetch` feature, is the generator's only networked step. It
fetches remote `$ref` targets over reqwest's rustls stack with the bundled Mozilla roots, pins
each fetched document's SHA-256 in `spargen.lock`, and generation then reads only those vendored
copies. A way to make it accept a document that does not match its pin, follow a fetch to an
unintended host, or trust a certificate rustls would refuse is in scope. A TLS protocol
conformance issue in rustls itself belongs to [rustls](https://github.com/rustls/rustls).

### Advisory pins

Where an advisory names a crate that `spargen`'s published dependency graph reaches, `spargen`
carries a minimum version requirement on the patched release in `spargen/Cargo.toml`, mirrored by
a `[bans]` entry in [`deny.toml`](deny.toml) that names the advisory, because Cargo ignores a
dependency's lockfile and a consumer's build would otherwise resolve the crate itself. The CLI reference's
[TLS note](docs/book/src/cli.md#tls) describes the one such strictness the test suite
pins: `spargen/tests/tls_strictness.rs` fails on a rustls that accepts the non-conformant peer
RUSTSEC-2026-0285 describes. The supply-chain audit in `.github/workflows/deny.yml` runs daily
on `master` against the RustSec database: its `deny` job audits the workspace and example
lockfiles (locally, `mise run deny`), and its `deny-published` job audits the `Cargo.lock` that
the latest release ships for `cargo install spargen --features cli --locked`. An advisory on a dependency is fixed in
its own pull request and, where it reaches a published crate, in a new release.

The runtime crates that generated code needs (`reqwest`, `serde`, `serde_json`, and the others
`spargen deps` prints) are dependencies of the consumer's own crate and are resolved from its
manifest; keeping them patched is the consumer's, through their own lockfile and audit.
