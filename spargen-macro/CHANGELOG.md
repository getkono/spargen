# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0](https://github.com/getkono/spargen/compare/spargen-macro-v0.7.0...spargen-macro-v0.8.0) - 2026-11-14

### Added

- *(macro)* accept no_<switch> for every generate_api! switch and document argument rules ([#1372](https://github.com/getkono/spargen/pull/1372))
- [**breaking**] tighten the Spec/Build facade: appending omit, enum switches, #[must_use], typed ConfigError ([#1356](https://github.com/getkono/spargen/pull/1356))
- *(codegen)* a typed testing::MockBackend behind Spec::testing ([#1293](https://github.com/getkono/spargen/pull/1293))
- [**breaking**] choose the date-time crate, time, jiff, chrono or string (Spec::datetime) ([#1286](https://github.com/getkono/spargen/pull/1286))
- [**breaking**] bind a header parameter once on the client (Spec::bind_header) ([#1282](https://github.com/getkono/spargen/pull/1282))
- *(codegen)* sanitize carried docs and allow unsatisfiable lints per item ([#1280](https://github.com/getkono/spargen/pull/1280))
- *(compat)* keep (allow-list) operation rules ([#1274](https://github.com/getkono/spargen/pull/1274))

### Fixed

- *(diag)* keep the rejecting error when warnings fill batch_cap ([#1367](https://github.com/getkono/spargen/pull/1367))

### Other

- *(api)* [**breaking**] give DateTimeBackend::from_str a typed UnknownDateTimeBackend error ([#1465](https://github.com/getkono/spargen/pull/1465))
- cut AGENTS.md to cross-cutting rules and scope the rest in .claude/rules ([#1396](https://github.com/getkono/spargen/pull/1396))
- cover the audit's remaining untested branches across support-runtime, spargen and spargen-macro ([#1378](https://github.com/getkono/spargen/pull/1378))

## [0.7.0](https://github.com/getkono/spargen/compare/spargen-macro-v0.6.0...spargen-macro-v0.7.0) - 2026-11-07

### Added

- *(codegen)* open a discriminated union to tags it does not list under open_unions ([#1246](https://github.com/getkono/spargen/pull/1246))

### Other

- bump the install lines to the released 0.6 minor ([#1240](https://github.com/getkono/spargen/pull/1240))

## [0.6.0](https://github.com/getkono/spargen/compare/spargen-macro-v0.5.0...spargen-macro-v0.6.0) - 2026-11-06

### Added

- [**breaking**] let CLI and macro switches override spargen.toml in either direction ([#1079](https://github.com/getkono/spargen/pull/1079))
- [**breaking**] return the warnings of successful requirements and diff runs ([#1078](https://github.com/getkono/spargen/pull/1078))
- open const-narrowed response strings, and read problem details across operations ([#396](https://github.com/getkono/spargen/pull/396))

### Fixed

- *(macro)* [**breaking**] reject a generate_api! cap given more than once ([#1199](https://github.com/getkono/spargen/pull/1199))
- *(macro)* render generate_api! failures with the shared diagnostic display ([#894](https://github.com/getkono/spargen/pull/894))
- rerun build.rs and re-expand generate_api! when an applied spargen.toml changes ([#836](https://github.com/getkono/spargen/pull/836))
- *(compat)* name every accepted value in an unknown omit token's error ([#819](https://github.com/getkono/spargen/pull/819))
- *(macro)* fail loudly instead of auditing ./Cargo.toml when Cargo names no crate ([#338](https://github.com/getkono/spargen/pull/338))

### Other

- fix the release and deny guards, run the bench asserts, and trim tooling comments ([#1217](https://github.com/getkono/spargen/pull/1217))
- *(macro)* derive switch names from their keys and move tests out of line ([#1220](https://github.com/getkono/spargen/pull/1220))
- bring the README, book and docs pages into line with the code ([#1144](https://github.com/getkono/spargen/pull/1144))
- *(macro)* give each generate_api! switch a distinct value and cover the no-error failure ([#1138](https://github.com/getkono/spargen/pull/1138))
- correct the facade, diag, config and CLI docs and trim explain texts ([#1135](https://github.com/getkono/spargen/pull/1135))
- move misplaced tests into the suites CLAUDE.md assigns them to ([#1060](https://github.com/getkono/spargen/pull/1060))
- *(spargen-macro)* pin the spargen dependency exactly ([#1053](https://github.com/getkono/spargen/pull/1053))
- bring the README, book and macro README into line with the code ([#1046](https://github.com/getkono/spargen/pull/1046))
- *(macro)* expect the file-qualified span in rendered failures ([#911](https://github.com/getkono/spargen/pull/911))
- *(macro)* parse the generated module and its tracking items once ([#890](https://github.com/getkono/spargen/pull/890))
- state the feature and gate setup the manifests, gate comments and examples describe as it is ([#880](https://github.com/getkono/spargen/pull/880))
- *(macro)* pin generate_api!'s failure text, config discovery, rebuild tracking and argument errors ([#803](https://github.com/getkono/spargen/pull/803))
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
