use std::sync::{Arc, Mutex};
use std::time::Duration;

use secrecy::SecretString;

use super::{ClientConfig, ClientCore, TransportConfig};
use crate::tests::poll_ready;
use crate::{
    send, send_streaming, Credential, Error, ExecuteFuture, HttpBackend, HttpBody, RequestBuilder,
    TotalTimeout,
};

#[test]
fn an_invalid_base_url_is_a_request_construction_error() {
    // The only fallible step in construction. A generated client cannot be built at all
    // without a parseable base, so this must be an error rather than a panic or a silent
    // fallback to a default host.
    let error = ClientCore::new("not a url").unwrap_err();
    assert!(
        error.to_string().contains("request construction"),
        "{error}"
    );
    assert!(std::error::Error::source(&error).is_some());
}

/// Every message in `error`'s `source()` chain, outermost first, joined.
fn chain(error: &Error<std::convert::Infallible>) -> String {
    let error: &(dyn std::error::Error + 'static) = error;
    std::iter::successors(Some(error), |error| error.source())
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
}

/// Issue #1335: a URL that parses but is not an `http` or `https` URL is refused as a base by
/// every constructor and by `set_base_url`, before anything is sent: `localhost:8080` parses with
/// the scheme `localhost` and the path `8080`, and `mailto:x` cannot be a base at all. The refusal
/// names the scheme and never the URL, which may carry userinfo.
#[test]
fn a_base_url_that_is_not_http_or_https_is_refused() {
    for base in [
        "localhost:8080",
        "mailto:x",
        "ftp://user:hunter2@files.example/",
        "file:///etc/",
        "data:text/plain,x",
    ] {
        let refusals = [
            ClientCore::new(base).unwrap_err(),
            ClientCore::with_config(base, ClientConfig::default(), TransportConfig::default())
                .unwrap_err(),
            ClientCore::with_client(reqwest::Client::new(), base).unwrap_err(),
            ClientCore::with_backend(Recorder::new(), base).unwrap_err(),
        ];
        for error in refusals {
            assert!(matches!(error, Error::RequestConstruction(_)), "{error:?}");
            let message = chain(&error);
            assert!(message.contains("`http` or `https`"), "{message}");
            assert!(!message.contains("hunter2"), "{message}");
        }
        let mut core = ClientCore::new("https://example.com/v1/").unwrap();
        let error = core.set_base_url(base).unwrap_err();
        assert!(matches!(error, Error::RequestConstruction(_)), "{error:?}");
        assert_eq!(core.base_url().as_str(), "https://example.com/v1/");
    }
    let scheme = ClientCore::new("localhost:8080").unwrap_err();
    let message = chain(&scheme);
    assert!(message.contains("`localhost`"), "{message}");
    for base in ["http://example.com", "HTTPS://example.com/api/"] {
        ClientCore::new(base).unwrap();
    }
}

#[test]
fn a_base_url_is_parsed_and_retained() {
    let core = ClientCore::new("https://example.com/api/").unwrap();
    assert_eq!(core.base_url().as_str(), "https://example.com/api/");
}

#[test]
fn the_default_error_body_cap_is_64_kib() {
    // Generated clients retain at most this much of an error body; the rest is dropped and
    // the error flags truncation. Consumers tune it through `config_mut`.
    assert_eq!(ClientConfig::default().max_error_body, 64 * 1024);
    let core = ClientCore::new("https://example.com").unwrap();
    assert_eq!(core.config().max_error_body, 64 * 1024);
}

/// Issue #1260: no success body limit by default, and a core starts with empty options, which
/// `set_options` replaces whole and `options` reads back.
#[test]
fn a_core_starts_unbounded_with_empty_options_that_set_options_replaces() {
    assert_eq!(ClientConfig::default().max_success_body, None);
    let mut core = ClientCore::new("https://example.com").unwrap();
    assert_eq!(core.config().max_success_body, None);
    let options = core.options();
    assert_eq!(options.max_body, None);
    assert!(options.headers.is_empty());
    assert!(options.extensions.is_empty());

    let mut options = super::RequestOptions {
        max_body: Some(9),
        ..super::RequestOptions::default()
    };
    options.headers.insert("x-a", "1".parse().unwrap());
    core.set_options(options);
    assert_eq!(core.options().max_body, Some(9));
    assert_eq!(core.options().headers["x-a"], "1");
    core.set_options(super::RequestOptions::default());
    assert_eq!(core.options().max_body, None);
    assert!(core.options().headers.is_empty());
}

#[test]
fn the_error_body_cap_is_tunable_through_config_mut() {
    let mut core = ClientCore::new("https://example.com").unwrap();
    core.config_mut().max_error_body = 128;
    assert_eq!(core.config().max_error_body, 128);
}

#[test]
fn credentials_round_trip_per_scheme_and_the_last_write_wins() {
    let mut core = ClientCore::new("https://example.com").unwrap();
    assert!(core.credential("token").is_none());

    core.set_credential("token", Credential::Bearer(SecretString::from("first")));
    assert!(matches!(
        core.credential("token"),
        Some(Credential::Bearer(_))
    ));
    // Registration is keyed by scheme name, so re-registering replaces rather than accumulates
    // — this is how a consumer rotates a static credential.
    core.set_credential(
        "token",
        Credential::Basic {
            username: "u".to_owned(),
            password: SecretString::from("p"),
        },
    );
    assert!(matches!(
        core.credential("token"),
        Some(Credential::Basic { .. })
    ));
    assert!(core.credential("other").is_none());
}

#[test]
fn a_removed_credential_is_returned_and_no_longer_registered() {
    let mut core = ClientCore::new("https://example.com").unwrap();
    core.set_credential("token", Credential::Bearer(SecretString::from("t0k")));
    core.set_credential("key", Credential::ApiKey(SecretString::from("k3y")));

    assert!(matches!(
        core.remove_credential("token"),
        Some(Credential::Bearer(_))
    ));
    assert!(core.credential("token").is_none());
    // Removal is keyed by scheme name: the other registration is untouched.
    assert!(matches!(
        core.credential("key"),
        Some(Credential::ApiKey(_))
    ));
    // Removing again, or removing a scheme never registered, is a no-op rather than a panic.
    assert!(core.remove_credential("token").is_none());
    assert!(core.remove_credential("never").is_none());
    // A removed scheme can be registered again.
    core.set_credential("token", Credential::Bearer(SecretString::from("t1k")));
    assert!(core.credential("token").is_some());
}

/// A bound header is matched in any letter case, as HTTP header names are: binding, reading and
/// unbinding agree however each spells the name, the last binding wins, and unbinding returns
/// the value it removes.
#[test]
fn a_bound_header_is_keyed_case_insensitively_and_the_last_binding_wins() {
    let mut core = ClientCore::new("https://example.com").unwrap();
    assert_eq!(core.bound_header("X-Api-Version"), None);

    core.bind_header("X-Api-Version", "1").unwrap();
    assert_eq!(core.bound_header("x-api-version"), Some("1"));
    core.bind_header("X-API-VERSION", String::from("2"))
        .unwrap();
    assert_eq!(core.bound_header("X-Api-Version"), Some("2"));
    assert_eq!(core.bound_header("X-Tenant"), None);

    assert_eq!(core.unbind_header("x-api-VERSION"), Some("2".to_owned()));
    assert_eq!(core.bound_header("X-Api-Version"), None);
    assert_eq!(core.unbind_header("X-Api-Version"), None);
}

/// A bound header's value is redacted in `ClientCore`'s `Debug`, as a credential's is, so a core
/// logged with a tenant or key header bound does not leak it; the bound name still prints.
#[test]
fn a_bound_header_value_is_redacted_in_debug() {
    let mut core = ClientCore::new("https://example.com").unwrap();
    core.bind_header("X-Tenant", "tenant-secret-7781").unwrap();
    let debug = format!("{core:?}");
    assert!(debug.contains("x-tenant"), "{debug}");
    assert!(!debug.contains("tenant-secret-7781"), "{debug}");
}

/// Issue #1335: a value no request could carry as a header value (CR/LF, NUL, DEL, any other
/// control character but a tab) is refused when it is bound, rather than failing every later
/// request, and the value already bound stays bound. The refusal names the header and does not
/// quote the value, which the core holds as a secret.
#[test]
fn an_invalid_bound_header_value_is_refused_and_keeps_the_binding() {
    let mut core = ClientCore::new("https://example.com").unwrap();
    core.bind_header("X-Tenant", "acme").unwrap();
    for value in [
        "a\r\nb",
        "secret\0value",
        "line\nfeed",
        "del\u{7f}x",
        "esc\u{1b}x",
    ] {
        let error = core.bind_header("X-Tenant", value).unwrap_err();
        assert!(matches!(error, Error::RequestConstruction(_)), "{error:?}");
        let message = chain(&error);
        assert!(message.contains("`X-Tenant`"), "{message}");
        assert!(!message.contains(value), "{message}");
        assert!(!format!("{error:?}").contains(value), "{error:?}");
        assert_eq!(core.bound_header("x-tenant"), Some("acme"));
    }
    // A tab, visible ASCII and bytes past ASCII (`obs-text`) are a valid header value, as they
    // are to the request builder that sends it.
    for value in ["a\tb ~", "tenant-é"] {
        core.bind_header("X-Tenant", value).unwrap();
        assert_eq!(core.bound_header("x-tenant"), Some(value));
    }
}

/// The supplied client is the one requests run on: an `https_only` client refuses a plain `http`
/// request at execute, which a default client would instead try to send. reqwest refuses it
/// without touching the network, so the future is ready on the first poll, and the refusal is a
/// builder-kind error that the taxonomy classifies as `RequestConstruction`.
#[test]
fn with_client_runs_requests_on_the_supplied_client() {
    let client = reqwest::Client::builder()
        .https_only(true)
        .build()
        .expect("build an https-only client");
    let core = ClientCore::with_client(client, "http://example.com").unwrap();
    let error =
        poll_ready(send(&core, get(&core))).expect_err("the https-only client refuses http");
    assert!(matches!(error, Error::RequestConstruction(_)), "{error:?}");
}

/// The installed backend runs on the caller's client, not a default one: its configuration is
/// what the caller injected it for. reqwest's `Debug` is the one view of a client's configuration
/// that needs no I/O, and it prints a configured total timeout only when one is set.
#[test]
fn with_client_installs_a_backend_on_the_injected_client() {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(7))
        .build()
        .expect("build a configured client");
    let core = ClientCore::with_client(client, "https://example.com").unwrap();
    let injected = format!("{:?}", core.backend());
    assert!(injected.starts_with("ReqwestBackend {"), "{injected}");
    assert!(injected.contains("7s"), "{injected}");
    let default = format!("{:?}", reqwest::Client::new());
    assert!(!default.contains("7s"), "{default}");
}

