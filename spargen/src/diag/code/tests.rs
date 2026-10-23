use std::str::FromStr;

use super::Code;
use crate::test_support::{read_repo_file, repo_root, rust_sources};

/// Every code must appear in the published index, with the same title, and nothing may appear
/// there that is not a real code. The index is product surface — `spargen explain` and
/// `docs/errors.md` are the same contract — and without this the two drift silently.
#[test]
fn the_published_index_lists_exactly_the_declared_codes() {
    // A packaged `.crate` carries no docs directory, so the check is skipped there — gated on
    // the workspace marker, not on the file, so that inside the repository a missing or
    // renamed index fails instead of passing with a line on stderr nobody reads.
    let Some(root) = repo_root() else {
        eprintln!("skipping: not tested from the workspace");
        return;
    };
    let index = read_repo_file(&root, "docs/errors.md");
    let rows: Vec<(String, String)> = index
        .lines()
        .filter(|line| line.starts_with("| `E") || line.starts_with("| `W"))
        .map(|line| {
            let mut cells = line.split('|').map(str::trim);
            cells.next();
            let code = cells
                .next()
                .unwrap_or_default()
                .trim_matches('`')
                .to_owned();
            let _severity = cells.next();
            let title = cells.next().unwrap_or_default().to_owned();
            (code, title)
        })
        .collect();

    for code in Code::all() {
        let row = rows
            .iter()
            .find(|(listed, _)| listed == code.as_str())
            .unwrap_or_else(|| panic!("{} is missing from docs/errors.md", code.as_str()));
        // The index may elaborate ("… (3.1.x and 3.2.x are supported)") and may add markdown
        // code spans, but it must not describe a different thing than `spargen explain` does.
        assert!(
            row.1.replace('`', "").contains(code.title()),
            "docs/errors.md describes {} as `{}`, but its title is `{}`",
            code.as_str(),
            row.1,
            code.title()
        );
        assert!(
            !code.explain().is_empty(),
            "{} has no explain text",
            code.as_str()
        );
    }
    for (listed, _) in &rows {
        assert!(
            Code::all().iter().any(|code| code.as_str() == listed),
            "docs/errors.md lists `{listed}`, which is not a declared code"
        );
    }
}

/// Read every diagnostic code cited by a support document, paired with the 1-based index of
/// the table column it sits in (`1` = Supported, `2` = Warned, `3` = Rejected).
fn cited_codes(markdown: &str) -> Vec<(String, usize)> {
    let mut cited = Vec::new();
    for line in markdown.lines() {
        if !line.starts_with("| ") {
            continue;
        }
        // The leading `|` yields an empty first piece and the trailing one an empty last, so
        // the data cells are everything between.
        let cells: Vec<&str> = line.split('|').collect();
        for (index, cell) in cells.iter().enumerate().skip(1) {
            for token in cell.split('`') {
                let token = token.trim();
                let is_code = token.len() == 4
                    && matches!(token.as_bytes()[0], b'E' | b'W')
                    && token[1..].bytes().all(|byte| byte.is_ascii_digit());
                if is_code {
                    cited.push((token.to_owned(), index));
                }
            }
        }
    }
    cited
}

/// `docs/errors.md` is machine-checked above; the two documents that describe *what spargen
/// does with a construct* were not, and drifted — a row claimed `prefixEncoding` was supported
/// while the code rejected it, and `E014` was described nowhere at all. A prose claim is not
/// checkable in general, but the code tokens in it are: every one must name a real code, every
/// declared code must be placed somewhere in the matrix, and a code must not be filed under a
/// column that disagrees with its own severity.
///
/// That is all it holds. The prose of a cell is constrained by nothing here: a row rewritten
/// to say the opposite of what the generator does keeps every assertion green, so the prose is
/// reviewed by hand, as `docs/support-matrix.md` itself says. A matrix cell names its
/// constructs and defers the cases reaching a code to `spargen explain`, the one copy of that
/// list, rather than restating an `explain()` body in words no test compares with it; the
/// clauses `E023`'s row does quote are held
/// verbatim by `the_e023_matrix_row_quotes_its_pinned_explain_text_verbatim`.
#[test]
fn the_support_documents_cite_the_codes_that_exist_where_they_belong() {
    // Skipped only from a packaged `.crate`, which carries no docs directory. In the
    // repository a missing or renamed support document fails rather than skipping the check.
    let Some(root) = repo_root() else {
        eprintln!("skipping: not tested from the workspace");
        return;
    };
    let matrix = read_repo_file(&root, "docs/support-matrix.md");
    let scope = read_repo_file(&root, "docs/openapi-3.2.md");

    let matrix_cited = cited_codes(&matrix);
    for (cited, _) in matrix_cited.iter().chain(cited_codes(&scope).iter()) {
        assert!(
            Code::from_str(cited).is_ok(),
            "a support document cites `{cited}`, which is not a declared code"
        );
    }

    // The matrix is the operational boundary: a construct spargen has an opinion about has a
    // cell describing it. Without this, adding a code and forgetting the matrix is invisible.
    for code in Code::all() {
        assert!(
            matrix_cited.iter().any(|(cited, _)| cited == code.as_str()),
            "{} is declared but appears nowhere in docs/support-matrix.md",
            code.as_str()
        );
    }

    // The matrix columns are Area | Supported | Warned | Rejected, so a code's home column is
    // 3 for a warning and 4 for a rejection. A code may *additionally* be named in the
    // Supported cell's prose — several are, explaining the boundary they sit on — but it must
    // appear in the column that matches its severity, or the table files it under an outcome
    // the generator does not produce.
    for code in Code::all() {
        let home = if code.as_str().starts_with('W') { 3 } else { 4 };
        let columns: Vec<usize> = matrix_cited
            .iter()
            .filter(|(cited, _)| cited == code.as_str())
            .map(|(_, column)| *column)
            .collect();
        assert!(
            columns.contains(&home),
            "docs/support-matrix.md cites {} only in column(s) {:?}, but a {} belongs in \
                 column {home} ({})",
            code.as_str(),
            columns,
            if home == 3 { "warning" } else { "rejection" },
            if home == 3 { "Warned" } else { "Rejected" }
        );
    }
}

