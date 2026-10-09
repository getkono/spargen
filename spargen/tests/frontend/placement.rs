//! Placement independence: a construct reaches the same verdict inline and behind a `$ref`, and
//! what a referenced file holds gets the audit the root document does.

use super::*;

// --- placement independence: an inline document and its `$ref`-split twins -------------------
//
// The metaschema used to run over the root document only, so a construct it closes was rejected
// when written inline and accepted once moved behind a `$ref` into a sibling file (#234). The two
// instances found that way — Responses keys and `additionalOperations` tokens — were each patched
// by transcribing one rule out of the metaschema into Rust. The property below is the general one:
// every fixture is written inline, then mechanically split at one Reference-able position into
// three twins — a whole-file `$ref`, a JSON Pointer into a file, and a two-hop chain through a
// file that is itself a Reference — and all four must reach the same verdict under the same codes,
// through both `generate` and `check`.

/// One inline document and the position to split it at.
struct Placement {
    name: &'static str,
    document: serde_json::Value,
    /// RFC 6901 pointer to the Reference-able value moved out of the root.
    split_at: &'static str,
    /// Whether the inline document is rejected. Asserted on the inline document itself, so a
    /// fixture cannot quietly stop exercising the violation it is named for.
    rejects: bool,
}

/// A root document with one `GET /pet` operation, plus any further top-level members.
fn placement_document(
    version: &str,
    get: serde_json::Value,
    extra: serde_json::Value,
) -> serde_json::Value {
    let mut document = serde_json::json!({
        "openapi": version,
        "info": { "title": "T", "version": "1.0.0" },
        "servers": [{ "url": "https://e.com" }],
        "paths": { "/pet": { "get": get } },
    });
    if let (Some(document), Some(extra)) = (document.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            document.insert(key.clone(), value.clone());
        }
    }
    document
}

fn ok_get() -> serde_json::Value {
    serde_json::json!({
        "operationId": "getPet",
        "responses": { "200": { "description": "ok" } },
    })
}

/// One fixture per Reference-able position the metaschema closes something at, plus valid twins
/// that pin the other direction: validating a referenced file must not over-reject what the same
/// construct inline accepts — a chained Reference included.
fn placement_fixtures() -> Vec<Placement> {
    use serde_json::json;
    let none = json!({});
    vec![
        Placement {
            name: "out-of-grammar Responses key in a Path Item",
            document: placement_document(
                "3.1.0",
                json!({ "operationId": "getPet", "responses": {
                "200": { "description": "ok" }, "0XX": { "description": "bad" } } }),
                none.clone(),
            ),
            split_at: "/paths/~1pet",
            rejects: true,
        },
        Placement {
            name: "unknown field in a Response",
            document: placement_document(
                "3.1.0",
                json!({ "operationId": "getPet", "responses": {
                "200": { "description": "ok", "bogus": 1 } } }),
                none.clone(),
            ),
            split_at: "/paths/~1pet/get/responses/200",
            rejects: true,
        },
        Placement {
            name: "Parameter without `in`",
            document: placement_document(
                "3.1.0",
                json!({ "operationId": "getPet",
                "parameters": [{ "name": "q", "schema": { "type": "string" } }],
                "responses": { "200": { "description": "ok" } } }),
                none.clone(),
            ),
            split_at: "/paths/~1pet/get/parameters/0",
            rejects: true,
        },
        // In 3.1 a Parameter's and a Header's `examples` are admitted only under the
        // `dependentSchemas: { schema: … }` branch of their definitions, so this is the one route
        // by which validation reaches a Reference there.
        Placement {
            name: "OpenAPI 3.1 Parameter Example with an unknown field",
            document: placement_document(
                "3.1.0",
                json!({ "operationId": "getPet",
                "parameters": [{ "name": "q", "in": "query", "schema": { "type": "string" },
                "examples": { "a": { "value": "x", "bogus": 1 } } }],
                "responses": { "200": { "description": "ok" } } }),
                none.clone(),
            ),
            split_at: "/paths/~1pet/get/parameters/0/examples/a",
            rejects: true,
        },
        Placement {
            name: "OpenAPI 3.1 Header Example with an unknown field",
            document: placement_document(
                "3.1.0",
                json!({ "operationId": "getPet", "responses": { "200": {
                "description": "ok", "headers": { "X-Rate": { "schema": { "type": "integer" },
                "examples": { "a": { "value": 1, "bogus": 1 } } } } } } }),
                none.clone(),
            ),
            split_at: "/paths/~1pet/get/responses/200/headers/X-Rate/examples/a",
            rejects: true,
        },
        Placement {
            name: "Request Body without `content`",
            document: placement_document(
                "3.1.0",
                json!({ "operationId": "getPet", "requestBody": { "description": "none" },
                "responses": { "200": { "description": "ok" } } }),
                none.clone(),
            ),
            split_at: "/paths/~1pet/get/requestBody",
            rejects: true,
        },
        Placement {
            name: "Header with neither `schema` nor `content`",
            document: placement_document(
                "3.1.0",
                json!({ "operationId": "getPet", "responses": { "200": {
                "description": "ok", "headers": { "X-Rate": { "description": "rate" } } } } }),
                none.clone(),
            ),
            split_at: "/paths/~1pet/get/responses/200/headers/X-Rate",
            rejects: true,
        },
        Placement {
            name: "Security Scheme without `type`",
            document: placement_document(
                "3.1.0",
                ok_get(),
                json!({ "components": { "securitySchemes": {
                "key": { "name": "k", "in": "header" } } } }),
            ),
            split_at: "/components/securitySchemes/key",
            rejects: true,
        },
        Placement {
            name: "component Response with an unknown field, reached through a component ref",
            document: placement_document(
                "3.1.0",
                json!({ "operationId": "getPet", "responses": {
                "200": { "$ref": "#/components/responses/Ok" } } }),
                json!({ "components": { "responses": {
                "Ok": { "description": "ok", "bogus": 1 } } } }),
            ),
            split_at: "/components/responses/Ok",
            rejects: true,
        },
        Placement {
            name: "Callback Path Item with an unknown field",
            document: placement_document(
                "3.1.0",
                json!({ "operationId": "getPet",
                "callbacks": { "onEvent": { "{$request.body#/url}": { "bogus": 1 } } },
                "responses": { "200": { "description": "ok" } } }),
                none.clone(),
            ),
            split_at: "/paths/~1pet/get/callbacks/onEvent",
            rejects: true,
        },
        Placement {
            name: "OpenAPI 3.2 `additionalOperations` key that is not a method token",
            document: json!({
                "openapi": "3.2.0",
                "info": { "title": "T", "version": "1.0.0" },
                "servers": [{ "url": "https://e.com" }],
                "paths": { "/pet": { "additionalOperations": { "pu rge": {
                "operationId": "purge", "responses": { "204": { "description": "ok" } } } } } },
            }),
            split_at: "/paths/~1pet",
            rejects: true,
        },
        Placement {
            name: "OpenAPI 3.2 Media Type with an unknown field",
            document: placement_document(
                "3.2.0",
                json!({ "operationId": "getPet", "responses": { "200": { "description": "ok",
                "content": { "application/json": {
                "schema": { "type": "string" }, "bogus": 1 } } } } }),
                none.clone(),
            ),
            split_at: "/paths/~1pet/get/responses/200/content/application~1json",
            rejects: true,
        },
        Placement {
            name: "a valid Response, split without changing the verdict",
            document: placement_document(
                "3.1.0",
                json!({ "operationId": "getPet", "responses": { "200": { "description": "ok",
                "headers": { "X-Rate": { "schema": { "type": "integer" } } },
                "content": { "application/json": { "schema": { "type": "string" } } } } } }),
                none.clone(),
            ),
            split_at: "/paths/~1pet/get/responses/200",
            rejects: false,
        },
        Placement {
            name: "a valid Path Item, split without changing the verdict",
            document: placement_document("3.1.0", ok_get(), none.clone()),
            split_at: "/paths/~1pet",
            rejects: false,
        },
        Placement {
            name: "a valid Parameter, split without changing the verdict",
            document: placement_document(
                "3.2.0",
                json!({ "operationId": "getPet",
                "parameters": [{ "name": "q", "in": "query", "schema": { "type": "string" } }],
                "responses": { "200": { "description": "ok" } } }),
                none.clone(),
            ),
            split_at: "/paths/~1pet/get/parameters/0",
            rejects: false,
        },
        Placement {
            name: "a valid Request Body, split without changing the verdict",
            document: placement_document(
                "3.1.0",
                json!({ "operationId": "getPet", "requestBody": { "required": true,
                "content": { "application/json": { "schema": { "type": "string" } } } },
                "responses": { "200": { "description": "ok" } } }),
                none.clone(),
            ),
            split_at: "/paths/~1pet/get/requestBody",
            rejects: false,
        },
        Placement {
            name: "a valid Header, split without changing the verdict",
            document: placement_document(
                "3.1.0",
                json!({ "operationId": "getPet", "responses": { "200": { "description": "ok",
                "headers": { "X-Rate": { "required": true, "schema": { "type": "integer" } } },
                "content": { "application/json": { "schema": { "type": "string" } } } } } }),
                none,
            ),
            split_at: "/paths/~1pet/get/responses/200/headers/X-Rate",
            rejects: false,
        },
    ]
}

