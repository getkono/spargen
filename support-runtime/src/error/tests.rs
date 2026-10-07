use std::future::Future;

use bytes::Bytes;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;

use crate::{AuthError, ResponseValue, TransportError};

use super::{Error, RequestError, ReqwestClass, TimeoutKind};

#[test]
fn retry_classifier_includes_timeouts_and_5xx() {
    let timeout = Error::<String>::Timeout(TimeoutKind::Total);
    assert!(timeout.is_transient());

    let status = Error::<String>::UnexpectedStatus {
        status: StatusCode::SERVICE_UNAVAILABLE,
        headers: HeaderMap::new(),
        body: Bytes::new(),
    };
    assert!(status.is_transient());
}

#[test]
fn retry_classifier_excludes_client_errors() {
    let api = Error::Api(ResponseValue::new(
        StatusCode::BAD_REQUEST,
        HeaderMap::new(),
        "bad".to_owned(),
    ));
    assert!(!api.is_transient());
}

#[test]
fn widen_preserves_the_variant() {
    let narrow = Error::<std::convert::Infallible>::Timeout(TimeoutKind::Total);
    let widened: Error<String> = narrow.widen();
    assert!(matches!(widened, Error::Timeout(TimeoutKind::Total)));
}

#[test]
fn request_message_source_displays_the_message() {
    let error = Error::<std::convert::Infallible>::request_message("no credential for `token`");
    assert!(matches!(error, Error::RequestConstruction(_)));
    let source = std::error::Error::source(&error).expect("request errors carry a source");
    assert_eq!(source.to_string(), "no credential for `token`");
}

/// The missing-credential cause is the payload itself, so the chain ends at `RequestError`:
/// its `source()` is `None`, where `Other` reaches one level further, to its boxed cause.
///
/// The rendered text groups the missing schemes per alternative, in declaration order —
/// `(missing: key + token)` — rather than listing them once, sorted and deduplicated across
/// the whole requirement. The grouping is the payload's whole point, and the exact string
/// below pins it.
#[test]
fn a_missing_credential_is_typed_and_ends_the_cause_chain() {
    let error = Error::<ApiBody>::RequestConstruction(RequestError::MissingCredential {
        alternatives: vec![vec!["key", "token"]],
    });
    assert!(!error.is_transient());
    assert_eq!(error.to_string(), "request construction failed");
    let source = std::error::Error::source(&error).expect("the typed cause is the source");
    assert_eq!(
        source.to_string(),
        "no registered credential satisfies the operation's security requirement \
         (missing: key + token)"
    );
    assert!(std::error::Error::source(source).is_none());
}

/// Any other cause is opaque: it cannot be moved out of `Other`, but it renders as itself and
/// `source()` reaches the cause (not its wrapper) at the same depth, so it still downcasts.
#[test]
fn an_other_cause_is_opaque_and_reachable_through_source() {
    #[derive(Debug)]
    struct Cause;

    impl std::fmt::Display for Cause {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("cause")
        }
    }

    impl std::error::Error for Cause {}

    let error = Error::<ApiBody>::request_construction(Cause);
    let Error::RequestConstruction(RequestError::Other(cause)) = &error else {
        panic!("expected Other, got {error:?}");
    };
    assert_eq!(cause.to_string(), "cause");
    let source = std::error::Error::source(&error).expect("the request error is the source");
    assert_eq!(source.to_string(), "cause");
    let inner = std::error::Error::source(source).expect("the cause is reachable");
    assert!(inner.downcast_ref::<Cause>().is_some());
    assert!(std::error::Error::source(inner).is_none());
}

/// `RequestCause` overrides `source` to forward to the boxed cause's *own* source rather than
/// to the box. Nothing else in the runtime calls it — `RequestError::source` hands out the box
/// itself — so without this the override could be deleted and nothing would notice.
#[test]
fn a_request_cause_forwards_source_to_the_boxed_causes_own_source() {
    #[derive(Debug)]
    struct Inner;

    impl std::fmt::Display for Inner {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("inner")
        }
    }

    impl std::error::Error for Inner {}

    #[derive(Debug)]
    struct Outer(Inner);

    impl std::fmt::Display for Outer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("outer")
        }
    }

    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    let error = Error::<ApiBody>::request_construction(Outer(Inner));
    let Error::RequestConstruction(RequestError::Other(cause)) = &error else {
        panic!("expected Other, got {error:?}");
    };
    assert_eq!(cause.to_string(), "outer");
    let forwarded = std::error::Error::source(cause)
        .expect("the wrapper forwards to the boxed cause's own source");
    assert_eq!(forwarded.to_string(), "inner");
    assert!(forwarded.downcast_ref::<Inner>().is_some());
}

/// How many variants `RequestError` has. `every_request_variant` returns an array of exactly
/// this length, so raising it will not compile until a value of the new variant is listed.
const REQUEST_VARIANTS: usize = 4;

