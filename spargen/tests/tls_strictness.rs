//! `spargen lock`'s real fetcher against a deliberately **non-conformant** TLS 1.3 peer.
//!
//! A conformant peer behaves identically under every rustls release, so a test built on one
//! cannot notice a change in what the fetcher accepts. RUSTSEC-2026-0285, fixed by the
//! `rustls 0.23.45` floor in `spargen/Cargo.toml`, is exactly such a change: releases before it
//! accepted a plaintext handshake record that carried a key-changing message *and* a further
//! handshake message, which RFC 8446 §5.1 forbids ("Handshake messages MUST NOT span key
//! changes"). The peer here sends that record, so this suite fails on a rustls that relaxes the
//! rule again.
//!
//! The key-changing message is a HelloRetryRequest, which the client acts on before any key
//! exchange, so the peer needs no certificate, no key share, and no crypto: it reads the
//! ClientHello, echoes its session id, and writes raw records. It runs before certificate
//! verification, so the fetcher's bundled web roots are never consulted and no trust seam is
//! needed. A control scenario sends the same HelloRetryRequest in a record of its own and
//! requires the client to answer it with a second ClientHello, which proves the message itself
//! is well-formed: the only thing the rejected scenario adds is the joined message.
//!
//! What this pins is the one strictness change an advisory has named. It is not a conformance
//! suite: spargen delegates TLS conformance to rustls, and the README states that position.

#![cfg(feature = "remote-fetch")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::JoinHandle;
use std::time::Duration;

use camino::Utf8PathBuf;
use spargen::{Code, Outcome, Spec};

const HANDSHAKE: u8 = 0x16;
const ALERT: u8 = 0x15;
const CHANGE_CIPHER_SPEC: u8 = 0x14;
const CLIENT_HELLO: u8 = 0x01;
const SERVER_HELLO: u8 = 0x02;
const ENCRYPTED_EXTENSIONS: u8 = 0x08;
/// RFC 8446 §4.1.3: a ServerHello with this `random` is a HelloRetryRequest.
const HRR_RANDOM: [u8; 32] = [
    0xCF, 0x21, 0xAD, 0x74, 0xE5, 0x9A, 0x61, 0x11, 0xBE, 0x1D, 0x8C, 0x02, 0x1E, 0x65, 0xB8, 0x91,
    0xC2, 0xA2, 0x11, 0x16, 0x7A, 0xBB, 0x8C, 0x5E, 0x07, 0x9E, 0x09, 0xE2, 0xC8, 0xA8, 0x33, 0x9C,
];
/// `fatal`, `unexpected_message` (RFC 8446 §6).
const FATAL_UNEXPECTED_MESSAGE: [u8; 2] = [0x02, 0x0A];

/// What the peer writes after reading the ClientHello.
#[derive(Clone, Copy)]
enum Flight {
    /// The HelloRetryRequest alone in its own record: conformant.
    RetryAlone,
    /// The HelloRetryRequest and an EncryptedExtensions in one plaintext record: the second
    /// message would be read across the key change the first one makes.
    RetryJoinedWithNextMessage,
}

/// What the client sent after the peer's flight, in order, until it sent an alert, a second
/// ClientHello, or closed the connection.
#[derive(Debug, PartialEq)]
enum Reply {
    SecondClientHello,
    Alert([u8; 2]),
    Closed,
}

/// A single-connection TLS peer on `127.0.0.1`, answering the first ClientHello with `flight`.
fn start_peer(flight: Flight) -> (String, JoinHandle<Reply>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("https://{}/pet.yaml", listener.local_addr().unwrap());
    // Accept exactly one connection and then drop the listener, so a retrying client meets a
    // refused connection instead of a peer that never answers.
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        drop(listener);
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let session_id = read_client_hello(&mut stream);
        stream
            .write_all(&flight_bytes(flight, &session_id))
            .unwrap();
        observe_reply(&mut stream)
    });
    (url, peer)
}

/// Read one TLS record, or `None` at end of stream.
fn read_record(stream: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
    let mut header = [0u8; 5];
    stream.read_exact(&mut header).ok()?;
    let length = usize::from(u16::from_be_bytes([header[3], header[4]]));
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).ok()?;
    Some((header[0], body))
}

/// Read the ClientHello and return its `legacy_session_id`, which the retry must echo.
fn read_client_hello(stream: &mut TcpStream) -> Vec<u8> {
    let (kind, body) = read_record(stream).expect("the client sends a ClientHello");
    assert_eq!(kind, HANDSHAKE);
    assert_eq!(body[0], CLIENT_HELLO);
    // type (1) + length (3) + legacy_version (2) + random (32), then the session id.
    let at = 1 + 3 + 2 + 32;
    let length = usize::from(body[at]);
    body[at + 1..at + 1 + length].to_vec()
}

