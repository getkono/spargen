use super::Code;

/// One case an enumerating explain body lists: the marker tag its emission sites carry, the
/// phrase of the body that states it, and — where the case has one — the message wording that
/// is reserved to it: every site of the case says it, and no other site of the code does.
pub(super) struct Case {
    pub(super) tag: &'static str,
    pub(super) stated_as: &'static str,
    pub(super) reserved_wording: Option<&'static str>,
}

/// The codes whose explain body enumerates the cases that reach them. See the comment in
/// [`Code::explain`].
pub(super) const ENUMERATED_CASES: &[(Code, &[Case])] = &[
    (
        Code::ValidationKeywordIgnored,
        &[
            Case {
                tag: "validation-keyword",
                stated_as: "The keyword affects runtime validation but not the static Rust \
                                shape",
                reserved_wording: None,
            },
            Case {
                tag: "pre-2020-keyword",
                stated_as: "a validation keyword of a JSON Schema draft before 2020-12 that \
                                2020-12 replaced",
                reserved_wording: None,
            },
            Case {
                tag: "oas30-nullable",
                stated_as: "OpenAPI 3.0's `nullable: true` written in a 3.1 or 3.2 document",
                reserved_wording: None,
            },
            Case {
                tag: "content-annotation",
                stated_as: "`contentMediaType`/`contentSchema` are consumed without this \
                                warning only on the string `data` property",
                reserved_wording: None,
            },
            Case {
                tag: "untyped-one-of-variant",
                stated_as: "a `oneOf` decoded by trial matching that keeps a variant",
                reserved_wording: Some("`serde_json::Value`"),
            },
            Case {
                tag: "indistinguishable-branches",
                stated_as: "union branches lower to the same generated type",
                reserved_wording: None,
            },
        ],
    ),
    (
        Code::OmitCreatedInvalidDocument,
        &[
            Case {
                tag: "required-root-field",
                stated_as: "a rule removed `openapi` or `info`",
                reserved_wording: None,
            },
            Case {
                tag: "no-root-member",
                stated_as: "the rules left none of `paths`, `components`, and `webhooks`",
                reserved_wording: None,
            },
        ],
    ),
    (
        Code::UnresolvedRef,
        &[
            Case {
                tag: "absent-target",
                stated_as: "the target is absent from the loaded input bundle",
                reserved_wording: Some("not found in the input bundle"),
            },
            Case {
                tag: "undeclared-component",
                stated_as: "a local component reference names an entry the document does not \
                            declare",
                reserved_wording: None,
            },
            Case {
                tag: "discriminator-target",
                stated_as: "a `discriminator.mapping` or `defaultMapping` value, a schema",
                reserved_wording: None,
            },
            Case {
                tag: "cycle",
                stated_as: "a chain of reference hops closes into a cycle",
                reserved_wording: Some("cycle"),
            },
            Case {
                tag: "declined-hop",
                stated_as: "a hop resolves but spargen declines to follow it",
                reserved_wording: None,
            },
            Case {
                tag: "resource-scope",
                stated_as: "static `$id`/`$anchor` schema resource scopes",
                reserved_wording: Some("`$id`/`$anchor`"),
            },
            Case {
                tag: "unsupported-or-unresolved",
                stated_as: "a reference reported as `unsupported or unresolved` is one spargen \
                            could not resolve *and* could not tell an absent target from a \
                            fragment form it declines to follow",
                reserved_wording: Some("unsupported or unresolved"),
            },
        ],
    ),
    (
        Code::UnsupportedMediaType,
        &[
            Case {
                tag: "no-codec",
                stated_as: "a media type that names no codec spargen has",
                reserved_wording: None,
            },
            Case {
                tag: "request-range",
                stated_as: "a media range as a request body media type",
                reserved_wording: None,
            },
            Case {
                tag: "streaming-request",
                stated_as: "a streaming request body",
                reserved_wording: None,
            },
            Case {
                tag: "form-body-not-object",
                stated_as: "a `multipart/form-data` or `application/x-www-form-urlencoded` \
                                request body whose schema is not an object",
                reserved_wording: None,
            },
            Case {
                tag: "raw-schema",
                stated_as: "a raw body whose schema its codec cannot carry",
                reserved_wording: None,
            },
            Case {
                tag: "raw-null",
                stated_as: "a raw body whose schema admits `null`",
                reserved_wording: None,
            },
            Case {
                tag: "form-response",
                stated_as: "a form-urlencoded or multipart response body",
                reserved_wording: None,
            },
            Case {
                tag: "streaming-response-position",
                stated_as: "a bodied streaming response anywhere but the operation's single \
                                bodied success",
                reserved_wording: None,
            },
            Case {
                tag: "xml-multi-body",
                stated_as: "an XML response body beside a second bodied status on the same \
                                side",
                reserved_wording: None,
            },
            Case {
                tag: "sequence-schema",
                stated_as: "a complete-sequence `schema` without `itemSchema` on OpenAPI 3.2 \
                                sequential media",
                reserved_wording: None,
            },
            Case {
                tag: "content-parameter",
                stated_as: "a `content` parameter whose media type has no single-token \
                                serialization",
                reserved_wording: None,
            },
            Case {
                tag: "positional-encoding",
                stated_as: "`prefixEncoding`/`itemEncoding` on multipart",
                reserved_wording: None,
            },
            Case {
                tag: "nested-encoding",
                stated_as: "a nested `encoding`, `prefixEncoding`, or `itemEncoding` inside \
                                an Encoding Object",
                reserved_wording: None,
            },
            Case {
                tag: "undefined-rfc6570",
                stated_as: "the RFC 6570 combinations the specification leaves undefined",
                reserved_wording: None,
            },
            Case {
                tag: "wildcard-content-type",
                stated_as: "a wildcard `encoding.contentType`",
                reserved_wording: None,
            },
            Case {
                tag: "content-type-not-media",
                stated_as: "an `encoding.contentType` whose sent (first) element is not a \
                                media type at all",
                reserved_wording: None,
            },
            Case {
                tag: "content-type-parameter",
                stated_as: "an `encoding.contentType` whose sent element carries a parameter \
                                that is not RFC 9110 `name=value`",
                reserved_wording: None,
            },
            Case {
                tag: "part-header",
                stated_as: "a multipart `encoding.headers` entry whose pinned `const` or \
                                `default` is sent as a part header",
                reserved_wording: None,
            },
            Case {
                tag: "non-json-structured",
                stated_as: "an explicit `contentType` that is not JSON",
                reserved_wording: None,
            },
            Case {
                tag: "form-binary",
                stated_as: "a binary property in a form-urlencoded body",
                reserved_wording: None,
            },
            Case {
                tag: "xml-hint",
                stated_as: "an unsupported XML hint on a type that is serialized as XML",
                reserved_wording: None,
            },
        ],
    ),
    (
        Code::UnsupportedParameterStyle,
        &[
            Case {
                tag: "delimited-explode",
                stated_as: "`spaceDelimited`/`pipeDelimited` with `explode: true`",
                reserved_wording: None,
            },
            Case {
                tag: "style-domain",
                stated_as: "a `spaceDelimited`/`pipeDelimited` parameter whose schema admits a \
                                value that is not an array or object and a `deepObject` \
                                parameter whose schema admits a value that is not an object",
                reserved_wording: None,
            },
            Case {
                tag: "nested-value",
                stated_as: "a nested array or object inside a `simple`/`form` value",
                reserved_wording: None,
            },
            Case {
                tag: "unconstrained-property",
                stated_as: "an object property whose value no schema constrains",
                reserved_wording: None,
            },
            Case {
                tag: "uninhabited-schema",
                stated_as: "a parameter schema that no value satisfies",
                reserved_wording: None,
            },
            Case {
                tag: "content-without-schema",
                stated_as: "a JSON or `text/*` `content` parameter whose entry has no schema",
                reserved_wording: None,
            },
            Case {
                tag: "querystring-content",
                stated_as: "a `querystring` parameter whose one content entry is neither JSON \
                                nor form-urlencoded or has no schema, or whose form-urlencoded \
                                schema is not an object",
                reserved_wording: None,
            },
        ],
    ),
    (
        Code::NonDisjointUnion,
        &[
            Case {
                tag: "unrepresentable-applicators",
                stated_as: "an applicator combination that is not yet representable",
                reserved_wording: None,
            },
            Case {
                tag: "resolves-to-itself",
                stated_as: "a union that resolves to *itself*",
                reserved_wording: None,
            },
            Case {
                tag: "no-branch-left",
                stated_as: "sibling keywords that leave the union with no branch at all",
                reserved_wording: None,
            },
            Case {
                tag: "optional-discriminator-property",
                stated_as: "a member the tag selects leaves optional",
                reserved_wording: Some("is optional in"),
            },
        ],
    ),
    (
        Code::AllOfIrreconcilable,
        &[
            Case {
                tag: "scalar-members",
                stated_as: "scalar members with no common value or no single type",
                reserved_wording: None,
            },
            Case {
                tag: "required-property",
                stated_as: "conflicting constraints on a property some member requires",
                reserved_wording: None,
            },
            Case {
                tag: "additional-values",
                stated_as: "conflicting additional-value constraints",
                reserved_wording: None,
            },
            Case {
                tag: "object-scalar-mix",
                stated_as: "an object/scalar mix",
                reserved_wording: None,
            },
            Case {
                tag: "several-unions",
                stated_as: "an object composition with two or more unions to meet is \
                                rejected with this code",
                reserved_wording: None,
            },
            Case {
                tag: "unrepresentable-meet",
                stated_as: "Two sides that share values no single Rust type represents",
                reserved_wording: None,
            },
            Case {
                tag: "false-member",
                stated_as: "an `allOf` member that is the boolean schema `false`",
                reserved_wording: None,
            },
            Case {
                tag: "inferred-category",
                stated_as: "With no `type` beside them, the object keywords",
                reserved_wording: None,
            },
            Case {
                tag: "cycle",
                stated_as: "a `$ref` that closes a reference cycle back to the schema \
                                enclosing it",
                reserved_wording: None,
            },
            Case {
                tag: "own-keywords-empty",
                stated_as: "a schema with neither an `allOf` nor a `$ref` whose own keywords \
                                leave no value",
                reserved_wording: None,
            },
        ],
    ),
    (
        Code::SpecUndefinedBehavior,
        &[Case {
            tag: "path-item-ref-siblings",
            stated_as: "a Path Item `$ref` declared alongside structural fields",
            reserved_wording: None,
        }],
    ),
    (
        Code::DeclarationHasNoEffect,
        &[
            Case {
                tag: "reserved-header-parameter",
                stated_as: "an `in: header` parameter named `Accept`, `Content-Type`, or \
                                `Authorization`",
                reserved_wording: None,
            },
            Case {
                tag: "allow-reserved-parameter",
                stated_as: "`allowReserved` on a parameter that is never percent-encoded",
                reserved_wording: None,
            },
            Case {
                tag: "allow-reserved-multipart",
                stated_as: "`allowReserved` on a `multipart` Encoding Object",
                reserved_wording: None,
            },
            Case {
                tag: "encoding-on-other-media",
                stated_as: "`encoding`, `prefixEncoding`, or `itemEncoding` on a media type \
                                that is neither `multipart` nor \
                                `application/x-www-form-urlencoded`",
                reserved_wording: None,
            },
            Case {
                tag: "positional-encoding-form",
                stated_as: "`prefixEncoding` or `itemEncoding` on \
                                `application/x-www-form-urlencoded`",
                reserved_wording: None,
            },
            Case {
                tag: "encoding-unknown-property",
                stated_as: "an `encoding` entry naming a property the body schema does not \
                                declare",
                reserved_wording: None,
            },
            Case {
                tag: "encoding-headers-non-multipart",
                stated_as: "`encoding.headers` on a non-`multipart` media type",
                reserved_wording: None,
            },
            Case {
                tag: "encoding-header-no-value",
                stated_as: "an `encoding.headers` Header Object that pins no `const`/`default` \
                                value",
                reserved_wording: None,
            },
            Case {
                tag: "encoding-header-content-disposition",
                stated_as: "a `multipart` `encoding.headers` entry pinning `Content-Disposition`",
                reserved_wording: None,
            },
            Case {
                tag: "allow-empty-value",
                stated_as: "`allowEmptyValue`, which is deprecated",
                reserved_wording: None,
            },
            Case {
                tag: "mutual-tls",
                stated_as: "a `mutualTLS` security scheme",
                reserved_wording: None,
            },
            Case {
                tag: "response-content-type",
                stated_as: "a response header named `Content-Type`",
                reserved_wording: None,
            },
            Case {
                tag: "response-header-untyped",
                stated_as: "none of which yields a typed accessor",
                reserved_wording: Some("typed accessor"),
            },
            Case {
                tag: "head-response-content",
                stated_as: "`content` on any response to a `HEAD` request",
                reserved_wording: None,
            },
            Case {
                tag: "bodyless-status-content",
                stated_as: "on a `204`, `205` or `304` response, none of which carries a body",
                reserved_wording: None,
            },
            Case {
                tag: "reference-docs",
                stated_as: "a `summary` or `description` on a Reference Object",
                reserved_wording: None,
            },
            Case {
                tag: "unused-server-variable",
                stated_as: "a server variable that its server's `url` never uses",
                reserved_wording: None,
            },
            Case {
                tag: "extra-servers",
                stated_as: "a `servers` entry past the first on a path item or operation",
                reserved_wording: None,
            },
            Case {
                tag: "excluded-union-branch",
                stated_as: "a union branch that the enclosing schema's own constraints have \
                                already made unsatisfiable",
                reserved_wording: None,
            },
            Case {
                tag: "unreached-union-sibling",
                stated_as: "untyped object or array keywords beside a union, or beside a \
                                `$ref` to one, when no branch has their category",
                reserved_wording: None,
            },
            Case {
                tag: "standalone-discriminator",
                stated_as: "a `discriminator` on a schema with no `oneOf` or `anyOf` of its \
                                own",
                reserved_wording: None,
            },
            Case {
                tag: "untagged-discriminated-member",
                stated_as: "a `discriminator` beside a union member that is no schema component",
                reserved_wording: None,
            },
            Case {
                tag: "uncarried-mapping-entry",
                stated_as: "naming a union member that cannot carry the discriminator property",
                reserved_wording: None,
            },
            Case {
                tag: "shadowed-component",
                stated_as: "a schema component declared inside a referenced sub-file whose \
                                name the root document also declares",
                reserved_wording: None,
            },
            Case {
                tag: "closed-all-of-merge",
                stated_as: "an `allOf` member's `additionalProperties: false`",
                reserved_wording: None,
            },
        ],
    ),
];