/// Each variant's position in `every_request_variant`. Indices are dense and unique, which is
/// what `every_request_variant_lists_each_variant_exactly_once` checks.
///
/// **What this actually enforces, and what it does not.** The match being exhaustive means a
/// variant added to `RequestError` cannot compile without being *classified* here and in every
/// other match over the enum. Whether it is *listed* in `every_request_variant` — and so
/// whether anything it claims is ever compared against anything — this module enforces only in
/// part, and a test outside it closes the rest. Each of these was run:
///
/// - Adding a variant, classifying it everywhere, giving it the next free index, and raising
///   `REQUEST_VARIANTS` to match: **caught**, at compile time — the array is then one element
///   short of its own declared length.
/// - Padding that array with a duplicate of some other variant to make it compile: **caught**,
///   by the bijection test — two entries take one index, and another index is unoccupied.
/// - Adding a variant, classifying it, giving it the next free index, and leaving
///   `REQUEST_VARIANTS` alone: **not caught here**. Nothing in this module ever evaluates this
///   function on a value of the new variant, because no such value is ever constructed. It is
///   **caught** by `every_error_variant_is_counted_by_its_enumeration` in
///   `spargen/tests/layering.rs`, which counts the top-level variants of `RequestError` and
///   `Error` in the text of `error.rs` and requires `REQUEST_VARIANTS` and `ERROR_VARIANTS`, read
///   from this file's text, to equal them. With the count held to the enum, the first case then forces the value to be listed.
///
/// The count is read from the text because no *language* construct yields it on stable:
/// `std::mem::variant_count` is nightly, and a `const` assertion over an exhaustive match
/// cannot help, because constructing one value of each variant *is* the list the guard is
/// trying to force. Reading it in `spargen/tests/` adds nothing to the embedded runtime or to
/// generated output. Rejected: declaring the enum through a macro that emits the count
/// alongside it, which ships a `macro_rules!` definition of a public type into every generated
/// client; `strum::EnumCount`, whose attribute would sit above the test-module marker the embed
/// splits on, so it would ship; and `#[cfg_attr(test, derive(..))]`, which ships too and
/// activates when the *consumer* runs `cargo test`.
fn request_variant_index(error: &RequestError) -> usize {
    match error {
        RequestError::MissingCredential { .. } => 0,
        RequestError::CredentialProvider { .. } => 1,
        RequestError::Other(_) => 2,
        RequestError::CredentialMismatch { .. } => 3,
    }
}

/// One value of every `RequestError` variant, mirroring `every_variant` for `Error`. Listing a
/// variant here is what makes its display, source, transience and response accessors actually
/// get asserted; an exhaustive match alone only forces it to be *classified*. See
/// `request_variant_index` for exactly how much of that listing is mechanically enforced.
fn every_request_variant() -> [RequestError; REQUEST_VARIANTS] {
    [
        RequestError::MissingCredential {
            alternatives: vec![vec!["token"], vec!["key", "tenant"]],
        },
        RequestError::CredentialProvider {
            scheme: "token",
            source: AuthError::new("refresh rejected"),
        },
        // The field is private, but these tests live in the defining module.
        RequestError::Other(super::RequestCause(Box::new(super::MessageError(
            "bad path segment".to_owned(),
        )))),
        RequestError::CredentialMismatch {
            scheme: "login",
            required: "http basic",
            registered: "Provider",
        },
    ]
}

/// The enumeration is a bijection onto the variant set: every entry takes a distinct index
/// inside the declared count, and every index is occupied. Without this, a variant could be
/// added, classified in each exhaustive match, and never constructed — so nothing it claims
/// would ever be compared against anything.
#[test]
fn every_request_variant_lists_each_variant_exactly_once() {
    let mut seen = [false; REQUEST_VARIANTS];
    for error in every_request_variant() {
        let index = request_variant_index(&error);
        assert!(
            index < REQUEST_VARIANTS,
            "`{error}` takes index {index}, outside the declared count of \
             {REQUEST_VARIANTS}: raise `REQUEST_VARIANTS` and list a value of the new variant \
             in `every_request_variant`"
        );
        assert!(!seen[index], "two entries share index {index}");
        seen[index] = true;
    }
    assert!(
        seen.iter().all(|occupied| *occupied),
        "an index in 0..{REQUEST_VARIANTS} is unoccupied: `every_request_variant` is missing \
         a variant"
    );
}

/// Every variant renders a non-empty string that names its own cause.
///
/// `CredentialProvider` renders a fixed sentence naming the scheme, not the provider's own
/// text (`"refresh rejected"`), which is one level further out as its `source()`, the
/// provider's `AuthError`. The chain is three levels — `Error`, `RequestError`, `AuthError` —
/// and the middle one does not repeat the last, so a full-chain renderer prints each piece
/// once. A consumer that prints exactly one `.source()` level, or that binds
/// `Err(Error::RequestConstruction(e))` and prints `{e}`, sees the scheme sentence.
/// `RequestError` is publicly re-exported, so that level is directly printable and is not an
/// internal detail.
#[test]
fn every_request_variant_displays_exactly_what_names_its_cause() {
    for error in every_request_variant() {
        let rendered = error.to_string();
        assert!(!rendered.is_empty(), "a variant renders as an empty string");
        let expected = match &error {
            RequestError::MissingCredential { .. } => {
                "no registered credential satisfies the operation's security requirement \
                 (missing: token or key + tenant)"
            }
            RequestError::CredentialProvider { .. } => {
                "the token provider registered for security scheme `token` failed"
            }
            RequestError::Other(_) => "bad path segment",
            RequestError::CredentialMismatch { .. } => {
                "the `Credential::Provider` registered for security scheme `login` cannot \
                 satisfy its `http basic` type"
            }
        };
        assert_eq!(rendered, expected, "a variant does not name its cause");
    }
}

/// `MissingCredential` and `CredentialMismatch` *are* the whole cause, so they end the chain;
/// the other two carry a separate cause and must hand it over. A consumer walking the chain
/// must not find a phantom source, nor lose a real one.
#[test]
fn request_source_is_present_exactly_where_the_cause_is_separate() {
    for error in every_request_variant() {
        let expected = match &error {
            RequestError::MissingCredential { .. } | RequestError::CredentialMismatch { .. } => {
                false
            }
            RequestError::CredentialProvider { .. } | RequestError::Other(_) => true,
        };
        assert_eq!(
            std::error::Error::source(&error).is_some(),
            expected,
            "source() disagrees for {error}"
        );
    }
}

/// No request-construction variant is classified transient, and none carries a response: no
/// status, no typed body, because taxonomy #1 never has one to carry. Non-transmission is
/// *not* the shared reason — `RequestError::Other` can arrive from inside reqwest's send, so
/// it is the one variant where "the request was never sent" does not hold; see its own
/// documentation before retrying on it. Pinned over every `RequestError` variant so a new one
/// cannot arrive misclassified.
#[test]
fn no_request_variant_is_transient_or_carries_a_response() {
    for request_error in every_request_variant() {
        let error = Error::<ApiBody>::RequestConstruction(request_error);
        assert!(!error.is_transient(), "{error} classified as transient");
        assert_eq!(error.status(), None);
        assert!(error.api_body().is_none());
    }
}

