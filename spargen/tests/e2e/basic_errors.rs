//! The `tests/errors.rs` that `generated_module_compiles_in_basic_oas31_crate` writes into the
//! `BASIC_SPEC` fixture crate and runs there.

pub(super) const ERRORS_TEST: &str = r##"fn assert_error<E: std::error::Error + 'static>() {}

// Every runtime type a generated signature mentions must also be nameable, because the embedded
// `support` module is private and the root re-export list is the whole surface. Naming each one in
// a type position is the assertion.
#[test]
fn every_runtime_type_in_a_signature_is_nameable() {
    fn header_result() -> Result<(), basic_client::HeaderError> {
        Ok(())
    }
    fn core(client: &basic_client::Client) -> &basic_client::ClientCore {
        client.core()
    }
    fn timeout_kind(kind: basic_client::TimeoutKind) -> basic_client::TimeoutKind {
        kind
    }
    fn config(config: &basic_client::ClientConfig) -> usize {
        config.max_error_body
    }
    // `RetryWait` is what a caller's own `RetryPolicy` has to return; before it was re-exported the
    // only way to write this was to spell out `Pin<Box<dyn Future<Output = ()> + Send + 'a>>`.
    struct NeverRetry;
    impl basic_client::RetryPolicy for NeverRetry {
        fn retry<'a>(
            &'a self,
            _attempt: u32,
            _outcome: &basic_client::RetryOutcome<'_>,
        ) -> Option<basic_client::RetryWait<'a>> {
            None
        }
    }
    // `ApiErrorBody` is the bound a caller writes to be generic over operations' error bodies.
    fn body_of<E: basic_client::ApiErrorBody>(error: &E) -> Option<&E::Body> {
        error.body()
    }
    // Naming each type above is the assertion; binding the items keeps them from reading as dead.
    let _ = (header_result, core, timeout_kind, config);
    let _ = body_of::<basic_client::GetTextErrorError>;
    let _: &dyn basic_client::RetryPolicy = &NeverRetry;
}

// The call-site shapes `impl Into<..>` / `impl Into<Option<..>>` are meant to widen, never to
// narrow: everything that compiled against the concrete signatures still compiles, and the shorter
// spellings compile too.
#[test]
fn required_string_params_and_the_params_bundle_accept_conversions() {
    fn call(client: &basic_client::Client) {
        let owned = String::from("k");
        // Widened: a literal and a `&String` now work where only `String` did.
        let _ = client.read_file("k");
        let _ = client.read_file(&owned);
        // Still accepted, so no existing call site breaks.
        let _ = client.read_file(owned.clone());
        // The params bundle takes `None` and `Some(..)` as before, and now the bundle itself.
        let _ = client.get_user("1", None);
        let _ = client.get_user("1", basic_client::GetUserParams::default());
        let _ = client.get_user("1", Some(basic_client::GetUserParams::default()));
    }
    let _ = call;
}

#[test]
fn the_client_is_debug_and_clone() {
    fn assert_debug<T: std::fmt::Debug>() {}
    fn assert_clone<T: Clone>() {}
    assert_debug::<basic_client::Client>();
    assert_clone::<basic_client::Client>();
    // A `Debug` client must never render a registered secret.
    let client = basic_client::Client::new("http://127.0.0.1:1")
        .unwrap()
        .with_credential(
            "bearerAuth",
            basic_client::Credential::Bearer(basic_client::SecretString::from("hunter2")),
        );
    let rendered = format!("{:?}", client.clone());
    assert!(!rendered.contains("hunter2"), "secret leaked: {rendered}");
}

#[test]
fn generated_error_types_are_std_errors() {
    // `GetMultiError` is the multi-status enum; `GetTextErrorError` the single-body newtype.
    assert_error::<basic_client::Error<basic_client::GetMultiError>>();
    assert_error::<basic_client::Error<basic_client::GetTextErrorError>>();
    // The no-documented-error shape stays the uninhabited alias.
    assert_error::<basic_client::Error<std::convert::Infallible>>();
}

