# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.1](https://github.com/getkono/spargen/compare/spargen-v0.4.0...spargen-v0.4.1) - 2026-09-02

### Fixed

- *(runtime-contract)* keep climbing past a manifest that does not parse
- *(oas31)* prove an alternative decodes identically before dropping W014
- *(runtime-contract)* name the workspace root a reader can open
- *(oas31)* accept the 3.1 spelling of a binary body and media ranges
- *(runtime-contract)* resolve workspace-inherited deps in more layouts
- *(surface)* serialize diff codes and impacts as their documented strings
- *(name)* escape `gen`, reserved since edition 2024
- *(codegen)* stop panicking on a keyword header or server variable

### Other

- Merge pull request #81 from getkono/refactor/scope-remaining-subsystems
- Merge pull request #80 from getkono/refactor/scope-name-and-source
- Merge pull request #79 from getkono/refactor/scope-oas31-to-crate
- Merge pull request #78 from getkono/refactor/scope-ir-to-crate
- Merge pull request #77 from getkono/refactor/scope-cli-fields
- Merge pull request #75 from getkono/docs/facade-layering-claims
- Merge pull request #74 from getkono/test/corpus-verifies-pinned-hashes
- *(corpus)* verify the pinned hashes the docs already promise
- Merge remote-tracking branch 'origin/master' into test/subsystem-strategy-compliance
- correct the parity claim frontend.rs credits to E013
- *(name)* guard the keyword table against a deleted entry
- *(diff)* pin that a keyword-named operation is not a false rename
- resolve the intra-doc links private modules hid
- *(frontend)* assert check/generate parity as a property, not per fixture
- *(corpus)* drive the corpus from its manifest, and close the drift it hid
- *(config)* exercise precedence, not just discovery
- *(surface)* classify every change kind, and keep the set complete
- give emit its first tests, and compat a real fingerprint check
- *(cache)* exercise the cache hit, and pin the fingerprint as complete
- *(name)* pin determinism and identifier validity, the two claims with no test
- lint the layering DAG and the runtime embed invariants
- *(diag)* enforce the per-code fixture rule the docs already claim

## [0.4.0](https://github.com/getkono/spargen/compare/spargen-v0.3.0...spargen-v0.4.0) - 2026-08-29

### Fixed

- use as_chunks in the SHA-256 block loop
- [**breaking**] reject an additionalOperations key that is not a method token
- [**breaking**] give every Media Type Object and XML node type a disposition

### Other

- remove backlog references and changelog narration from comments
- pin the support documents to the codes that exist

## [0.3.0](https://github.com/getkono/spargen/compare/spargen-v0.2.2...spargen-v0.3.0) - 2026-08-28

### Added

- [**breaking**] report a diagnostic list the batch cap truncated
- accept the 3.2 dialect URI its own schema publishes
- [**breaking**] take impl Into for required string params and params bundles
- derive Debug and Clone on the generated client
- name every runtime type a generated signature uses
- [**breaking**] give generated error types Display and std::error::Error
- [**breaking**] let omit and carve target OpenAPI 3.2 operations and components
- surface security scheme and path item documentation
- report the media types a body offers but does not generate
- [**breaking**] emit RFC 3339 date types instead of time's own serde form
- [**breaking**] split Config into Spec and Build, and add `spargen deps`
- [**breaking**] give Report, Outcome and Diagnostic a usable public shape
- [**breaking**] support discriminator.defaultMapping and 3.2 security requirement URIs
- give the remaining dropped metadata a disposition
- [**breaking**] generate typed accessors for documented response headers
- [**breaking**] model server variables and generate typed server selection
- [**breaking**] reject XML hints that change the wire, and acknowledge allowEmptyValue
- [**breaking**] resolve Path Item and multi-file references, and give every security scheme a disposition
- [**breaking**] support the Encoding Object and honor requestBody.required
- [**breaking**] implement every OpenAPI parameter serialization style
- [**breaking**] generate Beam-compatible typed SSE streams
- [**breaking**] enforce generated runtime dependency contracts
- *(oas)* complete OpenAPI 3.1 and 3.2 conformance

### Fixed

- [**breaking**] emit each diagnostic at its own severity, and rename the method
- keep a literal glob metacharacter out of a bulk omit rule
- resolve a $ref'd encoding header instead of warning about it
- resolve header and media type refs like every other component
- give every XML node type a disposition
- make the new omit constructs reachable from every surface
- keep generated error type names out of the runtime prelude
- derive Deserialize only where the decode path uses serde
- check response header types in the IR invariants
- stop inventing a `paths` requirement after an omit profile
- apply a Path Item $ref's documentation siblings
- reject an optional dependency that generated code names unconditionally
- read a documented Set-Cookie response header per line
- send RFC 6570 multipart parts unencoded and reject the undefined shapes
- honor Path Item and Operation servers overrides
- qualify the type path in generated response-header structs
- correct four constructs that were lowered or serialized wrongly
- prevent operation parameter shadowing

