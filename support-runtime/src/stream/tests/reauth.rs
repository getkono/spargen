//! How an `EventStream` signs its reconnects (#1398): afresh from the request as it stood before
//! its credentials were attached, with the credentials registered when the call was made, a token
//! provider asked again, and a rejected reconnect refreshed and replayed once.

use std::sync::{Arc, Mutex};

use super::{sse_reply, EventStream, Framing, ImmediateReconnect, Resend};
use crate::tests::{poll_ready, Reply, SequenceBackend};
use crate::{ClientCore, Error};
use reqwest::Method;

/// The bearer scheme the re-signing tests' operation requires.
const BEARER: &[&[crate::AuthScheme]] = &[&[crate::AuthScheme {
    name: "bearer",
    kind: crate::AuthKind::Bearer,
}]];

/// Open a reconnectable SSE stream on `core` the way a generated streaming operation does: sign
/// the request for [`BEARER`], prepare it, send it through [`crate::execute`] (which replays it
/// once after a rejected refreshable credential), and hand the stream what it needs to reconnect.
fn signed_stream(core: &ClientCore) -> EventStream<serde_json::Value> {
    let request = crate::tests::builder(Method::GET, "https://example.com/events");
    let (request, replay, resend) = poll_ready(Resend::attach(core, request, BEARER)).unwrap();
    let request = crate::prepare(core, request.build().unwrap(), true);
    let resend = resend.map(|resend| resend.carrying(&request));
    let response = poll_ready(crate::execute(core, request, replay, true)).unwrap();
    let policy = Arc::new(ImmediateReconnect {
        max_attempts: 1,
        ..ImmediateReconnect::default()
    });
    EventStream::new_reconnectable(response, Framing::SseJsonData, core, None)
        .resending(core, resend)
        .with_reconnect(policy)
        .unwrap()
}

/// Issue #1398: a stream whose first request was rejected, refreshed and replayed reconnects with
/// the refreshed token, not the one the server already rejected.
#[test]
fn a_reconnect_after_a_refreshed_replay_carries_the_refreshed_token() {
    let backend = SequenceBackend::new([
        Reply::status(401, ""),
        sse_reply(200, "data: {\"seq\":1}\n\n"),
        sse_reply(200, "data: {\"seq\":2}\n\n"),
    ]);
    let mut core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
    // `on_rejected` stores the fresh token where `token` reads it, as a refresh that outlives
    // the replay does.
    let current = Arc::new(Mutex::new("stale".to_owned()));
    let read = Arc::clone(&current);
    let store = Arc::clone(&current);
    core.set_credential(
        "bearer",
        crate::Credential::refreshable(
            crate::token_provider(move || {
                let token = read.lock().unwrap().clone();
                async move { Ok(crate::SecretString::from(token)) }
            }),
            |response| response.status() == 401,
            move |_stale| {
                *store.lock().unwrap() = "fresh".to_owned();
                async { Ok(crate::SecretString::from("fresh")) }
            },
        ),
    );
    let mut stream = signed_stream(&core);

    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 1})
    );
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 2})
    );
    assert_eq!(
        backend.header("authorization"),
        [
            Some("Bearer stale".to_owned()),
            Some("Bearer fresh".to_owned()),
            Some("Bearer fresh".to_owned()),
        ]
    );
}

/// Issue #1398: a [`crate::Credential::Provider`] is asked for a token again on each reconnect,
/// so a long-lived stream reconnects with a current token rather than the first one.
#[test]
fn a_reconnect_asks_a_token_provider_again() {
    let backend = SequenceBackend::new([
        sse_reply(200, "data: {\"seq\":1}\n\n"),
        sse_reply(200, "data: {\"seq\":2}\n\n"),
    ]);
    let mut core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
    let asked = Arc::new(Mutex::new(0_u32));
    let counter = Arc::clone(&asked);
    core.set_credential(
        "bearer",
        crate::Credential::Provider(crate::token_provider(move || {
            let mut asked = counter.lock().unwrap();
            *asked += 1;
            let token = format!("t{asked}");
            async move { Ok(crate::SecretString::from(token)) }
        })),
    );
    let mut stream = signed_stream(&core);

    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 1})
    );
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 2})
    );
    assert_eq!(
        backend.header("authorization"),
        [Some("Bearer t1".to_owned()), Some("Bearer t2".to_owned())]
    );
    assert_eq!(*asked.lock().unwrap(), 2);
}

