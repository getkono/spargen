//! The macro consumption path end to end: the client is generated **inline** by
//! `spargen_macro::generate_api!` — no build.rs, no `include!`, no CLI. This example proves the
//! macro-generated client compiles and drives real HTTP (against a local mock), and its Cargo.toml
//! proves no spargen crate is in the runtime graph.
//!
//! The full feature surface (the missing-credential and wrong-token auth failures, retry,
//! undocumented statuses, …) is covered by `examples/petstore`; this example is deliberately
//! compact — its job is to exercise the *macro*. It does mirror the typed error variants a
//! consumer matches on (a documented error body, a missing credential), so the macro expansion's
//! copies are matched at runtime too.

use secrecy::SecretString;

#[path = "../../mock_http.rs"]
#[expect(
    dead_code,
    reason = "shared with examples/petstore, which uses all of it; this example uses a subset"
)]
mod mock_http;

use mock_http::{Request, Response};

// The entire client, generated at compile time from the spec beside this crate's Cargo.toml.
// The expansion carries no inner attributes, so it drops straight into a module.
mod petstore {
    spargen_macro::generate_api!(
        spec = "petstore.yaml",
        no_uuid,
        datetime = "string",
        carve,
        error_body_cap = 32_768,
        batch_cap = 200,
        omit { components { schemas { "Legacy"; } } }
    );
}

use petstore::{types, Client, Error, RequestError};

const TOKEN: &str = "let-me-in";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let addr = mock_http::spawn(route);
    let base_url = format!("http://{addr}");
    println!("mock petstore listening on {base_url}");

    let client = Client::new(&base_url)
        .unwrap()
        .with_bearer_auth(SecretString::from(TOKEN));

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
    let pet = client.get_pet("1").await.expect("get_pet");
    assert_eq!(pet.into_inner().status, types::Status::Available);
    println!("fetched pet #1");

    // A documented 404 arrives as the operation's typed error body — no silent degradation.
    match client.get_pet("999").await {
        Err(Error::Api(response)) => {
            assert_eq!(response.status(), 404);
            println!("typed 404: {}", response.into_inner().message);
        }
        other => panic!("expected a typed 404, got {other:?}"),
    }

    // A missing credential fails before anything is sent, as the typed `MissingCredential`
    // variant naming the scheme alternatives — the same contract `examples/petstore` drives over
    // the build.rs path, held here for the macro expansion.
    let unauthenticated = Client::new(&base_url).unwrap();
    match unauthenticated.get_pet("1").await {
        Err(Error::RequestConstruction(RequestError::MissingCredential { alternatives })) => {
            assert_eq!(alternatives, [vec!["bearerAuth"]]);
            println!("missing credential rejected up front, naming {alternatives:?}");
        }
        other => panic!("expected a missing-credential error, got {other:?}"),
    }

    println!("petstore-macro example: all checks passed");
}

/// The petstore routes the mock serves.
fn route(request: Request) -> Response {
    if request.header("authorization") != Some(format!("Bearer {TOKEN}").as_str()) {
        return Response::new("401 Unauthorized", r#"{"message":"unauthorized"}"#);
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/pets") => Response::new(
            "200 OK",
            r#"[{"id":"1","name":"Rex","status":"available"}]"#,
        ),
        ("POST", "/pets") => {
            let new: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
            let name = new["name"].as_str().unwrap_or("unnamed");
            Response::new(
                "201 Created",
                format!(r#"{{"id":"2","name":"{name}","status":"available"}}"#),
            )
        }
        ("GET", "/pets/1") => {
            Response::new("200 OK", r#"{"id":"1","name":"Rex","status":"available"}"#)
        }
        ("GET", _) => Response::new("404 Not Found", r#"{"message":"no such pet"}"#),
        _ => Response::new(
            "500 Internal Server Error",
            r#"{"message":"unhandled route"}"#,
        ),
    }
}