/// A value moved out of the root, as `(label, root reference, files to write)`.
type Twin = (&'static str, String, Vec<(&'static str, serde_json::Value)>);

const CHAINED: &str = "a two-hop chain through a Reference";
const IN_ROOT: &str = "#/x-shared/item";

/// The ways a value can be moved out of the root document.
fn placement_twins(value: &serde_json::Value) -> Vec<Twin> {
    vec![
        (
            "a whole-file $ref",
            "./fragment.json".to_owned(),
            vec![("fragment.json", value.clone())],
        ),
        (
            "a JSON Pointer into a file",
            "./fragment.json#/shared/item".to_owned(),
            vec![(
                "fragment.json",
                serde_json::json!({ "shared": { "item": value.clone() } }),
            )],
        ),
        // A fragment is a URI fragment, so a key holding a space is percent-encoded in it, and one
        // holding a `/` is `~1`-escaped as well. Lowering decodes both; validation must address
        // the same node, or the target is lowered without ever being validated.
        (
            "a percent-encoded JSON Pointer into a file",
            "./fragment.json#/shared/an%20item~1b".to_owned(),
            vec![(
                "fragment.json",
                serde_json::json!({ "shared": { "an item/b": value.clone() } }),
            )],
        ),
        // A pointer into the root's own specification extension: the one place in the root the
        // whole-document validation admits anything at all, so the target must be validated at
        // the position its reference implies there too. The root rewrite below moves the value in.
        (
            "a JSON Pointer into the root document's own extension",
            IN_ROOT.to_owned(),
            Vec::new(),
        ),
        (
            CHAINED,
            "./hop.json".to_owned(),
            vec![
                ("hop.json", serde_json::json!({ "$ref": "./fragment.json" })),
                ("fragment.json", value.clone()),
            ],
        ),
    ]
}

/// The generated client with its provenance header dropped: the header stamps the input's
/// fingerprint, which differs between an inline document and its split twins by construction.
fn client_body(client: &str) -> &str {
    client.find("\n\n").map_or(client, |end| &client[end..])
}

fn distinct_codes(report: &Report) -> Vec<&'static str> {
    let mut codes = codes(report);
    codes.dedup();
    codes
}

/// An inline document and each of its `$ref`-split twins reach the same verdict under the same
/// diagnostic codes, through both `generate` and `check` — and, where the document is valid, the
/// same generated client. The verdict alone let a two-hop chain lower its intermediate Reference
/// Object as the target (#274): a Parameter lost its `in` and was rejected, while a Response,
/// Request Body or Header lost its content and schema and generated a client without them.
#[test]
fn a_construct_reaches_the_same_verdict_inline_and_behind_a_ref() {
    let mut divergent = Vec::new();
    for fixture in placement_fixtures() {
        let (inline_generated, inline_checked, inline_client) =
            run_placement_with_client(&[("openapi.json", fixture.document.clone())]);
        assert_eq!(
            inline_generated.outcome() == Outcome::Rejected,
            fixture.rejects,
            "{}: inline: {inline_generated:#?}",
            fixture.name
        );
        if fixture.rejects {
            assert!(
                has_code(&inline_generated, Code::InvalidInput),
                "{}: inline: {inline_generated:#?}",
                fixture.name
            );
        }
        let moved = fixture
            .document
            .pointer(fixture.split_at)
            .unwrap_or_else(|| panic!("{}: nothing at {}", fixture.name, fixture.split_at))
            .clone();
        for (label, reference, mut files) in placement_twins(&moved) {
            // A valid Path Item is the one position whose chain lowering does not follow: chained
            // Path Item references are rejected with `E004` by design (#135), and
            // `e004_a_chained_path_item_ref_is_rejected_at_the_path` pins that verdict.
            if label == CHAINED && !fixture.rejects && fixture.split_at == "/paths/~1pet" {
                continue;
            }
            let mut root = fixture.document.clone();
            *root.pointer_mut(fixture.split_at).unwrap() = serde_json::json!({ "$ref": reference });
            if reference == IN_ROOT {
                root.as_object_mut().unwrap().insert(
                    "x-shared".to_owned(),
                    serde_json::json!({ "item": moved.clone() }),
                );
            }
            files.push(("openapi.json", root));
            let (generated, checked, client) = run_placement_with_client(&files);
            if !fixture.rejects && client_body(&client) != client_body(&inline_client) {
                divergent.push(format!(
                    "{} through {label}: `generate` emitted a different client than inline\n\
                     split:\n{client}\ninline:\n{inline_client}",
                    fixture.name,
                ));
            }
            for (entry, inline, split) in [
                ("generate", &inline_generated, &generated),
                ("check", &inline_checked, &checked),
            ] {
                if split.outcome() != inline.outcome()
                    || distinct_codes(split) != distinct_codes(inline)
                {
                    divergent.push(format!(
                        "{} through {label}: `{entry}` reached {:?} {:?}, inline {:?} {:?}\n\
                         split: {split:#?}",
                        fixture.name,
                        split.outcome(),
                        distinct_codes(split),
                        inline.outcome(),
                        distinct_codes(inline),
                    ));
                }
            }
        }
    }
    // Collected rather than asserted one at a time, so a regression names every placement it
    // reaches rather than the first.
    assert!(divergent.is_empty(), "{}", divergent.join("\n\n"));
}

