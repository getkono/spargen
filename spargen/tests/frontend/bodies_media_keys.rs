//! Request and response bodies, third part: `E009` outside the byte families, the `W014`
//! decode-alike rules, the media-range and media-key grammar, structured-suffix ranges, and what a
//! rejected or accepted response selection claims.

use super::*;

#[test]
fn e009_a_concrete_type_outside_the_byte_families_stays_unsupported() {
    // `application/*` mixes binary (`pdf`) with textual (`sdp`) subtypes and `font/*` is not
    // claimed, so none of them may be read as bytes on the strength of a prefix. This is the
    // response-side twin of `e009_unsupported_media_type`, and what keeps the openai corpus
    // expectation honest.
    // A family key with no subtype, or with a wildcard that does not make it a range, is not a
    // member of the family either.
    for media in [
        "application/pdf",
        "application/sdp",
        "font/woff2",
        "image/",
        "image/pn*",
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "{media}": {{ schema: {{}} }}
"##
        );
        let report = generate(&spec);
        assert_eq!(report.outcome(), Outcome::Rejected, "{media}: {report:#?}");
        assert!(
            has_code(&report, Code::UnsupportedMediaType),
            "{media}: {report:#?}"
        );
    }

    // On a request such a key would be sent verbatim as `Content-Type`, so it must be rejected
    // there too rather than read as bytes.
    for media in ["image/", "image/pn*"] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    put:
      operationId: putX
      requestBody:
        required: true
        content:
          "{media}": {{ schema: {{}} }}
      responses:
        "204": {{ description: No Content }}
"##
        );
        let report = generate(&spec);
        assert_eq!(report.outcome(), Outcome::Rejected, "{media}: {report:#?}");
        assert!(
            has_code(&report, Code::UnsupportedMediaType),
            "{media}: {report:#?}"
        );
    }
}

#[test]
fn e009_a_structured_suffix_member_of_a_byte_family_stays_unsupported() {
    // `image/svg+xml` sits in the `image` family, but its RFC 6838 `+xml` suffix says the payload
    // is a text syntax, so bytes would be the silently-wrong reading the family rule exists to
    // avoid. It stays `E009`, as it was before the family rule existed.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /logo:
    get:
      operationId: getLogo
      responses:
        "200":
          description: OK
          content:
            image/svg+xml: { schema: {} }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
}

