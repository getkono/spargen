//! A mock HTTP/1.1 server for driving a generated client over a real socket. `e2e.rs` writes this
//! file into each fixture crate whose tests serve their client, as `tests/common/mod.rs`, and
//! declares it as its own `mock_server` module, so it compiles in the suite as well as in every
//! fixture, and the suite's own process can serve a fixture's client too.
//!
//! Each server binds a fresh loopback port and accepts one connection per response it is given, in
//! order. It reads each request whole (the head up to its blank line, then the body its
//! `Content-Length` declares, or a chunked body through its zero-size chunk and trailer section),
//! answers with that response's bytes, and hands back every request it read through its join
//! handle.

#![allow(
    dead_code,
    reason = "each crate that declares this module calls only the helpers its tests need"
)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::JoinHandle;

/// A request as the server read it.
pub struct Request {
    /// The request line and the header lines as sent, without the blank line that ends them.
    pub head: String,
    /// The body as sent, chunk framing included when it was chunked.
    pub body: Vec<u8>,
}

impl Request {
    /// The request line.
    pub fn line(&self) -> &str {
        self.head.lines().next().unwrap_or_default()
    }

    /// Every value of the header `name`, in order.
    pub fn header(&self, name: &str) -> Vec<&str> {
        header(&self.head, name)
    }

    /// The whole request as text: the head, the blank line, and the body.
    pub fn text(&self) -> String {
        format!(
            "{}\r\n\r\n{}",
            self.head,
            String::from_utf8_lossy(&self.body)
        )
    }
}

/// Every value of the header `name`, matched without regard to case, in the request head that
/// `request` starts with, in order. The request line and everything after the head's blank line are
/// skipped.
pub fn header<'a>(request: &'a str, name: &str) -> Vec<&'a str> {
    request
        .lines()
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_once(':'))
        .filter(|(field, _)| field.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
        .collect()
}

/// A response with `status` (`"200 OK"`), each of `headers`, and `body`, framed by its
/// `Content-Length` and closing the connection.
pub fn response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// A `204 No Content` response.
pub fn no_content() -> Vec<u8> {
    response("204 No Content", &[], b"")
}

/// Serve one connection per entry of `responses`, in order. Returns the base URL,
/// `http://127.0.0.1:<port>`, and a handle yielding the requests read, in the order they arrived.
pub fn serve(responses: Vec<Vec<u8>>) -> (String, JoinHandle<Vec<Request>>) {
    let (listener, base) = bind();
    let server = std::thread::spawn(move || {
        responses
            .iter()
            .map(|response| answer(&listener, response))
            .collect()
    });
    (base, server)
}

/// Serve one connection with `response`, as [`serve`] does.
pub fn serve_one(response: Vec<u8>) -> (String, JoinHandle<Request>) {
    let (listener, base) = bind();
    let server = std::thread::spawn(move || answer(&listener, &response));
    (base, server)
}

/// Serve one connection with a `status` response whose body is `body`, of type `content_type`.
pub fn serve_once(content_type: &str, status: &str, body: &[u8]) -> (String, JoinHandle<Request>) {
    serve_one(response(status, &[("Content-Type", content_type)], body))
}

/// Serve one connection with a `204 No Content` response.
pub fn serve_no_content() -> (String, JoinHandle<Request>) {
    serve_one(no_content())
}

fn bind() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    (listener, base)
}

/// Accept one connection on `listener`, read its request whole, and answer it with `response`.
fn answer(listener: &TcpListener, response: &[u8]) -> Request {
    let (mut stream, _) = listener.accept().unwrap();
    let request = read_request(&mut stream);
    stream.write_all(response).unwrap();
    stream.flush().unwrap();
    request
}

/// Read one request from `stream`: its head, then the body its `Content-Length` declares or, for a
/// chunked body, every chunk through the zero-size one and its trailer section
/// ([`chunked_body_end`]).
fn read_request(stream: &mut TcpStream) -> Request {
    let mut received = Vec::new();
    let mut buf = [0u8; 8192];
    let mut read_more = |received: &mut Vec<u8>| {
        let read = stream.read(&mut buf).unwrap();
        assert!(
            read > 0,
            "the connection closed before the request ended: {}",
            String::from_utf8_lossy(received)
        );
        received.extend_from_slice(&buf[..read]);
    };
    let head_end = loop {
        if let Some(end) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        read_more(&mut received);
    };
    let head = String::from_utf8_lossy(&received[..head_end]).into_owned();
    let mut body = received.split_off(head_end + 4);
    let chunked = header(&head, "transfer-encoding")
        .iter()
        .any(|value| value.eq_ignore_ascii_case("chunked"));
    let length = header(&head, "content-length")
        .first()
        .map_or(0, |value| value.parse::<usize>().unwrap());
    let complete = |body: &[u8]| {
        if chunked {
            chunked_body_end(body).is_some()
        } else {
            body.len() >= length
        }
    };
    while !complete(&body) {
        read_more(&mut body);
    }
    Request { head, body }
}

/// Where the chunked body that `body` starts with ends: after the blank line closing the trailer
/// section that follows its zero-size chunk. `None` while any of it is still to arrive. Each
/// chunk-size line is read, so a chunk whose data holds `0\r\n\r\n` does not end the body.
pub fn chunked_body_end(body: &[u8]) -> Option<usize> {
    let line_end = |from: usize| {
        body.get(from..)?
            .windows(2)
            .position(|window| window == b"\r\n")
            .map(|offset| from + offset)
    };
    let mut at = 0;
    loop {
        let end = line_end(at)?;
        let line = String::from_utf8_lossy(&body[at..end]);
        let size = line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size, 16)
            .unwrap_or_else(|_| panic!("a malformed chunk-size line: {line:?}"));
        at = end + 2;
        if size == 0 {
            loop {
                let end = line_end(at)?;
                let blank = end == at;
                at = end + 2;
                if blank {
                    return Some(at);
                }
            }
        }
        at += size + 2;
    }
}
