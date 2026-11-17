//! The refresh-and-replay path: a refreshable credential's rejection check, its refresh, the one
//! replay of the call, and the bounds each body read is held to.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use secrecy::{ExposeSecret, SecretString};

use crate::{ClientCore, Error, MultipartForm, MultipartPart, RequestBuilder, RequestError};

use super::super::{
    attach_auth, attach_auth_replayable, replay_rejected, token_provider, AuthError, AuthKind,
    AuthScheme, Credential, CredentialKind, SchemeCarrier,
};
use super::{get, request_to, BEARER};
use crate::tests::{core, poll_ready, Reply, SequenceBackend};

/// How often a [`refreshable`] credential's three parts were called.
#[derive(Default)]
struct Calls {
    token: AtomicUsize,
    checked: AtomicUsize,
    refreshed: AtomicUsize,
}

/// A refreshable credential whose `token` yields `stale`, which reads a 401 whose body is
/// `expired` as a rejection, and whose `on_rejected` yields `refreshed` (or fails, given `None`)
/// after checking it was handed `stale`. Each call is counted in `calls`.
fn refreshable(calls: &Arc<Calls>, refreshed: Option<&'static str>) -> Credential {
    let (token_calls, check_calls, refresh_calls) = (calls.clone(), calls.clone(), calls.clone());
    Credential::refreshable(
        token_provider(move || {
            token_calls.token.fetch_add(1, Ordering::SeqCst);
            async { Ok(SecretString::from("stale")) }
        }),
        move |response| {
            check_calls.checked.fetch_add(1, Ordering::SeqCst);
            response.status() == 401 && response.body().as_ref() == b"expired"
        },
        move |stale| {
            refresh_calls.refreshed.fetch_add(1, Ordering::SeqCst);
            assert_eq!(stale.expose_secret(), "stale");
            async move {
                refreshed
                    .map(SecretString::from)
                    .ok_or_else(|| AuthError::new("refresh refused"))
            }
        },
    )
}

/// One call as generated code makes it: attach through [`attach_auth_replayable`], send, and hand
/// the response to [`replay_rejected`]. Returns the response and the body read from it.
fn call(
    core: &ClientCore,
    request: RequestBuilder,
    requirements: &[&[AuthScheme]],
) -> (u16, http::HeaderMap, String) {
    let (request, replay) =
        poll_ready(attach_auth_replayable(core, request, requirements)).unwrap();
    let response = poll_ready(crate::send(core, request.build().unwrap())).unwrap();
    let response = poll_ready(replay_rejected(core, response, replay, false)).unwrap();
    let (parts, body) = response.into_parts();
    let body = poll_ready(body.collect()).unwrap();
    (
        parts.status.as_u16(),
        parts.headers,
        String::from_utf8(body.to_vec()).unwrap(),
    )
}

fn core_over(backend: &Arc<SequenceBackend>) -> ClientCore {
    ClientCore::with_backend(backend.clone(), "https://example.com").unwrap()
}

#[test]
fn a_rejected_token_is_refreshed_and_the_call_replayed_once() {
    let backend = SequenceBackend::new([Reply::status(401, "expired"), Reply::status(200, "ok")]);
    let mut core = core_over(&backend);
    let calls = Arc::new(Calls::default());
    core.set_credential("token", refreshable(&calls, Some("fresh")));

    let (status, _, body) = call(&core, get(&core), &[BEARER]);
    assert_eq!((status, body.as_str()), (200, "ok"));
    assert_eq!(
        backend.header("authorization"),
        [
            Some("Bearer stale".to_owned()),
            Some("Bearer fresh".to_owned())
        ]
    );
    // The replay carries the refreshed token; `token` is asked only for the first send.
    assert_eq!(calls.token.load(Ordering::SeqCst), 1);
    assert_eq!(calls.refreshed.load(Ordering::SeqCst), 1);
}