#[test]
fn a_concrete_media_type_outranks_a_range_that_precedes_it() {
    // Ranges rank below every codec, so a concrete sibling wins wherever it sits in the document
    // (the one type ranked below the ranges is a concrete `image`/`audio`/`video` member, and only
    // on a response; pinned elsewhere). Two ranges at the same rank still tie by source order, as
    // equal-ranked concrete media already do.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /thing:
    get:
      operationId: getThing
      responses:
        "200":
          description: OK
          content:
            "*/*": { schema: {} }
            application/json: { schema: { type: object, properties: { a: { type: string } } } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    // JSON won, so the alternative that was dropped is the range — and that one really is a
    // narrowing, because a JSON body and an opaque one decode differently.
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
    assert!(
        !code.contains("pub type ResponseBody = bytes::Bytes;"),
        "{code}"
    );
}

#[test]
fn w014_is_silent_when_every_alternative_decodes_identically() {
    // The ranged-media response shape from #72, as emitted for a byte range: three keys, one
    // representation. Every opaque-octets body is `bytes::Bytes` whatever its schema says, so
    // generating one of them gives up nothing and the warning was pure noise on a common shape.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /media/{id}:
    get:
      operationId: getMedia
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      responses:
        "200":
          description: OK
          content:
            video/*: { schema: {} }
            audio/*: { schema: {} }
            application/octet-stream: { schema: {} }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        !has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_for_an_alternative_spargen_cannot_decode() {
    // The narrowing above must not swallow a real one: an unregistered media type sitting beside a
    // byte body is a documented surface that is genuinely not generated.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            application/sdp: { schema: { type: string } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn e009_a_media_range_cannot_be_a_request_content_type() {
    // A range describes what a server may return, not what a client sends. A generated request puts
    // its media key on the wire verbatim, and `Content-Type: video/*` is not a dispatchable header
    // — while choosing a concrete member of the family would be spargen inventing what the document
    // declined to say.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /upload:
    post:
      operationId: upload
      requestBody:
        required: true
        content:
          video/*: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
}

#[test]
fn w014_still_fires_when_an_octet_alternative_constrains_a_shape() {
    // The suppression may only cover alternatives that really do decode identically. An
    // octet-classified alternative carrying an object schema would be *rejected* by the octet gate,
    // not turned into bytes, so it is a genuine narrowing and must still be reported.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: {}
            video/*: { schema: { type: object, properties: { a: { type: string } } } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_when_an_alternative_is_a_referenced_media_object() {
    // A 3.2 Media Type Object may itself be a Reference Object, which parses with no `schema` of
    // its own. Reading that absence as "constrains nothing" would call the alternative opaque on
    // the strength of a field the `$ref` spelling never sets, and drop the warning for a structured
    // body — the byte-identical inline spelling reports it, so the two must agree.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { $ref: "#/components/mediaTypes/Manifest" }
components:
  mediaTypes:
    Manifest:
      schema: { type: object, properties: { a: { type: string } } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_when_an_alternative_schema_is_a_ref() {
    // The other place a reference hides. Proving what a `$ref` points at would mean resolving it
    // during selection; answering "unknown" as "not opaque" only keeps a warning that was already
    // being reported.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { schema: { $ref: "#/components/schemas/Manifest" } }
components:
  schemas:
    Manifest: { type: object, properties: { a: { type: string } } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_fires_for_text_alternatives_that_only_look_identical() {
    // The identical-decode rule stays confined to octet-stream. Two textual bodies that constrain
    // nothing look interchangeable and are not: this pair is `serde_json::Value` twice, while the
    // same pair written without `schema:` at all is `()` twice, and a mixed pair is one of each.
    // Only the octet gate collapses every body it admits onto one type.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            text/plain: { schema: {} }
            text/csv: { schema: {} }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_fires_when_two_empty_spellings_lower_differently() {
    // `constrains nothing` has two spellings that disagree outside the octet gate: no `schema` key
    // lowers to `()`, `schema: {}` lowers to `Any`. Suppressing between them would make the *order*
    // of two content keys decide the response type, in silence.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            text/plain: {}
            text/csv: { schema: {} }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
    // The selection is the one with no schema at all, so the body really is unit-typed. The
    // `schema: {}` sibling would have been `serde_json::Value`, which is the whole point: the two
    // spellings are not interchangeable, so neither may silence the other.
    assert!(
        code.contains("support::ResponseValue<()>"),
        "the no-schema selection should lower to `()`: {code}"
    );
}

#[test]
fn w014_fires_for_an_octet_request_alternative_that_decodes_alike() {
    // A request narrows at the wire even when both entries are `bytes::Bytes`: the selected media
    // key becomes the `Content-Type` verbatim, so this client only ever sends octet-stream to a
    // server documented as also accepting `video/*`. The range-as-request rejection does not cover
    // this — that fires on the media actually selected, and a suppressed alternative never is.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /u:
    post:
      operationId: postU
      requestBody:
        required: true
        content:
          application/octet-stream: { schema: {} }
          video/*: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_fires_when_an_octet_alternative_describes_itself_outside_its_schema() {
    // A Media Type Object says things outside `schema`. `itemSchema` in particular carries a type,
    // so an entry declaring one is not interchangeable with an empty body even though its `schema`
    // is empty — reading only `schema` is how that slipped through before.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { itemSchema: { type: string } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );

    // `encoding` is the other half of the same claim, and the explain text names it. It is inert on
    // an octet media, but an entry that spells it out is still saying more than an empty one.
    let with_encoding = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { encoding: { part: { contentType: text/plain } } }
"##;
    let report = generate(with_encoding);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_fires_for_sequential_alternatives_with_different_item_types() {
    // A sequential media's item type lives in `itemSchema`, outside the body schema entirely. Two
    // entries can both constrain nothing in `schema` and still stream different types.
    let spec = r##"
openapi: 3.2.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/x-ndjson: { itemSchema: { type: string } }
            application/jsonl: { itemSchema: { type: integer } }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_fires_for_request_body_alternatives_that_decode_alike() {
    // A request narrows at the wire, not at the type. The chosen media key becomes `Content-Type`
    // verbatim, so a server documented as accepting both is only ever sent one — whatever the Rust
    // types do.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /u:
    post:
      operationId: postU
      requestBody:
        required: true
        content:
          application/json: { schema: {} }
          application/vnd.acme.v2+json: { schema: {} }
      responses:
        "204": { description: No Content }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_when_the_text_selection_is_typed() {
    // The *selection* has to reach the shared representation too. A textual body carrying a string
    // enum lowers to a typed value rather than to `String`, so an opaque sibling beside it really
    // does document something the client will never accept.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            text/plain: { schema: { type: string, enum: [a, b] } }
            text/csv: { schema: {} }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn an_octet_selection_with_a_binary_schema_still_silences_an_opaque_range() {
    // Octet-stream is the one codec whose gate admits only bodies that collapse to `bytes::Bytes`,
    // so the selection does not have to be empty for the alternatives to decode identically. The
    // 3.0 spelling of a binary body beside the 3.1 one narrows nothing.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: { type: string, format: binary } }
            video/*: { schema: {} }
"##;
    let report = generate(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_when_an_alternative_carries_a_validation_keyword() {
    // A validation keyword never changes the storage type, so both bodies are `bytes::Bytes` — but
    // the document still said something about one of them, and an alternative that says something
    // is not interchangeable with one that says nothing.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { schema: { maxLength: 10 } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_when_an_alternative_is_the_always_false_schema() {
    // `false` is the one boolean schema that constrains — to nothing at all. It is not the empty
    // schema and must not be read as one.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: {} }
            video/*: { schema: false }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}

#[test]
fn w014_still_fires_for_every_kind_of_constraining_alternative() {
    // `constrains_nothing` is a long conjunction, and dropping any one term would silently widen
    // the suppression — the exhaustive destructure it is built on catches a field that is *added*
    // to `Schema`, never one that stops being consulted. Each schema below trips exactly one term,
    // so deleting that term turns this test red instead of turning a real narrowing silent.
    for constraining in [
        "{ type: object, properties: { a: { type: string } } }",
        "{ required: [a] }",
        "{ additionalProperties: false }",
        "{ patternProperties: { \"^x-\": { type: string } } }",
        "{ items: { type: string } }",
        "{ prefixItems: [{ type: string }] }",
        "{ allOf: [{ type: string }] }",
        "{ oneOf: [{ type: string }] }",
        "{ anyOf: [{ type: string }] }",
        "{ enum: [a, b] }",
        "{ const: a }",
        "{ format: uuid }",
        "{ contentEncoding: base64 }",
        "{ contentMediaType: application/json }",
        "{ maxLength: 10 }",
        "false",
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: {{ schema: {{}} }}
            video/*: {{ schema: {constraining} }}
"##
        );
        let report = generate(&spec);
        assert!(
            has_code(&report, Code::AlternativeMediaIgnored),
            "an alternative constrained by `{constraining}` was suppressed as decoding \
             identically: {report:#?}"
        );
    }
}

#[test]
fn a_binary_body_whose_schema_failed_to_lower_is_not_retyped_to_bytes() {
    // Retyping to `Bytes` applies to a body that *said nothing*. A body that declared a schema
    // which then failed to lower has already reported its own failure, and quietly handing it back
    // as bytes would turn that error into a silently different type.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            application/octet-stream: { schema: { $ref: "#/components/schemas/Loop" } }
components:
  schemas:
    Loop: { $ref: "#/components/schemas/Loop" }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnresolvedRef), "{report:#?}");
}

#[test]
fn a_media_range_is_matched_case_insensitively() {
    // Media types are case-insensitive (RFC 9110 § 8.3.1). Reading `TEXT/*` as a binary family
    // would be silently wrong rather than loudly unsupported.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /note:
    get:
      operationId: getNote
      responses:
        "200":
          description: OK
          content:
            TEXT/*: { schema: { type: string } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !code.contains("pub type ResponseBody = bytes::Bytes;"),
        "an upper-case text range is still text: {code}"
    );
}

#[test]
fn e009_a_range_naming_no_family_is_unsupported() {
    // `/*` has no type before the slash, so it names nothing. It stays unsupported rather than
    // being read as opaque octets on the strength of its suffix alone.
    let report = generate(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "/*": { schema: {} }
"##,
    );
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
}

#[test]
fn e009_a_media_range_with_an_extra_slash_is_unsupported() {
    // `a/b/*` ends in `/*`, but what precedes the suffix is `a/b`, which is not a type name. It was
    // read as the family `a/b` and generated as opaque octets. A key that is not a media range names
    // no family, so it is unsupported like any other key that is not a media type.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "a/b/*": { schema: {} }
"##;
    let report = generate(spec);
    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
}

/// Assert that each `(key, request, schema)` case, as the sole `content` key of a request body
/// (`request`) or a response, is rejected with `E009` through both `generate` and `check`.
fn assert_each_media_key_is_unsupported(cases: &[(&str, bool, &str)]) {
    fn document(key: &str, request: bool, schema: &str) -> String {
        if request {
            format!(
                r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    post:
      operationId: postX
      requestBody:
        required: true
        content:
          "{key}": {{ schema: {schema} }}
      responses:
        "204": {{ description: No Content }}
"##
            )
        } else {
            format!(
                r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "{key}": {{ schema: {schema} }}
"##
            )
        }
    }
    for &(key, request, schema) in cases {
        let spec = document(key, request, schema);
        let report = generate(&spec);
        assert_eq!(report.outcome(), Outcome::Rejected, "`{key}`: {report:#?}");
        assert!(
            has_code(&report, Code::UnsupportedMediaType),
            "`{key}`: {report:#?}"
        );
    }
}

#[test]
fn e009_a_media_key_that_is_not_a_restricted_name_is_unsupported() {
    // A media type is exactly one `/` between two RFC 6838 § 4.2 restricted names. Each key below
    // breaks that, yet most reached a codec through an arm that matched only part of the key: the
    // `text/` prefix, the `application/…+json` suffix, or a range's `/*`. `image/jpeg/extra` pins
    // the concrete binary family, which must not be read as `image` octets.
    // One byte past the 127-byte limit on a restricted name.
    let too_long = format!("text/{}", "a".repeat(128));
    assert_each_media_key_is_unsupported(&[
        ("image/jpeg/extra", true, "{}"),
        ("text/plain/extra", false, "{ type: string }"),
        ("application/vnd.a/b+json", true, "{ type: object }"),
        ("text/", false, "{ type: string }"),
        ("text/pl ain", false, "{ type: string }"),
        ("**/*", false, "{}"),
        (too_long.as_str(), false, "{ type: string }"),
    ]);
}