/// A reconnect whose token is rejected is refreshed and replayed once, as a call is, and the replay
/// still carries the stream's `Last-Event-ID`.
#[test]
fn a_rejected_reconnect_is_refreshed_and_replayed_with_its_last_event_id() {
    let backend = SequenceBackend::new([
        sse_reply(200, "id: evt-1\ndata: {\"seq\":1}\n\n"),
        Reply::status(401, ""),
        sse_reply(200, "data: {\"seq\":2}\n\n"),
    ]);
    let mut core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
    core.set_credential(
        "bearer",
        crate::Credential::refreshable(
            crate::token_provider(|| async { Ok(crate::SecretString::from("stale")) }),
            |response| response.status() == 401,
            |_stale| async { Ok(crate::SecretString::from("fresh")) },
        ),
    );
    let mut stream = signed_stream(&core);

    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 1})
    );
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 2})
    );
    assert_eq!(
        backend.header("authorization"),
        [
            Some("Bearer stale".to_owned()),
            Some("Bearer stale".to_owned()),
            Some("Bearer fresh".to_owned()),
        ]
    );
    assert_eq!(
        backend.header("last-event-id"),
        [None, Some("evt-1".to_owned()), Some("evt-1".to_owned())]
    );
}

/// The stream signs its reconnects with the credentials registered when the call was made: one
/// registered on the client afterwards does not reach it.
#[test]
fn a_reconnect_is_signed_with_the_credentials_registered_when_the_call_was_made() {
    let backend = SequenceBackend::new([
        sse_reply(200, "data: {\"seq\":1}\n\n"),
        sse_reply(200, "data: {\"seq\":2}\n\n"),
    ]);
    let mut core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
    core.set_credential(
        "bearer",
        crate::Credential::Bearer(crate::SecretString::from("first")),
    );
    let mut stream = signed_stream(&core);
    core.set_credential(
        "bearer",
        crate::Credential::Bearer(crate::SecretString::from("later")),
    );

    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 1})
    );
    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 2})
    );
    assert_eq!(
        backend.header("authorization"),
        [
            Some("Bearer first".to_owned()),
            Some("Bearer first".to_owned()),
        ]
    );
}

/// A token provider failing on a reconnect sends nothing: the failure is offered to the policy,
/// and yielded once the policy declines it.
#[test]
fn a_token_provider_failing_on_a_reconnect_is_offered_to_the_policy() {
    let backend = SequenceBackend::new([sse_reply(200, "data: {\"seq\":1}\n\n")]);
    let mut core = ClientCore::with_backend(backend.clone(), "https://example.com").unwrap();
    let asked = Arc::new(Mutex::new(0_u32));
    let counter = Arc::clone(&asked);
    core.set_credential(
        "bearer",
        crate::Credential::Provider(crate::token_provider(move || {
            let mut asked = counter.lock().unwrap();
            *asked += 1;
            let first = *asked == 1;
            async move {
                if first {
                    Ok(crate::SecretString::from("t1"))
                } else {
                    Err(crate::AuthError::new("token endpoint down"))
                }
            }
        })),
    );
    let mut stream = signed_stream(&core);

    assert_eq!(
        poll_ready(stream.next()).unwrap().unwrap(),
        serde_json::json!({"seq": 1})
    );
    match poll_ready(stream.next()) {
        Some(Err(Error::RequestConstruction(crate::RequestError::CredentialProvider {
            scheme: "bearer",
            ..
        }))) => {}
        other => panic!("expected CredentialProvider, got {other:?}"),
    }
    assert!(poll_ready(stream.next()).is_none());
    assert_eq!(backend.calls(), 1);
    assert_eq!(*asked.lock().unwrap(), 2);
}
