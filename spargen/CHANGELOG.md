# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