#[test]
fn e009_a_wildcard_inside_a_name_is_unsupported() {
    // `*` is a whole-name wildcard, never part of a name: `*/json` is no range (a range fixes the
    // type and wildcards the subtype), and `image/pn*` is no type at all. Neither ever reached an
    // arm, so `text/pl*in` and `application/vn*+json` are here to discriminate: without the rule,
    // the `text/` prefix arm and the `+json` suffix arm would accept them.
    assert_each_media_key_is_unsupported(&[
        ("*/json", false, "{ type: object }"),
        ("image/pn*", false, "{}"),
        ("text/pl*in", false, "{ type: string }"),
        ("application/vn*+json", false, "{ type: object }"),
    ]);
}

#[test]
fn e009_a_name_starting_with_a_symbol_is_unsupported() {
    // `.`, `-` and the other symbols RFC 6838 § 4.2 permits may follow the first byte of a
    // restricted name but may not be it, in the type position or the subtype position.
    assert_each_media_key_is_unsupported(&[
        (".type/x", false, "{ type: string }"),
        ("-x/y", false, "{ type: string }"),
        ("text/.plain", false, "{ type: string }"),
        ("text/-plain", false, "{ type: string }"),
    ]);
}

#[test]
fn w014_a_malformed_key_beside_a_well_formed_sibling_is_ignored() {
    // A malformed key does not reject the whole map when a well-formed sibling exists: only that
    // key is dropped, and it is named under W014 like any other alternative that is not generated.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "text/plain/extra": { schema: { type: string } }
            application/json:
              schema: { type: object, required: [id], properties: { id: { type: integer } } }
"##;
    let (report, code) = generate_with_code(spec);
    let checked = check(spec);
    for report in [&report, &checked] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(!has_code(report, Code::UnsupportedMediaType), "{report:#?}");
        assert!(
            report.diagnostics().iter().any(|diagnostic| {
                diagnostic.code == Code::AlternativeMediaIgnored
                    && diagnostic.message
                        == "`application/json` is selected; the alternative media type(s) \
                            `text/plain/extra` are not"
            }),
            "{report:#?}"
        );
    }
    assert!(code.contains("pub type ResponseBodyid = i64;"), "{code}");
}