/// The words that mark a matrix clause as stating how `E023`'s audit reads workspace
/// inheritance: the subject the pinned explain body owns. Matched case-insensitively.
const E023_INHERITANCE_VOCABULARY: &[&str] = &["workspace", "inherit", "default-features"];

/// `E023`'s explain body is pinned byte for byte, so the matrix row citing `E023` defers to it
/// rather than restating it — but the row still states a few inheritance rules for a reader
/// scanning the table, and a paraphrase of a pinned body is a second copy no test compares
/// with the first. That copy drifted twice while every suite stayed green (#218). So every
/// clause of that row which touches workspace inheritance must be a **verbatim excerpt** of
/// the explain body, and the row must name `spargen explain E023` as where the rest lives.
///
/// A clause is a piece of a cell split on `. ` and `; `. The clause naming
/// `spargen explain E023` is the deferral itself and is exempt. What this cannot see: a
/// paraphrase that avoids every word in [`E023_INHERITANCE_VOCABULARY`].
#[test]
fn the_e023_matrix_row_quotes_its_pinned_explain_text_verbatim() {
    let Some(root) = repo_root() else {
        eprintln!("skipping: not tested from the workspace");
        return;
    };
    let matrix = read_repo_file(&root, "docs/support-matrix.md");
    let explain = Code::RuntimeDependencyContract.explain();
    let code = Code::RuntimeDependencyContract.as_str();

    let rows: Vec<&str> = matrix
        .lines()
        .filter(|line| {
            line.starts_with("| ")
                && cited_codes(line)
                    .iter()
                    .any(|(cited, column)| cited == code && *column == 4)
        })
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "exactly one docs/support-matrix.md row must reject with {code}, found {rows:#?}"
    );
    let row = rows[0];

    let deferral = format!("`spargen explain {code}`");
    assert!(
        row.contains(&deferral),
        "the {code} row must defer to {deferral} for the inheritance rules it does not quote"
    );

    let mut quoted = 0;
    for cell in row.split('|') {
        for clause in cell.split(". ").flat_map(|sentence| sentence.split("; ")) {
            let clause = clause.trim().trim_end_matches('.');
            let lower = clause.to_lowercase();
            if clause.contains(&deferral)
                || !E023_INHERITANCE_VOCABULARY
                    .iter()
                    .any(|word| lower.contains(word))
            {
                continue;
            }
            assert!(
                explain.contains(clause),
                "docs/support-matrix.md's {code} row states workspace inheritance in words \
                     `spargen explain {code}` does not use:\n\n  {clause}\n\nQuote the explain \
                     text verbatim, or leave the clause to it."
            );
            quoted += 1;
        }
    }
    // Without this, a vocabulary that stopped matching anything would pass vacuously.
    assert!(
        quoted > 0,
        "no clause of the {code} row matched {E023_INHERITANCE_VOCABULARY:?}; if the row no \
             longer quotes the explain text at all, retire this test with it"
    );
}

#[test]
fn all_codes_round_trip_from_stable_strings() {
    for code in Code::all() {
        assert_eq!(Code::from_str(code.as_str()).unwrap(), *code);
        match code.severity() {
            crate::diag::Severity::Error => assert!(code.as_str().starts_with('E')),
            crate::diag::Severity::Warning => assert!(code.as_str().starts_with('W')),
        }
    }
}

