# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/getkono/spargen/compare/spargen-v0.5.0...spargen-v0.6.0) - 2026-10-11

### Added

- *(diag)* [**breaking**] hold each diagnostic's outcome claim to the run's outcome ([#441](https://github.com/getkono/spargen/pull/441))
- *(runtime)* [**breaking**] let a registered credential be unregistered so selection can reach a later alternative ([#287](https://github.com/getkono/spargen/pull/287))
- open const-narrowed response strings, and read problem details across operations ([#396](https://github.com/getkono/spargen/pull/396))
- *(runtime)* [**breaking**] type a credential-kind mismatch as RequestError::CredentialMismatch ([#335](https://github.com/getkono/spargen/pull/335))
- *(oas31)* resolve a same-file $ref into a component subschema through the resolver ([#260](https://github.com/getkono/spargen/pull/260))

### Fixed

- *(oas31)* [**breaking**] keep an exact-null anyOf branch's own unit alias beside a sibling ([#662](https://github.com/getkono/spargen/pull/662))
- *(surface)* list an uninhabited component in the diff surface ([#663](https://github.com/getkono/spargen/pull/663))
- *(oas31)* [**breaking**] keep null for an allOf $ref member to an allOf wrapping a $ref union ([#658](https://github.com/getkono/spargen/pull/658))
- *(surface)* list a cycle-member newtype in the diff surface ([#656](https://github.com/getkono/spargen/pull/656))
- *(oas31)* [**breaking**] drop oneOf branches a sibling meet narrows to null beside a typed branch ([#659](https://github.com/getkono/spargen/pull/659))
- *(ir)* share the alias-cycle predicate with diff and error-body unification ([#655](https://github.com/getkono/spargen/pull/655))
- *(oas31)* [**breaking**] read an anyOf branch a sibling narrows to null as a branch null matches ([#652](https://github.com/getkono/spargen/pull/652))
- *(codegen)* [**breaking**] emit an array or tuple closing an alias cycle as a transparent newtype ([#651](https://github.com/getkono/spargen/pull/651))
- *(oas31)* [**breaking**] reject a oneOf whose sibling meet narrows every branch to null ([#646](https://github.com/getkono/spargen/pull/646))
- *(oas31)* meet a cycle-closing reservation with an untyped schema as itself ([#647](https://github.com/getkono/spargen/pull/647))
- *(oas31)* [**breaking**] keep null for several inline union allOf members beside a nullable member ([#644](https://github.com/getkono/spargen/pull/644))
- *(oas31)* [**breaking**] keep null for a $ref union member nested in an inner allOf ([#639](https://github.com/getkono/spargen/pull/639))
- *(oas31)* [**breaking**] read untyped array members of an untyped allOf as the shape the merge lowers to ([#640](https://github.com/getkono/spargen/pull/640))
- *(oas31)* [**breaking**] count a nested union branch admitting null beside a null branch in a oneOf ([#637](https://github.com/getkono/spargen/pull/637))
- *(oas31)* [**breaking**] count a cycle-closing $ref branch beside a null branch as the same union outside the cycle ([#635](https://github.com/getkono/spargen/pull/635))
- *(oas31)* [**breaking**] lower a nullable scalar type beside an untyped items anyOf as the null type ([#634](https://github.com/getkono/spargen/pull/634))
- *(oas31)* [**breaking**] keep null for an allOf $ref member to a union whose branches leave it undecided ([#629](https://github.com/getkono/spargen/pull/629))
- *(oas31)* [**breaking**] count an untyped object or array oneOf branch as matching null ([#626](https://github.com/getkono/spargen/pull/626))
- *(oas31)* [**breaking**] keep null for an allOf union member whose branches leave it undecided ([#623](https://github.com/getkono/spargen/pull/623))
- *(oas31)* [**breaking**] lower untyped items or prefixItems alone as an array ([#620](https://github.com/getkono/spargen/pull/620))
- *(oas31)* add each bundle allOf target to a composition once ([#619](https://github.com/getkono/spargen/pull/619))
- *(oas31)* [**breaking**] lower untyped required or additionalProperties alone as an object ([#618](https://github.com/getkono/spargen/pull/618))
- *(oas31)* keep a lone-false union uninhabited when an allOf or type sibling meets it ([#617](https://github.com/getkono/spargen/pull/617))
- *(oas31)* [**breaking**] refine an allOf's arrays through a $ref member to untyped items/prefixItems alone ([#612](https://github.com/getkono/spargen/pull/612))
- *(oas31)* [**breaking**] refine an allOf's arrays with a member of untyped items/prefixItems alone ([#611](https://github.com/getkono/spargen/pull/611))
- *(oas31)* [**breaking**] keep null for a oneOf a meet narrows to its stated-nothing branch ([#600](https://github.com/getkono/spargen/pull/600))
- *(oas31)* read a $ref union branch through its target when deciding null for an allOf meet ([#599](https://github.com/getkono/spargen/pull/599))
- *(oas31)* [**breaking**] count a stated-nothing union branch's null once in the $ref and allOf spellings ([#598](https://github.com/getkono/spargen/pull/598))
- *(oas31)* read a $ref union branch through its target when it denies null beside an untyped one ([#596](https://github.com/getkono/spargen/pull/596))
- *(oas31)* [**breaking**] keep a stated-nothing union branch's null undecided beside untyped sibling keywords ([#593](https://github.com/getkono/spargen/pull/593))
- *(oas31)* [**breaking**] agree on a union's null across spellings of a nullable-typed-plus-untyped meet ([#591](https://github.com/getkono/spargen/pull/591))
- *(oas31)* keep a union's null undecided beside an untyped branch in the allOf spellings ([#587](https://github.com/getkono/spargen/pull/587))
- *(oas31)* [**breaking**] decide a union's null from its branches, not its enclosing type array ([#584](https://github.com/getkono/spargen/pull/584))
- *(oas31)* withhold the unreferenced type of the keywords beside a $ref's union sibling ([#583](https://github.com/getkono/spargen/pull/583))
- *(oas31)* keep a nested allOf member's own type, enum or const in the null-only meet check ([#582](https://github.com/getkono/spargen/pull/582))
- *(oas31)* [**breaking**] count a held-back union's untyped object branch as accepting null ([#580](https://github.com/getkono/spargen/pull/580))
- *(oas31)* decide an intersection's kept default once over every member, not per pairwise fold ([#578](https://github.com/getkono/spargen/pull/578))
- *(oas31)* [**breaking**] let a $ref to an untyped allOf component admit null without deciding it ([#576](https://github.com/getkono/spargen/pull/576))
- *(oas31)* [**breaking**] reject null a oneOf's branches accept twice, merged or not ([#575](https://github.com/getkono/spargen/pull/575))
- *(oas31)* [**breaking**] let a $ref's untyped allOf sibling admit null without deciding it ([#573](https://github.com/getkono/spargen/pull/573))
- *(oas31)* [**breaking**] withhold the pre-meet union a met allOf or $ref union does not use ([#572](https://github.com/getkono/spargen/pull/572))
- *(oas31)* report the defaults an emptied $ref-sibling meet drops, as its allOf spellings do ([#570](https://github.com/getkono/spargen/pull/570))
- *(oas31)* lower an irreconcilable meet of nullable objects to the null type in every spelling ([#568](https://github.com/getkono/spargen/pull/568))
- *(oas31)* let a $ref to an untyped object component admit null in an object meet ([#566](https://github.com/getkono/spargen/pull/566))
- *(oas31)* [**breaking**] meet a $ref's union sibling with its other siblings as the allOf spellings do ([#564](https://github.com/getkono/spargen/pull/564))
- *(oas31)* warn when a trial-matched oneOf keeps a serde_json::Value variant ([#560](https://github.com/getkono/spargen/pull/560))
- *(oas31)* [**breaking**] merge oneOf variants of one structure and warn ([#559](https://github.com/getkono/spargen/pull/559))
- *(source)* locate E003 and E021 at the remote $ref that names the URL ([#557](https://github.com/getkono/spargen/pull/557))
- *(oas31)* report a dropped default at every pointer that wrote its value ([#558](https://github.com/getkono/spargen/pull/558))
- *(oas31)* count null across the whole union when merging oneOf groups ([#556](https://github.com/getkono/spargen/pull/556))
- *(oas31)* [**breaking**] discard the meets a re-emitted union collapse or $ref-union refinement does not use ([#552](https://github.com/getkono/spargen/pull/552))
- *(source)* classify a YAML integer literal above i64::MAX as a u64, as the JSON parser does ([#551](https://github.com/getkono/spargen/pull/551))
- *(source)* point E022 at the duplicate key in both parsers ([#549](https://github.com/getkono/spargen/pull/549))
- name E008's real cause and single-space the Cargo-integration messages ([#544](https://github.com/getkono/spargen/pull/544))
- *(oas31)* meet an allOf's oneOf/anyOf member with its other members as a $ref sibling union is ([#537](https://github.com/getkono/spargen/pull/537))
- *(oas31)* locate W005 and W006 on an allOf meet struct at the met property ([#531](https://github.com/getkono/spargen/pull/531))
- *(oas31)* [**breaking**] merge oneOf variants that lower to one generated type and warn ([#493](https://github.com/getkono/spargen/pull/493))
- *(oas31)* report a default an uninhabited meet drops as W005 ([#526](https://github.com/getkono/spargen/pull/526))
- *(name)* allocate server builders and variable enums from one scope ([#525](https://github.com/getkono/spargen/pull/525))
- *(runtime)* escape control characters in HeaderError::Parse's message ([#524](https://github.com/getkono/spargen/pull/524))
- *(oas31)* merge a repeated property's default from either side of an intersection ([#522](https://github.com/getkono/spargen/pull/522))
- *(oas31)* name an uninhabited part of a parameter union member ([#518](https://github.com/getkono/spargen/pull/518))
- *(compat)* read a backslash before any character the same way in exact and glob rules ([#516](https://github.com/getkono/spargen/pull/516))
- *(source)* key the lock walk's local documents by the build's lexical identity ([#512](https://github.com/getkono/spargen/pull/512))
- *(oas31)* type a nullable union a scoped refiner leaves only null as null ([#510](https://github.com/getkono/spargen/pull/510))
- *(oas31)* audit the objects a $ref reaches in other files ([#513](https://github.com/getkono/spargen/pull/513))
- *(surface)* key a parameter by its location as well as its name in spargen diff ([#501](https://github.com/getkono/spargen/pull/501))
- *(runtime-contract)* print the blocking feature wiring in spargen deps ([#503](https://github.com/getkono/spargen/pull/503))
- *(compat)* fingerprint omit rules over a canonical encoding, not derived Hash ([#502](https://github.com/getkono/spargen/pull/502))
- *(oas31)* make an object allOf nullable when every member admits null ([#498](https://github.com/getkono/spargen/pull/498))
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

- *(matrix)* cut the long support-matrix rows to one sentence per construct and reconcile moved paths ([#609](https://github.com/getkono/spargen/pull/609))
- *(oas31)* tidy lower/'s small duplicates and state its doc comments' edge cases ([#608](https://github.com/getkono/spargen/pull/608))
- *(oas31)* deduplicate lower/'s field merge, scoped-meet sequence and E013 reporters ([#606](https://github.com/getkono/spargen/pull/606))
- *(layering)* gate file length at 1666 lines against a baseline that only shrinks ([#605](https://github.com/getkono/spargen/pull/605))
- *(oas31)* deduplicate lower/'s reservation lifecycle and reference walkers ([#603](https://github.com/getkono/spargen/pull/603))
- *(tests)* split frontend.rs into a frontend/ test crate by family ([#604](https://github.com/getkono/spargen/pull/604))
- *(oas31)* split lower.rs into a lower/ module along its responsibilities ([#602](https://github.com/getkono/spargen/pull/602))
- *(oas31)* [**breaking**] pin a $ref-to-true branch beside an untyped one under items or required as Pick ([#601](https://github.com/getkono/spargen/pull/601))
- *(oas31)* pin the plain, inline and allOf-member oneOf of untyped branches beside a nullable type ([#585](https://github.com/getkono/spargen/pull/585))
- add a weekly cargo-mutants gate over the runtime, ir/media.rs and name/ ([#555](https://github.com/getkono/spargen/pull/555))
- bring the explain texts, support matrix and module docs into line with the code ([#553](https://github.com/getkono/spargen/pull/553))
- cut duplicated and narrative prose from AGENTS.md, README and the book ([#548](https://github.com/getkono/spargen/pull/548))
- split compat into rule, glob and carve, and dedupe the facade's carve dispatch ([#550](https://github.com/getkono/spargen/pull/550))
- state the intersection laws as lowering properties ([#546](https://github.com/getkono/spargen/pull/546))
- *(runtime)* collapse the three decode and three classify copies into two private helpers ([#547](https://github.com/getkono/spargen/pull/547))
- *(codegen)* split emit.rs into per-construct modules and share its token builders ([#539](https://github.com/getkono/spargen/pull/539))
- hold diagnostics to real locations and unions to distinguishable variants ([#536](https://github.com/getkono/spargen/pull/536))
- pin the exact diff, carve, macro and e2e outcomes their fixtures name ([#530](https://github.com/getkono/spargen/pull/530))
- split runtime_contract.rs into a directory module with out-of-line tests ([#532](https://github.com/getkono/spargen/pull/532))
- *(runtime)* move the dispatch, error and stream test modules out of line ([#529](https://github.com/getkono/spargen/pull/529))
- correct contradicting doc comments, cover the binary in doc-links, and bench real generation ([#527](https://github.com/getkono/spargen/pull/527))
- property-test identifier injectivity, diff labels, and single-line runtime errors ([#520](https://github.com/getkono/spargen/pull/520))
- *(emit)* drop the diag edge emit declares but never takes ([#519](https://github.com/getkono/spargen/pull/519))
- *(diag)* track paren depth so asserts_variant refuses a negated nested has_code call ([#515](https://github.com/getkono/spargen/pull/515))
- *(e2e)* pin the complete generated module for BASIC_SPEC as golden files ([#517](https://github.com/getkono/spargen/pull/517))
- give the fuzz and determinism suites real oracles, and pin Spec exhaustively ([#511](https://github.com/getkono/spargen/pull/511))
- stop the layering, corpus-manifest and re-export meta-tests passing on empty input ([#504](https://github.com/getkono/spargen/pull/504))
- make the docs' claims true: link gate, samples, install pins, stale counts ([#506](https://github.com/getkono/spargen/pull/506))
- *(ir)* property-test the Responses success/error partition ([#508](https://github.com/getkono/spargen/pull/508))
- make the frontend, recipe, lowering and diag oracles able to fail ([#505](https://github.com/getkono/spargen/pull/505))
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