#[test]
fn well_formed_media_keys_still_generate() {
    // The restricted-name check must not reject what real descriptions write: dotted vendor `+json`
    // types, a key with parameters (stripped before the check), both kinds of range, every
    // non-alphanumeric byte RFC 6838 permits, and a subtype at exactly the 127-byte limit.
    let at_limit = format!("text/{}", "a".repeat(127));
    let keys = [
        ("application/vnd.github+json", "{ type: object }"),
        ("application/vnd.github.v3.star+json", "{ type: object }"),
        ("text/plain; charset=utf-8", "{ type: string }"),
        ("*/*", "{}"),
        ("application/*", "{}"),
        ("text/x-a!b#c$d&e^f_g.h+i", "{ type: string }"),
        (at_limit.as_str(), "{ type: string }"),
    ];
    // One operation per key, each with a single content entry, so no key competes with another.
    let mut spec = String::from("openapi: 3.1.0\ninfo: { title: T, version: 1.0.0 }\npaths:\n");
    for (index, (key, schema)) in keys.into_iter().enumerate() {
        spec += &format!(
            r##"  /op{index}:
    get:
      operationId: op{index}
      responses:
        "200":
          description: OK
          content:
            "{key}": {{ schema: {schema} }}
"##
        );
    }
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    // Seven operations share the `ResponseBody` name, so each alias carries a disambiguating suffix.
    let opaque = code
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.starts_with("pub type ResponseBody") && line.ends_with(" = bytes::Bytes;")
        })
        .count();
    assert_eq!(
        opaque, 2,
        "exactly the `*/*` and `application/*` ranges are opaque octets: {code}"
    );
}