/// The runtime never builds an alternative list with nothing to name, but the fields are public
/// inside the consumer's crate. Rendering stays total rather than trailing an empty clause.
#[test]
fn a_missing_credential_with_nothing_to_name_renders_without_the_clause() {
    let empty = RequestError::MissingCredential {
        alternatives: Vec::new(),
    };
    assert_eq!(
        empty.to_string(),
        "no registered credential satisfies the operation's security requirement"
    );
    let all_empty = RequestError::MissingCredential {
        alternatives: vec![Vec::new(), Vec::new()],
    };
    assert_eq!(all_empty.to_string(), empty.to_string());
    // A named alternative beside an empty one still renders, without a stray separator.
    let mixed = RequestError::MissingCredential {
        alternatives: vec![Vec::new(), vec!["token"], Vec::new()],
    };
    assert_eq!(
        mixed.to_string(),
        "no registered credential satisfies the operation's security requirement \
         (missing: token)"
    );
}

/// A typed API error body that is itself an `Error`, so `Error::source` can reach it.
#[derive(Debug)]
struct ApiBody(&'static str);

impl std::fmt::Display for ApiBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ApiBody {}

/// A genuine `reqwest::Error`, obtained the only way that needs neither a network nor an async
/// runtime: an unparseable URL, whose failure `RequestBuilder::build` surfaces.
fn reqwest_error() -> reqwest::Error {
    reqwest::Client::new()
        .request(reqwest::Method::GET, "not a url")
        .build()
        .expect_err("an unparseable URL fails to build")
}

/// How many variants `Error` has. `every_variant` returns an array of exactly this length, so
/// raising it will not compile until a value of the new variant is listed.
const ERROR_VARIANTS: usize = 9;

/// Each variant's position in `every_variant`. Indices are dense and unique. This is the same
/// guard `request_variant_index` carries, applied to the other taxonomy that is a semver
/// surface — and it enforces exactly as much, and as little, as that function documents.
fn error_variant_index(error: &Error<ApiBody>) -> usize {
    match error {
        Error::RequestConstruction(_) => 0,
        Error::Transport(_) => 1,
        Error::Timeout(_) => 2,
        Error::Protocol(_) => 3,
        Error::Redirect(_) => 4,
        Error::Api(_) => 5,
        Error::UnexpectedStatus { .. } => 6,
        Error::Decode { .. } => 7,
        Error::InterruptedBody(_) => 8,
    }
}

/// The enumeration is a bijection onto the variant set. See
/// `every_request_variant_lists_each_variant_exactly_once` for why an exhaustive match alone
/// is not enough.
#[test]
fn every_variant_lists_each_variant_exactly_once() {
    let mut seen = [false; ERROR_VARIANTS];
    for error in every_variant() {
        let index = error_variant_index(&error);
        assert!(
            index < ERROR_VARIANTS,
            "`{error}` takes index {index}, outside the declared count of {ERROR_VARIANTS}: \
             raise `ERROR_VARIANTS` and list a value of the new variant in `every_variant`"
        );
        assert!(!seen[index], "two entries share index {index}");
        seen[index] = true;
    }
    assert!(
        seen.iter().all(|occupied| *occupied),
        "an index in 0..{ERROR_VARIANTS} is unoccupied: `every_variant` is missing a variant"
    );
}

/// One value of every variant. The match in each test below is exhaustive over this array by
/// construction, so a variant added to `Error` is classified by the compiler;
/// `error_variant_index` is what additionally forces it to be *listed*.
fn every_variant() -> [Error<ApiBody>; ERROR_VARIANTS] {
    [
        Error::request_message("bad path segment"),
        Error::Transport(TransportError::new(reqwest_error())),
        Error::Timeout(TimeoutKind::Total),
        // The field is private, but these tests live in the defining module.
        Error::Protocol(super::ProtocolError {
            source: reqwest_error(),
        }),
        Error::Redirect(super::RedirectError {
            source: reqwest_error(),
        }),
        Error::Api(ResponseValue::new(
            StatusCode::BAD_REQUEST,
            HeaderMap::new(),
            ApiBody("bad request"),
        )),
        Error::UnexpectedStatus {
            status: StatusCode::IM_A_TEAPOT,
            headers: HeaderMap::new(),
            body: Bytes::new(),
        },
        Error::Decode {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            path: "items[0].id".to_owned(),
            body: Bytes::from_static(b"{}"),
            truncated: false,
        },
        Error::InterruptedBody(TransportError::new(reqwest_error())),
    ]
}

/// The documents that describe the error taxonomy to a consumer as a whole, so each must name
/// every variant of both enums. `README.md` is also `spargen`'s declared `readme`, shipped in
/// the published crate.
const TAXONOMY_DOCUMENTS: [&str; 2] = ["README.md", "docs/book/src/getting-started.md"];

/// Documents that cite individual variants without describing the whole taxonomy. They are
/// held only to citing variants that exist, which every scanned document is; listing them here
/// makes the scan prove it reached them.
const CITING_DOCUMENTS: [&str; 1] = ["docs/support-matrix.md"];

/// The variant a derived `Debug` names: the identifier the rendering opens with.
fn variant_name(debug: String) -> String {
    debug
        .chars()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
        .collect()
}

/// The variant names of `Error` and of `RequestError`, read off the enumerations the bijection
/// tests above hold to exactly one value per variant — so neither set can miss one.
fn declared_variants() -> (
    std::collections::BTreeSet<String>,
    std::collections::BTreeSet<String>,
) {
    (
        every_variant()
            .iter()
            .map(|error| variant_name(format!("{error:?}")))
            .collect(),
        every_request_variant()
            .iter()
            .map(|error| variant_name(format!("{error:?}")))
            .collect(),
    )
}

/// Every `Error::Name` and `RequestError::Name` spelled in `text`, as (line, enum, variant).
/// Only a capitalised member is a variant, so `Error::status()` and the other methods are not
/// read. A path the name continues (`io::Error::Other`) or extends (`StreamError::…`) names
/// some other type, and is skipped.
fn cited_variants(text: &str) -> Vec<(usize, &'static str, String)> {
    let continues = |ch: char| ch.is_alphanumeric() || ch == '_' || ch == ':';
    let mut cited = Vec::new();
    for (number, line) in text.lines().enumerate() {
        for (at, marker) in line.match_indices("Error::") {
            let (enumeration, before) = match line[..at].strip_suffix("Request") {
                Some(before) => ("RequestError", before),
                None => ("Error", &line[..at]),
            };
            if before.ends_with(continues) {
                continue;
            }
            let name: String = line[at + marker.len()..]
                .chars()
                .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
                .collect();
            if name.starts_with(|ch: char| ch.is_ascii_uppercase()) {
                cited.push((number + 1, enumeration, name));
            }
        }
    }
    cited
}

#[test]
fn the_citation_reader_reads_variants_of_these_two_enums_only() {
    let text = "`Error::Api` and `RequestError::Other`, but not `Error::status()`,\n\
                `io::Error::Other`, `StreamError::Decode`, or `MyRequestError::Gone`.\n\
                (`Error::UnexpectedStatus { .. }`)";
    assert_eq!(
        cited_variants(text),
        [
            (1, "Error", "Api".to_owned()),
            (1, "RequestError", "Other".to_owned()),
            (3, "Error", "UnexpectedStatus".to_owned()),
        ]
    );
}

/// The repository root: this crate is a direct member of the workspace.
fn repository_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("support-runtime sits one level below the workspace root")
        .to_path_buf()
}