/// Where a violation in a referenced file is reported: at the offending node's pointer *within the
/// file it is written in* — not at the reference, and not at a root-document pointer that does not
/// exist — with a message naming the reference that reached it and the definition it was held to.
#[test]
fn a_violation_behind_a_ref_is_sited_in_the_file_that_holds_it() {
    let root = placement_document(
        "3.1.0",
        serde_json::json!({ "operationId": "getPet", "responses": {
        "200": { "$ref": "./fragment.json#/shared/item" } } }),
        serde_json::json!({}),
    );
    let fragment = serde_json::json!({ "shared": { "item": { "description": "ok", "bogus": 1 } } });
    let (generated, checked) =
        run_placement(&[("openapi.json", root), ("fragment.json", fragment)]);
    for report in [&generated, &checked] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        let sited: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == Code::InvalidInput)
            .collect();
        assert_eq!(sited.len(), 1, "{report:#?}");
        assert_eq!(sited[0].pointer.as_str(), "/shared/item", "{report:#?}");
        assert!(
            sited[0]
                .message
                .contains("reached through `$ref: ./fragment.json#/shared/item`")
                && sited[0].message.contains("validated as `response`"),
            "{report:#?}"
        );
    }
}

/// A Reference cycle across files (`r.json#/A` → `#/B` → `#/A`) terminates. Validation follows a
/// chain hop by hop through a queue whose only stop is the set of targets already validated at a
/// location, so a cycle must end at the first repeat rather than spin forever. Every hop is a valid
/// Reference Object, so validation itself reports nothing; the verdict on the cycle is lowering's,
/// and is deliberately not pinned here.
#[test]
fn a_cross_file_reference_cycle_terminates() {
    let root = placement_document(
        "3.1.0",
        serde_json::json!({ "operationId": "getPet", "responses": {
        "200": { "$ref": "./r.json#/A" } } }),
        serde_json::json!({}),
    );
    let cycle = serde_json::json!({ "A": { "$ref": "#/B" }, "B": { "$ref": "#/A" } });
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(run_placement(&[("openapi.json", root), ("r.json", cycle)]));
    });
    let (generated, checked) = receiver
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("a cross-file Reference cycle did not terminate within 60 s");
    for report in [&generated, &checked] {
        assert!(!has_code(report, Code::InvalidInput), "{report:#?}");
    }
}

/// A Parameter, Request Body, Response or Header chain through the bundle is followed hop by hop
/// from the file each hop is written in (#274), and its cycle check keys on the target each hop
/// resolves to, not on how the hop is spelled. A cross-file cycle is `E004`'s cycle case in that
/// kind's words; one relative spelling written in two directories names two targets, so a chain
/// through both is followed to the object rather than reported as a cycle.
#[test]
fn e004_a_chained_object_reference_is_followed_by_target_not_by_spelling() {
    let placed = |split_at: &str| {
        placement_fixtures()
            .into_iter()
            .find(|fixture| !fixture.rejects && fixture.split_at == split_at)
            .unwrap_or_else(|| panic!("no valid placement fixture at {split_at}"))
    };
    for (kind, split_at) in [
        ("parameter", "/paths/~1pet/get/parameters/0"),
        ("request body", "/paths/~1pet/get/requestBody"),
        ("response", "/paths/~1pet/get/responses/200"),
        ("header", "/paths/~1pet/get/responses/200/headers/X-Rate"),
    ] {
        let fixture = placed(split_at);
        let moved = fixture.document.pointer(split_at).unwrap().clone();
        let split = |reference: &str| {
            let mut root = fixture.document.clone();
            *root.pointer_mut(split_at).unwrap() = serde_json::json!({ "$ref": reference });
            root
        };

        let (generated, checked) = run_placement(&[
            ("openapi.json", split("./c.json#/A")),
            (
                "c.json",
                serde_json::json!({ "A": { "$ref": "#/B" }, "B": { "$ref": "#/A" } }),
            ),
        ]);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{kind}/{entry}: {report:#?}"
            );
            assert!(
                messages_for(report, Code::UnresolvedRef)
                    .iter()
                    .any(|m| *m == format!("{kind} reference cycle cannot be resolved")),
                "{kind}/{entry}: a cross-file cycle must say it is a cycle: {report:#?}"
            );
        }

        let temp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
        for sub in ["a", "b"] {
            std::fs::create_dir(dir.join(sub)).unwrap();
        }
        for (name, value) in [
            ("openapi.json", split("./a/hop.json")),
            ("a/hop.json", serde_json::json!({ "$ref": "./p.json" })),
            ("a/p.json", serde_json::json!({ "$ref": "../b/hop.json" })),
            ("b/hop.json", serde_json::json!({ "$ref": "./p.json" })),
            ("b/p.json", moved.clone()),
        ] {
            std::fs::write(dir.join(name), serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        }
        let generated = run_generate(&build(dir.join("openapi.json"), dir.join("client.rs")));
        let client = std::fs::read_to_string(dir.join("client.rs")).unwrap_or_default();
        let checked = run_check(&Spec::new(dir.join("openapi.json")));
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{kind}/{entry}: {report:#?}"
            );
            assert!(
                !has_code(report, Code::UnresolvedRef),
                "{kind}/{entry}: two `./p.json`s in two directories are not a cycle: {report:#?}"
            );
        }
        let (_, _, inline) = run_placement_with_client(&[("openapi.json", fixture.document)]);
        assert_eq!(
            client_body(&client),
            client_body(&inline),
            "{kind}: the chain must reach the object inline declares"
        );
    }
}