#[test]
fn a_structured_suffix_range_response_generates_json() {
    // `application/*+json` is a media range over every structured JSON subtype (RFC 9110's
    // media-range grammar, with RFC 6838 § 4.2.8 structured syntax suffixes). The support matrix
    // promises it as JSON, and a response offering it alone is decoded as JSON rather than
    // rejected for its `*`.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            "application/*+json":
              schema: { type: object, required: [id], properties: { id: { type: integer } } }
"##;
    let (report, code) = generate_with_code(spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(
        !has_code(&report, Code::UnsupportedMediaType),
        "{report:#?}"
    );
    assert!(
        code.contains("pub struct ResponseBody {")
            && code.contains("pub type ResponseBodyid = i64;"),
        "a typed JSON body, not bytes or text: {code}"
    );
}

#[test]
fn w014_a_structured_suffix_range_ties_its_codec_on_a_response() {
    // Unlike a family range, which every concrete sibling outranks, `application/*+json` ranks
    // level with `application/json` (the support matrix and `E009` say so). On a response nothing
    // withholds it, so the tie is broken by source order: whichever is listed first is generated,
    // and the other is reported as the alternative that is not. The two schemas differ so the
    // generated body shows which key won.
    let range = r#""application/*+json": { schema: { type: object, required: [fromRange], properties: { fromRange: { type: integer } } } }"#;
    let json = r#""application/json": { schema: { type: object, required: [fromJson], properties: { fromJson: { type: integer } } } }"#;
    for (first, second, generated, other, field, other_field) in [
        (
            range,
            json,
            "application/*+json",
            "application/json",
            "from_range",
            "from_json",
        ),
        (
            json,
            range,
            "application/json",
            "application/*+json",
            "from_json",
            "from_range",
        ),
    ] {
        let spec = format!(
            r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    get:
      operationId: getX
      responses:
        "200":
          description: OK
          content:
            {first}
            {second}
"##
        );
        let (report, code) = generate_with_code(&spec);
        assert!(
            code.contains(&format!("pub {field}: "))
                && !code.contains(&format!("pub {other_field}: ")),
            "`{generated}` listed first is generated, `{other}` is not: {code}"
        );
        let expected =
            format!("`{generated}` is selected; the alternative media type(s) `{other}` are not");
        for report in [report, check(&spec)] {
            assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
            assert!(
                report.diagnostics().iter().any(|diagnostic| {
                    diagnostic.code == Code::AlternativeMediaIgnored
                        && diagnostic.message == expected
                }),
                "{report:#?}"
            );
        }
    }
}

#[test]
fn e009_a_structured_suffix_range_cannot_be_a_request_content_type() {
    // A request puts its media key on the wire verbatim, and `Content-Type: application/*+json`
    // names a family rather than a type, exactly like `video/*`. It is rejected as a range.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /x:
    post:
      operationId: postX
      requestBody:
        required: true
        content:
          "application/*+json": { schema: { type: object } }
      responses:
        "204": { description: No Content }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            report
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == Code::UnsupportedMediaType
                    && diagnostic.message.contains("is a media range")),
            "{report:#?}"
        );
    }
}