/// Every Markdown file under `dir`, repository-relative with `/` separators, sorted. Skipped:
/// hidden and `target` directories, the vendored specification texts under `references/`,
/// and `CHANGELOG.md`, whose entries name variants as they were at each release.
fn repository_markdown(root: &std::path::Path) -> Vec<String> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, found: &mut Vec<String>) {
        let entries = std::fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("{} is readable: {error}", dir.display()));
        for entry in entries {
            let path = entry.expect("a readable directory entry").path();
            let name = path
                .file_name()
                .expect("a directory entry has a name")
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                if !(name.starts_with('.') || name == "target" || name == "references") {
                    walk(root, &path, found);
                }
            } else if name.ends_with(".md") && name != "CHANGELOG.md" {
                let relative = path
                    .strip_prefix(root)
                    .expect("the walk stays under the root");
                let parts: Vec<_> = relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect();
                found.push(parts.join("/"));
            }
        }
    }
    let mut found = Vec::new();
    walk(root, root, &mut found);
    found.sort();
    found
}

/// A document that names `Error::X` or `RequestError::X` is describing the generated client's
/// public error surface, and no other gate reads it: the `docs/` checks in spargen hold
/// diagnostic codes only. So every such citation in the repository's Markdown must name a
/// variant that exists — a variant renamed or removed here fails until every document stops
/// naming it.
#[test]
fn every_error_variant_a_document_cites_exists() {
    let (errors, requests) = declared_variants();
    let root = repository_root();
    let documents = repository_markdown(&root);
    for expected in TAXONOMY_DOCUMENTS.iter().chain(&CITING_DOCUMENTS) {
        assert!(
            documents.iter().any(|document| document == expected),
            "`{expected}` is not among the scanned documents {documents:?}"
        );
    }

    let mut stale = Vec::new();
    for document in &documents {
        let text = std::fs::read_to_string(root.join(document))
            .unwrap_or_else(|error| panic!("{document} is readable: {error}"));
        for (line, enumeration, name) in cited_variants(&text) {
            let declared = if enumeration == "Error" {
                &errors
            } else {
                &requests
            };
            if !declared.contains(&name) {
                stale.push(format!("{document}:{line}: `{enumeration}::{name}`"));
            }
        }
    }
    assert!(
        stale.is_empty(),
        "these documents cite error variants that do not exist (`Error` has {errors:?}, \
         `RequestError` has {requests:?}):\n{}",
        stale.join("\n")
    );
}

/// The other direction: a variant added to either enum is a breaking change to every generated
/// client, so the documents that lay out the taxonomy must name it, fully qualified, before it
/// lands.
#[test]
fn the_taxonomy_documents_name_every_error_variant() {
    let (errors, requests) = declared_variants();
    let root = repository_root();
    for document in TAXONOMY_DOCUMENTS {
        let text = std::fs::read_to_string(root.join(document))
            .unwrap_or_else(|error| panic!("{document} is readable: {error}"));
        let cited: std::collections::BTreeSet<(&str, String)> = cited_variants(&text)
            .into_iter()
            .map(|(_, enumeration, name)| (enumeration, name))
            .collect();
        let unnamed: Vec<String> = errors
            .iter()
            .map(|name| ("Error", name))
            .chain(requests.iter().map(|name| ("RequestError", name)))
            .filter(|(enumeration, name)| !cited.contains(&(*enumeration, (*name).clone())))
            .map(|(enumeration, name)| format!("`{enumeration}::{name}`"))
            .collect();
        assert!(
            unnamed.is_empty(),
            "{document} lays out the error taxonomy but never names {}: add each to its \
             error-taxonomy passage",
            unnamed.join(", ")
        );
    }
}