/// One `#/components/<kind>/` position lowering resolves itself (#397): a valid inline document,
/// the pointer of the object to move out of it, the `components` key of its kind, and a decoy of
/// that kind whose client differs from the moved object's.
struct ComponentScope {
    kind: &'static str,
    document: serde_json::Value,
    split_at: &'static str,
    key: &'static str,
    decoy: serde_json::Value,
}

fn component_scopes() -> Vec<ComponentScope> {
    use serde_json::json;
    let placed = |split_at: &str| {
        placement_fixtures()
            .into_iter()
            .find(|fixture| !fixture.rejects && fixture.split_at == split_at)
            .unwrap_or_else(|| panic!("no valid placement fixture at {split_at}"))
            .document
    };
    let integer_text = json!({ "text/plain": { "schema": { "type": "integer" } } });
    vec![
        ComponentScope {
            kind: "parameter",
            document: placed("/paths/~1pet/get/parameters/0"),
            split_at: "/paths/~1pet/get/parameters/0",
            key: "parameters",
            decoy: json!({ "name": "rootOnly", "in": "header", "schema": { "type": "string" } }),
        },
        ComponentScope {
            kind: "request body",
            document: placed("/paths/~1pet/get/requestBody"),
            split_at: "/paths/~1pet/get/requestBody",
            key: "requestBodies",
            decoy: json!({ "content": integer_text }),
        },
        ComponentScope {
            kind: "response",
            document: placed("/paths/~1pet/get/responses/200"),
            split_at: "/paths/~1pet/get/responses/200",
            key: "responses",
            decoy: json!({ "description": "decoy", "content": integer_text }),
        },
        ComponentScope {
            kind: "header",
            document: placed("/paths/~1pet/get/responses/200/headers/X-Rate"),
            split_at: "/paths/~1pet/get/responses/200/headers/X-Rate",
            key: "headers",
            decoy: json!({ "schema": { "type": "boolean" } }),
        },
        ComponentScope {
            kind: "Media Type Object",
            document: placement_document(
                "3.2.0",
                json!({ "operationId": "getPet", "responses": { "200": { "description": "ok",
                "content": { "application/json": { "schema": { "type": "string" } } } } } }),
                json!({}),
            ),
            split_at: "/paths/~1pet/get/responses/200/content/application~1json",
            key: "mediaTypes",
            decoy: json!({ "schema": { "type": "integer" } }),
        },
    ]
}

impl ComponentScope {
    /// The inline document with `name` declared in the root's own `components.<key>`.
    fn with_root_component(&self, name: &str, value: &serde_json::Value) -> serde_json::Value {
        let mut document = self.document.clone();
        document
            .as_object_mut()
            .unwrap()
            .entry("components")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .unwrap()
            .insert(self.key.to_owned(), serde_json::json!({ name: value }));
        document
    }

    /// `document` with the object at `split_at` replaced by a `$ref` to `reference`.
    fn referencing(&self, document: &serde_json::Value, reference: &str) -> serde_json::Value {
        let mut document = document.clone();
        *document.pointer_mut(self.split_at).unwrap() = serde_json::json!({ "$ref": reference });
        document
    }

    /// The moved object.
    fn moved(&self) -> serde_json::Value {
        self.document.pointer(self.split_at).unwrap().clone()
    }
}

/// A `#/components/<kind>/` reference written inside a referenced file addresses that file's own
/// components, as a JSON Pointer fragment addresses the document it appears in (#397). Header,
/// Parameter, Request Body, Response and Media Type references read the **root** document's
/// components instead: a sub-file chain `A` → `#/components/<kind>/B` was rejected with `E004`
/// when the root declared no `B`, and silently generated the root's `B` when it did. Both reach
/// the client the inline document generates, through `generate` and `check` alike — and the root's
/// decoy `B`, read in its place, is shown to generate a different one, so the comparison can fail.
#[test]
fn a_component_ref_in_a_sub_file_reads_that_files_components() {
    let mut divergent = Vec::new();
    for scope in component_scopes() {
        let kind = scope.kind;
        let reference = format!("./other.json#/components/{}/A", scope.key);
        let other = serde_json::json!({ "components": { scope.key: {
            "A": { "$ref": format!("#/components/{}/B", scope.key) },
            "B": scope.moved(),
        } } });
        for (label, inline) in [
            ("the root declares no `B`", scope.document.clone()),
            (
                "the root declares a decoy `B`",
                scope.with_root_component("B", &scope.decoy),
            ),
        ] {
            let (_, _, inline_client) =
                run_placement_with_client(&[("openapi.json", inline.clone())]);
            assert!(!inline_client.is_empty(), "{kind}/{label}: inline");
            if label.contains("decoy") {
                let (_, _, decoyed) = run_placement_with_client(&[(
                    "openapi.json",
                    scope.referencing(&inline, &format!("#/components/{}/B", scope.key)),
                )]);
                assert_ne!(
                    client_body(&decoyed),
                    client_body(&inline_client),
                    "{kind}: the decoy must generate a different client than the moved object"
                );
            }
            let (generated, checked, client) = run_placement_with_client(&[
                ("openapi.json", scope.referencing(&inline, &reference)),
                ("other.json", other.clone()),
            ]);
            for (entry, report) in [("generate", &generated), ("check", &checked)] {
                if report.outcome() == Outcome::Rejected || has_code(report, Code::UnresolvedRef) {
                    divergent.push(format!("{kind}/{label}/{entry}: {report:#?}"));
                }
            }
            if client_body(&client) != client_body(&inline_client) {
                divergent.push(format!(
                    "{kind}/{label}: the sub-file's `B` must be the one generated\n\
                     split:\n{client}\ninline:\n{inline_client}"
                ));
            }
        }
    }
    assert!(divergent.is_empty(), "{}", divergent.join("\n\n"));
}

/// The other direction of #397: a `#/components/<kind>/B` written inside a referenced file that
/// declares no `B` is unresolved, even where the root document declares one — the fragment names
/// the file it is written in, so reading the root's would answer a reference nobody wrote. It is
/// `E004`'s absent-target case in that kind's words, and its remedy says how to reach the root's.
#[test]
fn e004_a_component_ref_in_a_sub_file_does_not_read_the_roots_components() {
    for scope in component_scopes() {
        let kind = scope.kind;
        let root = scope.referencing(
            &scope.with_root_component("B", &scope.moved()),
            &format!("./other.json#/components/{}/A", scope.key),
        );
        let other = serde_json::json!({ "components": { scope.key: {
            "A": { "$ref": format!("#/components/{}/B", scope.key) },
        } } });
        let (generated, checked) = run_placement(&[("openapi.json", root), ("other.json", other)]);
        let wanted = format!(
            "{kind} reference target `#/components/{}/B` was not found in the input bundle",
            scope.key
        );
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{kind}/{entry}: {report:#?}"
            );
            let found: Vec<_> = report
                .diagnostics()
                .iter()
                .filter(|d| d.code == Code::UnresolvedRef && d.message == wanted)
                .collect();
            assert_eq!(found.len(), 1, "{kind}/{entry}: {report:#?}");
            assert!(
                found[0].remedy.as_deref().is_some_and(
                    |remedy| remedy.contains("the root document declares `#/components/")
                ),
                "{kind}/{entry}: the remedy must point at the root's declaration: {report:#?}"
            );
        }
    }
}