/// A `multipart/form-data` body is encoded in memory, so a call carrying one is replayed after a
/// refresh like any other, the replay sending the same `Content-Type` and the same bytes (#1289: a
/// form sent as a stream was never replayed).
#[test]
fn a_rejected_multipart_call_is_refreshed_and_replayed_with_its_body() {
    let backend = SequenceBackend::new([Reply::status(401, "expired"), Reply::status(200, "ok")]);
    let mut core = core_over(&backend);
    let calls = Arc::new(Calls::default());
    core.set_credential("token", refreshable(&calls, Some("fresh")));

    let request = request_to(&core, reqwest::Method::POST, "https://example.com/pets").multipart(
        MultipartForm::new()
            .part("name", MultipartPart::text("Rex"))
            .part("tag", MultipartPart::text("dog")),
    );
    let (status, _, body) = call(&core, request, &[BEARER]);
    assert_eq!((status, body.as_str()), (200, "ok"));
    assert_eq!(
        backend.header("authorization"),
        [
            Some("Bearer stale".to_owned()),
            Some("Bearer fresh".to_owned())
        ]
    );
    let bodies = backend.bodies();
    let sent = bodies[0].as_deref().expect("an in-memory body");
    let sent = std::str::from_utf8(sent).unwrap();
    assert!(sent.contains("name=\"name\"\r\n\r\nRex\r\n"), "{sent}");
    assert!(sent.contains("name=\"tag\"\r\n\r\ndog\r\n"), "{sent}");
    assert_eq!(bodies[0], bodies[1]);
    let content_type = backend.header("content-type");
    assert!(content_type[0]
        .as_deref()
        .is_some_and(|value| value.starts_with("multipart/form-data; boundary=")));
    assert_eq!(content_type[0], content_type[1]);
    assert_eq!(calls.refreshed.load(Ordering::SeqCst), 1);
}

/// Not every 401 is a stale token: a refusal the check does not read as one (a wrong password) is
/// returned as it came, with its body intact, and nothing is refreshed or resent.
#[test]
fn a_failure_the_check_does_not_read_as_a_rejection_is_returned_without_a_refresh() {
    let backend = SequenceBackend::new([
        Reply::status(401, "wrong_password"),
        Reply::status(200, "ok"),
    ]);
    let mut core = core_over(&backend);
    let calls = Arc::new(Calls::default());
    core.set_credential("token", refreshable(&calls, Some("fresh")));

    let (status, _, body) = call(&core, get(&core), &[BEARER]);
    assert_eq!((status, body.as_str()), (401, "wrong_password"));
    assert_eq!(backend.calls(), 1);
    assert_eq!(calls.checked.load(Ordering::SeqCst), 1);
    assert_eq!(calls.refreshed.load(Ordering::SeqCst), 0);
}

#[test]
fn a_success_is_returned_without_reading_the_check() {
    let backend = SequenceBackend::new([Reply::status(200, "ok")]);
    let mut core = core_over(&backend);
    let calls = Arc::new(Calls::default());
    core.set_credential("token", refreshable(&calls, Some("fresh")));

    let (status, _, body) = call(&core, get(&core), &[BEARER]);
    assert_eq!((status, body.as_str()), (200, "ok"));
    assert_eq!(calls.checked.load(Ordering::SeqCst), 0);
}

#[test]
fn a_failed_refresh_returns_the_original_response() {
    let backend = SequenceBackend::new([
        Reply::status(401, "expired").header("x-attempt", "first"),
        Reply::status(200, "ok"),
    ]);
    let mut core = core_over(&backend);
    let calls = Arc::new(Calls::default());
    core.set_credential("token", refreshable(&calls, None));

    let (status, headers, body) = call(&core, get(&core), &[BEARER]);
    assert_eq!((status, body.as_str()), (401, "expired"));
    assert_eq!(headers["x-attempt"], "first");
    assert_eq!(backend.calls(), 1);
    assert_eq!(calls.refreshed.load(Ordering::SeqCst), 1);
}

/// A replay is sent at most once: rejected again, the call returns the first response, not the
/// replay's, and the credential is not refreshed a second time.
#[test]
fn a_replay_rejected_again_returns_the_original_response() {
    let backend = SequenceBackend::new([
        Reply::status(401, "expired").header("x-attempt", "first"),
        Reply::status(401, "expired").header("x-attempt", "replay"),
        Reply::status(200, "ok"),
    ]);
    let mut core = core_over(&backend);
    let calls = Arc::new(Calls::default());
    core.set_credential("token", refreshable(&calls, Some("fresh")));

    let (status, headers, body) = call(&core, get(&core), &[BEARER]);
    assert_eq!((status, body.as_str()), (401, "expired"));
    assert_eq!(headers["x-attempt"], "first");
    assert_eq!(backend.calls(), 2);
    assert_eq!(calls.refreshed.load(Ordering::SeqCst), 1);
}