/// `from_reqwest` is the taxonomy: every failure a [`crate::HttpBackend`] reports is mapped
/// through it, which is what keeps a custom transport classifying identically to executing on
/// a `reqwest::Client` directly. Its timeout, redirect, decode, and request branches all need a
/// live connection attempt to reach (reqwest exposes no constructor for its own error; the two
/// timeout kinds are driven over a real client in `spargen/tests/e2e.rs`), so what is pinned
/// here is the builder branch, the one reqwest raises without any I/O.
///
/// A builder-kind error means reqwest refused the request: nothing was sent, and re-sending
/// it is refused again. reqwest raises it from `RequestBuilder::build` (an unparseable URL)
/// and also from `execute` on a request that built fine — a URL whose scheme is not
/// `http`/`https`, or plain `http` on an `https_only` client. The second arrives through the
/// backend as a `TransportError`, so without this branch the same refusal would be a
/// non-retryable `RequestConstruction` at build time and a retryable `Transport` at execute.
#[test]
fn from_reqwest_classifies_a_refused_request_as_request_construction() {
    let https_only = reqwest::Client::builder()
        .https_only(true)
        .build()
        .expect("build an https-only client");
    let refused_at_execute = |client: reqwest::Client, url: &str| {
        let request = client
            .request(reqwest::Method::GET, url)
            .build()
            .expect("reqwest builds the request; it refuses it only at execute");
        // reqwest answers without touching the network, so the first poll is ready.
        let mut future = std::pin::pin!(client.execute(request));
        match future
            .as_mut()
            .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
        {
            std::task::Poll::Ready(result) => result.expect_err("reqwest refuses the request"),
            std::task::Poll::Pending => panic!("reqwest touched the network for {url}"),
        }
    };
    for source in [
        reqwest_error(),
        refused_at_execute(reqwest::Client::new(), "ftp://example.com/op"),
        refused_at_execute(https_only, "http://example.com/op"),
    ] {
        assert!(source.is_builder(), "{source}");
        let error = Error::<ApiBody>::from_reqwest(source);
        assert!(
            matches!(error, Error::RequestConstruction(RequestError::Other(_))),
            "{error}"
        );
        assert!(!error.is_transient(), "{error}");
        assert!(std::error::Error::source(&error).is_some());
    }
}

/// `TransportError::is_transient` is what `RetryOutcome::is_transient` answers for a transport
/// failure, and it is decided on a borrow, apart from the variant `from_reqwest` builds. Pair
/// every class with that variant so the two cannot disagree: reqwest exposes no constructor
/// for the other kinds, so the variant is built around a stand-in source, which neither the
/// class nor `Error::is_transient` reads.
#[test]
fn every_reqwest_class_is_transient_exactly_when_its_variant_is() {
    for class in [
        ReqwestClass::Timeout(TimeoutKind::Connect),
        ReqwestClass::Timeout(TimeoutKind::Total),
        ReqwestClass::Redirect,
        ReqwestClass::Protocol,
        ReqwestClass::Transport,
        ReqwestClass::Request,
    ] {
        // Exhaustive, so a class added without a row here does not compile.
        match class {
            ReqwestClass::Timeout(_)
            | ReqwestClass::Redirect
            | ReqwestClass::Protocol
            | ReqwestClass::Transport
            | ReqwestClass::Request => {}
        }
        let error = Error::<ApiBody>::from_class(class, reqwest_error());
        assert_eq!(class.is_transient(), error.is_transient(), "{class:?}");
    }
    // The borrowed classification is the one `from_reqwest` applies to the same error.
    let transport = TransportError::new(reqwest_error());
    assert_eq!(
        transport.is_transient(),
        Error::<ApiBody>::from_reqwest(reqwest_error()).is_transient()
    );
}

/// The borrowed classification answers `true` too, not only `false`: the builder-kind error
/// above is the one permanent class, so a transient source is needed beside it. A connect
/// failure needs a socket and a reactor, which this suite does not run; reqwest's status-kind
/// error needs neither, and it reaches the same `Transport` class a failed connection does,
/// by the fall-through `ReqwestClass::of` ends in. A retry policy keying on the default
/// classifier therefore retries it.
#[test]
fn a_transport_class_failure_is_transient_on_the_borrow_and_to_a_retry_policy() {
    let status_error = || {
        reqwest::Response::from(
            http::Response::builder()
                .status(503)
                .body(String::new())
                .expect("valid synthetic response"),
        )
        .error_for_status()
        .expect_err("a 503 is an error status")
    };
    let source = status_error();
    assert!(source.is_status() && !source.is_connect(), "{source}");
    assert_eq!(ReqwestClass::of(&source), ReqwestClass::Transport);
    let transport = TransportError::new(source);
    assert!(transport.is_transient());
    assert!(crate::RetryOutcome::Transport(&transport).is_transient());
    let error = Error::<ApiBody>::from_reqwest(status_error());
    assert!(matches!(error, Error::Transport(_)), "{error}");
    assert!(error.is_transient());
}

#[test]
fn is_transient_classifies_every_variant() {
    for error in every_variant() {
        let expected = match &error {
            // Worth retrying: the failure is about the connection, not the request.
            Error::Transport(_) | Error::Timeout(_) | Error::InterruptedBody(_) => true,
            // A response status is retryable only when the server said so, whichever class
            // carries it: documented, undocumented, or undecodable.
            Error::Api(value) => {
                value.status() == StatusCode::TOO_MANY_REQUESTS || value.status().is_server_error()
            }
            Error::UnexpectedStatus { status, .. } | Error::Decode { status, .. } => {
                *status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
            }
            // Deterministic failures: retrying reproduces them.
            Error::RequestConstruction(_) | Error::Protocol(_) | Error::Redirect(_) => false,
        };
        assert_eq!(
            error.is_transient(),
            expected,
            "is_transient disagrees for {error}"
        );
    }
}