/// A Media Type Object chain's cycle check keys on the target each hop resolves to (#397), as the
/// Parameter, Request Body, Response and Header chains' do. A real cycle — through the root's
/// `components.mediaTypes`, or across a referenced file — is `E004`'s cycle case in Media Type
/// words. `#/components/mediaTypes/A` written in the root and again in a sub-file names two
/// targets, so a chain through both is followed to the sub-file's `A` rather than reported as a
/// cycle, and reaches the client the inline document generates.
#[test]
fn e004_a_media_type_chain_is_followed_by_target_not_by_spelling() {
    use serde_json::json;
    let with_content = |content: serde_json::Value, extra: serde_json::Value| {
        placement_document(
            "3.2.0",
            json!({ "operationId": "getPet", "responses": { "200": { "description": "ok",
            "content": { "application/json": content } } } }),
            extra,
        )
    };

    for (label, files) in [
        (
            "through the root's components",
            vec![(
                "openapi.json",
                with_content(
                    json!({ "$ref": "#/components/mediaTypes/A" }),
                    json!({ "components": { "mediaTypes": {
                        "A": { "$ref": "#/components/mediaTypes/B" },
                        "B": { "$ref": "#/components/mediaTypes/A" },
                    } } }),
                ),
            )],
        ),
        (
            "across a referenced file",
            vec![
                (
                    "openapi.json",
                    with_content(json!({ "$ref": "./c.json#/A" }), json!({})),
                ),
                (
                    "c.json",
                    json!({ "A": { "$ref": "#/B" }, "B": { "$ref": "#/A" } }),
                ),
            ],
        ),
    ] {
        let (generated, checked) = run_placement(&files);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "{label}/{entry}: {report:#?}"
            );
            assert!(
                messages_for(report, Code::UnresolvedRef)
                    .contains(&"media type reference cycle cannot be resolved"),
                "{label}/{entry}: a media type cycle must say it is a cycle: {report:#?}"
            );
        }
    }

    // `#/components/mediaTypes/A` is written twice: in the root (naming the root's `A`, which
    // hops to the sub-file) and in the sub-file (naming the sub-file's own `A`, the object).
    let root = with_content(
        json!({ "$ref": "#/components/mediaTypes/A" }),
        json!({ "components": { "mediaTypes": {
            "A": { "$ref": "./other.json#/components/mediaTypes/B" },
        } } }),
    );
    let other = json!({ "components": { "mediaTypes": {
        "B": { "$ref": "#/components/mediaTypes/A" },
        "A": { "schema": { "type": "integer" } },
    } } });
    let (generated, checked, client) =
        run_placement_with_client(&[("openapi.json", root), ("other.json", other)]);
    for (entry, report) in [("generate", &generated), ("check", &checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(report, Code::UnresolvedRef),
            "{entry}: one spelling written in two files is not a cycle: {report:#?}"
        );
    }
    let (_, _, inline) = run_placement_with_client(&[(
        "openapi.json",
        with_content(json!({ "schema": { "type": "integer" } }), json!({})),
    )]);
    assert!(!inline.is_empty(), "the inline document must generate");
    assert_eq!(
        client_body(&client),
        client_body(&inline),
        "the chain must reach the sub-file's `A`"
    );
}

/// A `summary`/`description` on a Reference Object written in a sub-file, at the second hop of a
/// Parameter, Request Body or Response chain, is `W011`'s reference-docs case, located in that
/// sub-file. Before bundle chains were followed (#274) the second hop was parsed as the object and
/// its `summary` never read; following it makes the hop a reference site like the first, so its
/// override is reported rather than dropped, and generation still succeeds.
#[test]
fn w011_a_documented_second_hop_reference_in_a_sub_file_is_reported_there() {
    for split_at in [
        "/paths/~1pet/get/parameters/0",
        "/paths/~1pet/get/requestBody",
        "/paths/~1pet/get/responses/200",
    ] {
        let fixture = placement_fixtures()
            .into_iter()
            .find(|fixture| !fixture.rejects && fixture.split_at == split_at)
            .unwrap_or_else(|| panic!("no valid placement fixture at {split_at}"));
        let moved = fixture.document.pointer(split_at).unwrap().clone();
        let mut root = fixture.document.clone();
        *root.pointer_mut(split_at).unwrap() =
            serde_json::json!({ "$ref": "./hop.json", "summary": "first hop" });
        let (generated, checked) = run_placement(&[
            ("openapi.json", root),
            (
                "hop.json",
                serde_json::json!({ "$ref": "./p.json", "description": "second hop" }),
            ),
            ("p.json", moved),
        ]);
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{split_at}/{entry}: {report:#?}"
            );
            let site = |target: &str| {
                let wanted = format!("the `summary`/`description` on the reference to `{target}`");
                let found: Vec<_> = report
                    .diagnostics()
                    .iter()
                    .filter(|d| d.code == Code::DeclarationHasNoEffect)
                    .filter(|d| d.message.starts_with(&wanted))
                    .collect();
                assert_eq!(
                    found.len(),
                    1,
                    "{split_at}/{entry}: exactly one W011 for the hop to `{target}`: {report:#?}"
                );
                found[0].span.unwrap_or_else(|| {
                    panic!("{split_at}/{entry}: the hop to `{target}` has no span: {report:#?}")
                })
            };
            let first = site("./hop.json");
            let second = site("./p.json");
            assert_ne!(
                first.file, second.file,
                "{split_at}/{entry}: the second hop is written in `hop.json`, not the root: \
                 {report:#?}"
            );
            // `hop.json` is pretty-printed: its Reference Object opens on line 1.
            assert_eq!(
                second.start.line, 1,
                "{split_at}/{entry}: the second hop's W011 points into `hop.json`: {report:#?}"
            );
        }
    }
}