#[test]
fn a_generated_error_boxes_and_renders() -> Result<(), Box<dyn std::error::Error>> {
    let error: basic_client::Error<basic_client::GetTextErrorError> =
        basic_client::Error::request_message("boom");
    let boxed: Box<dyn std::error::Error> = Box::new(error);
    assert!(!boxed.to_string().is_empty());
    Ok(())
}

// A missing credential is the one request-construction cause an application routes on, so it is
// a typed variant reachable from generated output — and it fails before anything is sent, so one
// poll with a no-op waker is enough: no server, no runtime.
#[test]
fn a_missing_credential_is_a_typed_request_construction_error() {
    use std::future::Future;
    // The opaque cause of every other request-construction failure is nameable too.
    let _: Option<basic_client::RequestCause> = None;
    let client = basic_client::Client::new("http://127.0.0.1:1").unwrap();
    let mut call = std::pin::pin!(client.get_user("1", None));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(result) = call.as_mut().poll(&mut cx) else {
        panic!("a missing credential must fail before anything is sent");
    };
    match result {
        Err(basic_client::Error::RequestConstruction(
            basic_client::RequestError::MissingCredential { alternatives },
        )) => assert_eq!(alternatives, [vec!["bearer"], vec!["apiKey"]]),
        other => panic!("expected MissingCredential, got {other:?}"),
    }
}

// `without_credential` on the async client reaches dispatch: a client derived from a registered
// one by unregistering its only credential is back to reporting that scheme as missing, and the
// client it was cloned from keeps its registration.
#[test]
fn without_credential_unregisters_a_scheme_on_a_derived_client() {
    use std::future::Future;
    let registered = basic_client::Client::new("http://127.0.0.1:1")
        .unwrap()
        .with_credential(
            "bearer",
            basic_client::Credential::Bearer(basic_client::SecretString::from("t0k")),
        );
    let derived = registered.clone().without_credential("bearer").without_credential("never");
    assert!(registered.core().credential("bearer").is_some());
    assert!(derived.core().credential("bearer").is_none());
    let mut call = std::pin::pin!(derived.get_user("1", None));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(result) = call.as_mut().poll(&mut cx) else {
        panic!("a missing credential must fail before anything is sent");
    };
    match result {
        Err(basic_client::Error::RequestConstruction(
            basic_client::RequestError::MissingCredential { alternatives },
        )) => assert_eq!(alternatives, [vec!["bearer"], vec!["apiKey"]]),
        other => panic!("expected MissingCredential, got {other:?}"),
    }
}

// The reason the payload is `Vec<Vec<&str>>` and not the flat `Vec<&str>` the issue proposed: an
// alternative is a conjunction, and flattening loses which schemes must be presented *together*.
// This is the only place a generated client is driven to produce that shape. It also pins the two
// properties the grouping exists for, which only a generated requirement can witness: spargen emits
// a conjunction into the requirement slice in declaration order, and a `mutualTLS` member is left
// out of the report entirely, because the transport satisfies it and the caller cannot register it.
#[test]
fn a_generated_conjunction_reports_each_alternative_grouped() {
    fn outstanding(client: &basic_client::Client) -> Vec<Vec<&'static str>> {
        use std::future::Future;
        let mut call = std::pin::pin!(client.get_conjunction());
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let std::task::Poll::Ready(result) = call.as_mut().poll(&mut cx) else {
            panic!("a missing credential must fail before anything is sent");
        };
        match result {
            Err(basic_client::Error::RequestConstruction(
                basic_client::RequestError::MissingCredential { alternatives },
            )) => alternatives,
            other => panic!("expected MissingCredential, got {other:?}"),
        }
    }

    let bare = basic_client::Client::new("http://127.0.0.1:1").unwrap();
    assert_eq!(
        outstanding(&bare),
        [
            // A TWO-SCHEME CONJUNCTION, in declaration order. This inner list having length 2 is
            // the whole reason the payload is not a flat `Vec<&str>`: a caller told "tenant,
            // bearer, apiKey" cannot tell that the first two must be presented together.
            vec!["tenant", "bearer"],
            vec!["apiKey"],
            // `mtls` is absent. mutualTLS is satisfied by the transport's client certificate, so
            // it is never a credential the caller can register and must never be reported as one.
            vec!["tenant"],
        ]
    );
    // The grouping survives rendering: `+` joins a conjunction, `or` separates alternatives.
    let rendered = basic_client::Error::<std::convert::Infallible>::RequestConstruction(
        basic_client::RequestError::MissingCredential {
            alternatives: outstanding(&bare),
        },
    );
    let rendered = std::error::Error::source(&rendered).unwrap().to_string();
    assert!(
        rendered.ends_with("(missing: tenant + bearer or apiKey or tenant)"),
        "{rendered}"
    );

    // Registering one member of a conjunction does not satisfy the alternative it appears in: that
    // alternative goes on reporting the members still unregistered. `bearer` is in exactly one of
    // the three alternatives here, so only that one's report changes — and all three stay
    // outstanding.
    let partial = basic_client::Client::new("http://127.0.0.1:1")
        .unwrap()
        .with_credential(
            "bearer",
            basic_client::Credential::Bearer(basic_client::SecretString::from("t0k")),
        );
    assert_eq!(
        outstanding(&partial),
        [vec!["tenant"], vec!["apiKey"], vec!["tenant"]]
    );
}

