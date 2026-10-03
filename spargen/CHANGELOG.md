# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/getkono/spargen/compare/spargen-v0.5.0...spargen-v0.6.0) - 2026-10-03

### Added

- *(diag)* [**breaking**] hold each diagnostic's outcome claim to the run's outcome ([#441](https://github.com/getkono/spargen/pull/441))
- *(runtime)* [**breaking**] let a registered credential be unregistered so selection can reach a later alternative ([#287](https://github.com/getkono/spargen/pull/287))
- open const-narrowed response strings, and read problem details across operations ([#396](https://github.com/getkono/spargen/pull/396))
- *(runtime)* [**breaking**] type a credential-kind mismatch as RequestError::CredentialMismatch ([#335](https://github.com/getkono/spargen/pull/335))
- *(oas31)* resolve a same-file $ref into a component subschema through the resolver ([#260](https://github.com/getkono/spargen/pull/260))

### Fixed

- *(surface)* label a one-position tuple (T,) in spargen diff ([#497](https://github.com/getkono/spargen/pull/497))
- *(name)* disambiguate parameters whose names escape to the same identifier ([#496](https://github.com/getkono/spargen/pull/496))
- *(oas31)* audit the schemas a $ref reaches in other files ([#494](https://github.com/getkono/spargen/pull/494))
- *(oas31)* intersect a $ref's oneOf/anyOf sibling with its target instead of dropping it ([#280](https://github.com/getkono/spargen/pull/280))
- *(oas31)* [**breaking**] elide the meets an object allOf's merged struct does not use ([#448](https://github.com/getkono/spargen/pull/448))
- *(oas31)* resolve references under subschemas lowering never reads ([#447](https://github.com/getkono/spargen/pull/447))
- *(oas31)* [**breaking**] intersect a oneOf/anyOf beside allOf on one schema instead of dropping it ([#444](https://github.com/getkono/spargen/pull/444))
- *(source)* resolve each document's relative refs from its $self in spargen lock ([#445](https://github.com/getkono/spargen/pull/445))
- *(codegen)* [**breaking**] emit a one-position tuple as (T,), not a parenthesized T ([#443](https://github.com/getkono/spargen/pull/443))
- *(oas31)* name an uninhabited parameter schema in E010 instead of reporting nesting ([#434](https://github.com/getkono/spargen/pull/434))
- *(oas31)* re-type a field default against the type an intersection narrows it to ([#431](https://github.com/getkono/spargen/pull/431))
- *(oas31)* [**breaking**] stop inventing a discriminator tag for a member no mapping entry names ([#430](https://github.com/getkono/spargen/pull/430))
- *(oas31)* select a form field's codec from its contentType case-insensitively ([#427](https://github.com/getkono/spargen/pull/427))
- *(oas31)* [**breaking**] discard the meets a re-emitted scalar allOf or $ref-sibling result does not use ([#429](https://github.com/getkono/spargen/pull/429))
- *(source)* resolve a redirected document's relative $refs against its retrieval URL ([#422](https://github.com/getkono/spargen/pull/422))
- *(oas31)* warn W011 on a header $ref with sibling summary/description ([#423](https://github.com/getkono/spargen/pull/423))
- *(oas31)* [**breaking**] resolve a sub-file's #/components/ object refs against that file ([#421](https://github.com/getkono/spargen/pull/421))
- *(oas31)* report a discriminator beside no oneOf/anyOf as W011 and resolve its mapping ([#420](https://github.com/getkono/spargen/pull/420))
- *(oas31)* [**breaking**] dispatch a discriminated member on every value that names it ([#418](https://github.com/getkono/spargen/pull/418))
- *(oas31)* keep a set narrowed against a uuid or date string closed under open_narrowing ([#417](https://github.com/getkono/spargen/pull/417))
- *(oas31)* [**breaking**] scope untyped union-sibling applicators to the branches of their category ([#416](https://github.com/getkono/spargen/pull/416))
- *(oas31)* [**breaking**] validate discriminator mapping targets instead of dropping them ([#262](https://github.com/getkono/spargen/pull/262))
- *(oas31)* carry bare refining $ref siblings and undeclared required names ([#283](https://github.com/getkono/spargen/pull/283))
- *(codegen)* percent-encode content-typed path parameters before splicing them ([#327](https://github.com/getkono/spargen/pull/327))
- *(runtime)* [**breaking**] narrow EventStream constructors to crate visibility ([#394](https://github.com/getkono/spargen/pull/394))
- *(oas31)* skip specification extensions of the Paths Object ([#393](https://github.com/getkono/spargen/pull/393))
- *(oas31)* reject a non-JSON contentType on a form-urlencoded non-scalar field ([#390](https://github.com/getkono/spargen/pull/390))
- *(name)* [**breaking**] reserve the names the types module uses before allocating model names ([#389](https://github.com/getkono/spargen/pull/389))
- *(oas31)* expand a file-referenced allOf member's allOf or alias target ([#387](https://github.com/getkono/spargen/pull/387))
- *(runtime-contract)* ignore a member's package key beside workspace = true, as Cargo does ([#388](https://github.com/getkono/spargen/pull/388))
- *(runtime-contract)* name the audited manifest in every E023 diagnostic ([#386](https://github.com/getkono/spargen/pull/386))
- *(codegen)* [**breaking**] reject a present null on an optional non-nullable field ([#385](https://github.com/getkono/spargen/pull/385))
- *(oas31)* intersect a component $ref union member with its own sibling keywords ([#382](https://github.com/getkono/spargen/pull/382))
- *(name)* [**breaking**] reserve the fixed Client and BlockingClient method names before allocating operation methods ([#381](https://github.com/getkono/spargen/pull/381))
- *(oas31)* follow a Parameter, Request Body, Response or Header $ref chain through the bundle ([#380](https://github.com/getkono/spargen/pull/380))
- *(oas31)* reject an inhabited intersection with no Rust type instead of typing it uninhabited ([#378](https://github.com/getkono/spargen/pull/378))
- *(runtime)* [**breaking**] keep the response headers on Error::Decode ([#377](https://github.com/getkono/spargen/pull/377))
- *(codegen)* [**breaking**] re-export the date parse error at the generated root as DateParseError ([#375](https://github.com/getkono/spargen/pull/375))
- *(oas31)* reject a malformed parameter on an encoding.contentType ([#373](https://github.com/getkono/spargen/pull/373))
- *(source)* stop the $ref walk at specification extensions ([#371](https://github.com/getkono/spargen/pull/371))
- *(oas31)* give a back-edge its target's lowered nullability, not the reserve-time guess ([#361](https://github.com/getkono/spargen/pull/361))
- *(source)* [**breaking**] identify a local $ref target by its normalised path, not its spelling ([#360](https://github.com/getkono/spargen/pull/360))
- *(runtime_contract)* bound the workspace-root walk so fixtures stop reading /tmp and / ([#355](https://github.com/getkono/spargen/pull/355))
- *(ir)* [**breaking**] give a documented bodyless error status beside one error body its own variant ([#348](https://github.com/getkono/spargen/pull/348))
- *(macro)* fail loudly instead of auditing ./Cargo.toml when Cargo names no crate ([#338](https://github.com/getkono/spargen/pull/338))
- *(source)* report a failed spargen lock fetch as E025, not E003 "not pinned" ([#331](https://github.com/getkono/spargen/pull/331))
- *(oas31)* intersect an allOf member's $ref siblings instead of dropping them ([#329](https://github.com/getkono/spargen/pull/329))
- *(deps)* floor rustls at 0.23.45 in spargen's manifest and deny.toml ([#325](https://github.com/getkono/spargen/pull/325))
- *(oas31)* reject a non-JSON contentType on a multipart part only JSON can render ([#322](https://github.com/getkono/spargen/pull/322))
- *(oas31)* say W014's selection is selected, not generated ([#319](https://github.com/getkono/spargen/pull/319))
- state every TypeKind match's answer for a reservation, and test that it does ([#316](https://github.com/getkono/spargen/pull/316))
- *(name)* [**breaking**] award a contested type name on the schema's own file and pointer ([#313](https://github.com/getkono/spargen/pull/313))
- *(runtime-contract)* report a missing workspace root as not found, naming a broken ancestor only as a hint (E023) ([#314](https://github.com/getkono/spargen/pull/314))
- *(runtime-contract)* accept an identity `package` key and reject only an actual rename (E023) ([#311](https://github.com/getkono/spargen/pull/311))
- *(oas31)* [**breaking**] keep resolved union components nullable, and pin three unguarded resolved-reference units ([#308](https://github.com/getkono/spargen/pull/308))
- *(oas31)* [**breaking**] expand a file-referenced allOf member once per target ([#307](https://github.com/getkono/spargen/pull/307))
- *(compat)* carve a sub-file rejection in the file it is reported in ([#305](https://github.com/getkono/spargen/pull/305))
- *(codegen)* reject a present null on an optional uninhabited field ([#304](https://github.com/getkono/spargen/pull/304))
- *(oas31)* type an optional conflicting allOf property uninhabited instead of rejecting ([#297](https://github.com/getkono/spargen/pull/297))
- *(oas31)* lower a component-root $ref with only shapeless siblings as an alias ([#291](https://github.com/getkono/spargen/pull/291))
- *(oas31)* validate every $ref target against the metaschema, not only the root document ([#275](https://github.com/getkono/spargen/pull/275))
- *(oas31)* document, pin, and say why chained Path Item and security-scheme `$ref`s reject ([#277](https://github.com/getkono/spargen/pull/277))
- *(oas31)* intersect binary content with a plain string, and a tuple with an array ([#271](https://github.com/getkono/spargen/pull/271))
- *(runtime)* classify an elapsed connect timeout as TimeoutKind::Connect ([#261](https://github.com/getkono/spargen/pull/261))
- *(ir)* [**breaking**] give a documented bodyless success beside a single body its own variant ([#259](https://github.com/getkono/spargen/pull/259))
- *(oas31)* [**breaking**] reject a bodied streaming response outside the single success body ([#258](https://github.com/getkono/spargen/pull/258))
- *(ir)* [**breaking**] let default type the success side when no success status is declared ([#254](https://github.com/getkono/spargen/pull/254))
- *(oas31)* emit W014 only once the selected body entry passes its gates ([#250](https://github.com/getkono/spargen/pull/250))
- *(oas31)* reject an encoding.contentType that is not a media type ([#249](https://github.com/getkono/spargen/pull/249))
- *(oas31)* [**breaking**] reject a raw body that admits null, and keep null on a type-array byte string ([#247](https://github.com/getkono/spargen/pull/247))
- *(runtime)* [**breaking**] keep the response status on Error::Decode ([#245](https://github.com/getkono/spargen/pull/245))

### Other

- *(deny)* refresh the index cache before each cargo audit and fail one that could not check a yank ([#442](https://github.com/getkono/spargen/pull/442))
- *(e2e)* hold is_tls_crate to the example gate's TLS-crate regex ([#439](https://github.com/getkono/spargen/pull/439))
- *(runtime)* describe error.rs test-module behavior without branch-relative prose ([#438](https://github.com/getkono/spargen/pull/438))
- fail the diff-fixture and corpus-hash tests when their input file is missing ([#437](https://github.com/getkono/spargen/pull/437))
- *(cache)* pin the line-break guard on build-script rerun directives ([#436](https://github.com/getkono/spargen/pull/436))
- *(layering)* hold every binary-spawning integration test to a cli feature gate ([#395](https://github.com/getkono/spargen/pull/395))
- gate the binary-spawning carve and config tests on the cli feature ([#392](https://github.com/getkono/spargen/pull/392))
- *(source)* complete a TLS handshake through spargen lock's fetch path ([#383](https://github.com/getkono/spargen/pull/383))
- *(diag)* hold W011, E016, E007 and E013 emission sites to the cases their explain text lists ([#379](https://github.com/getkono/spargen/pull/379))
- *(corpus)* hold every audit flag of the deny gate to an allow-list, and deny.toml to the checks' tables ([#369](https://github.com/getkono/spargen/pull/369))
- *(ir)* give the default response its own StatusSpec::Default variant ([#367](https://github.com/getkono/spargen/pull/367))
- give corpus_manifest.rs's gate-configuration tests a testing-strategy row, and hold both rows to its tests ([#366](https://github.com/getkono/spargen/pull/366))
- *(e2e)* assert the audited --all-features graph carries a TLS stack ([#365](https://github.com/getkono/spargen/pull/365))
- *(spargen)* gate the cli test target on the cli feature ([#364](https://github.com/getkono/spargen/pull/364))
- *(frontend)* assert the server-variable enum's variant set, not only its name ([#362](https://github.com/getkono/spargen/pull/362))
- *(runtime_contract)* pin the E023 read-failure reason, rebuild input, and package.workspace path ([#359](https://github.com/getkono/spargen/pull/359))
- *(diag)* hold the E023 matrix row's inheritance clauses to verbatim excerpts of its explain text ([#358](https://github.com/getkono/spargen/pull/358))
- *(corpus)* pin Mastodon's 3.1 description as a generating case with recursive-nullable $refs ([#357](https://github.com/getkono/spargen/pull/357))
- *(ir)* key ErrorShape::None and ::Single on the lowered error entries ([#354](https://github.com/getkono/spargen/pull/354))
- pin plural bodyless enum entries and status-label sensitivity of spargen diff ([#353](https://github.com/getkono/spargen/pull/353))
- *(e2e)* hold the success and error shapes to one body-count rule ([#352](https://github.com/getkono/spargen/pull/352))
- *(runtime_contract)* require every clause of the E023 explain body to be classified ([#351](https://github.com/getkono/spargen/pull/351))
- *(e2e)* pin a failed token provider's rendered message from generated output ([#350](https://github.com/getkono/spargen/pull/350))
- *(layering)* hold every file spargen's sources include in the published crate ([#347](https://github.com/getkono/spargen/pull/347))
- *(runtime-contract)* pin the workspace-root resolver's inputs and boundaries that survived mutation ([#346](https://github.com/getkono/spargen/pull/346))
- *(runtime)* state which security schemes accept a Credential::Provider ([#344](https://github.com/getkono/spargen/pull/344))
- *(ir)* pin every non-2XX status, 304 included, to the error side of the response shape ([#342](https://github.com/getkono/spargen/pull/342))
- *(ir)* pin Responses::success's default claims with a doctest through generate ([#341](https://github.com/getkono/spargen/pull/341))
- *(changelog)* list #87 under Other in 0.5.0, not Fixed ([#340](https://github.com/getkono/spargen/pull/340))
- *(runtime)* hold every error variant the docs cite to the taxonomy, and name each in its layout ([#337](https://github.com/getkono/spargen/pull/337))
- preview the next release's CHANGELOG with release-plz in a paired gate ([#334](https://github.com/getkono/spargen/pull/334))
- *(source)* pin the TLS strictness spargen lock enforces, and state its TLS position ([#333](https://github.com/getkono/spargen/pull/333))
- *(deny)* audit every entry of every committed lockfile, not only the activated graph ([#330](https://github.com/getkono/spargen/pull/330))
- *(deny)* audit every example workspace's committed lockfile under the one deny.toml ([#328](https://github.com/getkono/spargen/pull/328))
- *(oas31)* pin the two surviving TypeKind::Reserved arms with fixtures that reach them ([#326](https://github.com/getkono/spargen/pull/326))
- *(oas31)* hold every reserve-then-lower site to the body's nullability, cached or not ([#324](https://github.com/getkono/spargen/pull/324))
- *(deny)* audit the Cargo.lock the latest spargen release ships ([#323](https://github.com/getkono/spargen/pull/323))
- *(e2e)* hold the E023 workspace-inheritance audit to Cargo's own resolution ([#318](https://github.com/getkono/spargen/pull/318))
- *(diag)* state E023's capability clause as a rule instead of an incomplete list ([#315](https://github.com/getkono/spargen/pull/315))
- *(diag)* record the fixtures that enforce E023's consumer-obligation clauses ([#312](https://github.com/getkono/spargen/pull/312))
- *(diag)* fail when a checked document is absent, and state that support-matrix prose is human-reviewed ([#310](https://github.com/getkono/spargen/pull/310))
- *(e2e)* build every nested fixture crate in its own target directory ([#309](https://github.com/getkono/spargen/pull/309))
- *(e2e)* hold `spargen deps` advice to the E023 audit, directly and through workspace inheritance ([#301](https://github.com/getkono/spargen/pull/301))
- *(oas31)* hold E013's published sibling keywords equal to the gate that decides ([#298](https://github.com/getkono/spargen/pull/298))
- *(runtime-contract)* constrain or fold three fixtures that only executed, and read core floors once ([#296](https://github.com/getkono/spargen/pull/296))
- *(ir)* decide that default satisfies no undeclared 2xx beside a declared success ([#295](https://github.com/getkono/spargen/pull/295))
- *(runtime-contract)* derive the blocking-wiring fixture's reqwest entry from CORE_MANIFEST ([#294](https://github.com/getkono/spargen/pull/294))
- *(source)* drive spargen lock's real fetcher over a local HTTP socket ([#293](https://github.com/getkono/spargen/pull/293))
- pin the sub-file carve limit and the E004 webhook clause ([#290](https://github.com/getkono/spargen/pull/290))
- *(layering)* fail when an error variant is added without raising its enumeration count ([#289](https://github.com/getkono/spargen/pull/289))
- *(layering)* bar embedded runtime comments from naming test-only items undisclosed ([#288](https://github.com/getkono/spargen/pull/288))
- *(diag)* hold every E004 emission site to a case its explain text lists ([#285](https://github.com/getkono/spargen/pull/285))
- *(oas31)* assert the emitted method when JSON and a stream share a response ([#281](https://github.com/getkono/spargen/pull/281))
- *(ir)* pin precedence_key's ascending order within the exact and range classes ([#278](https://github.com/getkono/spargen/pull/278))
- *(oas31)* pin the multi-status streaming rejection for every framing and version ([#276](https://github.com/getkono/spargen/pull/276))
- *(ir)* key Responses::error() and precedence_key docs on lowered entries and selectors ([#273](https://github.com/getkono/spargen/pull/273))
- audit the committed lockfile, daily on master, and hold example lockfiles TLS-free ([#272](https://github.com/getkono/spargen/pull/272))
- *(e2e)* drive the emitted error dispatch through exact, range, undocumented and decode arms ([#267](https://github.com/getkono/spargen/pull/267))
- cover the bodyless-default error-shape grid and the single-body StatusSpec::Any guard ([#266](https://github.com/getkono/spargen/pull/266))
- *(codegen)* hold the generated re-export lists to each other and pin the root surface ([#255](https://github.com/getkono/spargen/pull/255))
- *(ir)* cover how default routes beside explicit statuses in the response shapes ([#253](https://github.com/getkono/spargen/pull/253))
- scope the "concrete sibling outranks a range" claim to family ranges ([#251](https://github.com/getkono/spargen/pull/251))
- scope the "default last" dispatch precedence to the error enum ([#252](https://github.com/getkono/spargen/pull/252))

## [0.5.0](https://github.com/getkono/spargen/compare/spargen-v0.4.0...spargen-v0.5.0) - 2026-09-24

### Added

- *(runtime)* [**breaking**] type the missing-credential request-construction error ([#94](https://github.com/getkono/spargen/pull/94))

### Fixed

- *(oas31)* reject Responses keys outside the specification's grammar ([#235](https://github.com/getkono/spargen/pull/235))
- *(oas31)* [**breaking**] report an empty $ref/sibling intersection instead of dropping it ([#125](https://github.com/getkono/spargen/pull/125))
- *(oas31)* [**breaking**] reject a $ref to an undeclared component schema with E004 ([#112](https://github.com/getkono/spargen/pull/112))
- *(runtime-contract)* say why an unreadable workspace root could not be read
- *(oas31)* report a narrowed request body even when both sides are bytes
- *(runtime-contract)* keep an unreadable ancestor out of the resolved root
- *(oas31)* confine the identical-decode rule to octet-stream again
- *(runtime-contract)* keep climbing past a manifest that does not parse
- *(oas31)* prove an alternative decodes identically before dropping W014
- *(runtime-contract)* name the workspace root a reader can open
- *(oas31)* accept the 3.1 spelling of a binary body and media ranges
- *(runtime-contract)* resolve workspace-inherited deps in more layouts
- *(surface)* serialize diff codes and impacts as their documented strings
- *(name)* escape `gen`, reserved since edition 2024
- *(codegen)* stop panicking on a keyword header or server variable

### Other

- pin the Rust toolchain and every tool to the versions mise pins, and hold CI to them ([#243](https://github.com/getkono/spargen/pull/243))
- hold every mise task identical to its CI job, and run MSRV on the declared toolchain ([#242](https://github.com/getkono/spargen/pull/242))
- *(corpus)* stop calling command-arguments the only input that narrows the deny audit ([#241](https://github.com/getkono/spargen/pull/241))
- *(cli)* execute spargen explain, including its --format json branch ([#225](https://github.com/getkono/spargen/pull/225))
- state cargo-deny's --all-features scope on the step ([#224](https://github.com/getkono/spargen/pull/224))
- *(ir)* state where a default response lands in the success shape ([#111](https://github.com/getkono/spargen/pull/111))
- *(runtime-contract)* pin workspace inheritance, and correct the E023 explain text and support-matrix row ([#87](https://github.com/getkono/spargen/pull/87))
- *(diag)* scope the request range wording to family ranges
- *(oas31)* pin a concrete binary key as the sendable sibling that withholds a suffix range
- Merge remote-tracking branch 'origin/master' into fix/82-concrete-binary-media-types
- *(runtime)* point the api_body docs at Error::status and pin both accessors together
- Merge remote-tracking branch 'origin/master' into feat/85-error-body-accessor
- Merge pull request #102 from getkono/fix/88-e023-cfg-aware-target-table
- Merge pull request #103 from getkono/fix/95-media-type-restricted-names
- Merge pull request #97 from getkono/refactor/92-octet-request-single-send-path
- *(ir)* stop listing unreported request-body lowering failures
- *(ir)* stop claiming every request-body lowering failure was reported
- *(ir)* name both cases where a request body's type is None
- *(ir)* state that the octet-stream check covers the definition's kind only
- *(ir)* pin that a dangling octet-stream request body type is reported once
- *(codegen)* send an octet-stream request body through the raw-bytes path
- *(ir)* check that an octet-stream request body lowered to bytes
- *(runtime-contract)* capture the walk's read failure from one read
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
