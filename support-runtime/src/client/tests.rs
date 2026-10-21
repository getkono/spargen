use secrecy::SecretString;

use super::{ClientConfig, ClientCore};
use crate::tests::poll_ready;
use crate::{send, Credential, Error};

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
    let request = core
        .http()
        .request(reqwest::Method::GET, "http://example.com/x")
        .build()
        .expect("reqwest builds the request; it refuses it only at execute");
    let error = poll_ready(send(&core, request)).expect_err("the https-only client refuses http");
    assert!(matches!(error, Error::RequestConstruction(_)), "{error:?}");
}

/// `http` hands back the caller's client, not a default one: its configuration is what the
/// caller injected it for. reqwest's `Debug` is the one view of a client's configuration that
/// needs no I/O, and it prints a configured total timeout only when one is set.
#[test]
fn http_returns_the_injected_client_with_its_configuration() {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(7))
        .build()
        .expect("build a configured client");
    let core = ClientCore::with_client(client, "https://example.com").unwrap();
    let injected = format!("{:?}", core.http());
    assert!(injected.contains("7s"), "{injected}");
    let default = format!("{:?}", reqwest::Client::new());
    assert!(!default.contains("7s"), "{default}");
}