/// A `$ref` that names the root document through its own file is the root, however the path is
/// spelled (#220). The bundle once compared paths as written, so `sub/../openapi.yaml` loaded the
/// root a second time under a second file id: every component was emitted twice, and `W011`
/// reported the root as shadowing a declaration in `sub/../openapi.yaml`, which is the root. The
/// bare `spargen check openapi.yaml` spelling from the issue is driven through the binary in
/// `cli.rs`, since only a child process owns its working directory.
#[test]
fn a_reference_naming_the_root_by_another_spelling_is_the_root() {
    let spec = |reference: &str| {
        format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
servers: [{{ url: 'https://e.com' }}]
paths:
  /n:
    get:
      operationId: getN
      responses:
        '200':
          description: ok
          content:
            application/json: {{ schema: {{ $ref: '#/components/schemas/Node' }} }}
components:
  schemas:
    Node:
      type: object
      properties:
        parent: {{ $ref: '#/components/schemas/MaybeNode' }}
    MaybeNode:
      oneOf:
        - $ref: '{reference}#/components/schemas/Node'
        - type: 'null'
"##
        )
    };
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    std::fs::create_dir(dir.join("sub")).unwrap();
    let root = dir.join("openapi.yaml");
    for reference in [
        "./openapi.yaml".to_owned(),
        "openapi.yaml".to_owned(),
        "sub/../openapi.yaml".to_owned(),
        root.to_string(),
    ] {
        std::fs::write(&root, spec(&reference)).unwrap();
        let out = dir.join("client.rs");
        let generated = run_generate(&build(root.clone(), out.clone()));
        let code = std::fs::read_to_string(&out).unwrap_or_default();
        let checked = run_check(&Spec::new(root.clone()));
        for (entry, report) in [("generate", &generated), ("check", &checked)] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{reference}/{entry}: {report:#?}"
            );
            assert!(
                !has_code(report, Code::DeclarationHasNoEffect),
                "{reference}/{entry}: the root cannot shadow itself: {report:#?}"
            );
        }
        let nodes = code.matches("pub struct Node").count();
        assert_eq!(
            nodes, 1,
            "{reference}: `Node` emitted {nodes} times, so the root was loaded twice: {code}"
        );
    }
}

/// `generate_and_check_files` over `root` and `lib.yaml`, as `(entry, report)` pairs.
fn root_and_lib(root: &str, lib: &str) -> [(&'static str, Report); 2] {
    let (generated, checked, _) =
        generate_and_check_files(&[("openapi.yaml", root), ("lib.yaml", lib)]);
    [("generate", generated), ("check", checked)]
}

/// A reachable `A` whose only content under test sits at `site` (indented as a schema keyword of
/// `A`), with `Present` and `Wrapper` declared so a control can name something that exists.
fn validation_ref_spec(site: &str) -> String {
    format!(
        "openapi: 3.1.0\n\
         info: {{ title: T, version: 1.0.0 }}\n\
         servers: [{{ url: 'https://e.com' }}]\n\
         paths:\n  \
         /a:\n    \
         get:\n      \
         operationId: getA\n      \
         responses:\n        \
         '200':\n          \
         description: ok\n          \
         content:\n            \
         application/json:\n              \
         schema: {{ $ref: '#/components/schemas/A' }}\n\
         components:\n  \
         schemas:\n    \
         Present: {{ type: string }}\n    \
         Wrapper:\n      \
         type: object\n      \
         properties:\n        \
         w: {{ type: integer }}\n    \
         A:\n      \
         type: object\n\
         {site}"
    )
}

/// The keyword positions lowering never reads (#424): `not`, `if`/`then`/`else`, `contains`,
/// `propertyNames`, `unevaluated*`, `dependentSchemas`, an unconsumed `contentSchema`, and an
/// unreferenced `$defs` entry, each at depth one and nested inside another such subtree. `{target}`
/// is the reference each one carries.
fn unlowered_ref_sites(target: &str) -> Vec<(&'static str, String, String)> {
    let at = "/components/schemas/A";
    vec![
        (
            "not",
            format!("      not: {{ $ref: '{target}' }}\n"),
            format!("{at}/not"),
        ),
        (
            "if",
            format!("      if: {{ $ref: '{target}' }}\n"),
            format!("{at}/if"),
        ),
        (
            "then",
            format!("      then: {{ $ref: '{target}' }}\n"),
            format!("{at}/then"),
        ),
        (
            "else",
            format!("      else: {{ $ref: '{target}' }}\n"),
            format!("{at}/else"),
        ),
        (
            "contains",
            format!(
                "      properties:\n        l:\n          type: array\n          items: {{ type: string }}\n          contains: {{ $ref: '{target}' }}\n"
            ),
            format!("{at}/properties/l/contains"),
        ),
        (
            "propertyNames",
            format!("      propertyNames: {{ $ref: '{target}' }}\n"),
            format!("{at}/propertyNames"),
        ),
        (
            "unevaluatedProperties",
            format!("      unevaluatedProperties: {{ $ref: '{target}' }}\n"),
            format!("{at}/unevaluatedProperties"),
        ),
        (
            "unevaluatedItems",
            format!(
                "      properties:\n        l:\n          type: array\n          unevaluatedItems: {{ $ref: '{target}' }}\n"
            ),
            format!("{at}/properties/l/unevaluatedItems"),
        ),
        (
            "dependentSchemas",
            format!("      dependentSchemas:\n        x: {{ $ref: '{target}' }}\n"),
            format!("{at}/dependentSchemas/x"),
        ),
        (
            "contentSchema",
            format!(
                "      properties:\n        s:\n          type: string\n          contentMediaType: application/json\n          contentSchema: {{ $ref: '{target}' }}\n"
            ),
            format!("{at}/properties/s/contentSchema"),
        ),
        (
            "an unreferenced $defs entry",
            format!("      $defs:\n        D: {{ $ref: '{target}' }}\n"),
            format!("{at}/$defs/D"),
        ),
        (
            "a property inside not",
            format!(
                "      not:\n        properties:\n          y: {{ $ref: '{target}' }}\n"
            ),
            format!("{at}/not/properties/y"),
        ),
        (
            "an allOf member inside if",
            format!("      if:\n        allOf:\n          - {{ $ref: '{target}' }}\n"),
            format!("{at}/if/allOf/0"),
        ),
        (
            "a not inside contains",
            format!(
                "      properties:\n        l:\n          type: array\n          contains:\n            not: {{ $ref: '{target}' }}\n"
            ),
            format!("{at}/properties/l/contains/not"),
        ),
    ]
}

/// #424: a `$ref` under a keyword lowering never reads was never resolved, so a dangling one
/// audited clean beside only the parent's `W001`, where the same reference under `properties` is
/// `E004`. The subschema is still not lowered, but its references are resolved, and one naming
/// nothing is `E004` at the reference through both entry points — whether it is a component name
/// the document does not declare or a pointer into a file that holds nothing there.
#[test]
fn a_dangling_ref_under_a_validation_only_keyword_is_e004() {
    for target in [
        "#/components/schemas/Missing",
        "#/components/schemas/A/nothing",
    ] {
        for (what, site, pointer) in unlowered_ref_sites(target) {
            let spec = validation_ref_spec(&site);
            for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
                assert_eq!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{target} under {what} through {entry}: {report:#?}"
                );
                assert!(
                    report
                        .diagnostics()
                        .iter()
                        .any(|d| d.code == Code::UnresolvedRef && d.pointer.as_str() == pointer),
                    "{target} under {what} through {entry}: E004 must sit at {pointer}: \
                     {report:#?}"
                );
            }
        }
    }
}