// The other credential state an application routes on: a credential *is* registered, but the
// provider behind it could not refresh it. That is still "unauthenticated", and it is still raised
// before anything is sent — so the same poll-once shape reaches it, with no server and no runtime.
#[test]
fn a_failed_token_provider_is_a_typed_request_construction_error() {
    use std::future::Future;
    let client = basic_client::Client::new("http://127.0.0.1:1")
        .unwrap()
        .with_credential(
            "bearer",
            basic_client::Credential::Provider(std::sync::Arc::new(|| {
                Box::pin(async { Err(basic_client::AuthError::new("refresh rejected")) })
                    as basic_client::TokenFuture
            })),
        );
    let mut call = std::pin::pin!(client.get_user("1", None));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(result) = call.as_mut().poll(&mut cx) else {
        panic!("a failed token provider must fail before anything is sent");
    };
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("a failed token provider cannot produce a response"),
    };
    match &error {
        basic_client::Error::RequestConstruction(
            basic_client::RequestError::CredentialProvider { scheme, source },
        ) => {
            assert_eq!(*scheme, "bearer");
            assert_eq!(source.to_string(), "refresh rejected");
        }
        other => panic!("expected CredentialProvider, got {other:?}"),
    }
    // Nothing was sent, so there is nothing to retry — and the provider's own error stays reachable
    // through the chain, which is how an application reports *why* the refresh failed.
    assert!(!error.is_transient());
    let cause = std::error::Error::source(&error).unwrap();
    // The request-level message is a fixed sentence naming the scheme, not the provider's text:
    // the provider's own message is one level further down, where it is downcast below.
    assert_eq!(
        cause.to_string(),
        "the token provider registered for security scheme `bearer` failed"
    );
    let provider = std::error::Error::source(cause).unwrap();
    assert!(provider.downcast_ref::<basic_client::AuthError>().is_some());
}

// The third credential state: a credential is registered and selects its alternative, but it is of
// a kind the scheme cannot carry. It is typed from generated output too, with nothing beneath it,
// so a consumer never has to match on its text.
#[test]
fn a_credential_of_the_wrong_kind_is_a_typed_request_construction_error() {
    use std::future::Future;
    let client = basic_client::Client::new("http://127.0.0.1:1")
        .unwrap()
        .with_credential(
            "bearer",
            basic_client::Credential::Basic {
                username: "aladdin".to_owned(),
                password: basic_client::SecretString::from("open sesame"),
            },
        );
    let mut call = std::pin::pin!(client.get_user("1", None));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(result) = call.as_mut().poll(&mut cx) else {
        panic!("a credential mismatch must fail before anything is sent");
    };
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("a credential mismatch cannot produce a response"),
    };
    match &error {
        basic_client::Error::RequestConstruction(
            basic_client::RequestError::CredentialMismatch { scheme, required, registered },
        ) => {
            assert_eq!(*scheme, "bearer");
            assert_eq!(*required, "bearer");
            assert_eq!(*registered, "Basic");
        }
        other => panic!("expected CredentialMismatch, got {other:?}"),
    }
    assert!(!error.is_transient());
    let cause = std::error::Error::source(&error).unwrap();
    assert!(std::error::Error::source(cause).is_none());
}