/// The Rust identifier of each variant. The match is exhaustive, so a variant added to `Code`
/// does not compile until it is named here — which is what makes the two tests below closed
/// over the whole enum rather than over whatever `all()` happens to list.
fn variant_name(code: Code) -> &'static str {
    match code {
        Code::UnsupportedOpenApiVersion => "UnsupportedOpenApiVersion",
        Code::UnsupportedDialect => "UnsupportedDialect",
        Code::AbsoluteRefUnsupported => "AbsoluteRefUnsupported",
        Code::UnresolvedRef => "UnresolvedRef",
        Code::VendoredRefDrift => "VendoredRefDrift",
        Code::RemoteFetchFailed => "RemoteFetchFailed",
        Code::DuplicateObjectKey => "DuplicateObjectKey",
        Code::PatternPropertiesRejected => "PatternPropertiesRejected",
        Code::DynamicRefRejected => "DynamicRefRejected",
        Code::NonDisjointUnion => "NonDisjointUnion",
        Code::NonScalarEnum => "NonScalarEnum",
        Code::UnsupportedMediaType => "UnsupportedMediaType",
        Code::UnsupportedParameterStyle => "UnsupportedParameterStyle",
        Code::InvalidInput => "InvalidInput",
        Code::UnknownSecurityScheme => "UnknownSecurityScheme",
        Code::AllOfIrreconcilable => "AllOfIrreconcilable",
        Code::InvalidOmitRule => "InvalidOmitRule",
        Code::OmitCreatedInvalidDocument => "OmitCreatedInvalidDocument",
        Code::ValidationKeywordIgnored => "ValidationKeywordIgnored",
        Code::ServerInitiatedFlowIgnored => "ServerInitiatedFlowIgnored",
        Code::OmittedConstruct => "OmittedConstruct",
        Code::SchemaDefaultNotApplied => "SchemaDefaultNotApplied",
        Code::XmlHintIgnored => "XmlHintIgnored",
        Code::Oas32ConstructIgnored => "Oas32ConstructIgnored",
        Code::AlternativeMediaIgnored => "AlternativeMediaIgnored",
        Code::SchemaNestingTooDeep => "SchemaNestingTooDeep",
        Code::RuntimeDependencyContract => "RuntimeDependencyContract",
        Code::SpecUndefinedBehavior => "SpecUndefinedBehavior",
        Code::TupleRestNotRepresentable => "TupleRestNotRepresentable",
        Code::DeclarationHasNoEffect => "DeclarationHasNoEffect",
        Code::RuntimeAuditSkipped => "RuntimeAuditSkipped",
        Code::CargoIntegrationDegraded => "CargoIntegrationDegraded",
        Code::CargoIntegrationRequired => "CargoIntegrationRequired",
    }
}

/// Does `haystack` name the path `Code::<variant>` as a whole path segment? A plain
/// `contains` would accept `Code::UnresolvedRefTypo` as evidence for `Code::UnresolvedRef`,
/// so every occurrence must be followed by a non-identifier character.
fn mentions_variant(haystack: &str, variant: &str) -> bool {
    let needle = format!("Code::{variant}");
    haystack.match_indices(&needle).any(|(at, _)| {
        haystack[at + needle.len()..]
            .chars()
            .next()
            .is_none_or(|next| !next.is_alphanumeric() && next != '_')
    })
}

/// Does `suite` assert `Code::<variant>` in a positive form — evidence that the code fires,
/// not only a mention? [`mentions_variant`] alone is satisfied by a comment naming the code or
/// by a fixture asserting its *absence*, neither of which is a fixture for it. So a comment
/// (everything from `//` to the end of a line) is discarded first, and an occurrence does not
/// count when it is the code argument of a negated `!has_code(...)` call or the right side of
/// a `!=`. Every other whole-segment occurrence (`has_code(r, Code::X)`, `d.code == Code::X`
/// in a `find`/`any`/`filter`, `messages_for(r, Code::X)`) is accepted.
fn asserts_variant(suite: &str, variant: &str) -> bool {
    let code: String = suite
        .lines()
        .map(|line| line.find("//").map_or(line, |at| &line[..at]))
        .collect::<Vec<_>>()
        .join("\n");
    let needle = format!("Code::{variant}");
    code.match_indices(&needle).any(|(at, _)| {
        let whole_segment = code[at + needle.len()..]
            .chars()
            .next()
            .is_none_or(|next| !next.is_alphanumeric() && next != '_');
        let before = &code[..at];
        let negated_call =
            enclosing_has_code(before).is_some_and(|call| before[..call].trim_end().ends_with('!'));
        let not_equal = before.trim_end().ends_with("!=");
        whole_segment && !negated_call && !not_equal
    })
}