/// A `Decode` error is retryable exactly when its status is: a `429` or `5xx` whose body did
/// not match the schema is still the server saying "try again", while a `2xx` or `4xx` body
/// that does not decode reproduces on retry. This is the rule `RetryOutcome::is_transient`
/// applies to the same response before its body is read.
#[test]
fn a_decode_error_is_transient_exactly_when_its_status_is() {
    let decode = |code: u16| Error::<ApiBody>::Decode {
        status: StatusCode::from_u16(code).unwrap(),
        headers: HeaderMap::new(),
        path: "x".to_owned(),
        body: Bytes::new(),
        truncated: false,
    };
    for code in [429, 500, 502, 503] {
        assert!(decode(code).is_transient(), "{code} should be transient");
    }
    for code in [200, 201, 400, 404, 422, 499] {
        assert!(
            !decode(code).is_transient(),
            "{code} should not be transient"
        );
    }
}

/// `status` answers exactly for the three classes that carry a response status; every other
/// class produced no response to read one from.
#[test]
fn status_is_present_exactly_on_the_three_status_variants() {
    for error in every_variant() {
        let expected = match &error {
            Error::Api(value) => Some(value.status()),
            Error::UnexpectedStatus { status, .. } | Error::Decode { status, .. } => Some(*status),
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::InterruptedBody(_) => None,
        };
        assert_eq!(error.status(), expected, "status() disagrees for {error}");
    }
    let statuses: Vec<_> = every_variant().iter().filter_map(Error::status).collect();
    assert_eq!(
        statuses,
        [
            StatusCode::BAD_REQUEST,
            StatusCode::IM_A_TEAPOT,
            StatusCode::OK
        ]
    );
}

/// Generated clients hold `Error<Infallible>` for an operation with no documented error body,
/// so `status` is pinned on that instantiation too, over every variant it can hold (`Api` is
/// statically unreachable there): `UnexpectedStatus` and `Decode` answer, each with its own
/// code.
#[test]
fn status_on_an_uninhabited_api_error_is_present_only_for_the_response_variants() {
    let narrow: Vec<Error<std::convert::Infallible>> = vec![
        Error::request_message("bad path segment"),
        Error::Transport(TransportError::new(reqwest_error())),
        Error::Timeout(TimeoutKind::Connect),
        Error::Protocol(super::ProtocolError {
            source: reqwest_error(),
        }),
        Error::Redirect(super::RedirectError {
            source: reqwest_error(),
        }),
        Error::UnexpectedStatus {
            status: StatusCode::IM_A_TEAPOT,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"teapot"),
        },
        Error::Decode {
            status: StatusCode::PARTIAL_CONTENT,
            headers: HeaderMap::new(),
            path: "items[0].id".to_owned(),
            body: Bytes::from_static(b"{}"),
            truncated: true,
        },
        Error::InterruptedBody(TransportError::new(reqwest_error())),
    ];
    for error in &narrow {
        let expected = match error {
            Error::Api(value) => match *value.inner() {},
            Error::UnexpectedStatus { status, .. } | Error::Decode { status, .. } => Some(*status),
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::InterruptedBody(_) => None,
        };
        assert_eq!(error.status(), expected, "status() disagrees for {error}");
    }
    let statuses: Vec<_> = narrow.iter().filter_map(Error::status).collect();
    assert_eq!(
        statuses,
        [StatusCode::IM_A_TEAPOT, StatusCode::PARTIAL_CONTENT]
    );
}

#[test]
fn a_retryable_status_is_retryable_through_both_status_variants() {
    for status in [StatusCode::TOO_MANY_REQUESTS, StatusCode::BAD_GATEWAY] {
        assert!(
            Error::<ApiBody>::UnexpectedStatus {
                status,
                headers: HeaderMap::new(),
                body: Bytes::new(),
            }
            .is_transient(),
            "{status} should be transient as an undocumented status"
        );
        assert!(
            Error::Api(ResponseValue::new(status, HeaderMap::new(), ApiBody("x"))).is_transient(),
            "{status} should be transient as a documented status"
        );
    }
    // The boundary: 499 is a client error, 500 is not.
    assert!(!Error::<ApiBody>::UnexpectedStatus {
        status: StatusCode::from_u16(499).unwrap(),
        headers: HeaderMap::new(),
        body: Bytes::new(),
    }
    .is_transient());
}

#[test]
fn every_variant_displays_something_that_names_its_class() {
    for error in every_variant() {
        let rendered = error.to_string();
        assert!(!rendered.is_empty(), "a variant renders as an empty string");
        let expected = match &error {
            Error::RequestConstruction(_) => "request construction failed",
            Error::Transport(_) => "transport failed",
            Error::Timeout(_) => "timeout elapsed",
            Error::Protocol(_) => "protocol error",
            Error::Redirect(_) => "redirect policy exhausted",
            Error::Api(_) => "documented API error",
            Error::UnexpectedStatus { .. } => "unexpected response status",
            Error::Decode { .. } => "response decode failed",
            Error::InterruptedBody(_) => "response body was interrupted",
        };
        assert!(
            rendered.contains(expected),
            "{rendered:?} does not name its class ({expected:?})"
        );
    }
}

/// A decode failure's message names the status it arrived with, so a log line tells a drifted
/// `200` from an error page served under a documented status without matching the variant.
#[test]
fn a_decode_error_displays_its_status_and_path() {
    let error = Error::<ApiBody>::Decode {
        status: StatusCode::BAD_GATEWAY,
        headers: HeaderMap::new(),
        path: "items[0].id".to_owned(),
        body: Bytes::new(),
        truncated: false,
    };
    assert_eq!(
        error.to_string(),
        "response decode failed (502 Bad Gateway) at items[0].id"
    );
}

/// #457: the path quotes server-supplied text; its LF, CR, tab and other control characters
/// are escaped so the message stays on one line, while the field keeps serde's text.
#[test]
fn a_decode_error_escapes_its_paths_control_characters() {
    let path = "unknown variant `x\ny\r\tz\u{1b}`, expected `ready`";
    let error = Error::<ApiBody>::Decode {
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        path: path.to_owned(),
        body: Bytes::new(),
        truncated: false,
    };
    assert_eq!(
        error.to_string(),
        r"response decode failed (200 OK) at unknown variant `x\ny\r\tz\u{1b}`, expected `ready`"
    );
    let Error::Decode { path: kept, .. } = error else {
        unreachable!()
    };
    assert_eq!(kept, path);
}