/// A replay that fails for another reason than the token is the call's answer: the refreshed token
/// was accepted, and what the server said next is what the caller needs.
#[test]
fn a_replay_failing_for_another_reason_returns_its_own_response() {
    let backend = SequenceBackend::new([
        Reply::status(401, "expired"),
        Reply::status(403, "forbidden"),
    ]);
    let mut core = core_over(&backend);
    let calls = Arc::new(Calls::default());
    core.set_credential("token", refreshable(&calls, Some("fresh")));

    let (status, _, body) = call(&core, get(&core), &[BEARER]);
    assert_eq!((status, body.as_str()), (403, "forbidden"));
    assert_eq!(backend.calls(), 2);
}

/// A credential replaced by one that is not refreshable, or removed, between the send and
/// `replay_rejected` is not refreshed: the rejected response is the call's answer, unreplayed, and
/// neither the old credential's check nor its refresh is asked.
#[test]
fn a_credential_replaced_or_removed_after_the_send_is_not_refreshed() {
    for replace in [true, false] {
        let backend =
            SequenceBackend::new([Reply::status(401, "expired"), Reply::status(200, "ok")]);
        let mut core = core_over(&backend);
        let calls = Arc::new(Calls::default());
        core.set_credential("token", refreshable(&calls, Some("fresh")));
        let (request, replay) =
            poll_ready(attach_auth_replayable(&core, get(&core), &[BEARER])).unwrap();
        assert!(replay.is_some());
        let response = poll_ready(crate::send(&core, request.build().unwrap())).unwrap();

        if replace {
            core.set_credential("token", Credential::Bearer(SecretString::from("other")));
        } else {
            assert!(core.remove_credential("token").is_some());
        }
        let response = poll_ready(replay_rejected(&core, response, replay, false)).unwrap();

        let status = response.status().as_u16();
        let body = poll_ready(response.into_body().collect()).unwrap();
        let checked = calls.checked.load(Ordering::SeqCst);
        let refreshed = calls.refreshed.load(Ordering::SeqCst);
        let observed = (status, &body[..], backend.calls(), checked, refreshed);
        assert_eq!(observed, (401, &b"expired"[..], 1, 0, 0), "{replace}");
    }
}

#[test]
fn a_replay_the_transport_fails_returns_the_original_response() {
    let backend = SequenceBackend::new([Reply::status(401, "expired"), Reply::failure(503)]);
    let mut core = core_over(&backend);
    let calls = Arc::new(Calls::default());
    core.set_credential("token", refreshable(&calls, Some("fresh")));

    let (status, _, body) = call(&core, get(&core), &[BEARER]);
    assert_eq!((status, body.as_str()), (401, "expired"));
    assert_eq!(backend.calls(), 2);
}

/// 1 KiB chunks against a 4 KiB `max_error_body`: four pulls reach the cap exactly, and the fifth
/// is the first past it, where reading stops, as `read_capped` stops.
const PULLS_FOR_4K_CAP: usize = 5;

/// A failed body `replay_rejected` handed on still classifies as the cut-short body it is: the
/// operation's error retains `max_error_body` bytes of it and marks them truncated.
fn assert_classified_as_truncated(core: &ClientCore, response: http::Response<crate::HttpBody>) {
    let (_, _, retained, truncated) =
        poll_ready(crate::dispatch::read_error_body::<std::convert::Infallible>(core, response))
            .unwrap();
    assert_eq!((retained.len(), truncated), (4096, true));
}

/// `max_error_body` bounds the read of a failed body `is_rejection` is handed, as it bounds the
/// error classifier's: a 1 MiB failed body stops being pulled at the first chunk past the cap, so
/// a refreshable credential does not hold a server's whole error body in memory.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_failed_body_the_check_reads_is_capped_at_max_error_body() {
    let backend = SequenceBackend::new([Reply::status(200, "ok")]);
    let mut core = core_over(&backend);
    core.config_mut().max_error_body = 4096;
    let seen = Arc::new(AtomicUsize::new(0));
    let seen_by_check = seen.clone();
    core.set_credential(
        "token",
        Credential::refreshable(
            token_provider(|| async { Ok(SecretString::from("stale")) }),
            move |response| {
                seen_by_check.store(response.body().len(), Ordering::SeqCst);
                false
            },
            |_| async { Ok(SecretString::from("fresh")) },
        ),
    );
    let (_, replay) = poll_ready(attach_auth_replayable(&core, get(&core), &[BEARER])).unwrap();
    let body = crate::tests::Chunks::repeat(1024, 1024);
    let pulled = body.pulled();
    let response = http::Response::builder()
        .status(401)
        .body(crate::HttpBody::wrap(body))
        .unwrap();

    let response = poll_ready(replay_rejected(&core, response, replay, false)).unwrap();
    assert_eq!(pulled.load(Ordering::SeqCst), PULLS_FOR_4K_CAP);
    assert_eq!(seen.load(Ordering::SeqCst), PULLS_FOR_4K_CAP * 1024);
    assert_eq!(
        backend.calls(),
        0,
        "nothing was rejected, so nothing is resent"
    );
    assert_classified_as_truncated(&core, response);
}