/// A handshake message: type, 24-bit length, body.
fn handshake_message(kind: u8, body: &[u8]) -> Vec<u8> {
    let length = u32::try_from(body.len()).unwrap().to_be_bytes();
    let mut message = vec![kind, length[1], length[2], length[3]];
    message.extend_from_slice(body);
    message
}

/// A HelloRetryRequest the client can act on: it echoes the session id, selects
/// `TLS_AES_128_GCM_SHA256` and TLS 1.3, and asks for a `secp256r1` key share, a group rustls
/// supports but does not send a share for on its first ClientHello.
fn hello_retry_request(session_id: &[u8]) -> Vec<u8> {
    let mut body = vec![0x03, 0x03];
    body.extend_from_slice(&HRR_RANDOM);
    body.push(u8::try_from(session_id.len()).unwrap());
    body.extend_from_slice(session_id);
    body.extend_from_slice(&[0x13, 0x01, 0x00]);
    let extensions: &[u8] = &[
        0x00, 0x2B, 0x00, 0x02, 0x03, 0x04, // supported_versions: TLS 1.3
        0x00, 0x33, 0x00, 0x02, 0x00, 0x17, // key_share: selected_group secp256r1
    ];
    body.extend_from_slice(&u16::try_from(extensions.len()).unwrap().to_be_bytes());
    body.extend_from_slice(extensions);
    handshake_message(SERVER_HELLO, &body)
}

fn record(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![kind, 0x03, 0x03];
    bytes.extend_from_slice(&u16::try_from(payload.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

fn flight_bytes(flight: Flight, session_id: &[u8]) -> Vec<u8> {
    let mut payload = hello_retry_request(session_id);
    if let Flight::RetryJoinedWithNextMessage = flight {
        // An empty EncryptedExtensions: a complete message that is legal only once the
        // handshake keys are in use, riding in the same plaintext record.
        payload.extend(handshake_message(ENCRYPTED_EXTENSIONS, &[0x00, 0x00]));
    }
    record(HANDSHAKE, &payload)
}

/// Skip the middlebox-compatibility ChangeCipherSpec and report what the client did next.
fn observe_reply(stream: &mut TcpStream) -> Reply {
    loop {
        match read_record(stream) {
            None => return Reply::Closed,
            Some((CHANGE_CIPHER_SPEC, _)) => {}
            Some((ALERT, body)) => return Reply::Alert([body[0], body[1]]),
            Some((HANDSHAKE, body)) if body.first() == Some(&CLIENT_HELLO) => {
                return Reply::SecondClientHello;
            }
            Some((kind, body)) => panic!("unexpected record {kind:#04x}: {body:02x?}"),
        }
    }
}

/// A root description with one operation whose response schema is the remote `url`.
fn workspace(url: &str) -> (tempfile::TempDir, Utf8PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let dir = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).unwrap();
    let spec = dir.join("openapi.yaml");
    let document = format!(
        "openapi: 3.1.0\n\
         info: {{ title: T, version: 1.0.0 }}\n\
         paths:\n  \
         /pet:\n    \
         get:\n      \
         operationId: getPet\n      \
         responses:\n        \
         '200':\n          \
         description: ok\n          \
         content:\n            \
         application/json:\n              \
         schema: {{ $ref: '{url}' }}\n"
    );
    std::fs::write(&spec, document).unwrap();
    (temp, spec)
}

/// Run `spargen lock`'s fetch against `flight` and return the E025 message and the peer's view.
fn lock_against(flight: Flight) -> (String, Reply) {
    let (url, peer) = start_peer(flight);
    let (_temp, spec) = workspace(&url);

    let report = spargen::vendor(&Spec::new(spec.clone())).unwrap_err();
    let reply = peer.join().unwrap();

    assert_eq!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let diagnostics = report.diagnostics();
    assert_eq!(diagnostics.len(), 1, "{report:#?}");
    assert_eq!(diagnostics[0].code, Code::RemoteFetchFailed, "{report:#?}");
    let message = diagnostics[0].message.clone();
    assert!(message.contains(&url), "{message}");
    assert!(
        !spec.with_file_name(".spargen").exists(),
        "a failed fetch must vendor nothing"
    );
    (message, reply)
}

#[test]
fn a_retry_request_in_its_own_record_is_answered_with_a_second_client_hello() {
    let (_, reply) = lock_against(Flight::RetryAlone);

    // The retry is well-formed and acted on; the fetch then fails only because the peer hangs up.
    assert_eq!(reply, Reply::SecondClientHello);
}

#[test]
fn a_handshake_message_joined_to_a_retry_request_across_the_key_change_is_refused() {
    let (message, reply) = lock_against(Flight::RetryJoinedWithNextMessage);

    // The client refuses the record before acting on the retry: it never sends the second
    // ClientHello a conformant retry draws, and it names why, through the E025 message.
    assert_eq!(reply, Reply::Alert(FATAL_UNEXPECTED_MESSAGE));
    assert!(
        message.contains("peer misbehaved: KeyEpochWithPendingFragment"),
        "{message}"
    );
}