/// The byte offset of the innermost `has_code(` call still open at the end of `before`, or
/// `None` when no such call encloses that point. Parentheses are matched by depth, so a nested
/// call that has already closed (`check(spec)` in `has_code(&check(spec), Code::X)`) does not
/// end the enclosing `has_code` call; a `;` outside every open parenthesis ends the search.
fn enclosing_has_code(before: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (at, byte) in before.bytes().enumerate().rev() {
        match byte {
            b')' => depth += 1,
            b'(' if depth > 0 => depth -= 1,
            b'(' if before[..at].ends_with("has_code") => return Some(at - "has_code".len()),
            b';' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

/// [`asserts_variant`] refuses exactly the two forms that let a code with no fixture pass: a
/// comment naming it and an assertion that it is absent.
#[test]
fn a_comment_or_an_absence_is_not_a_fixture() {
    for vacuous in [
        "// Code::UnresolvedRef is covered elsewhere",
        "let x = 1; // has_code(&report, Code::UnresolvedRef)",
        "assert!(!has_code(&report, Code::UnresolvedRef));",
        "assert!(\n    !has_code(\n        &report,\n        Code::UnresolvedRef\n    ),\n);",
        "assert!(!has_code(&check(spec), Code::UnresolvedRef));",
        "assert!(!has_code(&report, pick(Code::UnresolvedRef)));",
        "assert!(d.code != Code::UnresolvedRef);",
        "assert!(has_code(&report, Code::UnresolvedRefTypo));",
    ] {
        assert!(
            !asserts_variant(vacuous, "UnresolvedRef"),
            "accepted as a fixture: {vacuous}"
        );
    }
    for positive in [
            "assert!(has_code(&report, Code::UnresolvedRef));",
            "assert!(\n    has_code(\n        &report,\n        Code::UnresolvedRef\n    ),\n);",
            "assert!(has_code(&check(spec), Code::UnresolvedRef));",
            ".find(|d| d.code == Code::UnresolvedRef)",
            "let messages = messages_for(&report, Code::UnresolvedRef);",
            "assert!(!has_code(&report, Code::InvalidInput));\nassert!(has_code(&report, Code::UnresolvedRef));",
        ] {
            assert!(
                asserts_variant(positive, "UnresolvedRef"),
                "refused as a fixture: {positive}"
            );
        }
}

/// `all()` is a hand-written `const ALL`, and every docs/behavior test iterates it — so a
/// variant added to the enum but forgotten there would be invisible to all of them. Adding a
/// variant first fails to compile in `variant_name`; once named, `DECLARED` no longer matches
/// and this fails until `all()` lists it too.
#[test]
fn all_lists_every_declared_variant() {
    const DECLARED: usize = 33;

    assert_eq!(
        Code::all().len(),
        DECLARED,
        "Code::all() lists {} codes but {DECLARED} variants are declared — a variant reached \
             `Code` and `variant_name` without reaching `ALL`",
        Code::all().len()
    );

    let variants: std::collections::BTreeSet<&str> =
        Code::all().iter().map(|code| variant_name(*code)).collect();
    assert_eq!(
        variants.len(),
        DECLARED,
        "Code::all() lists the same variant twice"
    );

    let strings: std::collections::BTreeSet<&str> =
        Code::all().iter().map(|code| code.as_str()).collect();
    assert_eq!(
        strings.len(),
        DECLARED,
        "two variants share a code string, so `from_str` cannot round-trip both"
    );
}

/// Titles and explain text are product surface reached by `spargen explain`, independently of
/// whether the docs tree is present. The assertions on them inside
/// `the_published_index_lists_exactly_the_declared_codes` sit *after* its early return, so a
/// packaged build skips them; these always run. The title check there is a `contains` against
/// the docs row, which an empty title would satisfy trivially — this is what rules that out.
#[test]
fn every_code_has_title_and_explain_text() {
    for code in Code::all() {
        assert!(!code.title().is_empty(), "{} has no title", code.as_str());
        assert!(
            !code.explain().is_empty(),
            "{} has no explain text",
            code.as_str()
        );
    }
}

/// One case an enumerating explain body lists: the marker tag its emission sites carry, the
/// phrase of the body that states it, and — where the case has one — the message wording that
/// is reserved to it: every site of the case says it, and no other site of the code does.
struct Case {
    tag: &'static str,
    stated_as: &'static str,
    reserved_wording: Option<&'static str>,
}

/// The codes whose explain body enumerates the cases that reach them. See [`Code::explain`].
const ENUMERATED_CASES: &[(Code, &[Case])] = &[
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
                tag: "content-annotation",
                stated_as: "`contentMediaType`/`contentSchema` are consumed without this \
                                warning only on the string `data` property",
                reserved_wording: None,
            },
            Case {
                tag: "untyped-one-of-variant",
                stated_as: "a `oneOf` decoded by trial matching that keeps a variant \
                                lowering to `serde_json::Value`",
                reserved_wording: Some("`serde_json::Value`"),
            },
            Case {
                tag: "indistinguishable-branches",
                stated_as: "union branches lower to the same generated type, or to \
                                identically structured ones",
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
                stated_as: "a `discriminator.mapping` or `defaultMapping` value, a schema \
                                name or URI reference, names a schema the loaded description \
                                does not hold, whether the discriminator sits beside a union or \
                                on a schema with none",
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
                tag: "null-only-left",
                stated_as: "a schema with neither an `allOf` nor a `$ref` whose own `enum`, \
                                `const` or `anyOf` members leave only the `null` its `type` \
                                refuses",
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
                stated_as: "a `discriminator` beside a union member that is no schema \
                                component",
                reserved_wording: None,
            },
            Case {
                tag: "shadowed-component",
                stated_as: "a schema component declared inside a referenced sub-file whose \
                                name the root document also declares",
                reserved_wording: None,
            },
        ],
    ),
];