/// Issue #1308: where `RequestOptions::max_body` is below `max_error_body`, the failed body
/// `is_rejection` is handed is read no further than `max_body`, up to the first chunk past it, and
/// the operation's error refuses it as `BodyTooLarge`, as it would with no refreshable credential.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_failed_body_the_check_reads_is_bounded_by_a_smaller_max_body() {
    let backend = SequenceBackend::new([Reply::status(200, "ok")]);
    let mut core = core_over(&backend);
    core.config_mut().max_error_body = 4096;
    core.set_options(crate::RequestOptions {
        max_body: Some(2048),
        ..crate::RequestOptions::default()
    });
    let seen = Arc::new(AtomicUsize::new(0));
    let seen_by_check = seen.clone();
    core.set_credential(
        "token",
        Credential::refreshable(
            token_provider(|| async { Ok(SecretString::from("stale")) }),
            move |response| {
                seen_by_check.store(response.body().len(), Ordering::SeqCst);
                false
            },
            |_| async { Ok(SecretString::from("fresh")) },
        ),
    );
    let (_, replay) = poll_ready(attach_auth_replayable(&core, get(&core), &[BEARER])).unwrap();
    let body = crate::tests::Chunks::repeat(1024, 1024);
    let pulled = body.pulled();
    let response = http::Response::builder()
        .status(401)
        .body(crate::HttpBody::wrap(body))
        .unwrap();
    let response = crate::dispatch::with_body_limits(&core, response);

    let response = poll_ready(replay_rejected(&core, response, replay, false)).unwrap();
    // Two pulls reach `max_body` exactly, and the third is the first past it.
    assert_eq!(pulled.load(Ordering::SeqCst), 3);
    assert_eq!(seen.load(Ordering::SeqCst), 3 * 1024);
    let error =
        poll_ready(crate::dispatch::read_error_body::<std::convert::Infallible>(&core, response))
            .unwrap_err();
    assert!(
        matches!(error, Error::BodyTooLarge { limit: 2048, status } if status == 401),
        "{error:?}"
    );
}

/// A transport answering a 401 its refreshable credential rejects, then a 1 MiB failed body in
/// 1 KiB chunks, counting the chunks pulled from it.
struct OversizedReplay {
    calls: AtomicUsize,
    pulled: std::sync::Mutex<Option<Arc<AtomicUsize>>>,
}

impl std::fmt::Debug for OversizedReplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OversizedReplay")
    }
}

impl crate::HttpBackend for OversizedReplay {
    fn execute(&self, _: http::Request<crate::HttpBody>) -> crate::ExecuteFuture<'_> {
        let body = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            crate::HttpBody::from("expired")
        } else {
            let body = crate::tests::Chunks::repeat(1024, 1024);
            *self.pulled.lock().unwrap() = Some(body.pulled());
            crate::HttpBody::wrap(body)
        };
        let response = http::Response::builder().status(401).body(body).unwrap();
        Box::pin(async move { Ok(response) })
    }
}

/// The replay's failed body is read under the same bound before `is_rejection` reads it.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_failed_body_of_a_replay_is_capped_at_max_error_body() {
    let backend = Arc::new(OversizedReplay {
        calls: AtomicUsize::new(0),
        pulled: std::sync::Mutex::new(None),
    });
    let mut core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
    core.config_mut().max_error_body = 4096;
    let calls = Arc::new(Calls::default());
    core.set_credential("token", refreshable(&calls, Some("fresh")));
    let (request, replay) =
        poll_ready(attach_auth_replayable(&core, get(&core), &[BEARER])).unwrap();
    let response = poll_ready(crate::send(&core, request.build().unwrap())).unwrap();

    // The oversized body is not `expired`, so the replay's own response is the call's answer.
    let response = poll_ready(replay_rejected(&core, response, replay, false)).unwrap();
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    let pulled = backend.pulled.lock().unwrap().clone().unwrap();
    assert_eq!(pulled.load(Ordering::SeqCst), PULLS_FOR_4K_CAP);
    assert_classified_as_truncated(&core, response);
}

