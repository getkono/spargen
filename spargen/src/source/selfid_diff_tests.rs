//! `spargen lock` and the build each keep their own reading of a document's `$self`, their own
//! choice of parse format, and their own `http`/`https` predicate. These tables drive both copies
//! over one set of URLs and paths and hold them to agreement row by row. A row where the copies
//! disagree today records each side's current answer and names the bug issue filed for it, so the
//! disagreement can neither widen nor close without this file changing.
//!
//! The lock walk is reachable only under `remote-fetch`, which gates this module as it gates
//! `vendor`.

use camino::{Utf8Path, Utf8PathBuf};

use crate::diag::{Diagnostics, FileId};

use super::bundle::{classify_self, parse_by_name, SelfRegistration};
use super::lock::{vendor_path_for_url, Lock};
use super::parse_json;
use super::remote::is_http_url;
use super::vendor::{parse_scratch_local, parse_scratch_remote, self_identity, Base, SelfIdentity};

/// What a `$self` makes a document, in terms both copies can be compared in.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    Remote(String),
    Local(Utf8PathBuf),
    Opaque(String),
    Unregistered,
}

/// Where the document carrying the `$self` was reached.
#[derive(Debug, Clone, Copy)]
enum Reached {
    /// Read from this local path.
    Local(&'static str),
    /// Retrieved from this URL.
    Remote(&'static str),
}

/// What a row expects of the two copies.
#[derive(Clone, Copy)]
enum Expect<T: 'static> {
    /// Both copies give the same answer.
    Agree,
    /// Both copies give this answer.
    Both(T),
    /// The copies disagree today: each side's current answer, and the bug issue filed for it.
    Disagree { lock: T, build: T, bug: u32 },
}

impl<T: std::fmt::Debug + PartialEq> Expect<T> {
    /// Hold `got`, the (lock, build) answers for `row`, to this expectation; a mismatch is pushed
    /// onto `failures`.
    fn check(&self, row: &str, got: (T, T), failures: &mut Vec<String>) {
        let met = match self {
            Expect::Agree => got.0 == got.1,
            Expect::Both(both) => got.0 == *both && got.1 == *both,
            Expect::Disagree { lock, build, .. } => got.0 == *lock && got.1 == *build,
        };
        if !met {
            let want = match self {
                Expect::Agree => "agreement".to_owned(),
                Expect::Both(both) => format!("agreement on {both:?}"),
                Expect::Disagree { lock, build, bug } => {
                    format!("({lock:?}, {build:?}), the disagreement #{bug} records")
                }
            };
            failures.push(format!("{row}: (lock, build) = {got:?}, want {want}"));
        }
    }
}

/// The lock walk's reading (`vendor::self_identity`) of `self_uri` in a document reached at
/// `reached`.
fn lock_self(self_uri: &str, reached: Reached) -> Seen {
    let text = format!(
        "{{\"$self\": {}}}",
        serde_json::to_string(self_uri).unwrap()
    );
    let value = parse_json(FileId(0), &text, &mut Diagnostics::default()).unwrap();
    let base = match reached {
        Reached::Local(path) => Base::Local(Utf8PathBuf::from(path)),
        Reached::Remote(url) => Base::Remote(url.to_owned()),
    };
    match self_identity(&value, &base) {
        Some(SelfIdentity::Remote(url)) => Seen::Remote(url),
        Some(SelfIdentity::Local(path)) => Seen::Local(path),
        Some(SelfIdentity::Opaque(uri)) => Seen::Opaque(uri),
        None => Seen::Unregistered,
    }
}

/// The build's reading (`InputBundle::register_self_identity`) of the same `$self`. A remote
/// document's base path is never read, so it is passed empty.
fn build_self(self_uri: &str, reached: Reached) -> Seen {
    let identity = match reached {
        Reached::Local(path) => classify_self(self_uri, None, Utf8Path::new(path)),
        Reached::Remote(url) => classify_self(self_uri, Some(url), Utf8Path::new("")),
    };
    match identity {
        Some(SelfRegistration::Remote(url)) => Seen::Remote(url),
        Some(SelfRegistration::Local(path)) => Seen::Local(path),
        Some(SelfRegistration::Opaque(uri)) => Seen::Opaque(uri),
        None => Seen::Unregistered,
    }
}

const LOCAL: Reached = Reached::Local("specs/root.yaml");
const REMOTE: Reached = Reached::Remote("https://h.example/v1/root.yaml");

#[test]
fn the_lock_and_the_build_read_every_self_alike() {
    let rows: &[(&str, Reached, Expect<Seen>)] = &[
        // Absent or fragment-only.
        ("", LOCAL, Expect::Agree),
        ("#/frag", LOCAL, Expect::Agree),
        ("", REMOTE, Expect::Agree),
        ("#/frag", REMOTE, Expect::Agree),
        // Scheme case.
        ("https://h.example/api.yaml", LOCAL, Expect::Agree),
        ("http://h.example/api.yaml", LOCAL, Expect::Agree),
        ("HTTPS://h.example/api.yaml", LOCAL, Expect::Agree),
        ("Http://h.example/api.yaml", LOCAL, Expect::Agree),
        ("https://other.example/api.yaml", REMOTE, Expect::Agree),
        ("HTTPS://other.example/api.yaml", REMOTE, Expect::Agree),
        // Other schemes, a drive letter, and a colon in a relative path.
        ("urn:example:api", LOCAL, Expect::Agree),
        ("urn:example:api", REMOTE, Expect::Agree),
        ("C:/specs/api.yaml", LOCAL, Expect::Agree),
        ("C:/specs/api.yaml", REMOTE, Expect::Agree),
        ("./a:b.yaml", LOCAL, Expect::Agree),
        ("./a:b.yaml", REMOTE, Expect::Agree),
        // Query strings and fragments.
        ("self.yaml?v=1", LOCAL, Expect::Agree),
        ("self.yaml?v=1", REMOTE, Expect::Agree),
        ("?v=1", REMOTE, Expect::Agree),
        ("self.yaml#/components", LOCAL, Expect::Agree),
        ("self.yaml#/components", REMOTE, Expect::Agree),
        (
            "https://other.example/api.yaml?v=1#/x",
            REMOTE,
            Expect::Agree,
        ),
        // Path normalization.
        ("lib/self.yaml", LOCAL, Expect::Agree),
        ("./lib/../self.yaml", LOCAL, Expect::Agree),
        ("../self.yaml", LOCAL, Expect::Agree),
        ("/abs/self.yaml", LOCAL, Expect::Agree),
        ("//h.example/self.yaml", LOCAL, Expect::Agree),
        ("lib/self.yaml", REMOTE, Expect::Agree),
        ("./lib/../self.yaml", REMOTE, Expect::Agree),
        ("../../self.yaml", REMOTE, Expect::Agree),
        ("/abs/self.yaml", REMOTE, Expect::Agree),
        ("//other.example/self.yaml", REMOTE, Expect::Agree),
        // Extension case.
        ("Self.JSON", LOCAL, Expect::Agree),
        ("Self.JSON", REMOTE, Expect::Agree),
    ];
    let mut failures = Vec::new();
    for (self_uri, reached, expect) in rows {
        let got = (
            lock_self(self_uri, *reached),
            build_self(self_uri, *reached),
        );
        let row = format!("`$self: {self_uri}` in {reached:?}");
        expect.check(&row, got, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The format a copy parses a document as, observed rather than read from its code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Json,
    Yaml,
    /// YAML, then JSON.
    Either,
}

/// YAML that is not JSON.
const YAML_ONLY: &str = "a: 1\n";
/// JSON that YAML refuses: a UTF-16 surrogate pair, where YAML's `\u` escape names one code point.
const JSON_ONLY: &str = r#"{"a": "\ud83d\ude00"}"#;

fn observe(accepts: impl Fn(&str) -> bool) -> Format {
    match (accepts(YAML_ONLY), accepts(JSON_ONLY)) {
        (false, true) => Format::Json,
        (true, false) => Format::Yaml,
        (true, true) => Format::Either,
        (false, false) => panic!("neither probe parsed"),
    }
}

/// The lock walk's choice for the document named `name`: a local path (`parse_scratch_local`), or
/// the URL it was fetched from (`parse_scratch_remote`).
fn lock_format(name: &str, remote: bool) -> Format {
    if remote {
        observe(|text| parse_scratch_remote(name, text).is_some())
    } else {
        observe(|text| parse_scratch_local(name, text).is_some())
    }
}

/// The build's choice (`parse_by_name`): a local document by its path, and a remote one by the
/// vendored path `spargen lock` records for its URL.
fn build_format(name: &str, remote: bool) -> Format {
    let name = if remote {
        vendor_path_for_url(name)
    } else {
        name.to_owned()
    };
    observe(|text| parse_by_name(FileId(0), &name, text, &mut Diagnostics::default()).is_ok())
}

#[test]
fn the_lock_and_the_build_parse_every_document_alike() {
    use Expect::{Both, Disagree};
    use Format::{Either, Json, Yaml};
    // The lock matches the suffix of a dot-file name; the build reads it as having no extension.
    const DOTFILE_BUG: u32 = 917;
    // The lock reads a remote URL's suffix through a trailing `/`; the build reads the vendored
    // path, which drops the empty last segment.
    const SLASH_BUG: u32 = 918;
    let local: &[(&str, Expect<Format>)] = &[
        ("specs/api.json", Both(Json)),
        ("specs/api.yaml", Both(Yaml)),
        ("specs/api.yml", Both(Yaml)),
        ("specs/api", Both(Either)),
        ("specs/api.txt", Both(Either)),
        ("specs/./lib/../api.json", Both(Json)),
        ("specs.json/api", Both(Either)),
        // Extension case.
        ("specs/api.JSON", Both(Json)),
        ("specs/api.Yaml", Both(Yaml)),
        ("specs/api.YML", Both(Yaml)),
        // A query string, which a relative `$ref` carries into the local path it names (#916).
        ("specs/api.json?v=1", Both(Either)),
        ("specs/api.yaml?v=1", Both(Either)),
        ("specs/api.json#x", Both(Either)),
        // A name that is only a dot and an extension.
        (
            "specs/.json",
            Disagree {
                lock: Json,
                build: Either,
                bug: DOTFILE_BUG,
            },
        ),
        (
            "specs/.yaml",
            Disagree {
                lock: Yaml,
                build: Either,
                bug: DOTFILE_BUG,
            },
        ),
    ];
    let remote: &[(&str, Expect<Format>)] = &[
        ("https://h.example/api.json", Both(Json)),
        ("https://h.example/api.yaml", Both(Yaml)),
        ("https://h.example/api.yml", Both(Yaml)),
        ("https://h.example/api", Both(Either)),
        ("https://h.example/", Both(Either)),
        ("https://h.example/v1/../api.json", Both(Json)),
        // Scheme and host case.
        ("HTTPS://H.EXAMPLE/api.json", Both(Json)),
        // Query strings and fragments.
        ("https://h.example/api.json?v=1", Both(Json)),
        ("https://h.example/api.yaml?format=json", Both(Yaml)),
        ("https://h.example/api?format=json", Both(Either)),
        ("https://h.example/api.json#/components", Both(Json)),
        // Characters the vendored path folds to `_`.
        ("https://h.example/api.json;v=1", Both(Either)),
        ("https://h.example/api%2Ejson", Both(Either)),
        // Extension case.
        ("https://h.example/api.JSON", Both(Json)),
        ("https://h.example/api.Yaml", Both(Yaml)),
        // A trailing slash.
        (
            "https://h.example/api.json/",
            Disagree {
                lock: Either,
                build: Json,
                bug: SLASH_BUG,
            },
        ),
        // A name that is only a dot and an extension.
        (
            "https://h.example/.json",
            Disagree {
                lock: Json,
                build: Either,
                bug: DOTFILE_BUG,
            },
        ),
    ];
    let rows = local.iter().map(|row| (row, false));
    let rows = rows.chain(remote.iter().map(|row| (row, true)));
    let mut failures = Vec::new();
    for ((name, expect), remote) in rows {
        let got = (lock_format(name, remote), build_format(name, remote));
        expect.check(&format!("`{name}`"), got, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Whether `spargen.lock` accepts `url` as a `retrieval_url` (`Lock::parse`).
fn lock_accepts_retrieval_url(url: &str) -> bool {
    let text = format!(
        "version = 1\n[[remote]]\nurl = \"https://h.example/x\"\nsha256 = \"a\"\n\
         path = \"h.example/x\"\nretrieval_url = \"{url}\"\n"
    );
    Lock::parse(&text).is_ok()
}

/// The three `http`/`https` predicates: `remote::is_http_url`, the lock's `retrieval_url` check,
/// and the build's reading of a local document's absolute `$self`.
#[test]
fn the_three_http_predicates_agree() {
    let rows: &[(&str, Expect<bool>)] = &[
        ("http://h.example/x", Expect::Agree),
        ("https://h.example/x", Expect::Agree),
        ("HTTP://h.example/x", Expect::Agree),
        ("Https://h.example/x", Expect::Agree),
        ("http:/h.example/x", Expect::Agree),
        ("https:h.example/x", Expect::Agree),
        ("ftp://h.example/x", Expect::Agree),
        ("httpx://h.example/x", Expect::Agree),
        ("//h.example/x", Expect::Agree),
        (" https://h.example/x", Expect::Agree),
    ];
    let mut failures = Vec::new();
    for (url, expect) in rows {
        let lock = lock_accepts_retrieval_url(url);
        let build = matches!(
            classify_self(url, None, Utf8Path::new("root.yaml")),
            Some(SelfRegistration::Remote(_))
        );
        // Agreement here is agreement with `is_http_url` too, the third copy.
        let expect = match expect {
            Expect::Agree => Expect::Both(is_http_url(url)),
            other => *other,
        };
        expect.check(&format!("`{url}`"), (lock, build), &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
