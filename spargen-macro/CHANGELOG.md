# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/getkono/spargen/compare/spargen-macro-v0.5.0...spargen-macro-v0.6.0) - 2026-10-11

### Added

- open const-narrowed response strings, and read problem details across operations ([#396](https://github.com/getkono/spargen/pull/396))

### Fixed

- *(macro)* fail loudly instead of auditing ./Cargo.toml when Cargo names no crate ([#338](https://github.com/getkono/spargen/pull/338))

### Other

- cut duplicated and narrative prose from AGENTS.md, README and the book ([#548](https://github.com/getkono/spargen/pull/548))
- pin the exact diff, carve, macro and e2e outcomes their fixtures name ([#530](https://github.com/getkono/spargen/pull/530))
- make the docs' claims true: link gate, samples, install pins, stale counts ([#506](https://github.com/getkono/spargen/pull/506))

## [0.5.0](https://github.com/getkono/spargen/compare/spargen-macro-v0.4.0...spargen-macro-v0.5.0) - 2026-09-24

### Other

- *(macro)* pin the install line to the released version

## [0.4.0](https://github.com/getkono/spargen/compare/spargen-macro-v0.3.0...spargen-macro-v0.4.0) - 2026-08-29

### Other

- remove backlog references and changelog narration from comments

## [0.3.0](https://github.com/getkono/spargen/compare/spargen-macro-v0.2.2...spargen-macro-v0.3.0) - 2026-08-28

### Added

- [**breaking**] report a diagnostic list the batch cap truncated
- [**breaking**] split Config into Spec and Build, and add `spargen deps`
- [**breaking**] give Report, Outcome and Diagnostic a usable public shape
- [**breaking**] enforce generated runtime dependency contracts
- *(oas)* complete OpenAPI 3.1 and 3.2 conformance

### Fixed

- make the new omit constructs reachable from every surface

### Other

- correct the claims the code contradicts
- [**breaking**] restrict generation to compile-time Rust APIs

## [0.2.1](https://github.com/getkono/spargen/compare/spargen-v0.2.0...spargen-macro-v0.2.1) - 2026-07-20

### Fixed

- *(release)* finalize macro trusted publishing

## [0.2.0](https://github.com/getkono/spargen/compare/spargen-macro-v0.1.0...spargen-macro-v0.2.0) - 2026-07-20

### Added

- *(macro)* spargen-macro proc-macro crate with generate_api!

### Fixed

- *(codegen)* silence inline blocking cfg warnings