/// The replay is signed from the request as it stood before any credential was attached, so the
/// rest of the alternative re-attaches once (a static apiKey beside the refreshed token) and a
/// query apiKey is not appended a second time.
#[test]
fn a_replay_signs_the_whole_alternative_again_from_the_unauthenticated_request() {
    let backend = SequenceBackend::new([Reply::status(401, "expired"), Reply::status(200, "ok")]);
    let mut core = core_over(&backend);
    let calls = Arc::new(Calls::default());
    core.set_credential("session", refreshable(&calls, Some("fresh")));
    core.set_credential("tenant", Credential::ApiKey(SecretString::from("acme")));
    let alternative: &[AuthScheme] = &[
        AuthScheme {
            name: "session",
            kind: AuthKind::ApiKeyQuery("key"),
        },
        AuthScheme {
            name: "tenant",
            kind: AuthKind::ApiKeyHeader("X-Tenant"),
        },
    ];

    let (status, _, _) = call(&core, get(&core), &[alternative]);
    assert_eq!(status, 200);
    assert_eq!(
        backend.urls(),
        [
            "https://example.com/op?key=stale",
            "https://example.com/op?key=fresh"
        ]
    );
    assert_eq!(
        backend.header("x-tenant"),
        [Some("acme".to_owned()), Some("acme".to_owned())]
    );
}

#[test]
fn an_alternative_without_a_refreshable_credential_has_no_replay() {
    let mut core = core();
    core.set_credential("token", Credential::Bearer(SecretString::from("t0k")));
    let (_, replay) = poll_ready(attach_auth_replayable(&core, get(&core), &[BEARER])).unwrap();
    assert!(replay.is_none());

    let (_, replay) = poll_ready(attach_auth_replayable(&core, get(&core), &[])).unwrap();
    assert!(replay.is_none());
}

/// A streaming upload is read once, so there is nothing to replay: the rejected response is the
/// call's answer, and the check is never asked.
#[test]
fn a_request_whose_body_cannot_be_cloned_is_not_replayed() {
    use crate::tests::{Chunk, Chunks};

    let backend = SequenceBackend::new([Reply::status(401, "expired"), Reply::status(200, "ok")]);
    let mut core = core_over(&backend);
    let calls = Arc::new(Calls::default());
    core.set_credential("token", refreshable(&calls, Some("fresh")));
    let upload = request_to(&core, reqwest::Method::POST, "https://example.com/op")
        .body(crate::HttpBody::wrap(Chunks::new([Chunk::data("ab")])));

    let (status, _, body) = call(&core, upload, &[BEARER]);
    assert_eq!((status, body.as_str()), (401, "expired"));
    assert_eq!(backend.calls(), 1);
    assert_eq!(calls.checked.load(Ordering::SeqCst), 0);
}

/// Through [`attach_auth`], which never replays, a refreshable credential attaches as a provider
/// does: its `token` is asked each call, a failing `token` is a `CredentialProvider` error, and an
/// `http basic` scheme rejects it by name without asking it.
#[test]
fn a_refreshable_credential_attaches_as_a_provider_does() {
    let calls = Arc::new(Calls::default());
    let mut core = core();
    core.set_credential("token", refreshable(&calls, Some("fresh")));
    let request = poll_ready(attach_auth(&core, get(&core), &[BEARER]))
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(
        request.headers()[reqwest::header::AUTHORIZATION],
        "Bearer stale"
    );
    assert_eq!(calls.token.load(Ordering::SeqCst), 1);

    let basic: &[AuthScheme] = &[AuthScheme {
        name: "token",
        kind: AuthKind::Basic,
    }];
    let error = poll_ready(attach_auth(&core, get(&core), &[basic])).unwrap_err();
    assert!(
        matches!(
            error,
            Error::RequestConstruction(RequestError::CredentialMismatch {
                registered: CredentialKind::Refreshable,
                required: SchemeCarrier::HttpBasic,
                ..
            })
        ),
        "{error:?}"
    );
    assert_eq!(calls.token.load(Ordering::SeqCst), 1);

    core.set_credential(
        "token",
        Credential::refreshable(
            token_provider(|| async { Err(AuthError::new("no session")) }),
            |_| true,
            |_| async { Ok(SecretString::from("fresh")) },
        ),
    );
    let error = poll_ready(attach_auth_replayable(&core, get(&core), &[BEARER])).unwrap_err();
    assert!(
        matches!(
            error,
            Error::RequestConstruction(RequestError::CredentialProvider {
                scheme: "token",
                ..
            })
        ),
        "{error:?}"
    );
}
