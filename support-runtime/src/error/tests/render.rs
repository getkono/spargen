//! How an error renders (#1334): a `Debug` that redacts the response headers carrying secrets,
//! and a cause chain whose levels each print only their own text.

use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::StatusCode;

use super::{every_request_variant, every_variant, reqwest_error, ApiBody};
use crate::parameter::ParameterError;
use crate::{Error, ResponseValue, TimeoutKind, TransportError};

/// The value every sensitive response header below carries.
const HEADER_SECRET: &str = "session=HEADER-SECRET-77";

/// Response headers carrying [`HEADER_SECRET`] under every name `Debug` redacts, and under a
/// header whose value is marked sensitive, beside an ordinary header that stays readable.
fn headers_with_secrets() -> HeaderMap {
    let mut headers = HeaderMap::new();
    let secret = HeaderValue::from_static(HEADER_SECRET);
    headers.insert(reqwest::header::SET_COOKIE, secret.clone());
    headers.append(reqwest::header::SET_COOKIE, secret.clone());
    headers.insert(reqwest::header::AUTHORIZATION, secret.clone());
    headers.insert(reqwest::header::PROXY_AUTHENTICATE, secret.clone());
    let mut marked = secret;
    marked.set_sensitive(true);
    headers.insert("x-session-token", marked);
    headers.insert(
        reqwest::header::RETRY_AFTER,
        HeaderValue::from_static("120"),
    );
    headers
}

/// The derived `Debug` of the three response-carrying variants printed every response header, a
/// `Set-Cookie` session included, so `tracing::error!(?err)` logged it. Each redacted value now
/// prints as `[redacted]`, once per value, while every other header, the status, and the body
/// still print.
#[test]
fn debug_redacts_sensitive_response_headers_and_keeps_the_rest() {
    let errors: [Error<ApiBody>; 3] = [
        Error::Api(ResponseValue::new(
            StatusCode::TOO_MANY_REQUESTS,
            headers_with_secrets(),
            ApiBody("slow down"),
        )),
        Error::UnexpectedStatus {
            status: StatusCode::TOO_MANY_REQUESTS,
            headers: headers_with_secrets(),
            body: Bytes::from_static(b"raw-body-kept"),
        },
        Error::Decode {
            status: StatusCode::TOO_MANY_REQUESTS,
            headers: headers_with_secrets(),
            path: "items[0].id".to_owned(),
            body: Bytes::from_static(b"raw-body-kept"),
            truncated: false,
        },
    ];
    for error in errors {
        for debug in [format!("{error:?}"), format!("{error:#?}")] {
            assert!(!debug.contains("HEADER-SECRET"), "Debug leaks: {debug}");
            // Five redacted values: two `set-cookie`, and one each of the other three names.
            assert_eq!(debug.matches("[redacted]").count(), 5, "{debug}");
            for name in [
                "set-cookie",
                "authorization",
                "proxy-authenticate",
                "x-session-token",
            ] {
                assert!(
                    debug.contains(name),
                    "the header name {name} is kept: {debug}"
                );
            }
            assert!(debug.contains("\"retry-after\""), "{debug}");
            assert!(
                debug.contains("\"120\""),
                "an ordinary header is kept: {debug}"
            );
            assert!(debug.contains("429"), "the status is kept: {debug}");
            let body = match &error {
                Error::Api(_) => "slow down",
                _ => "raw-body-kept",
            };
            assert!(debug.contains(body), "the body is kept: {debug}");
        }
    }
}

/// The redacting `Debug` prints each variant under its own name and fields, as the derive did,
/// so a log line still says which class failed.
#[test]
fn debug_names_every_variant() {
    for error in every_variant() {
        let debug = format!("{error:?}");
        let name = match &error {
            Error::RequestConstruction(_) => "RequestConstruction(Other(",
            Error::Transport(_) => "Transport(TransportError { class: Request, source: ",
            Error::Timeout(_) => "Timeout(Total)",
            Error::Protocol(_) => "Protocol(ProtocolError { source: ",
            Error::Redirect(_) => "Redirect(RedirectError { source: ",
            Error::Api(_) => {
                "Api(ResponseValue { status: 400, headers: {}, inner: ApiBody(\"bad request\") })"
            }
            Error::UnexpectedStatus { .. } => {
                "UnexpectedStatus { status: 418, headers: {}, body: b\"\" }"
            }
            Error::Decode { .. } => {
                "Decode { status: 200, headers: {}, path: \"items[0].id\", body: b\"{}\", \
                 truncated: false }"
            }
            Error::InterruptedBody(_) => "InterruptedBody(TransportError { class: Interrupted, ",
            Error::PaginationCycle { .. } => "PaginationCycle { url: ",
            Error::PaginationOriginChanged { .. } => "PaginationOriginChanged { from: ",
            Error::BodyTooLarge { .. } => "BodyTooLarge { limit: 1024, status: 201 }",
        };
        assert!(debug.starts_with(name), "{debug:?} does not start {name:?}");
    }
}

/// Every error with a source, at the top of its chain: each taxonomy variant, each request
/// variant, and a transport error of each class standing alone, as a backend reports it.
fn every_error_with_a_source() -> Vec<Box<dyn std::error::Error + 'static>> {
    let mut errors: Vec<Box<dyn std::error::Error + 'static>> = Vec::new();
    for error in every_variant() {
        errors.push(Box::new(error));
    }
    for error in every_request_variant() {
        errors.push(Box::new(Error::<ApiBody>::RequestConstruction(error)));
    }
    for failure in [
        TransportError::connection(reqwest_error()),
        TransportError::timeout(TimeoutKind::Connect, reqwest_error()),
        TransportError::protocol(reqwest_error()),
        TransportError::redirect(reqwest_error()),
        TransportError::request(reqwest_error()),
        TransportError::interrupted(reqwest_error()),
    ] {
        errors.push(Box::new(failure));
    }
    let parameter = || ParameterError::from(serde_json::from_str::<u8>("x").unwrap_err());
    errors.push(Box::new(parameter()));
    errors.push(Box::new(
        Error::<ApiBody>::request_construction(parameter()),
    ));
    errors.retain(|error| error.source().is_some());
    errors
}

/// `RequestError::Other`, `TransportError`, `ProtocolError`, `RedirectError` and
/// `ParameterError::Serialize` printed `"label: {source}"` and returned that same cause from
/// `source()`, so a full-chain reporter (anyhow's `{:#}`, eyre) printed the cause twice. Walking the chain the way those reporters do,
/// no level's `Display` repeats the text a deeper level prints.
#[test]
fn the_full_cause_chain_repeats_no_segment() {
    let errors = every_error_with_a_source();
    assert!(errors.len() >= 15, "the table covers the sourced errors");
    for error in errors {
        let mut segments = Vec::new();
        let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error.as_ref());
        while let Some(level) = current {
            segments.push(level.to_string());
            current = level.source();
        }
        let chain = segments.join(": ");
        for (index, outer) in segments.iter().enumerate() {
            assert!(!outer.is_empty(), "an empty segment in {chain:?}");
            for inner in &segments[index + 1..] {
                assert!(
                    !outer.contains(inner.as_str()),
                    "{outer:?} repeats its cause {inner:?}: {chain:?}"
                );
            }
        }
    }
}
