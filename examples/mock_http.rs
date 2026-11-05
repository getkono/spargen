//! The tiny HTTP/1.1 mock the petstore examples serve on an ephemeral localhost port, so running
//! them repeatedly reaches no external service. Each example includes this file with
//! `#[path = "../../mock_http.rs"] mod mock_http;` and supplies its own routes as a [`Route`].

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU32, Ordering};

/// How many requests the mock has read a request line for, so a scenario can assert that a
/// pre-send failure put nothing on the wire.
static REQUESTS_READ: AtomicU32 = AtomicU32::new(0);

/// A request as the mock read it off the wire.
pub struct Request {
    pub method: String,
    /// The request target up to its `?`.
    pub path: String,
    /// The request target after its `?`, empty when there is none.
    pub query: String,
    /// Each header line's name, lowercased, and its trimmed value, in arrival order.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The value of the last header named `name` (lowercase), as a repeated header's last line
    /// wins.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .rev()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A JSON response: its status line, the headers a route adds beside `Content-Type` and
/// `Content-Length`, and its body.
pub struct Response {
    pub status: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

impl Response {
    pub fn new(status: &'static str, body: impl Into<String>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }
}

/// The routes an example serves: one response per request.
pub type Route = fn(Request) -> Response;

/// Serves `route` on an ephemeral `127.0.0.1` port, one thread per connection, and returns the
/// address it listens on.
pub fn spawn(route: Route) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || handle(stream, route));
        }
    });
    addr
}

/// How many requests the mock has read a request line for since the process started.
pub fn requests_read() -> u32 {
    REQUESTS_READ.load(Ordering::SeqCst)
}

fn handle(mut stream: TcpStream, route: Route) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    REQUESTS_READ.fetch_add(1, Ordering::SeqCst);
    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return;
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let (name, value) = (name.to_ascii_lowercase(), value.trim().to_owned());
        if name == "content-length" {
            content_length = value.parse().unwrap_or(0);
        }
        headers.push((name, value));
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }

    let response = route(Request {
        method: method.to_owned(),
        path: path.to_owned(),
        query: query.to_owned(),
        headers,
        body,
    });
    let extra_headers: String = response
        .headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    let _ = write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: application/json\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.status,
        response.body.len(),
        response.body,
    );
}