### Other

- kill the mutants that survived in the new glob-escape logic
- narrow the file-level items that were wider than they need to be
- make four claims match what the code does
- build every feature combination, not just --all-features
- give the four facade-only diagnostics a fixture
- [**breaking**] report vendoring failure the way every other entry point does
- correct the API claims the code contradicts
- compile-verify the remaining OpenAPI 3.2 constructs
- [**breaking**] mark the growable public enums non_exhaustive
- [**breaking**] hide the unusable diagnostic builder constructors
- fold the generator API reference into Getting Started
- drop the 0.3 migration guide
- drop the validation plan page
- drop the benchmarks page
- drop the corpus page in favour of the corpus README
- *(mise)* define the hook gates as tasks
- correct the claims the code contradicts
- pin the diagnostic index against the declared codes
- bring the support matrix and 3.2 scope up to what the code now does
- exercise servers, deepObject, response headers and multipart in the example
- pin the serialization constructs at the wire, including a 3.2 arm
- [**breaking**] restrict generation to compile-time Rust APIs

## [0.2.2](https://github.com/getkono/spargen/compare/spargen-v0.2.1...spargen-v0.2.2) - 2026-07-22

### Added

- *(media)* decode textual and binary responses
- *(oas31)* support overlapping typed unions
- *(oas31)* intersect compatible allOf schemas

### Fixed

- *(codegen)* normalize rustdoc continuations
- *(codegen)* box multi-status response payloads
- *(codegen)* omit empty rustdoc attributes
- *(codegen)* box generated union payloads
- *(codegen)* lint deprecated blocking shims
- *(codegen)* normalize generated rustdoc whitespace
- *(codegen)* serialize typed OpenAPI parameters
- *(runtime)* satisfy strict generated-client lints
- *(diag)* deduplicate identical diagnostics

### Other

- *(recipes)* generate overlapping utoipa unions
- *(oas31)* cover typed overlapping unions
- *(compat)* keep carve fixtures unsupported
- *(corpus)* gate the complete GitHub API client
- Update README with project status and description

## [0.2.1](https://github.com/getkono/spargen/compare/spargen-v0.2.0...spargen-v0.2.1) - 2026-07-20

### Fixed

- *(release)* finalize macro trusted publishing

## [0.2.0](https://github.com/getkono/spargen/compare/spargen-v0.1.0...spargen-v0.2.0) - 2026-07-20

### Added

- *(cli)* preview generated code to stdout with 'generate --out -'
- in-memory preview() facade returning rendered files
- *(cli)* watch mode — regenerate on spec/config/ref changes
- *(cli)* spargen diff — semver impact between spec versions
- *(compat)* omit globbing / bulk + auto-carve
- *(cli)* spargen.toml config file + CLI omit-profile surface
- *(source)* line-precise diagnostic spans (add E022)
- *(runtime)* WASM / browser target support
- *(codegen)* blocking (sync) client mode behind an optional feature
- *(runtime)* middleware / interceptor hooks on the transport seam
- *(runtime)* retry adapter (bring-your-own policy) on the transport seam
- *(runtime)* HTTP-backend transport seam
- *(runtime)* generic Link-header pagination helper
- *(codegen)* fluent setters on the optional-params struct
- *(oas31)* accept OpenAPI 3.2.x through the extended frontend
- *(source)* resolve remote/cross-file $ref via deterministic hash pinning, narrow E003
- *(codegen)* support XML request/response bodies behind an optional feature, narrow E009
- *(runtime)* typed streaming SSE / x-ndjson responses, narrow E009
- *(codegen)* support multipart/form-data request bodies, narrow E009
- *(codegen)* typed multi-status response enums, retire W003
- *(oas31)* lower oneOf/anyOf unions, narrow E007, flip ollama to generate
- *(oas31)* merge allOf composition into a struct, repurpose E013
- *(oas31)* represent null-mixed enums as nullable, narrow E008
- *(oas31)* lower patternProperties to a typed map, narrow E005
- *(oas31)* support schema default values and close the silent-drop gap (W005)
- *(oas31)* box recursive $ref cycles instead of rejecting (retire E014)

### Fixed

- *(codegen)* silence inline blocking cfg warnings
- *(deps)* bump quick-xml to 0.41 to clear RUSTSEC-2026-0194/0195
- *(codegen)* escape keyword-named params/fields to raw identifiers

### Other

- document the three generation modes and two-crate layout
- *(ecosystem)* mdBook documentation site
- *(bench)* generation benchmarks + progenitor/openapi-generator comparison
- *(ecosystem)* utoipa / aide / poem-openapi round-trip recipes
- *(trust)* fuzz the oas31 frontend (+ fix deep-recursion stack overflow)
- *(trust)* insta snapshot suite across the corpus
- *(trust)* property tests for union / allOf round-trip