#[test]
fn the_single_error_body_stays_one_deref_away() {
    let wrapped = basic_client::GetTextErrorError("nope".to_owned());
    assert_eq!(wrapped.len(), 4); // through `Deref` to `String`
    assert_eq!(wrapped.into_inner(), "nope");
    assert_eq!(String::from(basic_client::GetTextErrorError("x".to_owned())), "x");
}

#[test]
fn a_multi_status_error_names_its_status() {
    let error = basic_client::GetMultiError::Status404(Box::new(
        basic_client::types::NotFoundError { reason: "gone".to_owned() },
    ));
    assert!(error.to_string().contains("404"), "{error}");
}

#[test]
fn a_uniform_body_error_enum_hands_back_its_body_from_any_status() {
    let problem = basic_client::types::Problem { title: "nope".to_owned(), detail: "gone".to_owned() };
    let not_found = basic_client::GetSharedError::Status404(Box::new(problem.clone()));
    let conflict = basic_client::GetSharedError::Status409(Box::new(problem));
    assert_eq!(not_found.body().map(|p| p.detail.as_str()), Some("gone"));
    assert_eq!(conflict.body().map(|p| p.detail.as_str()), Some("gone"));
    // A documented bodyless status carries nothing to hand back.
    assert!(basic_client::GetSharedError::Status401.body().is_none());
}

#[test]
fn the_taxonomy_reaches_the_body_generically_over_the_operation() {
    // Generic over `E`: this is the "thirty distinct error types" case from the issue.
    fn detail<E>(error: &basic_client::Error<E>) -> Option<&str>
    where
        E: basic_client::ApiErrorBody<Body = basic_client::types::Problem>,
    {
        error.api_body().map(|problem| problem.detail.as_str())
    }
    let body = basic_client::GetSharedError::Status409(Box::new(basic_client::types::Problem {
        title: "conflict".to_owned(),
        detail: "dup".to_owned(),
    }));
    let error = basic_client::Error::Api(basic_client::ResponseValue::new(
        reqwest::StatusCode::CONFLICT,
        Default::default(),
        body,
    ));
    assert_eq!(detail(&error), Some("dup"));
    let transport: basic_client::Error<basic_client::GetSharedError> =
        basic_client::Error::request_message("boom");
    assert_eq!(detail(&transport), None);
}

#[test]
fn every_error_shape_implements_api_error_body() {
    use basic_client::ApiErrorBody;
    // The single-body newtype: `Body` is the inner type.
    let wrapped = basic_client::GetTextErrorError("nope".to_owned());
    assert_eq!(wrapped.body().map(String::as_str), Some("nope"));
    // The no-documented-error shape: exists so generic code compiles, and is always `None`.
    fn never(error: &basic_client::Error<std::convert::Infallible>) -> bool {
        error.api_body().is_none()
    }
    assert!(never(&basic_client::Error::request_message("x")));
}