/// Issue #1130: the default client bounds connecting and stalled reads, and sets no total bound,
/// so a long download or upload is not cut off by default.
#[test]
fn the_default_timeouts_are_a_30_s_connect_a_5_min_read_and_no_total() {
    let transport = TransportConfig::default();
    assert_eq!(transport.connect_timeout, Some(Duration::from_secs(30)));
    assert_eq!(transport.read_timeout, Some(Duration::from_secs(300)));
    assert_eq!(ClientConfig::default().timeout, None);
    let core = ClientCore::new("https://example.com").unwrap();
    assert_eq!(core.config().timeout, None);
    let built = format!("{:?}", core.backend());
    assert!(built.contains("read_timeout: 300s"), "{built}");
}

/// The client a core builds for its default backend carries the read timeout of the
/// `TransportConfig` it was built from: the default one's for `new`, the caller's for
/// `with_config`. reqwest's `Debug` of a client prints its read timeout and only when one is set;
/// its connect timeout is printed nowhere, so this is the one observable without I/O.
#[test]
fn the_built_client_carries_the_configured_read_timeout() {
    let default = format!(
        "{:?}",
        ClientCore::new("https://example.com").unwrap().backend()
    );
    assert!(default.contains("read_timeout: 300s"), "{default}");

    let transport = TransportConfig {
        read_timeout: Some(Duration::from_secs(7)),
        ..TransportConfig::default()
    };
    let core =
        ClientCore::with_config("https://example.com", ClientConfig::default(), transport).unwrap();
    let configured = format!("{:?}", core.backend());
    assert!(configured.contains("read_timeout: 7s"), "{configured}");

    let transport = TransportConfig {
        read_timeout: None,
        ..TransportConfig::default()
    };
    let core =
        ClientCore::with_config("https://example.com", ClientConfig::default(), transport).unwrap();
    let unbounded = format!("{:?}", core.backend());
    assert!(!unbounded.contains("read_timeout"), "{unbounded}");
}