/// The variants that wrap a cause expose it; the three that carry only data do not. A caller
/// walking the chain must not find a phantom source, nor lose a real one.
#[test]
fn source_is_present_exactly_where_the_taxonomy_carries_a_cause() {
    for error in every_variant() {
        let expected = match &error {
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::Api(_)
            | Error::InterruptedBody(_) => true,
            Error::Timeout(_) | Error::UnexpectedStatus { .. } | Error::Decode { .. } => false,
        };
        assert_eq!(
            std::error::Error::source(&error).is_some(),
            expected,
            "source() disagrees for {error}"
        );
    }
}

/// `widen` is applied by every generated shim, so dropping or reclassifying a variant there
/// would silently change the error a consumer matches on. `Api` is statically unreachable in an
/// `Error<Infallible>` and so is not in this list.
#[test]
fn widen_preserves_every_reachable_variant() {
    let narrow: Vec<Error<std::convert::Infallible>> = vec![
        Error::request_message("bad path segment"),
        Error::RequestConstruction(RequestError::MissingCredential {
            alternatives: vec![vec!["token"]],
        }),
        Error::RequestConstruction(RequestError::CredentialProvider {
            scheme: "token",
            source: AuthError::new("x"),
        }),
        Error::Transport(TransportError::new(reqwest_error())),
        Error::Timeout(TimeoutKind::Connect),
        Error::Protocol(super::ProtocolError {
            source: reqwest_error(),
        }),
        Error::Redirect(super::RedirectError {
            source: reqwest_error(),
        }),
        Error::UnexpectedStatus {
            status: StatusCode::IM_A_TEAPOT,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"teapot"),
        },
        Error::Decode {
            status: StatusCode::SERVICE_UNAVAILABLE,
            headers: HeaderMap::new(),
            path: "items[0].id".to_owned(),
            body: Bytes::from_static(b"{}"),
            truncated: true,
        },
        Error::InterruptedBody(TransportError::new(reqwest_error())),
    ];

    for error in narrow {
        let before = error.to_string();
        let transient = error.is_transient();
        let widened: Error<ApiBody> = error.widen();
        assert_eq!(widened.to_string(), before, "widen changed the variant");
        assert_eq!(
            widened.is_transient(),
            transient,
            "widen changed retryability of {before}"
        );
    }

    // The payload fields survive, not just the discriminant.
    let mut sent = HeaderMap::new();
    sent.insert(
        "retry-after",
        reqwest::header::HeaderValue::from_static("30"),
    );
    let widened: Error<ApiBody> = Error::<std::convert::Infallible>::Decode {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        headers: sent.clone(),
        path: "a.b".to_owned(),
        body: Bytes::from_static(b"raw"),
        truncated: true,
    }
    .widen();
    let Error::Decode {
        status,
        headers,
        path,
        body,
        truncated,
    } = widened
    else {
        panic!("widen changed the variant");
    };
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(headers, sent);
    assert_eq!(path, "a.b");
    assert_eq!(body, Bytes::from_static(b"raw"));
    assert!(truncated);

    // The typed request-construction cause keeps its payload too.
    let widened: Error<ApiBody> =
        Error::<std::convert::Infallible>::RequestConstruction(RequestError::MissingCredential {
            alternatives: vec![vec!["a"], vec!["b"]],
        })
        .widen();
    let Error::RequestConstruction(RequestError::MissingCredential { alternatives }) = widened
    else {
        panic!("widen changed the variant");
    };
    assert_eq!(alternatives, [vec!["a"], vec!["b"]]);
}

impl super::ApiErrorBody for ApiBody {
    type Body = str;
    fn body(&self) -> Option<&str> {
        Some(self.0)
    }
}

/// `api_body` is `Some` exactly on the documented-API variant; every other class carries no
/// typed body, and a future variant added to `every_variant` is classified here too.
#[test]
fn api_body_is_present_exactly_on_the_documented_api_error() {
    for error in every_variant() {
        let expected = match &error {
            // The documented API error carries the operation's typed body.
            Error::Api(_) => true,
            // Every other class carries none.
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::UnexpectedStatus { .. }
            | Error::Decode { .. }
            | Error::InterruptedBody(_) => false,
        };
        assert_eq!(
            error.api_body().is_some(),
            expected,
            "api_body disagrees for {error}"
        );
    }
    let api = Error::Api(ResponseValue::new(
        StatusCode::BAD_REQUEST,
        HeaderMap::new(),
        ApiBody("bad request"),
    ));
    assert_eq!(api.api_body(), Some("bad request"));
}

/// `status` and `api_body` read one `Error` from two sides, so they must agree on what each
/// class carries: a documented API error answers both from the same `ResponseValue`, an
/// undocumented status and an undecodable body have a status but no typed body, and every
/// other class has neither.
#[test]
fn status_and_api_body_agree_on_every_variant() {
    for error in every_variant() {
        match &error {
            Error::Api(value) => {
                assert_eq!(error.status(), Some(value.status()), "{error}");
                let same_body = match (error.api_body(), super::ApiErrorBody::body(value.inner())) {
                    (Some(answered), Some(carried)) => std::ptr::eq(answered, carried),
                    _ => false,
                };
                assert!(
                    same_body,
                    "api_body is not the Api value's own body: {error}"
                );
            }
            Error::UnexpectedStatus { status, .. } | Error::Decode { status, .. } => {
                assert_eq!(error.status(), Some(*status), "{error}");
                assert!(error.api_body().is_none(), "{error}");
            }
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::InterruptedBody(_) => {
                assert!(error.status().is_none(), "{error}");
                assert!(error.api_body().is_none(), "{error}");
            }
        }
    }
}