/// The control for #424: the same positions naming a schema that exists — a root component, a
/// pointer into the document, a sub-file — still resolve and generate with no `E004`, and the
/// subschema is still not lowered, so the emitted types are those of the document without it.
#[test]
fn a_resolvable_ref_under_a_validation_only_keyword_stays_clean() {
    let (_, base_code) = generate_with_code(&validation_ref_spec(""));
    let base_types = types_module(&base_code);
    for target in [
        "#/components/schemas/Present",
        "#/components/schemas/Wrapper/properties/w",
        "#/components/schemas/A",
    ] {
        for (what, site, _) in unlowered_ref_sites(target) {
            let spec = validation_ref_spec(&site);
            for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
                assert_ne!(
                    report.outcome(),
                    Outcome::Rejected,
                    "{target} under {what} through {entry}: {report:#?}"
                );
                assert!(
                    !has_code(&report, Code::UnresolvedRef),
                    "{target} under {what} through {entry}: {report:#?}"
                );
            }
            // `contains`/`unevaluatedItems` add an array property and `contentSchema` a string
            // one, so only the positions that add no shape are compared with the baseline.
            if !site.contains("properties:") {
                let (_, code) = generate_with_code(&spec);
                assert_eq!(
                    types_module(&code),
                    base_types,
                    "{target} under {what} changed the emitted types, so it was lowered"
                );
            }
        }
    }
}

/// #424 across files: a target reached only through `not` is never lowered, so its own references
/// are resolved by following it — a dangling one inside it is `E004` at that reference, in the
/// file it is written in. A cycle of such targets terminates. A bare `#/components/schemas/<name>`
/// written in a sub-file names the root's component when the root declares one, as lowering reads
/// it, so it is not reported against the sub-file.
#[test]
fn a_ref_under_a_validation_only_keyword_is_followed_into_other_files() {
    let root = validation_ref_spec("      not: { $ref: './lib.yaml#/Outer' }\n");

    let dangling = "Outer:\n  type: object\n  properties:\n    y: { $ref: '#/Gone' }\n";
    for (entry, report) in root_and_lib(&root, dangling) {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|d| d.code == Code::UnresolvedRef
                    && d.pointer.as_str() == "/Outer/properties/y"),
            "{entry}: E004 must sit at lib.yaml's /Outer/properties/y: {report:#?}"
        );
    }

    let resolvable = "Outer:\n  \
                      type: object\n  \
                      properties:\n    \
                      y: { $ref: '#/Inner' }\n    \
                      z: { $ref: '#/components/schemas/Present' }\n\
                      Inner:\n  \
                      not: { $ref: '#/Outer' }\n";
    for (entry, report) in root_and_lib(&root, resolvable) {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(&report, Code::UnresolvedRef),
            "{entry}: {report:#?}"
        );
    }

    // A bare `#/components/schemas/<name>` written in a sub-file that the sub-file declares and
    // the root does not addresses the sub-file, as lowering reads it: only a reference written in
    // the root is a missing root component when the root does not declare its name.
    let local_component = "Outer:\n  \
                           type: object\n  \
                           properties:\n    \
                           y: { $ref: '#/components/schemas/Local' }\n\
                           components:\n  \
                           schemas:\n    \
                           Local: { type: string }\n";
    for (entry, report) in root_and_lib(&root, local_component) {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            !has_code(&report, Code::UnresolvedRef),
            "{entry}: a sub-file's own component is not a missing root component: {report:#?}"
        );
    }

    let missing_file = validation_ref_spec("      not: { $ref: './absent.yaml#/Outer' }\n");
    for (entry, report) in [
        ("generate", generate(&missing_file)),
        ("check", check(&missing_file)),
    ] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
    }
}

/// Whether `report` carries `code` at `pointer`.
fn has_code_at(report: &Report, code: Code, pointer: &str) -> bool {
    report
        .diagnostics()
        .iter()
        .any(|d| d.code == code && d.pointer.as_str() == pointer)
}

/// #446: the audit walked only the schemas written in the root document, so a schema lowering
/// reaches through a `$ref` into another file got no `W001` for its validation-only keywords, and
/// a dangling reference under its `not` audited clean. The issue's reproducer: `check` printed
/// `clean`. The audit now follows a reference it meets in a position lowering reads into the file
/// it names, so the sub-file's schema gets the audit a root one does, through both entry points.
#[test]
fn a_schema_in_a_referenced_file_gets_the_audit_a_root_schema_does() {
    let reproducer = "X:\n  type: object\n  maxProperties: 3\n  not: { $ref: '#/Missing' }\n";
    let (generated, checked, _) = split("./lib.yaml#/X", reproducer);
    for (entry, report) in [("generate", generated), ("check", checked)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            has_code_at(&report, Code::ValidationKeywordIgnored, "/X"),
            "{entry}: lib.yaml's /X carries `maxProperties` and `not`, so W001: {report:#?}"
        );
        assert!(
            has_code_at(&report, Code::UnresolvedRef, "/X/not"),
            "{entry}: `#/Missing` names nothing in lib.yaml, so E004 at /X/not: {report:#?}"
        );
    }

    // Without the dangling reference the same schema generates, still warned. A target reached
    // only through another sub-file schema's property — and a cycle back to the first — is
    // audited too, and the walk terminates.
    let chained = "X:\n  \
                   type: object\n  \
                   maxProperties: 3\n  \
                   properties:\n    \
                   y: { $ref: '#/Y' }\n\
                   Y:\n  \
                   type: object\n  \
                   properties:\n    \
                   s: { type: string, pattern: '^a' }\n    \
                   back: { $ref: '#/X' }\n";
    let (generated, checked, _) = split("./lib.yaml#/X", chained);
    for (entry, report) in [("generate", generated), ("check", checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        for pointer in ["/X", "/Y/properties/s"] {
            assert!(
                has_code_at(&report, Code::ValidationKeywordIgnored, pointer),
                "{entry}: W001 must sit at lib.yaml's {pointer}: {report:#?}"
            );
        }
    }
}