/// `with_client` keeps the caller's client as it is: none of the configured timeouts is added to
/// it.
#[test]
fn with_client_adds_no_timeout_to_the_injected_client() {
    let core = ClientCore::with_client(reqwest::Client::new(), "https://example.com").unwrap();
    let injected = format!("{:?}", core.backend());
    assert!(!injected.contains("timeout"), "{injected}");
}

/// Issue #1287: `with_backend` builds nothing besides the core: the backend it is given is the one
/// installed, and `ClientCore`'s `Debug` names no other (no `reqwest::Client` sits under it);
/// `send` executes on it, with the default configuration's total timeout (none). An invalid base
/// URL is a request-construction error.
#[test]
fn with_backend_runs_on_the_given_backend_and_builds_no_client() {
    let backend = Recorder::new();
    let core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
    let installed: Arc<dyn HttpBackend> = backend.clone();
    assert!(Arc::ptr_eq(core.backend(), &installed));
    let debug = format!("{core:?}");
    assert!(debug.contains("backend: Recorder"), "{debug}");
    assert!(!debug.contains("Client {"), "{debug}");
    assert_eq!(core.base_url().as_str(), "https://example.com/");
    assert_eq!(core.config().timeout, None);

    poll_ready(send(&core, get(&core))).unwrap();
    assert_eq!(backend.timeouts(), [None]);

    let error = ClientCore::with_backend(backend, "not a url").unwrap_err();
    assert!(matches!(error, Error::RequestConstruction(_)), "{error:?}");
}