/// A request body whose `content` lists `application/*+json` first and then `sibling`.
fn suffix_range_request_document(sibling: &str, sibling_schema: &str) -> String {
    format!(
        r##"
openapi: 3.1.0
info: {{ title: T, version: 1.0.0 }}
paths:
  /x:
    post:
      operationId: postX
      requestBody:
        required: true
        content:
          "application/*+json": {{ schema: {{ type: object }} }}
          "{sibling}": {{ schema: {sibling_schema} }}
      responses:
        "204": {{ description: No Content }}
"##
    )
}

#[test]
fn w014_a_structured_suffix_range_yields_to_a_sendable_request_sibling() {
    // `application/*+json` ranks with the concrete JSON types, so listed first it would win the
    // tie by source order and then be refused as a range, rejecting a body that offered something
    // sendable. While a concrete sibling can be sent, the range is not a candidate: the sibling is
    // generated and the range is reported as the alternative that is not, whatever the sibling's
    // rank (`text/plain` ranks below every JSON type).
    for (sibling, schema) in [
        ("application/json", "{ type: object }"),
        ("application/merge-patch+json", "{ type: object }"),
        ("text/plain", "{ type: string }"),
    ] {
        let spec = suffix_range_request_document(sibling, schema);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::UnsupportedMediaType),
                "`{sibling}` through {entry}: {report:#?}"
            );
            let expected = format!(
                "`{sibling}` is selected; the alternative media type(s) `application/*+json` are not"
            );
            assert!(
                report.diagnostics().iter().any(|diagnostic| {
                    diagnostic.code == Code::AlternativeMediaIgnored
                        && diagnostic.message == expected
                }),
                "`{sibling}` through {entry}: {report:#?}"
            );
        }
    }
}