/// #495: the audit walked only the Parameter, Request Body and Response Objects written in the
/// root document, so one that is a `$ref` into another file was skipped and the schemas it holds
/// got no `W001`. The issue's reproducer — a whole-file `'200': { $ref: './resp.yaml' }` — checked
/// `clean`. The audit now follows each such reference, as lowering does, to the object its chain
/// ends at: a whole file, a pointer into a file, and a root component that is itself a reference
/// whose next hop is written in the sub-file.
#[test]
fn an_object_in_a_referenced_file_gets_the_audit_a_root_object_does() {
    const ROOT: &str = "openapi: 3.1.0\n\
                        info: { title: T, version: 1.0.0 }\n\
                        servers: [{ url: 'https://e.com' }]\n\
                        paths:\n  \
                        /a:\n    \
                        post:\n      \
                        operationId: postA\n      \
                        parameters: [{ $ref: './objects.yaml#/Param' }]\n      \
                        requestBody: { $ref: './objects.yaml#/Body' }\n      \
                        responses:\n        \
                        '200': { $ref: './resp.yaml' }\n        \
                        default: { $ref: '#/components/responses/Failure' }\n  \
                        /b:\n    \
                        get:\n      \
                        operationId: getB\n      \
                        responses:\n        \
                        '200': { $ref: './resp.yaml' }\n\
                        components:\n  \
                        responses:\n    \
                        Failure: { $ref: './objects.yaml#/Chained' }\n";
    const RESP: &str = "description: ok\n\
                        content:\n  \
                        application/json:\n    \
                        schema: { type: object, maxProperties: 3 }\n";
    const OBJECTS: &str = "Param:\n  \
                           name: q\n  \
                           in: query\n  \
                           schema: { type: string, maxLength: 3 }\n\
                           Body:\n  \
                           content:\n    \
                           application/json:\n      \
                           schema: { type: object, minProperties: 1 }\n\
                           Chained: { $ref: '#/Error' }\n\
                           Error:\n  \
                           description: err\n  \
                           content:\n    \
                           application/json:\n      \
                           schema: { type: object, maxProperties: 2 }\n";
    let (generated, checked, _) = generate_and_check_files(&[
        ("openapi.yaml", ROOT),
        ("resp.yaml", RESP),
        ("objects.yaml", OBJECTS),
    ]);
    for (entry, report) in [("generate", generated), ("check", checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        for pointer in [
            "/content/application~1json/schema",
            "/Param/schema",
            "/Body/content/application~1json/schema",
            "/Error/content/application~1json/schema",
        ] {
            assert!(
                has_code_at(&report, Code::ValidationKeywordIgnored, pointer),
                "{entry}: W001 must sit at the sub-file's {pointer}: {report:#?}"
            );
        }
        // `/a` and `/b` both reach resp.yaml: its schema is audited once.
        let resp_warnings = report
            .diagnostics()
            .iter()
            .filter(|d| {
                d.code == Code::ValidationKeywordIgnored
                    && d.pointer.as_str() == "/content/application~1json/schema"
            })
            .count();
        assert_eq!(resp_warnings, 1, "{entry}: {report:#?}");
    }

    // A response chain that returns to itself is lowering's `E004`; the audit's walk terminates.
    let (generated, checked, _) = generate_and_check_files(&[
        ("openapi.yaml", ROOT),
        ("resp.yaml", "$ref: './resp.yaml'\n"),
        ("objects.yaml", OBJECTS),
    ]);
    for (entry, report) in [("generate", generated), ("check", checked)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        assert!(
            has_code(&report, Code::UnresolvedRef),
            "{entry}: {report:#?}"
        );
    }
}

/// #495, for the OpenAPI 3.2 Media Type Object that is itself a Reference Object: one naming
/// another file is followed to the object written there, whose schema gets `W001`. And a response
/// in another file whose `text/event-stream` envelope consumes `contentMediaType`/`contentSchema`
/// is recognised as consumed, so the walk that now reaches it does not warn about them.
#[test]
fn a_media_type_in_a_referenced_file_gets_the_audit_a_root_one_does() {
    const ROOT: &str = "openapi: 3.2.0\n\
                        info: { title: T, version: 1.0.0 }\n\
                        servers: [{ url: 'https://e.com' }]\n\
                        paths:\n  \
                        /a:\n    \
                        get:\n      \
                        operationId: getA\n      \
                        responses:\n        \
                        '200':\n          \
                        description: ok\n          \
                        content:\n            \
                        application/json: { $ref: './media.yaml' }\n  \
                        /events:\n    \
                        get:\n      \
                        operationId: getEvents\n      \
                        responses:\n        \
                        '200': { $ref: './stream.yaml' }\n";
    const MEDIA: &str = "schema: { type: string, pattern: '^a' }\n";
    const STREAM: &str = "description: ok\n\
                          content:\n  \
                          text/event-stream:\n    \
                          itemSchema:\n      \
                          type: object\n      \
                          required: [data]\n      \
                          properties:\n        \
                          data:\n          \
                          type: string\n          \
                          contentMediaType: application/json\n          \
                          contentSchema: { type: object, properties: { id: { type: string } } }\n";
    let (generated, checked, _) = generate_and_check_files(&[
        ("openapi.yaml", ROOT),
        ("media.yaml", MEDIA),
        ("stream.yaml", STREAM),
    ]);
    for (entry, report) in [("generate", generated), ("check", checked)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let w001: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::ValidationKeywordIgnored)
            .map(|d| d.pointer.as_str())
            .collect();
        assert_eq!(
            w001,
            ["/schema"],
            "{entry}: only media.yaml's pattern warns; the SSE annotations are consumed: {report:#?}"
        );
    }
}

/// #424: a `discriminator` inside a subschema lowering never reads was never checked, so a
/// `mapping` or `defaultMapping` value naming no schema went unreported. Each is resolved as a
/// lowered discriminator's would be, and one naming nothing is `E004` at the entry; a value naming
/// a schema that exists is not.
#[test]
fn a_discriminator_under_a_validation_only_keyword_resolves_its_mapping() {
    let site = |target: &str| {
        format!(
            "      else:\n        \
             discriminator:\n          \
             propertyName: kind\n          \
             mapping:\n            \
             p: {target}\n          \
             defaultMapping: Present\n        \
             oneOf:\n          \
             - {{ $ref: '#/components/schemas/Present' }}\n"
        )
    };
    let dangling = validation_ref_spec(&site("Missing"));
    for (entry, report) in [
        ("generate", generate(&dangling)),
        ("check", check(&dangling)),
    ] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{entry}: {report:#?}");
        let e004: Vec<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code == Code::UnresolvedRef)
            .map(|d| d.pointer.as_str())
            .collect();
        assert_eq!(
            e004,
            ["/components/schemas/A/else/discriminator/mapping/p"],
            "{entry}: only the dangling mapping entry is E004: {report:#?}"
        );
    }
    for target in ["Present", "'#/components/schemas/Present'"] {
        let clean = validation_ref_spec(&site(target));
        for (entry, report) in [("generate", generate(&clean)), ("check", check(&clean))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "{target}, {entry}: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::UnresolvedRef),
                "{target}, {entry}: {report:#?}"
            );
        }
    }
}