/// `set_base_url` repoints the core and keeps everything else; an invalid URL is a
/// request-construction error that leaves the base URL as it was.
#[test]
fn set_base_url_repoints_the_core_and_keeps_its_credentials() {
    let mut core = ClientCore::new("https://example.com/v1/").unwrap();
    core.set_credential("token", Credential::Bearer(SecretString::from("t0k")));
    core.config_mut().max_error_body = 128;

    core.set_base_url("https://peer.example/v2/").unwrap();
    assert_eq!(core.base_url().as_str(), "https://peer.example/v2/");
    assert!(core.credential("token").is_some());
    assert_eq!(core.config().max_error_body, 128);

    let error = core.set_base_url("not a url").unwrap_err();
    assert!(matches!(error, Error::RequestConstruction(_)), "{error:?}");
    assert_eq!(core.base_url().as_str(), "https://peer.example/v2/");
}

/// A transport that answers `204` and records the total timeout each request carries across the
/// seam, read from its `TotalTimeout` extension as `ReqwestBackend` reads it.
#[derive(Debug, Default)]
struct Recorder {
    timeouts: Mutex<Vec<Option<Duration>>>,
}

impl Recorder {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn timeouts(&self) -> Vec<Option<Duration>> {
        self.timeouts.lock().unwrap().clone()
    }
}

impl HttpBackend for Recorder {
    fn execute(&self, request: http::Request<HttpBody>) -> ExecuteFuture<'_> {
        self.timeouts.lock().unwrap().push(
            request
                .extensions()
                .get::<TotalTimeout>()
                .map(|timeout| timeout.0),
        );
        Box::pin(async {
            Ok(http::Response::builder()
                .status(204)
                .body(HttpBody::empty())
                .expect("valid synthetic response"))
        })
    }
}

fn get(core: &ClientCore) -> http::Request<HttpBody> {
    RequestBuilder::new(http::Method::GET, core.base_url().join("x").unwrap())
        .build()
        .expect("a valid request")
}

/// `get`, carrying a total timeout of its own.
fn get_within(core: &ClientCore, timeout: Duration) -> http::Request<HttpBody> {
    let mut request = get(core);
    request.extensions_mut().insert(TotalTimeout(timeout));
    request
}

/// `send` gives a request the core's total timeout as it is when the request is sent, so a
/// change through `config_mut` applies from the next request on, and it reaches a caller-supplied
/// backend on the request itself. A request carrying a timeout of its own keeps it.
#[test]
fn send_applies_the_configured_total_timeout_read_at_send_time() {
    let backend = Recorder::new();
    let mut core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
    poll_ready(send(&core, get(&core))).unwrap();

    core.config_mut().timeout = Some(Duration::from_secs(9));
    poll_ready(send(&core, get(&core))).unwrap();

    let own = get_within(&core, Duration::from_secs(2));
    poll_ready(send(&core, own)).unwrap();

    assert_eq!(
        backend.timeouts(),
        [
            None,
            Some(Duration::from_secs(9)),
            Some(Duration::from_secs(2))
        ]
    );
}

/// A streaming response is exempt from the total timeout: `send_streaming` never applies the
/// core's, and a request's own still crosses unchanged.
#[test]
fn send_streaming_leaves_the_configured_total_timeout_off() {
    let backend = Recorder::new();
    let mut core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
    core.config_mut().timeout = Some(Duration::from_secs(9));
    poll_ready(send_streaming(&core, get(&core))).unwrap();

    let own = get_within(&core, Duration::from_secs(2));
    poll_ready(send_streaming(&core, own)).unwrap();

    assert_eq!(backend.timeouts(), [None, Some(Duration::from_secs(2))]);
}
