//! The macro consumption path end to end: the client is generated **inline** by
//! `spargen_macro::generate_api!` — no build.rs, no `include!`, no CLI. This example proves the
//! macro-generated client compiles and drives real HTTP (against a local mock), and its Cargo.toml
//! proves no spargen crate is in the runtime graph.
//!
//! The full feature surface (auth failure modes, retry, undocumented statuses, …) is covered by
//! `examples/petstore`; this example is deliberately compact — its job is to exercise the *macro*.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};

use secrecy::SecretString;

// The entire client, generated at compile time from the spec beside this crate's Cargo.toml.
// The expansion carries no inner attributes, so it drops straight into a module.
mod petstore {
    spargen_macro::generate_api!("petstore.yaml");
}

use petstore::{types, Client, Credential, Error};

const TOKEN: &str = "let-me-in";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let addr = spawn_mock_server();
    let base_url = format!("http://{addr}");
    println!("mock petstore listening on {base_url}");

    let client = Client::new(&base_url)
        .unwrap()
        .with_credential("bearerAuth", Credential::Bearer(SecretString::from(TOKEN)));

    // A typed list response.
    let pets = client.list_pets(None).await.expect("list_pets");
    assert_eq!(pets.status(), 200);
    let pets = pets.into_inner();
    assert_eq!(pets.len(), 1);
    println!("listed {} pet(s): {}", pets.len(), pets[0].name);

    // A typed JSON body in, a typed model out.
    let created = client
        .create_pet(&types::NewPet {
            name: "Bella".to_owned(),
            tag: Some("dog".to_owned()),
        })
        .await
        .expect("create_pet");
    assert_eq!(created.status(), 201);
    println!("created pet #{}", created.into_inner().id);

    // Fetch by path parameter.
    let pet = client.get_pet("1".to_owned()).await.expect("get_pet");
    assert_eq!(pet.into_inner().status, types::Status::Available);
    println!("fetched pet #1");

    // A documented 404 arrives as the operation's typed error body — no silent degradation.
    match client.get_pet("999".to_owned()).await {
        Err(Error::Api(response)) => {
            assert_eq!(response.status(), 404);
            println!("typed 404: {}", response.into_inner().message);
        }
        other => panic!("expected a typed 404, got {other:?}"),
    }

    println!("petstore-macro example: all checks passed");
}

/// A deliberately tiny HTTP/1.1 mock on an ephemeral localhost port — no external service.
fn spawn_mock_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || handle(stream));
        }
    });
    addr
}

fn handle(mut stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return;
    };
    let path = target.split('?').next().unwrap_or(target);

    let mut authorization = String::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.to_ascii_lowercase().as_str() {
            "authorization" => authorization = value.trim().to_owned(),
            "content-length" => content_length = value.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }

    let (status, response_body) = if authorization != format!("Bearer {TOKEN}") {
        ("401 Unauthorized", r#"{"message":"unauthorized"}"#.to_owned())
    } else {
        match (method, path) {
            ("GET", "/pets") => (
                "200 OK",
                r#"[{"id":"1","name":"Rex","status":"available"}]"#.to_owned(),
            ),
            ("POST", "/pets") => {
                let new: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
                let name = new["name"].as_str().unwrap_or("unnamed");
                (
                    "201 Created",
                    format!(r#"{{"id":"2","name":"{name}","status":"available"}}"#),
                )
            }
            ("GET", "/pets/1") => (
                "200 OK",
                r#"{"id":"1","name":"Rex","status":"available"}"#.to_owned(),
            ),
            ("GET", _) => ("404 Not Found", r#"{"message":"no such pet"}"#.to_owned()),
            _ => (
                "500 Internal Server Error",
                r#"{"message":"unhandled route"}"#.to_owned(),
            ),
        }
    };

    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
        response_body.len(),
    );
}