/// The no-documented-error shape is `Error<Infallible>`; `api_body` must still exist there so
/// generic callers compile against every operation, and it can only ever be `None`.
#[test]
fn an_uninhabited_error_body_is_never_present() {
    let error = Error::<std::convert::Infallible>::Timeout(TimeoutKind::Total);
    assert!(error.api_body().is_none());
    assert!(error.problem().is_none());
}

/// `ApiBody` stands in for a generated error type whose documented body is a problem object:
/// it answers with a fixed problem, so the test can tell the typed path from the raw one.
impl super::ApiErrorProblem for ApiBody {
    fn problem(&self) -> Option<super::ProblemDetails> {
        Some(super::ProblemDetails {
            detail: Some(self.0.to_owned()),
            ..super::ProblemDetails::default()
        })
    }
}

/// `problem` answers from the typed body on `Api`, from the raw body on the two classes that
/// retain one (only for an error status), and `None` everywhere else.
#[test]
fn problem_is_read_exactly_where_an_error_body_was_received() {
    for error in every_variant() {
        let problem = error.problem();
        match &error {
            Error::Api(_) => {
                assert_eq!(
                    problem.and_then(|problem| problem.detail).as_deref(),
                    Some("bad request"),
                    "the typed body answers on Api"
                );
            }
            // `every_variant`'s undocumented `418` carries an empty body, and its `Decode` a
            // `200` success status: neither is a problem object.
            Error::UnexpectedStatus { .. } | Error::Decode { .. } => {
                assert!(problem.is_none(), "{error}");
            }
            Error::RequestConstruction(_)
            | Error::Transport(_)
            | Error::Timeout(_)
            | Error::Protocol(_)
            | Error::Redirect(_)
            | Error::InterruptedBody(_) => assert!(problem.is_none(), "{error}"),
        }
    }
}

const PROBLEM: &[u8] = br#"{"type":"https://example.com/probs/out-of-credit","title":"Out of credit","status":403,"detail":"Your balance is 30.","instance":"/account/12345/msgs/abc","balance":30}"#;

/// A documented error status whose body failed to decode — a problem `type` the description
/// does not list — still yields its members, which is what #268 needed of a generic reader.
#[test]
fn a_decode_failure_on_an_error_status_reads_the_raw_problem() {
    let error = Error::<ApiBody>::Decode {
        status: StatusCode::FORBIDDEN,
        headers: HeaderMap::new(),
        path: "type".to_owned(),
        body: Bytes::from_static(PROBLEM),
        truncated: false,
    };
    assert_eq!(
        error.problem(),
        Some(super::ProblemDetails {
            problem_type: Some("https://example.com/probs/out-of-credit".to_owned()),
            title: Some("Out of credit".to_owned()),
            status: Some(403),
            detail: Some("Your balance is 30.".to_owned()),
            instance: Some("/account/12345/msgs/abc".to_owned()),
        })
    );
    let undocumented = Error::<ApiBody>::UnexpectedStatus {
        status: StatusCode::SERVICE_UNAVAILABLE,
        headers: HeaderMap::new(),
        body: Bytes::from_static(PROBLEM),
    };
    assert_eq!(
        undocumented
            .problem()
            .and_then(|problem| problem.problem_type),
        Some("https://example.com/probs/out-of-credit".to_owned())
    );
}

/// A success status is never read, even when its body looks like a problem object: an
/// undecodable `200` is a success body that did not match, not an error response.
#[test]
fn a_success_status_is_never_read_as_a_problem() {
    for status in [StatusCode::OK, StatusCode::NOT_MODIFIED] {
        let decode = Error::<ApiBody>::Decode {
            status,
            headers: HeaderMap::new(),
            path: String::new(),
            body: Bytes::from_static(PROBLEM),
            truncated: false,
        };
        assert!(decode.problem().is_none(), "{status}");
        let unexpected = Error::<ApiBody>::UnexpectedStatus {
            status,
            headers: HeaderMap::new(),
            body: Bytes::from_static(PROBLEM),
        };
        assert!(unexpected.problem().is_none(), "{status}");
    }
}

/// Members are read by their RFC 9457 type: a wrongly-typed member is `None` without costing
/// the others, a non-object body answers `None` as a whole, and so does a truncated one.
#[test]
fn problem_members_are_read_only_with_their_rfc_types() {
    let mixed = super::ProblemDetails::from_json(
        br#"{"type":7,"title":"t","status":"403","detail":null,"instance":"/i"}"#,
    )
    .expect("an object body");
    assert_eq!(
        mixed,
        super::ProblemDetails {
            problem_type: None,
            title: Some("t".to_owned()),
            status: None,
            detail: None,
            instance: Some("/i".to_owned()),
        }
    );
    assert_eq!(mixed.problem_type_or_blank(), "about:blank");
    for status in ["99", "600", "70000", "-1", "403.5"] {
        let body = format!(r#"{{"status":{status}}}"#);
        let read = super::ProblemDetails::from_json(body.as_bytes()).expect("an object");
        assert_eq!(
            read.status, None,
            "status {status} is outside the HTTP range"
        );
    }
    for body in [
        &b"[]"[..],
        b"\"text\"",
        b"null",
        b"<html>",
        b"",
        &PROBLEM[..40],
    ] {
        assert!(super::ProblemDetails::from_json(body).is_none());
    }
}

/// `of` reads a typed value through its serialization, so it agrees with `from_json` over the
/// bytes that value would put on the wire.
#[test]
fn a_typed_body_reads_as_its_serialization() {
    let value: serde_json::Value = serde_json::from_slice(PROBLEM).unwrap();
    assert_eq!(
        super::ProblemDetails::of(&value),
        super::ProblemDetails::from_json(PROBLEM)
    );
    assert_eq!(super::ProblemDetails::of("not an object"), None);
    assert_eq!(super::ProblemDetails::of(&Option::<u8>::None), None);
}