#[test]
fn e009_a_structured_suffix_range_with_no_sendable_request_sibling_is_unsupported() {
    // With nothing beside it that a request could send, the suffix range is still what the body
    // offers, and it is still refused as a range. Neither another range nor a streaming media
    // counts as sendable.
    for (sibling, schema) in [("video/*", "{}"), ("text/event-stream", "{ type: string }")] {
        let spec = suffix_range_request_document(sibling, schema);
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_eq!(
                report.outcome(),
                Outcome::Rejected,
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert!(
                report.diagnostics().iter().any(|diagnostic| {
                    diagnostic.code == Code::UnsupportedMediaType
                        && diagnostic
                            .message
                            .starts_with("media type `application/*+json` is a media range")
                }),
                "`{sibling}` through {entry}: {report:#?}"
            );
        }
    }
}

#[test]
fn w014_a_suffix_range_listed_after_a_sendable_request_sibling_is_withheld() {
    // Order does not decide it. A sendable sibling listed before the range is generated and the
    // range is named as not generated, even for a sibling whose rank the range's rank 0 would
    // otherwise beat (`text/plain`).
    for (sibling, schema) in [
        ("application/json", "{ type: object }"),
        ("text/plain", "{ type: string }"),
    ] {
        let spec = request_body_document(&[
            (sibling, schema),
            ("application/*+json", "{ type: object }"),
        ]);
        let expected = format!(
            "`{sibling}` is selected; the alternative media type(s) `application/*+json` are not"
        );
        for (entry, report) in [("generate", generate(&spec)), ("check", check(&spec))] {
            assert_ne!(
                report.outcome(),
                Outcome::Rejected,
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert!(
                !has_code(&report, Code::UnsupportedMediaType),
                "`{sibling}` through {entry}: {report:#?}"
            );
            assert_eq!(
                messages_with_code(&report, Code::AlternativeMediaIgnored),
                [expected.as_str()],
                "`{sibling}` through {entry}: {report:#?}"
            );
        }
    }
}

#[test]
fn w014_every_suffix_range_beside_a_sendable_request_sibling_is_withheld() {
    // Every structured-suffix range that classifies is withheld, not only the first, and all of
    // them are named in one report.
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("application/*+json-seq", "{ type: object }"),
        ("application/json", "{ type: object }"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
        assert_eq!(
            messages_with_code(&report, Code::AlternativeMediaIgnored),
            [
                "`application/json` is selected; the alternative media type(s) \
                 `application/*+json`, `application/*+json-seq` are not"
            ],
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_suffix_range_beside_only_an_unclassified_request_sibling_is_a_media_range() {
    // A sibling that does not classify is not sendable, so nothing is withheld: the range is still
    // the only thing the body offers, and it is refused as a range.
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("application/pdf", "{}"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            messages_with_code(&report, Code::UnsupportedMediaType)
                .iter()
                .any(|message| message
                    .starts_with("media type `application/*+json` is a media range")),
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_sendable_request_sibling_that_fails_its_own_gate_is_reported_for_itself() {
    // Sendable is decided by classification alone. A `text/plain` sibling carrying an object schema
    // is still chosen over the range, and then refused by the raw-text gate for its own reason
    // rather than the range's. Neither the withheld range nor anything else is then reported as
    // passed over for a `text/plain` selection that is then refused (#110).
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("text/plain", "{ type: object }"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
        let unsupported = messages_with_code(&report, Code::UnsupportedMediaType);
        assert!(
            unsupported
                .iter()
                .all(|message| !message.contains("is a media range")),
            "{report:#?}"
        );
        assert!(
            unsupported
                .iter()
                .any(|message| message.contains("text/plain")),
            "{report:#?}"
        );
    }
}

#[test]
fn w014_a_withheld_suffix_range_beside_two_request_entries_is_reported_separately() {
    // Pinned as it stands. `choose_media` names the alternatives it passed over, and the withheld
    // range gets its own W014 right after, so this body carries two: both true, always in this
    // order, and both emitted only once the selection has passed every request-body gate.
    let spec = request_body_document(&[
        ("application/*+json", "{ type: object }"),
        ("application/json", "{ type: object }"),
        ("application/xml", "{ type: object }"),
    ]);
    for report in [generate(&spec), check(&spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::UnsupportedMediaType),
            "{report:#?}"
        );
        assert_eq!(
            messages_with_code(&report, Code::AlternativeMediaIgnored),
            [
                "`application/json` is selected; the alternative media type(s) \
                 `application/xml` are not",
                "`application/json` is selected; the alternative media type(s) \
                 `application/*+json` are not",
            ],
            "{report:#?}"
        );
    }
}

#[test]
fn e009_a_rejected_response_selection_claims_nothing_is_generated() {
    // The response side of #110: `text/plain` is selected over `text/html` (same rank, listed
    // first) and refused by the raw-text gate, so its `E009` alone reports it and no `W014` names
    // `text/html` as passed over.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /page:
    get:
      operationId: getPage
      responses:
        "200":
          description: OK
          content:
            text/plain: { schema: { type: object } }
            text/html: { schema: { type: object } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(
            !has_code(&report, Code::AlternativeMediaIgnored),
            "{report:#?}"
        );
        assert_eq!(
            messages_with_code(&report, Code::UnsupportedMediaType),
            ["media type `text/plain` requires a string-like or binary response schema"],
            "{report:#?}"
        );
    }
}

#[test]
fn w014_an_accepted_response_selection_still_reports_its_alternatives() {
    // The counterpart: the same shape with a string schema passes the gate, so the narrowing it
    // makes is still disclosed.
    let spec = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /page:
    get:
      operationId: getPage
      responses:
        "200":
          description: OK
          content:
            text/plain: { schema: { type: string } }
            text/html: { schema: { type: string } }
"##;
    for report in [generate(spec), check(spec)] {
        assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert_eq!(
            messages_with_code(&report, Code::AlternativeMediaIgnored),
            ["`text/plain` is selected; the alternative media type(s) `text/html` are not"],
            "{report:#?}"
        );
    }
}

#[test]
fn w014_on_a_document_rejected_elsewhere_claims_only_the_selection() {
    // `/page` passes every one of its own gates, so its `W014` is emitted; `/doc` is rejected, so
    // the run generates nothing — and `check` never generates on any document. The message must
    // therefore assert only what its emission site decides, the selection, and never that anything
    // "is generated" (#174). The `openai_openapi` corpus snapshot carries this shape at scale:
    // `Rejected` with `E009` beside many `W014`s.
    //
    // This pins the wording. The principle is held by `run_generate`/`run_check`, which compare
    // each diagnostic's structured `claim` with the run's outcome (#413); see
    // `a_generation_claim_on_w014_is_contradicted_wherever_174_found_it_false`.
    let spec = W014_REJECTED_ELSEWHERE;
    for report in [generate(spec), check(spec)] {
        assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
        assert!(has_code(&report, Code::UnsupportedMediaType), "{report:#?}");
        assert_eq!(
            messages_with_code(&report, Code::AlternativeMediaIgnored),
            ["`text/plain` is selected; the alternative media type(s) `text/html` are not"],
            "{report:#?}"
        );
    }
}

#[test]
fn a_sequential_media_outranks_a_text_range() {
    // `text/*` used to classify as `Text` by accident of the `text/` prefix arm, at the same rank
    // as a concrete textual type and *above* sequential media — so this response was a whole-body
    // `String`. As a range it now ranks below both, and the concrete streaming type wins. That is a
    // change to the generated public API, so it is pinned rather than left to be rediscovered.
    let (report, code) = generate_with_code(
        r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: getEvents
      responses:
        "200":
          description: OK
          content:
            text/*: { schema: { type: string } }
            text/event-stream:
              schema: { type: object, required: [seq], properties: { seq: { type: integer } } }
"##,
    );
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(code.contains("EventStream"), "{code}");
    // Two media types that decode differently: the range really is not generated.
    assert!(
        has_code(&report, Code::AlternativeMediaIgnored),
        "{report:#?}"
    );
}