/// How far above a `Code::<Variant>` line its case marker may sit: far enough for a
/// `Diagnostic::error(` that rustfmt breaks before its first argument, near enough that a
/// marker cannot drift onto an unrelated site.
const MARKER_REACH: usize = 3;

/// The `(code, tags)` a `// E### case: a, b` marker line declares, if it is one.
fn case_marker(line: &str) -> Option<(&str, Vec<&str>)> {
    let rest = line.trim().strip_prefix("// ")?;
    let (code, tags) = rest.split_once(" case: ")?;
    let is_code = code.len() == 4
        && matches!(code.as_bytes()[0], b'E' | b'W')
        && code[1..].bytes().all(|byte| byte.is_ascii_digit());
    is_code.then(|| (code, tags.split(',').map(str::trim).collect()))
}

/// `spargen explain` prints an explain body verbatim, and a body that lists the cases reaching
/// its code makes a claim about the generator a test can check: that the list is exhaustive.
/// `E004`'s once was not — a security scheme alias miss and a chained Path Item `$ref` reached
/// the code while the body listed neither — and the only assertion on it was `!is_empty()`.
///
/// So every occurrence of such a code in the crate's sources (outside `diag`, which declares
/// it) must carry a case marker within [`MARKER_REACH`] lines above it, every tag a marker
/// names must be a case of that code, every case must have a site and be stated in the body,
/// and a case's reserved wording must appear in exactly its own sites — from the marker to the
/// `.emit(` that ends the diagnostic. A new emission site fails until someone decides which
/// listed case it is, or extends the list; a case deleted from the body fails too.
#[test]
fn every_emission_site_falls_into_a_case_its_explain_text_lists() {
    let src = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let files = rust_sources(src);
    let diag = src.join("diag");

    let mut failures = Vec::new();
    let mut sites: std::collections::BTreeMap<(&str, &str), usize> = Default::default();
    for path in files.iter().filter(|path| !path.starts_with(&diag)) {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()));
        let lines: Vec<&str> = text.lines().collect();
        let shown = path.strip_prefix(src).unwrap_or(path).display().to_string();

        // Every marker must name an enumerating code and sit above a site of it.
        for (at, line) in lines.iter().enumerate() {
            let Some((code, tags)) = case_marker(line) else {
                continue;
            };
            let Some((declared, cases)) = ENUMERATED_CASES
                .iter()
                .find(|(declared, _)| declared.as_str() == code)
            else {
                failures.push(format!(
                    "src/{shown}:{}: a `{code} case:` marker, but {code} is not in \
                         ENUMERATED_CASES",
                    at + 1
                ));
                continue;
            };
            let variant = variant_name(*declared);
            let attached = lines[at + 1..]
                .iter()
                .take(MARKER_REACH)
                .any(|below| mentions_variant(below, variant));
            if !attached {
                failures.push(format!(
                    "src/{shown}:{}: a `{code} case:` marker with no `Code::{variant}` in the \
                         {MARKER_REACH} lines below it",
                    at + 1
                ));
            }
            let region: String = lines[at..]
                .iter()
                .take_while({
                    let mut done = false;
                    move |line| !std::mem::replace(&mut done, line.contains(".emit("))
                })
                .copied()
                .collect::<Vec<_>>()
                .join("\n");
            for tag in &tags {
                match cases.iter().find(|case| case.tag == *tag) {
                    Some(case) => *sites.entry((declared.as_str(), case.tag)).or_default() += 1,
                    None => failures.push(format!(
                        "src/{shown}:{}: {code} has no case `{tag}`; its explain text lists \
                             {:?}",
                        at + 1,
                        cases.iter().map(|case| case.tag).collect::<Vec<_>>()
                    )),
                }
            }
            for case in cases.iter() {
                let Some(wording) = case.reserved_wording else {
                    continue;
                };
                let tagged = tags.contains(&case.tag);
                let says = region.contains(wording);
                if tagged != says {
                    failures.push(format!(
                        "src/{shown}:{}: a {code} site {} `{}` but {} {wording:?}, the wording \
                             reserved to that case",
                        at + 1,
                        if tagged { "tagged" } else { "not tagged" },
                        case.tag,
                        if says { "says" } else { "does not say" },
                    ));
                }
            }
        }

        // Every site of an enumerating code must carry a marker for it.
        for (code, _) in ENUMERATED_CASES {
            let variant = variant_name(*code);
            for (at, line) in lines.iter().enumerate() {
                if !mentions_variant(line, variant) {
                    continue;
                }
                let marked = lines[at.saturating_sub(MARKER_REACH)..at]
                    .iter()
                    .any(|above| case_marker(above).is_some_and(|(c, _)| c == code.as_str()));
                if !marked {
                    failures.push(format!(
                        "src/{shown}:{}: `Code::{variant}` with no `// {code} case: <case>` \
                             marker in the {MARKER_REACH} lines above it — decide which case of \
                             `spargen explain {code}` this site is, or add one to the explain text \
                             and to ENUMERATED_CASES",
                        at + 1
                    ));
                }
            }
        }
    }

    for (code, cases) in ENUMERATED_CASES {
        for case in cases.iter() {
            if !code.explain().contains(case.stated_as) {
                failures.push(format!(
                    "`spargen explain {code}` no longer states case `{}`: {:?}",
                    case.tag, case.stated_as
                ));
            }
            if !sites.contains_key(&(code.as_str(), case.tag)) {
                failures.push(format!(
                    "{code} case `{}` has no emission site; drop it from the explain text and \
                         from ENUMERATED_CASES, or mark the site that reports it",
                    case.tag
                ));
            }
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// CLAUDE.md: every code gets "a fixture in `spargen/tests/frontend/`", enforced by tests
/// rather than convention. Frontend codes are asserted there; the eight the frontend cannot
/// produce — the `compat` omit rules, the facade's own Cargo-integration and runtime-audit
/// diagnostics, and `spargen lock`'s fetch failure — are asserted in the suite that *can* produce them, and each
/// must say so here. A new code that lands in neither place fails, which is the point. The
/// owning suite must assert the code in a positive form ([`asserts_variant`]); the frontend
/// suite must not name an elsewhere-owned code at all, not even negatively.
#[test]
fn every_code_is_asserted_by_the_suite_that_owns_it() {
    const OWNED_ELSEWHERE: &[(&str, &str)] = &[
        // `compat` rules: the frontend never sees an omit rule.
        ("InvalidOmitRule", "carve.rs"),
        ("OmitCreatedInvalidDocument", "carve.rs"),
        ("OmittedConstruct", "carve.rs"),
        // The runtime-dependency contract needs a real consumer manifest to audit.
        ("RuntimeDependencyContract", "e2e/manifest_audit.rs"),
        ("RuntimeAuditSkipped", "e2e/manifest_audit.rs"),
        // The Cargo-integration policy is a property of the build environment, not of the
        // spec; `frontend/` deliberately runs every fixture with the integration off.
        ("CargoIntegrationDegraded", "config.rs"),
        ("CargoIntegrationRequired", "config.rs"),
        // `spargen lock`'s fetch failure: `check`/`generate` never fetch, so only a vendor
        // run over the real fetcher can produce it. Its one positive assertion is the
        // `fetch_failure` helper, which `vendor_remote.rs` and `tls_strictness.rs` both call.
        ("RemoteFetchFailed", "shared/remote.rs"),
    ];

    let Some(root) = repo_root() else {
        return;
    };
    let tests = root.join("spargen/tests");
    let read = |name: &str| {
        std::fs::read_to_string(tests.join(name))
            .unwrap_or_else(|error| panic!("spargen/tests/{name} must be readable: {error}"))
    };

    // The frontend suite is a directory test crate: `main.rs` holds the harness and each
    // family module beside it holds fixtures, so every source file in it is read. A missing
    // module would let a code pass unasserted, so the scan must find `main.rs` and at least
    // one module beside it.
    let frontend_dir = tests.join("frontend");
    let mut frontend_files: Vec<_> = std::fs::read_dir(&frontend_dir)
        .unwrap_or_else(|error| panic!("spargen/tests/frontend/ must be readable: {error}"))
        .map(|entry| entry.expect("readable directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    frontend_files.sort();
    assert!(
        frontend_files.len() > 1 && frontend_files.iter().any(|path| path.ends_with("main.rs")),
        "spargen/tests/frontend/ holds {frontend_files:?}; expected main.rs and its family \
             modules"
    );
    let frontend = frontend_files
        .iter()
        .map(|path| {
            std::fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()))
        })
        .collect::<Vec<_>>()
        .join("\n");
    for code in Code::all() {
        let variant = variant_name(*code);
        match OWNED_ELSEWHERE.iter().find(|(owned, _)| *owned == variant) {
            Some((_, suite)) => {
                assert!(
                    asserts_variant(&read(suite), variant),
                    "{} is declared to be asserted in {suite}, but `Code::{variant}` appears \
                         nowhere in it outside a comment or an assertion of its absence",
                    code.as_str()
                );
                assert!(
                        !mentions_variant(&frontend, variant),
                        "{} is listed in OWNED_ELSEWHERE but spargen/tests/frontend/ now asserts it too — \
                         drop the entry so one suite owns it",
                        code.as_str()
                    );
            }
            None => assert!(
                asserts_variant(&frontend, variant),
                "{} has no fixture: `Code::{variant}` appears nowhere in \
                     spargen/tests/frontend/ outside a comment or an assertion of its \
                     absence. Add one, or add the code to OWNED_ELSEWHERE naming the suite that \
                     asserts it.",
                code.as_str()
            ),
        }
    }

    for (variant, _) in OWNED_ELSEWHERE {
        assert!(
            Code::all()
                .iter()
                .any(|code| variant_name(*code) == *variant),
            "OWNED_ELSEWHERE names `{variant}`, which is not a declared Code variant"
        );
    }
}

/// A clause of an `explain()` body whose **behaviour** is enforced by fixtures in another
/// module, recorded so that the connection between the prose and those fixtures is written
/// down and checkable, rather than re-derived by whoever notices the clause is unasserted.
struct ExplainClauseOwner {
    code: Code,
    /// Verbatim text of the body; it must occur there exactly once.
    clause: &'static str,
    /// The module whose `#[test]`s enforce the clause, and its source.
    module: (&'static str, &'static str),
    /// Names of `#[test]` functions in `module` that fail when the behaviour breaks. Where they
    /// establish less than the clause says, a comment on the row states the gap.
    fixtures: &'static [&'static str],
}

/// Every file of `runtime_contract`'s test module, joined: its fixtures are split across them
/// by concern. A listed file that goes missing fails to compile, and a cited fixture moved
/// into a file not listed here fails `explain_clauses_owned_elsewhere_resolve_to_fixtures_that_exist`
/// as unfound, so neither passes by reading less.
const RUNTIME_CONTRACT: (&str, &str) = (
    "runtime_contract/tests/",
    concat!(
        include_str!("../../runtime_contract/tests/deps_block.rs"),
        include_str!("../../runtime_contract/tests/explain.rs"),
        include_str!("../../runtime_contract/tests/mod.rs"),
        include_str!("../../runtime_contract/tests/targets.rs"),
        include_str!("../../runtime_contract/tests/versions.rs"),
        include_str!("../../runtime_contract/tests/workspace.rs"),
    ),
);

/// The sibling of `OWNED_ELSEWHERE` for explain **prose**. `OWNED_ELSEWHERE` records which
/// suite asserts that a code is *emitted*; this records which fixtures enforce what a body
/// *says*, for clauses no test asserts as text.
///
/// `E023`'s body is pinned byte for byte by
/// `runtime_contract::tests::explain::the_e023_explain_text_states_the_inheritance_rules_this_module_enforces`,
/// which additionally cites a fixture for each sentence stating what the resolver does, and by
/// its own rule does not cite fixtures for sentences that give advice. The consumer-obligations
/// clauses below fall outside that rule — most read as advice — yet each is enforced by an
/// existing fixture, and one (reqwest's default features) is a named `E023` trigger in the
/// support matrix's rejected column, not advice at all. This table is where that ownership is
/// recorded, so the byte-for-byte test's rule need not change.
///
/// What the check is worth, precisely: it catches a clause edited or deleted from the body and
/// a fixture renamed or removed. It does not verify that a cited fixture has anything to do
/// with the clause citing it; that is a reviewer's reading, and holding prose to behaviour in
/// general is #137.
const EXPLAIN_CLAUSES_OWNED_ELSEWHERE: &[ExplainClauseOwner] = &[
    ExplainClauseOwner {
        code: Code::RuntimeDependencyContract,
        clause: "its consuming Cargo package must declare the crates and dependency features \
                     referenced by that specific API",
        module: RUNTIME_CONTRACT,
        fixtures: &[
            // Each missing crate or feature is reported, and only when the API uses it.
            "conditional_dependencies_and_features_are_required_only_when_used",
            "reqwest_defaults_and_blocking_wiring_are_part_of_the_contract",
        ],
    },
    ExplainClauseOwner {
        code: Code::RuntimeDependencyContract,
        clause: "Use the documented tested lower bounds",
        module: RUNTIME_CONTRACT,
        fixtures: &["a_requirement_that_admits_a_version_below_the_floor_is_rejected"],
    },
    ExplainClauseOwner {
        code: Code::RuntimeDependencyContract,
        clause: "(or a higher semver-compatible caret floor, or an exact `=x.y.z` requirement \
                     at or above the floor on the same semver-compatible line)",
        module: RUNTIME_CONTRACT,
        fixtures: &[
            "exact_floors_and_higher_compatible_caret_requirements_are_supported",
            "an_exact_requirement_at_the_floor_passes_the_audit",
        ],
    },
    ExplainClauseOwner {
        code: Code::RuntimeDependencyContract,
        clause: "keep reqwest default features disabled",
        module: RUNTIME_CONTRACT,
        fixtures: &["reqwest_defaults_and_blocking_wiring_are_part_of_the_contract"],
    },
    ExplainClauseOwner {
        code: Code::RuntimeDependencyContract,
        // A rule, not a list: the body once enumerated "reqwest/bytes/XML/UUID/time" and so
        // omitted `futures-core` and tokio's `rt`, which the diagnostic also names (#172).
        // Whatever the requirement table demands, the diagnostic names, so this cannot go
        // stale as the table grows.
        clause: "enable only the capabilities the diagnostic names",
        module: RUNTIME_CONTRACT,
        // Not covered: these pin that the diagnostic names exactly the capabilities the API
        // uses, including `futures-core` for streams and tokio for `blocking` (and never
        // `serde` on `time`). The audit does not reject a capability enabled beyond that set,
        // so "only" is advice, and no fixture enforces it.
        fixtures: &[
            "conditional_dependencies_and_features_are_required_only_when_used",
            "reqwest_defaults_and_blocking_wiring_are_part_of_the_contract",
            "the_time_requirement_never_asks_for_serde",
        ],
    },
];

/// Every way `owner` has gone stale: its clause no longer occurs exactly once in the body, it
/// cites no fixture, or a fixture it cites is not a `#[test]` in its module.
fn explain_clause_owner_failures(owner: &ExplainClauseOwner) -> Vec<String> {
    let mut failures = Vec::new();
    let code = owner.code;
    let occurrences = code.explain().matches(owner.clause).count();
    if occurrences != 1 {
        failures.push(format!(
            "`spargen explain {code}` says {:?} {occurrences} times, expected exactly once; \
                 bring EXPLAIN_CLAUSES_OWNED_ELSEWHERE in line with the body",
            owner.clause
        ));
    }
    if owner.fixtures.is_empty() {
        failures.push(format!("no fixture cited for {code}'s {:?}", owner.clause));
    }
    let (module, source) = owner.module;
    for fixture in owner.fixtures {
        if !crate::diag::is_test_fn(source, fixture) {
            failures.push(format!(
                "{code}'s {:?} names `{fixture}` as a fixture that enforces it, and no \
                     `#[test]` of that name exists in {module}",
                owner.clause
            ));
        }
    }
    failures
}

#[test]
fn explain_clauses_owned_elsewhere_resolve_to_fixtures_that_exist() {
    let failures: Vec<String> = EXPLAIN_CLAUSES_OWNED_ELSEWHERE
        .iter()
        .flat_map(explain_clause_owner_failures)
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The check's own falsifiers: a row whose clause is not in the body, whose fixture is a
/// helper rather than a `#[test]`, whose fixture does not exist, or that cites nothing, must
/// each be refused.
#[test]
fn the_explain_ownership_check_refuses_a_stale_clause_or_fixture() {
    let row = |clause, fixtures| ExplainClauseOwner {
        code: Code::RuntimeDependencyContract,
        clause,
        module: RUNTIME_CONTRACT,
        fixtures,
    };
    const LIVE: &[&str] = &["reqwest_defaults_and_blocking_wiring_are_part_of_the_contract"];
    let clause = "keep reqwest default features disabled";

    assert!(explain_clause_owner_failures(&row(clause, LIVE)).is_empty());
    for stale in [
        row("keep reqwest default features enabled", LIVE),
        row(clause, &["replace_once"]),
        row(clause, &["no_such_fixture_exists"]),
        row(clause, &[]),
    ] {
        assert_eq!(
            explain_clause_owner_failures(&stale).len(),
            1,
            "{:?} citing {:?}",
            stale.clause,
            stale.fixtures
        );
    }
}
