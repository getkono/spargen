# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