// `Error::problem` is generic over every operation, including an enum whose statuses carry
// DIFFERENT body types (`getMulti`), which has no `ApiErrorBody` and so no `api_body`.
#[test]
fn the_problem_reader_is_generic_over_every_error_shape() {
    fn detail<E: basic_client::ApiErrorProblem>(error: &basic_client::Error<E>) -> Option<String> {
        error.problem().and_then(|problem| problem.detail)
    }
    fn api<E>(status: reqwest::StatusCode, body: E) -> basic_client::Error<E> {
        basic_client::Error::Api(basic_client::ResponseValue::new(status, Default::default(), body))
    }
    // The uniform-body enum.
    let shared = basic_client::GetSharedError::Status409(Box::new(basic_client::types::Problem {
        title: "conflict".to_owned(),
        detail: "dup".to_owned(),
    }));
    assert_eq!(detail(&api(reqwest::StatusCode::CONFLICT, shared)), Some("dup".to_owned()));
    // The heterogeneous enum: a body with no `detail` member answers with the members it has.
    let multi = basic_client::GetMultiError::Status404(Box::new(
        basic_client::types::NotFoundError { reason: "gone".to_owned() },
    ));
    let problem = api(reqwest::StatusCode::NOT_FOUND, multi).problem().expect("an object body");
    assert_eq!(problem, basic_client::ProblemDetails::default());
    // A documented bodyless status, a textual body, and a raw-bytes body have no members.
    let unit = api(reqwest::StatusCode::UNAUTHORIZED, basic_client::GetSharedError::Status401);
    assert!(unit.problem().is_none());
    let text = basic_client::GetRawMultiError::Status400(Box::new("bad".to_owned()));
    assert!(api(reqwest::StatusCode::BAD_REQUEST, text).problem().is_none());
    let raw = basic_client::GetRawMultiError::Status409(Box::new(bytes::Bytes::from_static(
        br#"{"detail":"never read"}"#,
    )));
    assert!(api(reqwest::StatusCode::CONFLICT, raw).problem().is_none());
    // The single-body newtype and the uninhabited shape.
    let wrapped = basic_client::GetTextErrorError("nope".to_owned());
    assert!(api(reqwest::StatusCode::BAD_REQUEST, wrapped).problem().is_none());
    let never: basic_client::Error<std::convert::Infallible> =
        basic_client::Error::request_message("x");
    assert!(never.problem().is_none());
}

#[test]
fn a_nullable_error_body_answers_none_for_null_on_both_shapes() {
    use basic_client::ApiErrorBody;
    let problem = basic_client::types::MaybeProblem { title: "nope".to_owned() };
    // The enum shape: a nullable payload is `Option<Box<T>>`, so `null` is `None`, a value is
    // `Some`, and the `default` response's variant is reached like any exact status.
    assert!(basic_client::GetMaybeError::Status404(None).body().is_none());
    let not_found = basic_client::GetMaybeError::Status404(Some(Box::new(problem.clone())));
    assert_eq!(not_found.body().map(|p| p.title.as_str()), Some("nope"));
    let fallback = basic_client::GetMaybeError::Default(Some(Box::new(problem.clone())));
    assert_eq!(fallback.body().map(|p| p.title.as_str()), Some("nope"));
    // The newtype shape over the same component: `Body` is the bare `MaybeProblem`, not the
    // `Option` the newtype wraps, and `null` answers `None` exactly as the enum does.
    assert!(basic_client::GetMaybeSingleError(None).body().is_none());
    let single = basic_client::GetMaybeSingleError(Some(problem.clone()));
    assert_eq!(single.body().map(|p| p.title.as_str()), Some("nope"));
    // One bound names the component and accepts both shapes.
    fn title<E>(error: &E) -> Option<&str>
    where
        E: basic_client::ApiErrorBody<Body = basic_client::types::MaybeProblem>,
    {
        error.body().map(|p| p.title.as_str())
    }
    let conflict = basic_client::GetMaybeError::Status409(Some(Box::new(problem.clone())));
    assert_eq!(title(&conflict), Some("nope"));
    assert_eq!(title(&basic_client::GetMaybeSingleError(Some(problem))), Some("nope"));
    assert_eq!(title(&basic_client::GetMaybeSingleError(None)), None);
}

#[test]
fn an_alias_equal_error_body_is_one_body_type() {
    // `PlainMessage` and the inline 409 schema are distinct schemas, but both generate `String`:
    // one body type, so the inherent accessor and the trait both exist.
    let not_found = basic_client::GetAliasSharedError::Status404(Box::new("gone".to_owned()));
    let conflict = basic_client::GetAliasSharedError::Status409(Box::new("dup".to_owned()));
    assert_eq!(not_found.body().map(String::as_str), Some("gone"));
    assert_eq!(conflict.body().map(String::as_str), Some("dup"));
    fn text<E: basic_client::ApiErrorBody<Body = String>>(error: &E) -> Option<&str> {
        error.body().map(String::as_str)
    }
    assert_eq!(text(&not_found), Some("gone"));
    assert_eq!(text(&conflict), Some("dup"));
}
"##;
