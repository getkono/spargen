# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.0](https://github.com/getkono/spargen/compare/spargen-v0.7.0...spargen-v0.8.0) - 2026-11-14

### Added

- *(runtime)* [**breaking**] drop From<Option<T>> for Nullable<T> ([#1456](https://github.com/getkono/spargen/pull/1456))
- *(codegen)* [**breaking**] let a caller set the filename a binary multipart part sends ([#1394](https://github.com/getkono/spargen/pull/1394))
- *(codegen)* [**breaking**] move required query, header and cookie parameters into `…Params::new` ([#1392](https://github.com/getkono/spargen/pull/1392))
- *(codegen)* [**breaking**] typed per-scheme credential setters replace with_credential ([#1390](https://github.com/getkono/spargen/pull/1390))
- *(runtime)* [**breaking**] name ParameterError, add ResponseValue::into_parts and full_jitter, drop BlockingRuntime safely, refuse non-canonical base64 ([#1369](https://github.com/getkono/spargen/pull/1369))
- [**breaking**] a Nullable<T> tri-state for optional + nullable fields and content parameters ([#1355](https://github.com/getkono/spargen/pull/1355))
- [**breaking**] tighten the Spec/Build facade: appending omit, enum switches, #[must_use], typed ConfigError ([#1356](https://github.com/getkono/spargen/pull/1356))
- *(codegen)* [**breaking**] stream the raw binary arms of a success enum through a _stream twin ([#1317](https://github.com/getkono/spargen/pull/1317))
- *(codegen)* a request builder and decoder for each binary _stream twin ([#1297](https://github.com/getkono/spargen/pull/1297))
- *(codegen)* a typed testing::MockBackend behind Spec::testing ([#1293](https://github.com/getkono/spargen/pull/1293))
- *(runtime)* [**breaking**] build requests as http::Request in the runtime ([#1292](https://github.com/getkono/spargen/pull/1292))
- *(codegen)* split each operation into a request builder and a decoder, with ops constants ([#1283](https://github.com/getkono/spargen/pull/1283))
- [**breaking**] choose the date-time crate, time, jiff, chrono or string (Spec::datetime) ([#1286](https://github.com/getkono/spargen/pull/1286))
- [**breaking**] stream binary request and response bodies, bounding the buffered read ([#1285](https://github.com/getkono/spargen/pull/1285))
- *(codegen)* [**breaking**] give closed string enums as_str, ALL, FromStr, Copy and Hash ([#1284](https://github.com/getkono/spargen/pull/1284))
- [**breaking**] bind a header parameter once on the client (Spec::bind_header) ([#1282](https://github.com/getkono/spargen/pull/1282))
- *(runtime)* [**breaking**] per-call RequestOptions with a body limit, extensions and headers ([#1281](https://github.com/getkono/spargen/pull/1281))
- *(codegen)* sanitize carried docs and allow unsatisfiable lints per item ([#1280](https://github.com/getkono/spargen/pull/1280))
- *(runtime)* [**breaking**] replay a call once after a refreshable credential is rejected ([#1272](https://github.com/getkono/spargen/pull/1272))
- *(codegen)* [**breaking**] give object structs new, with_* setters and PartialEq ([#1275](https://github.com/getkono/spargen/pull/1275))
- *(compat)* keep (allow-list) operation rules ([#1274](https://github.com/getkono/spargen/pull/1274))
- *(name)* [**breaking**] name an inline request body after its operation ([#1277](https://github.com/getkono/spargen/pull/1277))
- *(codegen)* [**breaking**] status() and owning body accessors on status enums ([#1276](https://github.com/getkono/spargen/pull/1276))
- *(codegen)* [**breaking**] emit Client::with_client_and_backend and with_base_url ([#1278](https://github.com/getkono/spargen/pull/1278))

### Fixed

- *(name)* [**breaking**] rank response-header fields, and report their rebinding in diff as major ([#1470](https://github.com/getkono/spargen/pull/1470))
- *(runtime)* keep LinkPaginator armed when a next_page future is dropped ([#1463](https://github.com/getkono/spargen/pull/1463))
- *(cli)* exit cleanly on a closed stdout instead of panicking ([#1476](https://github.com/getkono/spargen/pull/1476))
- *(frontend)* [**breaking**] reject an optional discriminator property without defaultMapping ([#1466](https://github.com/getkono/spargen/pull/1466))
- *(ir)* check the defaultMapping index, enum value kinds, querystring count and status selectors ([#1473](https://github.com/getkono/spargen/pull/1473))
- *(frontend)* skip x- keys as Path Item $ref siblings ([#1472](https://github.com/getkono/spargen/pull/1472))
- *(output)* write through a symlinked output path instead of replacing the link ([#1469](https://github.com/getkono/spargen/pull/1469))
- *(source)* accept a leading UTF-8 byte-order mark in a spec, $ref target or vendored copy ([#1467](https://github.com/getkono/spargen/pull/1467))
- *(runtime)* MultipartPart::headers refuses Content-Type ([#1461](https://github.com/getkono/spargen/pull/1461))
- *(compat)* apply a repeated omit rule once, satisfy a covered rule, and compare config paths by identity ([#1457](https://github.com/getkono/spargen/pull/1457))
- *(cli)* add --testing/--no-testing and hold every switch to all four surfaces ([#1458](https://github.com/getkono/spargen/pull/1458))
- *(runtime)* [**breaking**] stop at a cross-origin redirect instead of forwarding apiKey and bound headers ([#1460](https://github.com/getkono/spargen/pull/1460))
- *(frontend)* [**breaking**] reject enum/const members the type excludes, and intersect enum with const ([#1462](https://github.com/getkono/spargen/pull/1462))
- *(frontend)* [**breaking**] keep discriminator tag dispatch for recursive and union members ([#1455](https://github.com/getkono/spargen/pull/1455))
- *(name)* [**breaking**] rank field and variant identifiers, and report a rebinding in diff as major ([#1454](https://github.com/getkono/spargen/pull/1454))
- *(codegen)* [**breaking**] read HEAD and 204/205/304 responses as bodyless whatever content they declare ([#1393](https://github.com/getkono/spargen/pull/1393))
- *(frontend)* warn and drop an Encoding Object header pinning Content-Disposition ([#1391](https://github.com/getkono/spargen/pull/1391))
- *(pipeline)* keep build-environment warnings on write failures, warn on a failed cache write, and cap the carve rejection ([#1388](https://github.com/getkono/spargen/pull/1388))
- *(runtime)* [**breaking**] refuse invalid base URLs and bound header values, and make transport timeouts construction-only ([#1389](https://github.com/getkono/spargen/pull/1389))
- *(codegen)* [**breaking**] emit integer and boolean enum/const sets as checked newtypes ([#1387](https://github.com/getkono/spargen/pull/1387))
- *(codegen)* [**breaking**] relax `required` on readOnly/writeOnly properties to their own direction ([#1383](https://github.com/getkono/spargen/pull/1383))
- *(runtime)* [**breaking**] give a binary multipart part a filename, and refuse a part header setting Content-Disposition ([#1386](https://github.com/getkono/spargen/pull/1386))
- *(codegen)* refuse a defaultMapping payload an untagged variant would claim ([#1382](https://github.com/getkono/spargen/pull/1382))
- *(runtime)* keep PaginationCycle's URL without userinfo and query ([#1381](https://github.com/getkono/spargen/pull/1381))
- *(compat)* match a file-scoped omit rule's file by whole path components ([#1380](https://github.com/getkono/spargen/pull/1380))
- *(pipeline)* name every input to Cargo from check and from a failed load ([#1379](https://github.com/getkono/spargen/pull/1379))
- *(codegen)* [**breaking**] send a request body declared with no schema as any value of its media ([#1377](https://github.com/getkono/spargen/pull/1377))
- *(codegen)* refuse to serialize a discriminator tag that does not select the variant ([#1375](https://github.com/getkono/spargen/pull/1375))
- *(runtime)* redact secret response headers in error Debug, and print each cause once ([#1374](https://github.com/getkono/spargen/pull/1374))
- *(frontend)* warn on OpenAPI 3.0 nullable in a 3.1/3.2 document ([#1373](https://github.com/getkono/spargen/pull/1373))
- *(emit)* make the output header and input digest independent of the checkout path and OS ([#1368](https://github.com/getkono/spargen/pull/1368))
- *(surface)* classify differing uuid, datetime and testing settings in `spargen diff` ([#1371](https://github.com/getkono/spargen/pull/1371))
- *(runtime)* carry scalar text/* bodies as bare text in both directions ([#1360](https://github.com/getkono/spargen/pull/1360))
- *(name)* reserve the std and builder methods generated clients, params and servers answer to ([#1370](https://github.com/getkono/spargen/pull/1370))
- *(codegen)* try each trial union variant only on a payload of its JSON category ([#1363](https://github.com/getkono/spargen/pull/1363))
- *(runtime)* [**breaking**] refuse a LinkPaginator next link that leaves the first page's origin ([#1357](https://github.com/getkono/spargen/pull/1357))
- *(source)* refuse https downgrades, bound bodies and write atomically in spargen lock ([#1365](https://github.com/getkono/spargen/pull/1365))
- *(runtime)* stream failure handling — read errors by class, SSE EOF discard, terminal reconnect responses ([#1354](https://github.com/getkono/spargen/pull/1354))
- *(diag)* keep the rejecting error when warnings fill batch_cap ([#1367](https://github.com/getkono/spargen/pull/1367))
- *(runtime)* refuse a colon in a Basic username, zeroize credential intermediates, and send one Authorization ([#1358](https://github.com/getkono/spargen/pull/1358))
- *(name)* disambiguate colliding synthesized operationIds instead of rejecting them ([#1364](https://github.com/getkono/spargen/pull/1364))
- *(runtime)* read non-ASCII UTF-8 response headers and split header lists by RFC 9110's list rule ([#1359](https://github.com/getkono/spargen/pull/1359))
- *(runtime)* stop refusing a bodiless response's Content-Length and bound the refresh read by max_body ([#1321](https://github.com/getkono/spargen/pull/1321))
- *(codegen)* read an over-indented list continuation as paragraph text, not a fence ([#1318](https://github.com/getkono/spargen/pull/1318))
- *(runtime)* [**breaking**] read and write one RFC 3339 date-time grammar under every backend ([#1319](https://github.com/getkono/spargen/pull/1319))
- *(runtime)* link no private runtime item from a public item's docs ([#1320](https://github.com/getkono/spargen/pull/1320))
- *(codegen)* keep a lone CR, bidi controls, non-void self-closing tags and over-indented list items out of carried docs ([#1315](https://github.com/getkono/spargen/pull/1315))
- *(runtime)* [**breaking**] read a binary _stream twin's body chunk by chunk on wasm32 ([#1313](https://github.com/getkono/spargen/pull/1313))
- *(codegen)* keep carried code blocks out of the consumer's doctests ([#1312](https://github.com/getkono/spargen/pull/1312))
- *(runtime)* keep a query apiKey and bound header values out of logged text ([#1310](https://github.com/getkono/spargen/pull/1310))
- *(codegen)* testing module reply defects: testing::testing, XML sequences, claimed statuses ([#1311](https://github.com/getkono/spargen/pull/1311))
- *(codegen)* spell UnknownVariant in a string enum's from_str signature ([#1309](https://github.com/getkono/spargen/pull/1309))
- *(surface)* report a renamed _stream twin or its companions in spargen diff ([#1298](https://github.com/getkono/spargen/pull/1298))

### Other

- *(hooks)* drop clippy and the test suite from pre-push ([#1471](https://github.com/getkono/spargen/pull/1471))
- *(api)* [**breaking**] give DateTimeBackend::from_str a typed UnknownDateTimeBackend error ([#1465](https://github.com/getkono/spargen/pull/1465))
- cut AGENTS.md to cross-cutting rules and scope the rest in .claude/rules ([#1396](https://github.com/getkono/spargen/pull/1396))
- cover the audit's remaining untested branches across support-runtime, spargen and spargen-macro ([#1378](https://github.com/getkono/spargen/pull/1378))
- rewrite the README for clarity and public sharing
- *(book)* move the generated-surface reference and guarantees out of the README
- *(runtime)* pin multipart calls retried and replayed after a refresh ([#1295](https://github.com/getkono/spargen/pull/1295))
- *(runtime)* make enforcing TotalTimeout part of the HttpBackend contract ([#1294](https://github.com/getkono/spargen/pull/1294))

## [0.7.0](https://github.com/getkono/spargen/compare/spargen-v0.6.0...spargen-v0.7.0) - 2026-11-07

### Added

- *(codegen)* [**breaking**] emit Client::with_timeout for a per-client-value total timeout ([#1255](https://github.com/getkono/spargen/pull/1255))
- *(runtime)* [**breaking**] bound the default client's connect and read time, with an opt-in total timeout ([#1254](https://github.com/getkono/spargen/pull/1254))
- *(codegen)* open a discriminated union to tags it does not list under open_unions ([#1246](https://github.com/getkono/spargen/pull/1246))
- *(codegen)* [**breaking**] name inline discriminated variants from their pinned tag ([#1243](https://github.com/getkono/spargen/pull/1243))
- *(runtime_contract)* check locked versions for an unpublished package ([#1237](https://github.com/getkono/spargen/pull/1237))
- *(runtime)* [**breaking**] read RFC 9457 extension members into ProblemDetails::extensions ([#1236](https://github.com/getkono/spargen/pull/1236))

### Fixed

- *(codegen)* [**breaking**] serialize contentEncoding: base64 members as base64 strings ([#1238](https://github.com/getkono/spargen/pull/1238))
- *(oas31)* leave declared components out of the pinned-variant rename ([#1252](https://github.com/getkono/spargen/pull/1252))
- *(runtime_contract)* check the requirement of a package from a directory source ([#1244](https://github.com/getkono/spargen/pull/1244))

### Other

- *(runtime)* join parameter and query fragments in one buffer ([#1256](https://github.com/getkono/spargen/pull/1256))
- *(runtime)* [**breaking**] move the reqwest requirement to 0.13 and quick-xml to 0.42 ([#1253](https://github.com/getkono/spargen/pull/1253))
- qualify E007 and the never-Value claims for open_unions' Unknown variant ([#1251](https://github.com/getkono/spargen/pull/1251))
- *(deps)* hold releases on runtime requirements behind their latest release ([#1249](https://github.com/getkono/spargen/pull/1249))

## [0.6.0](https://github.com/getkono/spargen/compare/spargen-v0.5.0...spargen-v0.6.0) - 2026-11-06

### Added

- *(runtime)* [**breaking**] retype HttpBackend onto http::Request and http::Response ([#1222](https://github.com/getkono/spargen/pull/1222))
- derive Hash on public value types and add error accessors ([#1208](https://github.com/getkono/spargen/pull/1208))
- *(codegen)* emit with_default_server and the scheme docs on BlockingClient ([#1186](https://github.com/getkono/spargen/pull/1186))
- *(codegen)* note a deprecated required parameter in its method docs ([#1185](https://github.com/getkono/spargen/pull/1185))
- *(compat)* accept every component kind spelling in omit! ([#1160](https://github.com/getkono/spargen/pull/1160))
- [**breaking**] let CLI and macro switches override spargen.toml in either direction ([#1079](https://github.com/getkono/spargen/pull/1079))
- [**breaking**] return the warnings of successful requirements and diff runs ([#1078](https://github.com/getkono/spargen/pull/1078))
- *(codegen)* expose each operation's resolved URL through a {op}_url accessor ([#1077](https://github.com/getkono/spargen/pull/1077))
- *(config)* let callers read back Spec and Build settings ([#1076](https://github.com/getkono/spargen/pull/1076))
- give the facade's sibling types consistent standard traits ([#1061](https://github.com/getkono/spargen/pull/1061))
- warn when generate's or check's report is discarded ([#1059](https://github.com/getkono/spargen/pull/1059))
- *(oas31)* generate application/cbor and +cbor bodies as bytes ([#990](https://github.com/getkono/spargen/pull/990))
- *(diag)* [**breaking**] hold each diagnostic's outcome claim to the run's outcome ([#441](https://github.com/getkono/spargen/pull/441))
- *(runtime)* [**breaking**] let a registered credential be unregistered so selection can reach a later alternative ([#287](https://github.com/getkono/spargen/pull/287))
- open const-narrowed response strings, and read problem details across operations ([#396](https://github.com/getkono/spargen/pull/396))
- *(runtime)* [**breaking**] type a credential-kind mismatch as RequestError::CredentialMismatch ([#335](https://github.com/getkono/spargen/pull/335))
- *(oas31)* resolve a same-file $ref into a component subschema through the resolver ([#260](https://github.com/getkono/spargen/pull/260))

### Fixed

- *(oas31)* keep meet_inserts aligned with union members kept unmet ([#1228](https://github.com/getkono/spargen/pull/1228))
- *(oas31)* follow a bare alias to an open target in a nullable-alias body ([#1215](https://github.com/getkono/spargen/pull/1215))
- *(oas31)* [**breaking**] make a composed schema of exactly null the nullable null type ([#1214](https://github.com/getkono/spargen/pull/1214))
- *(oas31)* word a non-string openapi field apart from an absent one ([#1210](https://github.com/getkono/spargen/pull/1210))
- *(oas31)* lower a component body repeating one cycle-closing ref as an alias ([#1209](https://github.com/getkono/spargen/pull/1209))
- *(runtime)* name the emitter in a parameter error that expected an object ([#1211](https://github.com/getkono/spargen/pull/1211))
- *(runtime)* [**breaking**] classify a body that fails mid-read as InterruptedBody ([#1204](https://github.com/getkono/spargen/pull/1204))
- *(oas31)* name the 3.2 Path Item field a 3.1 document is rejected for ([#1197](https://github.com/getkono/spargen/pull/1197))
- *(runtime)* [**breaking**] serialize an empty array or object parameter as undefined ([#1196](https://github.com/getkono/spargen/pull/1196))
- *(runtime)* [**breaking**] end a LinkPaginator cycle with Error::PaginationCycle ([#1195](https://github.com/getkono/spargen/pull/1195))
- *(surface)* classify a change to a URL accessor's arguments as major ([#1188](https://github.com/getkono/spargen/pull/1188))
- *(name)* [**breaking**] rank operation methods, accessors, params and header structs ([#1192](https://github.com/getkono/spargen/pull/1192))
- *(runtime)* read a finite float back from a simple header exactly ([#1193](https://github.com/getkono/spargen/pull/1193))
- *(cli)* warn when spargen lock is given spec flags it ignores ([#1191](https://github.com/getkono/spargen/pull/1191))
- *(oas31)* [**breaking**] make the allOf meet of two null types the nullable null type ([#1183](https://github.com/getkono/spargen/pull/1183))
- *(runtime)* end SSE lines at a bare CR and strip a leading byte order mark ([#1180](https://github.com/getkono/spargen/pull/1180))
- *(oas31)* lower a oneOf repeating a cycle-closing ref instead of failing E011 ([#1177](https://github.com/getkono/spargen/pull/1177))
- *(name)* [**breaking**] seed a disambiguated name's suffix from case-folded media keys ([#1182](https://github.com/getkono/spargen/pull/1182))
- *(runtime)* send cookie parameters and cookie API keys in one Cookie header ([#1187](https://github.com/getkono/spargen/pull/1187))
- *(runtime)* [**breaking**] retry only idempotent requests and honour Retry-After ([#1181](https://github.com/getkono/spargen/pull/1181))
- *(surface)* keep the case of additionalOperations methods in diff keys ([#1178](https://github.com/getkono/spargen/pull/1178))
- *(codegen)* build QUERY and custom methods without expect in generated code ([#1179](https://github.com/getkono/spargen/pull/1179))
- *(deps)* raise the tokio floor to 1.53.3, past RUSTSEC-2026-0386 ([#1175](https://github.com/getkono/spargen/pull/1175))
- *(facade)* parse in vendor and the macro preview on the frontend stack ([#1165](https://github.com/getkono/spargen/pull/1165))
- *(oas31)* select an inline discriminated member by the tag value it pins ([#1012](https://github.com/getkono/spargen/pull/1012))
- *(oas31)* make a recursive allOf member's verdict independent of declaration order ([#1010](https://github.com/getkono/spargen/pull/1010))
- *(oas31)* [**breaking**] merge the union branches an untyped allOf member refines alike ([#1024](https://github.com/getkono/spargen/pull/1024))
- *(oas31)* [**breaking**] merge the branches a $ref's non-union sibling leaves alike in its union target ([#1022](https://github.com/getkono/spargen/pull/1022))
- *(oas31)* [**breaking**] reject an xml.name that is not an XML name as E011 ([#1019](https://github.com/getkono/spargen/pull/1019))
- *(oas31)* collapse the unions a union conjunct's meet nests per variant ([#1015](https://github.com/getkono/spargen/pull/1015))
- *(oas31)* record the xml merger when a $ref is intersected with its siblings ([#1016](https://github.com/getkono/spargen/pull/1016))
- *(codegen)* [**breaking**] let an optional nullable JSON content parameter send an explicit null ([#1013](https://github.com/getkono/spargen/pull/1013))
- *(oas31)* judge a merged xml member through the elided types its owners merged ([#1018](https://github.com/getkono/spargen/pull/1018))
- *(oas31)* [**breaking**] elide every type no component or operation reaches after lowering ([#1011](https://github.com/getkono/spargen/pull/1011))
- *(codegen)* [**breaking**] report an unparseable generated module as E011 instead of a compile_error! stub ([#948](https://github.com/getkono/spargen/pull/948))
- *(oas31)* [**breaking**] reject a delimited or deepObject style over a value it cannot serialize ([#838](https://github.com/getkono/spargen/pull/838))
- *(tests)* keep only aliased anchors' copies when the oracles read YAML ([#1009](https://github.com/getkono/spargen/pull/1009))
- *(ir)* count only a bodied XML response as XML in a multi-status enum ([#853](https://github.com/getkono/spargen/pull/853))
- *(oas31)* [**breaking**] report a merged composition member's xml object instead of dropping it ([#994](https://github.com/getkono/spargen/pull/994))
- *(oas31)* [**breaking**] elide a single-variant union's unused member and sibling lowerings ([#993](https://github.com/getkono/spargen/pull/993))
- *(oas31)* [**breaking**] read the one null decision at the remaining null counters ([#986](https://github.com/getkono/spargen/pull/986))
- *(oas31)* [**breaking**] reject the remaining ill-typed Schema keyword values as E011 ([#989](https://github.com/getkono/spargen/pull/989))
- *(oas31)* agree every allOf union spelling beside object members ([#985](https://github.com/getkono/spargen/pull/985))
- *(oas31)* withhold the unreferenced composition an allOf union meet consumed ([#982](https://github.com/getkono/spargen/pull/982))
- *(oas31)* name a $ref property's XML element after the component under OpenAPI 3.2 ([#976](https://github.com/getkono/spargen/pull/976))
- *(oas31)* report a remote $ref alias cycle at the $ref that reached it ([#981](https://github.com/getkono/spargen/pull/981))
- *(runtime)* drop the doc links to private ParameterError variants ([#980](https://github.com/getkono/spargen/pull/980))
- *(oas31)* keep the union branches an allOf type array naming object admits ([#974](https://github.com/getkono/spargen/pull/974))
- *(oas31)* elide the unused copy of a schema lowered twice ([#973](https://github.com/getkono/spargen/pull/973))
- *(source)* read a remote URL ending in a slash by the vendored path when spargen lock picks the parse format ([#972](https://github.com/getkono/spargen/pull/972))
- *(source)* read a dot-file name as having no extension when spargen lock picks the parse format ([#961](https://github.com/getkono/spargen/pull/961))
- *(source)* read a local path's query as part of its name when spargen lock picks the parse format ([#957](https://github.com/getkono/spargen/pull/957))
- *(codegen)* send pinned encoding headers on RFC 6570-serialized multipart parts ([#959](https://github.com/getkono/spargen/pull/959))
- *(source)* address a non-string YAML mapping key by the mapping that holds it ([#954](https://github.com/getkono/spargen/pull/954))
- *(oas31)* name an XML body root element after its component ([#950](https://github.com/getkono/spargen/pull/950))
- *(source)* choose the parse format from a case-folded extension in both lock and build ([#953](https://github.com/getkono/spargen/pull/953))
- *(oas31)* elide the types a binary parameter or a skipped response header leaves unreferenced ([#942](https://github.com/getkono/spargen/pull/942))
- *(oas31)* read an integral float enum/const member of an integer schema as that integer ([#946](https://github.com/getkono/spargen/pull/946))
- *(oas31)* derive item-name renames only on XML-reachable owners ([#941](https://github.com/getkono/spargen/pull/941))
- *(codegen)* emit #[deprecated] on enum aliases, cycle newtypes and generic aliases ([#937](https://github.com/getkono/spargen/pull/937))
- *(compat)* remove glob omit matches in a total order ([#935](https://github.com/getkono/spargen/pull/935))
- *(oas31)* locate an allOf member that is false at its own entry ([#936](https://github.com/getkono/spargen/pull/936))
- *(runtime_contract)* accept `tokio/<feature>` as wiring the blocking feature ([#932](https://github.com/getkono/spargen/pull/932))
- *(codegen)* send typed additionalProperties members in form and multipart bodies ([#902](https://github.com/getkono/spargen/pull/902))
- *(diff)* carve each side when Spec::carve is set ([#904](https://github.com/getkono/spargen/pull/904))
- *(source)* register a remote document's opaque $self as written ([#903](https://github.com/getkono/spargen/pull/903))
- *(oas31)* decide patternProperties homogeneity by the emitted Rust type ([#891](https://github.com/getkono/spargen/pull/891))
- *(oas31)* classify the OpenAPI version once, so a malformed 3.1/3.2 version reports E001 ([#899](https://github.com/getkono/spargen/pull/899))
- *(diag)* [**breaking**] name the file a diagnostic's span lies in, in human and JSON output ([#889](https://github.com/getkono/spargen/pull/889))
- *(codegen)* send JSON and XML request bodies under their declared Content-Type ([#886](https://github.com/getkono/spargen/pull/886))
- *(source)* give each remote URL a vendored file of its own ([#884](https://github.com/getkono/spargen/pull/884))
- *(surface)* model response headers, server builders and typed additionalProperties in spargen diff ([#870](https://github.com/getkono/spargen/pull/870))
- *(oas31)* classify media types case-insensitively, and consume SSE content by that classification ([#871](https://github.com/getkono/spargen/pull/871))
- *(support-runtime)* [**breaking**] bound the bytes EventStream buffers for an unterminated item ([#855](https://github.com/getkono/spargen/pull/855))
- *(name)* allocate operation error and success-enum type names in the root scope ([#856](https://github.com/getkono/spargen/pull/856))
- *(oas31)* treat an empty description as absent before a component-root default note ([#877](https://github.com/getkono/spargen/pull/877))
- *(oas31)* meet an allOf union member before merging its indistinguishable branches ([#868](https://github.com/getkono/spargen/pull/868))
- *(lib)* emit every line of a cargo diagnostic as a directive ([#876](https://github.com/getkono/spargen/pull/876))
- *(ir)* derive the time and bytes serde requirements from emitted types only ([#866](https://github.com/getkono/spargen/pull/866))
- *(source)* reject a repeated key or a repeated url in spargen.lock ([#858](https://github.com/getkono/spargen/pull/858))
- *(source)* keep only the kept attempt's diagnostics in the extensionless YAML-then-JSON fallback ([#878](https://github.com/getkono/spargen/pull/878))
- *(oas31)* [**breaking**] reject ill-typed Schema keyword values as E011 instead of dropping them ([#862](https://github.com/getkono/spargen/pull/862))
- *(runtime)* reject sign-prefixed fields in Date::from_str ([#854](https://github.com/getkono/spargen/pull/854))
- *(deps)* floor rustls-webpki and webpki-roots for RUSTSEC-2026-0382 and RUSTSEC-2026-0381 ([#873](https://github.com/getkono/spargen/pull/873))
- *(source)* detect a $ref scheme by the RFC 3986 grammar, so git+https: is E003 ([#865](https://github.com/getkono/spargen/pull/865))
- *(config)* [**breaking**] reject an [[omit]] key the entry's kind does not use ([#869](https://github.com/getkono/spargen/pull/869))
- *(source)* keep a failed spargen lock fetch's earlier pins instead of dropping them ([#867](https://github.com/getkono/spargen/pull/867))
- *(cache)* replay the batch-cap truncation of the run a cache hit replays ([#864](https://github.com/getkono/spargen/pull/864))
- *(surface)* classify a requestBody.required flip as a major change ([#863](https://github.com/getkono/spargen/pull/863))
- *(source)* cap YAML nesting at the JSON parser's depth ([#860](https://github.com/getkono/spargen/pull/860))
- *(source)* locate a failed spargen lock fetch (E025) at the referring $ref ([#859](https://github.com/getkono/spargen/pull/859))
- *(oas31)* [**breaking**] honour root and items XML Objects instead of dropping them ([#826](https://github.com/getkono/spargen/pull/826))
- *(source)* keep a `#` inside a quoted spargen.lock value as part of the value ([#848](https://github.com/getkono/spargen/pull/848))
- cap the body a multi-success operation keeps in an error ([#850](https://github.com/getkono/spargen/pull/850))
- *(oas31)* audit schemas under headers, encoding headers and referenced path items ([#849](https://github.com/getkono/spargen/pull/849))
- *(oas31)* resolve a $ref'd multipart encoding header schema before deciding it pins no value ([#840](https://github.com/getkono/spargen/pull/840))
- *(oas31)* report object members beside several unions as their own E013 case ([#842](https://github.com/getkono/spargen/pull/842))
- *(source)* locate an unreadable $ref target at the referring $ref ([#846](https://github.com/getkono/spargen/pull/846))
- *(source)* budget YAML alias expansion and reject a document past it with E011 ([#845](https://github.com/getkono/spargen/pull/845))
- *(oas31)* reject a content parameter whose media entry has no schema ([#843](https://github.com/getkono/spargen/pull/843))
- rerun build.rs and re-expand generate_api! when an applied spargen.toml changes ([#836](https://github.com/getkono/spargen/pull/836))
- *(support-runtime)* reconnect without Last-Event-ID when the SSE id is not a valid header value ([#837](https://github.com/getkono/spargen/pull/837))
- *(oas31)* restrict union branches to the categories a multi-category type array beside them names ([#816](https://github.com/getkono/spargen/pull/816))
- *(oas31)* classify an integral float default as the integer it equals ([#822](https://github.com/getkono/spargen/pull/822))
- *(compat)* name every accepted value in an unknown omit token's error ([#819](https://github.com/getkono/spargen/pull/819))
- *(source)* [**breaking**] reject non-finite YAML floats as E011 instead of reading them as 0.0 ([#820](https://github.com/getkono/spargen/pull/820))
- *(oas31)* [**breaking**] reject a server URL template whose braces do not form `{name}` references ([#823](https://github.com/getkono/spargen/pull/823))
- *(oas31)* stop treating contentSchema as a reference-cycle edge ([#817](https://github.com/getkono/spargen/pull/817))
- *(oas31)* match an http security scheme name in any letter case ([#815](https://github.com/getkono/spargen/pull/815))
- *(oas31)* [**breaking**] reject a multipart encoding header that is not an HTTP field ([#813](https://github.com/getkono/spargen/pull/813))
- *(oas31)* [**breaking**] decide null admission once for every spelling of a constraint set ([#679](https://github.com/getkono/spargen/pull/679))
- *(runtime_contract)* read default_features as Cargo does below edition 2024 ([#808](https://github.com/getkono/spargen/pull/808))
- *(support-runtime)* report a malformed JSON-content header as a parse error, not null ([#807](https://github.com/getkono/spargen/pull/807))
- *(deps)* floor rustls at 0.23.46 and bump the locked quinn-proto to 0.11.18 ([#683](https://github.com/getkono/spargen/pull/683))
- *(oas31)* [**breaking**] give an allOf-spelled oneOf's stated-nothing branch the sibling spelling's null ([#672](https://github.com/getkono/spargen/pull/672))
- *(oas31)* [**breaking**] drop a oneOf exact-null branch beside another branch null matches ([#669](https://github.com/getkono/spargen/pull/669))
- *(oas31)* [**breaking**] read an allOf member's own enum and const before giving a union member null ([#667](https://github.com/getkono/spargen/pull/667))
- *(oas31)* [**breaking**] collapse oneOf variants an allOf's scalar meet leaves indistinguishable ([#665](https://github.com/getkono/spargen/pull/665))
- *(oas31)* [**breaking**] keep null for an inline union nested in an inner allOf ([#664](https://github.com/getkono/spargen/pull/664))
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

- *(ir)* carry each union variant's dispatch on its UnionVariant ([#1229](https://github.com/getkono/spargen/pull/1229))
- *(frontend)* collapse fixtures that re-run check beside generate, and settle the two shape readers ([#1227](https://github.com/getkono/spargen/pull/1227))
- *(mutants)* extend the mutation gate to runtime_contract, cache and the IR and codegen helpers ([#1224](https://github.com/getkono/spargen/pull/1224))
- *(oas31)* consolidate union variants and the shared union/allOf lowering steps ([#1225](https://github.com/getkono/spargen/pull/1225))
- *(mutants)* widen the mutation gate to the small pure modules the suites constrain ([#1223](https://github.com/getkono/spargen/pull/1223))
- reconcile AGENTS.md and module docs with the restructured code ([#1221](https://github.com/getkono/spargen/pull/1221))
- *(oas31)* reach the reservation branch's elisions with types to elide ([#1219](https://github.com/getkono/spargen/pull/1219))
- *(runtime)* reconnect a stream through the shared backend, not a cloned ClientCore ([#1218](https://github.com/getkono/spargen/pull/1218))
- fix the release and deny guards, run the bench asserts, and trim tooling comments ([#1217](https://github.com/getkono/spargen/pull/1217))
- *(runtime)* frame stream bodies in time linear in their size ([#1202](https://github.com/getkono/spargen/pull/1202))
- restructure AGENTS.md around its rules and add an Invariants section ([#1213](https://github.com/getkono/spargen/pull/1213))
- *(diag)* find duplicate diagnostics through a hash index ([#1212](https://github.com/getkono/spargen/pull/1212))
- *(fuzz)* hold the libFuzzer target to the shared oracles, with a dictionary and corpus seeding ([#1205](https://github.com/getkono/spargen/pull/1205))
- *(source)* share the URL and $self classifiers and split parse.rs and vendor.rs ([#1198](https://github.com/getkono/spargen/pull/1198))
- *(cache)* hash build inputs in place with an incremental SHA-256 ([#1201](https://github.com/getkono/spargen/pull/1201))
- *(compat)* compile each omit glob once per rule ([#1200](https://github.com/getkono/spargen/pull/1200))
- *(e2e)* drive the QUERY operation over the wire in the oas32 suite ([#1194](https://github.com/getkono/spargen/pull/1194))
- *(runtime)* drop per-request copies and collapse duplicated codec and alias boilerplate ([#1167](https://github.com/getkono/spargen/pull/1167))
- run the slow gates only after merge and on release PRs ([#1169](https://github.com/getkono/spargen/pull/1169))
- *(oas31)* key lowering's reservation state and share its duplicated helpers ([#1164](https://github.com/getkono/spargen/pull/1164))
- *(cache)* read cached claims through a private CachedClaim and keep OutcomeClaim serialize-only ([#1163](https://github.com/getkono/spargen/pull/1163))
- *(facade)* split lib.rs into report, pipeline and diff_api modules and simplify its plumbing ([#1161](https://github.com/getkono/spargen/pull/1161))
- pin the facade's untested paths and make the self-reading gates fail loudly ([#1159](https://github.com/getkono/spargen/pull/1159))
- give codegen one per-operation name record and signature ([#1156](https://github.com/getkono/spargen/pull/1156))
- *(frontend)* pin each diagnostic case to a fixture and keep fixtures in their family modules ([#1149](https://github.com/getkono/spargen/pull/1149))
- share one generation helper and one mock server across the integration suites ([#1151](https://github.com/getkono/spargen/pull/1151))
- widen the lowering and fuzz generators to nested, null and operation shapes ([#1150](https://github.com/getkono/spargen/pull/1150))
- *(frontend)* hold every fixture run to the IR invariants and full check/generate parity ([#1147](https://github.com/getkono/spargen/pull/1147))
- pin the middle- and back-end behaviours and tighten loose assertions ([#1145](https://github.com/getkono/spargen/pull/1145))
- bring the README, book and docs pages into line with the code ([#1144](https://github.com/getkono/spargen/pull/1144))
- *(runtime)* pin failure, pending and overflow paths and share the test fakes ([#1140](https://github.com/getkono/spargen/pull/1140))
- keep Severity exhaustive ([#1139](https://github.com/getkono/spargen/pull/1139))
- correct the middle- and back-end comments and emitted doc strings ([#1134](https://github.com/getkono/spargen/pull/1134))
- correct the facade, diag, config and CLI docs and trim explain texts ([#1135](https://github.com/getkono/spargen/pull/1135))
- *(oas31)* correct and tighten the frontend comments ([#1136](https://github.com/getkono/spargen/pull/1136))
- *(runtime)* correct the support-runtime docs every generated client carries ([#1137](https://github.com/getkono/spargen/pull/1137))
- *(diag)* pin FileId::get against ids other than 0 and 1 ([#1075](https://github.com/getkono/spargen/pull/1075))
- *(compat)* kill the carve.rs mutants the mutation gate reports missed ([#1072](https://github.com/getkono/spargen/pull/1072))
- *(compat)* pin the Display forms of ComponentKind and OmitMethod ([#1074](https://github.com/getkono/spargen/pull/1074))
- *(diag)* pin Diagnostics::cap_reached below the cap ([#1073](https://github.com/getkono/spargen/pull/1073))
- *(diag)* [**breaking**] hide the id and code newtypes' fields behind accessors ([#1064](https://github.com/getkono/spargen/pull/1064))
- *(compat)* [**breaking**] make Omit::fingerprint crate-private ([#1063](https://github.com/getkono/spargen/pull/1063))
- [**breaking**] rename DiffRejection accessors to old_report and new_report ([#1062](https://github.com/getkono/spargen/pull/1062))
- move misplaced tests into the suites CLAUDE.md assigns them to ([#1060](https://github.com/getkono/spargen/pull/1060))
- *(diag)* [**breaking**] keep the diagnostic builder crate-private ([#1058](https://github.com/getkono/spargen/pull/1058))
- [**breaking**] mark CargoIntegration and Severity non_exhaustive ([#1057](https://github.com/getkono/spargen/pull/1057))
- [**breaking**] mark the facade's public-field data structs non_exhaustive ([#1056](https://github.com/getkono/spargen/pull/1056))
- correct facade rustdoc and name the in-module suites in the testing table ([#1052](https://github.com/getkono/spargen/pull/1052))
- *(name)* [**breaking**] drop the unused HeaderShape root re-export from generated clients ([#1054](https://github.com/getkono/spargen/pull/1054))
- remove tests another test already asserts on the same inputs ([#1055](https://github.com/getkono/spargen/pull/1055))
- bring the README, book and macro README into line with the code ([#1046](https://github.com/getkono/spargen/pull/1046))
- *(lib)* pin the Report severity partition and Outcome rendering in the facade tests ([#1049](https://github.com/getkono/spargen/pull/1049))
- *(oas31)* hold every harness case to the no-unreferenced-type oracle ([#1021](https://github.com/getkono/spargen/pull/1021))
- *(oas31)* pin that a merged, collapsed or narrowed union emits none of its branch types ([#1020](https://github.com/getkono/spargen/pull/1020))
- *(oas31)* pin the remote-ref xml merger record with a vendored fixture ([#1005](https://github.com/getkono/spargen/pull/1005))
- *(oas31)* split the long lowering functions and report E007 through one helper ([#991](https://github.com/getkono/spargen/pull/991))
- make the repository-gate tests check every cell, task, floor and target they name ([#988](https://github.com/getkono/spargen/pull/988))
- *(layering)* see test-only edges, cycles, root re-exports, conditional modules and file growth ([#984](https://github.com/getkono/spargen/pull/984))
- *(tests)* split the repository-gate and layering suites by gate ([#983](https://github.com/getkono/spargen/pull/983))
- make the config, cache, compat, CLI and runtime-contract comments state present behaviour ([#977](https://github.com/getkono/spargen/pull/977))
- *(runtime)* serialize object parameters through shared object_flat and object_pairs helpers ([#975](https://github.com/getkono/spargen/pull/975))
- *(emit)* collapse the generated-code plan to the one file it carries ([#971](https://github.com/getkono/spargen/pull/971))
- *(diag)* pin E009 and E010 explain cases to their emission sites ([#969](https://github.com/getkono/spargen/pull/969))
- *(codegen)* build the blocking client's runtime through one emitted helper ([#968](https://github.com/getkono/spargen/pull/968))
- *(frontend)* collapse the remaining [generate, check] loops into one generate call ([#970](https://github.com/getkono/spargen/pull/970))
- *(oas31)* share lowering's reservation, $ref-sibling and keyword predicates ([#963](https://github.com/getkono/spargen/pull/963))
- *(lib)* give the facade's audit merge, input list and root diagnostics one helper each ([#966](https://github.com/getkono/spargen/pull/966))
- *(tests)* split the bodies, recursion and refs frontend suites under the file-length limit ([#964](https://github.com/getkono/spargen/pull/964))
- *(diag)* describe when W001, W009, W012, E012, E019 and E020 fire ([#962](https://github.com/getkono/spargen/pull/962))
- *(runtime)* delete the unused build_url and build_url_with_query_string ([#965](https://github.com/getkono/spargen/pull/965))
- split surface/mod.rs and the diff and e2e test suites under the file-length limit ([#960](https://github.com/getkono/spargen/pull/960))
- *(runtime)* state query appending, header decoding and full-date formatting once ([#958](https://github.com/getkono/spargen/pull/958))
- *(oas31)* split lowering's parameter, encoding and response functions ([#956](https://github.com/getkono/spargen/pull/956))
- *(ir)* kill the missed stream_framing, xml_in_multi_status and api_error_body mutants ([#949](https://github.com/getkono/spargen/pull/949))
- *(oas31)* hold every spelling of an exact-null arm beside a multi-category type array to the () arm ([#955](https://github.com/getkono/spargen/pull/955))
- *(e2e)* drop reqwest's json feature from the fixture manifests ([#951](https://github.com/getkono/spargen/pull/951))
- *(ir)* split the response partition out of ir/media.rs into ir/responses.rs ([#919](https://github.com/getkono/spargen/pull/919))
- *(oas31)* restate lowering comments as the rules they follow ([#943](https://github.com/getkono/spargen/pull/943))
- *(runtime)* make the embedded runtime rustdoc state what each item does ([#952](https://github.com/getkono/spargen/pull/952))
- *(runtime)* split Link entries and parameters with one quote-aware splitter ([#945](https://github.com/getkono/spargen/pull/945))
- *(runtime_contract)* fold the blocking-wiring check into the requirement table and share fixture floors ([#944](https://github.com/getkono/spargen/pull/944))
- *(codegen)* split naming lookups into idents.rs and fold repeated token builders ([#933](https://github.com/getkono/spargen/pull/933))
- *(source)* make the loader comments describe the lock, remote and hashing code as it is ([#939](https://github.com/getkono/spargen/pull/939))
- *(frontend)* split the allOf, union and $ref-sibling suites into sub-families ([#940](https://github.com/getkono/spargen/pull/940))
- *(frontend)* pin the nullable-variant guard in array_admits_all_of ([#938](https://github.com/getkono/spargen/pull/938))
- *(oas31)* read the one null decision instead of per-path null counters ([#897](https://github.com/getkono/spargen/pull/897))
- *(diag)* [**breaking**] mark Diagnostic, VendorReport and VendoredRef non_exhaustive ([#930](https://github.com/getkono/spargen/pull/930))
- *(oas31)* make lowering's comments state the cases their bodies handle ([#931](https://github.com/getkono/spargen/pull/931))
- *(frontend)* make the shared oracles and the fuzz harness fail on what they claim ([#929](https://github.com/getkono/spargen/pull/929))
- make the recipe, example, determinism, naming and hashing tests constrain what they name ([#921](https://github.com/getkono/spargen/pull/921))
- *(frontend)* make fixtures assert the output their names promise ([#922](https://github.com/getkono/spargen/pull/922))
- *(source)* hold the lock's and the build's $self and format readings to one table ([#920](https://github.com/getkono/spargen/pull/920))
- *(support-runtime)* fold the stream reader's CR trims and stop exits ([#914](https://github.com/getkono/spargen/pull/914))
- make the facade and diag rustdoc describe the public items as they are ([#912](https://github.com/getkono/spargen/pull/912))
- *(oas31)* split the schema parser out of deserialize.rs and remove its duplicated builders, keyword lists and redundant cfgs ([#908](https://github.com/getkono/spargen/pull/908))
- *(tests)* make test headers, fixture comments and recipe tables describe what the code does ([#909](https://github.com/getkono/spargen/pull/909))
- make the ir, surface, name and codegen comments match the code they describe ([#906](https://github.com/getkono/spargen/pull/906))
- *(support-runtime)* escape control characters in error messages through one writer ([#905](https://github.com/getkono/spargen/pull/905))
- drop the dead smoke flag, multipart feature, duplicate bench cases and fuzz seek ([#895](https://github.com/getkono/spargen/pull/895))
- *(oas31)* share the audit and resolver walkers, discriminator entries and root-schema test ([#901](https://github.com/getkono/spargen/pull/901))
- *(support-runtime)* decide the 429/5xx transient rule in one function ([#896](https://github.com/getkono/spargen/pull/896))
- *(runtime)* state what each capability adds and that blocking compiles only off wasm32 ([#893](https://github.com/getkono/spargen/pull/893))
- *(oas31)* state the cases the frontend comments handle today ([#892](https://github.com/getkono/spargen/pull/892))
- *(support-runtime)* express the byte classifiers and success decoder through the shared read helpers ([#888](https://github.com/getkono/spargen/pull/888))
- *(name)* pin the disambiguator and the anonymous-definition rank ([#885](https://github.com/getkono/spargen/pull/885))
- *(source)* share the security requirement key URI predicate ([#887](https://github.com/getkono/spargen/pull/887))
- *(frontend)* remove fixtures and check loops another fixture already covers ([#874](https://github.com/getkono/spargen/pull/874))
- state the feature and gate setup the manifests, gate comments and examples describe as it is ([#880](https://github.com/getkono/spargen/pull/880))
- align the README and book with the CLI's subcommands, options, exit statuses and spans ([#875](https://github.com/getkono/spargen/pull/875))
- dispatch the benchmarks on each spargen release tag ([#847](https://github.com/getkono/spargen/pull/847))
- share the integration tests' generate, manifest, strategy and remote-fixture helpers ([#835](https://github.com/getkono/spargen/pull/835))
- *(support-runtime)* move auth resolution from dispatch into auth ([#834](https://github.com/getkono/spargen/pull/834))
- *(frontend)* pin lowering's parameter, encoding, server, default, refinement and allOf-false rules ([#831](https://github.com/getkono/spargen/pull/831))
- give ir, name, diag and surface one helper per repeated rule ([#829](https://github.com/getkono/spargen/pull/829))
- *(runtime_contract)* pin the manifest audit's feature wiring, version requirements and deps block ([#811](https://github.com/getkono/spargen/pull/811))
- compile every auth kind and stream framing the generator emits, and pin rustdoc normalization ([#814](https://github.com/getkono/spargen/pull/814))
- pin the facade's diff rejection and report, error-body cap and pointer rules ([#810](https://github.com/getkono/spargen/pull/810))
- pin the runtime's transient-status, date, paginator and stream rules before deduplication ([#804](https://github.com/getkono/spargen/pull/804))
- *(cli)* pin the CLI's exit codes, flag effects and output routing ([#801](https://github.com/getkono/spargen/pull/801))
- *(diag)* make the E002, E007, E009, E010, E011, E014 and W014 texts describe what generates ([#805](https://github.com/getkono/spargen/pull/805))
- pin the parser's and deserializer's guards and emission sites ([#802](https://github.com/getkono/spargen/pull/802))
- cut the workspace test suite ~4.7x on the 4-core CI runner ([#681](https://github.com/getkono/spargen/pull/681))
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
