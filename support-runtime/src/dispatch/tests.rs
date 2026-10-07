use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use secrecy::SecretString;

use crate::{AuthError, AuthKind, AuthScheme, ClientCore, Credential, RequestError, TokenFuture};

use super::attach_auth;
use bytes::Bytes;

/// The static-credential paths never actually suspend, so a single poll with a noop waker is
/// enough — no async runtime needed in the runtime's own test suite.
fn poll_ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("future was not immediately ready"),
    }
}

fn core() -> ClientCore {
    ClientCore::new("https://example.com").unwrap()
}

fn get(core: &ClientCore) -> reqwest::RequestBuilder {
    core.http()
        .request(reqwest::Method::GET, "https://example.com/op")
}

const BEARER: &[AuthScheme] = &[AuthScheme {
    name: "token",
    kind: AuthKind::Bearer,
}];

#[test]
fn attaches_bearer_credential() {
    let mut core = core();
    core.set_credential("token", Credential::Bearer(SecretString::from("t0k")));
    let request = poll_ready(attach_auth(&core, get(&core), &[BEARER]))
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(
        request.headers()[reqwest::header::AUTHORIZATION],
        "Bearer t0k"
    );
}

#[test]
fn attaches_provider_token_as_bearer() {
    let mut core = core();
    core.set_credential(
        "token",
        Credential::Provider(Arc::new(|| {
            Box::pin(async { Ok(SecretString::from("fresh")) }) as TokenFuture
        })),
    );
    let request = poll_ready(attach_auth(&core, get(&core), &[BEARER]))
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(
        request.headers()[reqwest::header::AUTHORIZATION],
        "Bearer fresh"
    );
}

/// A provider yields one secret, usable anywhere a token fits — not just as a bearer. The guard
/// that decides whether to ask it keys off the scheme *kind*, so the three `apiKey` kinds are
/// the branch a bearer-only reading of it would silently drop: every call would then fail as a
/// registration mismatch instead of attaching the refreshed token. One case per kind, since
/// each installs the token somewhere different.
#[test]
fn attaches_provider_token_under_every_api_key_kind() {
    let refreshed = || {
        Credential::Provider(Arc::new(|| {
            Box::pin(async { Ok(SecretString::from("fresh")) }) as TokenFuture
        }))
    };

    let mut core = core();
    core.set_credential("key", refreshed());
    let header = poll_ready(attach_auth(
        &core,
        get(&core),
        &[&[AuthScheme {
            name: "key",
            kind: AuthKind::ApiKeyHeader("X-Api-Key"),
        }]],
    ))
    .unwrap()
    .build()
    .unwrap();
    assert_eq!(header.headers()["X-Api-Key"], "fresh");
    // The refreshed token is a secret like any other, so it must not be printable.
    assert!(header.headers()["X-Api-Key"].is_sensitive());

    let query = poll_ready(attach_auth(
        &core,
        get(&core),
        &[&[AuthScheme {
            name: "key",
            kind: AuthKind::ApiKeyQuery("api_key"),
        }]],
    ))
    .unwrap()
    .build()
    .unwrap();
    assert_eq!(query.url().query(), Some("api_key=fresh"));

    let cookie = poll_ready(attach_auth(
        &core,
        get(&core),
        &[&[AuthScheme {
            name: "key",
            kind: AuthKind::ApiKeyCookie("session"),
        }]],
    ))
    .unwrap()
    .build()
    .unwrap();
    assert_eq!(cookie.headers()[reqwest::header::COOKIE], "session=fresh");
    assert!(cookie.headers()[reqwest::header::COOKIE].is_sensitive());
}

#[test]
fn attaches_api_key_query_from_first_satisfiable_alternative() {
    let mut core = core();
    core.set_credential("key", Credential::ApiKey(SecretString::from("k3y")));
    let request = poll_ready(attach_auth(
        &core,
        get(&core),
        &[
            BEARER,
            &[AuthScheme {
                name: "key",
                kind: AuthKind::ApiKeyQuery("api_key"),
            }],
        ],
    ))
    .unwrap()
    .build()
    .unwrap();
    assert_eq!(request.url().query(), Some("api_key=k3y"));
}

#[test]
fn empty_alternative_marks_security_optional() {
    let core = core();
    let request = poll_ready(attach_auth(&core, get(&core), &[BEARER, &[]]))
        .unwrap()
        .build()
        .unwrap();
    assert!(request.headers().is_empty());
}

/// No requirement at all is distinct from a requirement nothing satisfies: it attaches nothing
/// and succeeds. Without the early return an empty slice falls through the selection loop
/// without choosing anything, and the call would fail as `MissingCredential` naming no
/// schemes — the degenerate rendering `Display` was made total for. Generated output cannot reach this (`emit.rs` omits
/// the call when an operation declares no `security`), but the function is public.
#[test]
fn no_requirement_attaches_nothing() {
    let mut core = core();
    // A registered credential must not be attached speculatively either.
    core.set_credential("token", Credential::Bearer(SecretString::from("t0k")));
    let request = poll_ready(attach_auth(&core, get(&core), &[]))
        .unwrap()
        .build()
        .unwrap();
    assert!(request.headers().is_empty(), "{:?}", request.headers());
    assert_eq!(request.url().query(), None);
}

#[test]
fn missing_credential_fails_before_send() {
    let core = core();
    let error = poll_ready(attach_auth(&core, get(&core), &[BEARER])).unwrap_err();
    assert!(error.to_string().contains("request construction"));
    // Typed, so a consumer routes on the variant rather than on the message text.
    let Error::RequestConstruction(RequestError::MissingCredential { alternatives }) = &error
    else {
        panic!("expected MissingCredential, got {error:?}");
    };
    assert_eq!(*alternatives, [vec!["token"]]);
    // The chain ends at the typed cause.
    let source = std::error::Error::source(&error).unwrap();
    assert_eq!(
        source.to_string(),
        "no registered credential satisfies the operation's security requirement \
         (missing: token)"
    );
    assert!(std::error::Error::source(source).is_none());
}

/// Each alternative reports only what the caller still has to register: declaration order
/// (not sorted), a registered key and a transport-satisfied `mutualTLS` key omitted, and a key
/// shared by two alternatives listed under each.
#[test]
fn missing_credential_lists_each_alternatives_unregistered_schemes_in_declared_order() {
    let mut core = core();
    core.set_credential("key", Credential::ApiKey(SecretString::from("k3y")));
    let error = poll_ready(attach_auth(
        &core,
        get(&core),
        &[
            BEARER,
            &[
                AuthScheme {
                    name: "zeta",
                    kind: AuthKind::Bearer,
                },
                AuthScheme {
                    name: "key",
                    kind: AuthKind::ApiKeyQuery("api_key"),
                },
                AuthScheme {
                    name: "alpha",
                    kind: AuthKind::Bearer,
                },
            ],
            &[
                AuthScheme {
                    name: "mtls",
                    kind: AuthKind::MutualTls,
                },
                AuthScheme {
                    name: "token",
                    kind: AuthKind::Bearer,
                },
            ],
        ],
    ))
    .unwrap_err();
    let rendered = std::error::Error::source(&error).unwrap().to_string();
    let Error::RequestConstruction(RequestError::MissingCredential { alternatives }) = error else {
        panic!("expected MissingCredential, got {error:?}");
    };
    assert_eq!(
        alternatives,
        [vec!["token"], vec!["zeta", "alpha"], vec!["token"]]
    );
    assert!(
        rendered.ends_with("(missing: token or zeta + alpha or token)"),
        "{rendered}"
    );
}

/// One scheme of every `AuthKind`, each under its own name, and a credential of the kind that
/// scheme accepts.
fn every_kind() -> [(AuthScheme, Credential); 6] {
    let key = || Credential::ApiKey(SecretString::from("k3y"));
    [
        (
            AuthScheme {
                name: "bearer",
                kind: AuthKind::Bearer,
            },
            key(),
        ),
        (
            AuthScheme {
                name: "basic",
                kind: AuthKind::Basic,
            },
            Credential::Basic {
                username: "u".to_owned(),
                password: SecretString::from("p"),
            },
        ),
        (
            AuthScheme {
                name: "header",
                kind: AuthKind::ApiKeyHeader("x-key"),
            },
            key(),
        ),
        (
            AuthScheme {
                name: "query",
                kind: AuthKind::ApiKeyQuery("key"),
            },
            key(),
        ),
        (
            AuthScheme {
                name: "cookie",
                kind: AuthKind::ApiKeyCookie("key"),
            },
            key(),
        ),
        (
            AuthScheme {
                name: "mtls",
                kind: AuthKind::MutualTls,
            },
            key(),
        ),
    ]
}

/// The schemes of `alternative` a caller still has to register under `registered`: every
/// scheme but a registered one or `mutualTLS`, in declaration order. Written independently of
/// `attach_auth` so the two can disagree.
fn expected_missing(alternative: &[AuthScheme], registered: &[&str]) -> Vec<&'static str> {
    alternative
        .iter()
        .filter(|scheme| {
            !matches!(scheme.kind, AuthKind::MutualTls) && !registered.contains(&scheme.name)
        })
        .map(|scheme| scheme.name)
        .collect()
}

/// Selection and the `MissingCredential` report agree over every `AuthKind`, every
/// registration subset, and every non-empty combination of schemes as an alternative: the
/// call succeeds exactly when some alternative has nothing missing, and otherwise reports,
/// per alternative in order, exactly what is missing — so neither list is ever empty (#205).
#[test]
fn selection_and_the_missing_credential_report_agree_over_every_kind_and_registration() {
    let kinds = every_kind();
    let subsets = |mask: usize| -> Vec<AuthScheme> {
        kinds
            .iter()
            .enumerate()
            .filter(|(bit, _)| mask & (1 << bit) != 0)
            .map(|(_, (scheme, _))| *scheme)
            .collect()
    };
    let all = (1usize << kinds.len()) - 1;
    let alternatives: Vec<Vec<AuthScheme>> = (1..=all).map(subsets).collect();
    let check = |core: &ClientCore, registered: &[&str], requirements: &[&[AuthScheme]]| {
        let expected: Vec<Vec<&'static str>> = requirements
            .iter()
            .map(|alternative| expected_missing(alternative, registered))
            .collect();
        let satisfiable = expected.iter().any(Vec::is_empty);
        match poll_ready(attach_auth(core, get(core), requirements)) {
            Ok(_) => assert!(satisfiable, "{registered:?} {requirements:?}"),
            Err(Error::RequestConstruction(RequestError::MissingCredential { alternatives })) => {
                assert!(!satisfiable, "{registered:?} {requirements:?}");
                assert_eq!(alternatives, expected, "{registered:?} {requirements:?}");
                assert!(!alternatives.is_empty());
                assert!(alternatives.iter().all(|missing| !missing.is_empty()));
            }
            Err(error) => panic!("{registered:?} {requirements:?}: {error:?}"),
        }
    };
    for registration in 0..=all {
        let mut core = core();
        let mut registered = Vec::new();
        for (bit, (scheme, credential)) in kinds.iter().enumerate() {
            if registration & (1 << bit) != 0 {
                core.set_credential(scheme.name, credential.clone());
                registered.push(scheme.name);
            }
        }
        // Each combination alone, then every combination at once as alternatives of one
        // requirement, and again without the ones `mutualTLS` alone satisfies.
        for alternative in &alternatives {
            check(&core, &registered, &[alternative]);
        }
        let every: Vec<&[AuthScheme]> = alternatives.iter().map(Vec::as_slice).collect();
        check(&core, &registered, &every);
        let blocking: Vec<&[AuthScheme]> = every
            .iter()
            .copied()
            .filter(|alternative| {
                alternative
                    .iter()
                    .any(|scheme| !matches!(scheme.kind, AuthKind::MutualTls))
            })
            .collect();
        check(&core, &registered, &blocking);
    }
}

/// A client that registers a token provider always has a credential registered, so a failed
/// refresh is its "unauthenticated" state — typed, with the provider's error as the cause.
#[test]
fn a_failed_token_provider_is_a_typed_credential_provider_error() {
    let mut core = core();
    core.set_credential(
        "token",
        Credential::Provider(Arc::new(|| {
            Box::pin(async { Err(AuthError::new("refresh rejected")) }) as TokenFuture
        })),
    );
    let error = poll_ready(attach_auth(&core, get(&core), &[BEARER])).unwrap_err();
    let Error::RequestConstruction(RequestError::CredentialProvider { scheme, source }) = &error
    else {
        panic!("expected CredentialProvider, got {error:?}");
    };
    assert_eq!(*scheme, "token");
    assert_eq!(source.to_string(), "refresh rejected");
    assert!(!error.is_transient());
    let cause = std::error::Error::source(&error).unwrap();
    assert_eq!(
        cause.to_string(),
        "the token provider registered for security scheme `token` failed"
    );
    let provider = std::error::Error::source(cause).expect("the provider's error is the cause");
    assert!(provider.downcast_ref::<AuthError>().is_some());
}

/// A provider under a scheme that takes no token is a registration mismatch, and it must stay
/// one regardless of whether a refresh would have worked: the provider is never asked.
///
/// Not asking means no round trip to the identity provider, and the outcome does not depend
/// on what the provider would have returned: a provider that would fail and one that would
/// succeed give the same mismatch, because a token cannot satisfy an `http basic` scheme
/// either way. The test registers a failing provider and asserts both that it is never called
/// and that its `AuthError` appears nowhere in the error's chain.
///
/// The mismatch is typed: `RequestError::CredentialMismatch`, naming the scheme, the kind it
/// carries, and the kind registered, and ending the chain, so a consumer routes it without
/// matching on text.
#[test]
fn a_token_provider_under_a_basic_scheme_is_a_mismatch_without_calling_it() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let called = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&called);
    let mut core = core();
    core.set_credential(
        "login",
        Credential::Provider(Arc::new(move || {
            flag.store(true, Ordering::SeqCst);
            Box::pin(async { Err(AuthError::new("refresh rejected")) }) as TokenFuture
        })),
    );
    let error = poll_ready(attach_auth(
        &core,
        get(&core),
        &[&[AuthScheme {
            name: "login",
            kind: AuthKind::Basic,
        }]],
    ))
    .unwrap_err();
    assert!(
        matches!(
            error,
            Error::RequestConstruction(RequestError::CredentialMismatch {
                scheme: "login",
                required: "http basic",
                registered: "Provider",
            })
        ),
        "{error:?}"
    );
    // The payload is the whole cause: nothing below it, so nothing downcasts to `AuthError`.
    let source = std::error::Error::source(&error).unwrap();
    assert!(std::error::Error::source(source).is_none(), "{source}");
    assert!(!called.load(Ordering::SeqCst), "the provider was called");
}

/// A bearer alternative, then an apiKey-in-query fallback: the requirement both
/// fall-through tests below select over.
const FIRST_THEN_FALLBACK: &[&[AuthScheme]] = &[
    &[AuthScheme {
        name: "primary",
        kind: AuthKind::Bearer,
    }],
    &[AuthScheme {
        name: "fallback",
        kind: AuthKind::ApiKeyQuery("api_key"),
    }],
];

/// Selection is on registration, not on success, and there is no fall-through: once an
/// alternative is chosen, a failure while attaching it fails the call even though a later
/// alternative is fully registered and would have succeeded. Both ways of failing after
/// selection are pinned, because falling through is a silent behaviour — the caller would get
/// a 200 carrying credentials their registration did not select, with nothing to observe.
#[test]
fn a_failure_after_selection_does_not_fall_through_to_a_later_alternative() {
    // A registered fallback that would satisfy the second alternative outright.
    let register_fallback = |core: &mut ClientCore| {
        core.set_credential("fallback", Credential::ApiKey(SecretString::from("k3y")));
    };

    // (a) the selected alternative's token provider fails.
    let mut failing_provider = core();
    failing_provider.set_credential(
        "primary",
        Credential::Provider(Arc::new(|| {
            Box::pin(async { Err(AuthError::new("refresh rejected")) }) as TokenFuture
        })),
    );
    register_fallback(&mut failing_provider);
    let error = poll_ready(attach_auth(
        &failing_provider,
        get(&failing_provider),
        FIRST_THEN_FALLBACK,
    ))
    .unwrap_err();
    let Error::RequestConstruction(RequestError::CredentialProvider { scheme, .. }) = &error else {
        panic!("expected the selected alternative's failure, got {error:?}");
    };
    assert_eq!(*scheme, "primary");

    // (b) the selected alternative's credential is registered under a kind it cannot satisfy.
    let mut wrong_kind = core();
    wrong_kind.set_credential(
        "primary",
        Credential::Basic {
            username: "u".to_owned(),
            password: SecretString::from("p"),
        },
    );
    register_fallback(&mut wrong_kind);
    let error = poll_ready(attach_auth(
        &wrong_kind,
        get(&wrong_kind),
        FIRST_THEN_FALLBACK,
    ))
    .unwrap_err();
    let Error::RequestConstruction(RequestError::CredentialMismatch { scheme, .. }) = &error else {
        panic!("expected the selected alternative's mismatch, got {error:?}");
    };
    assert_eq!(*scheme, "primary");
}

/// The remedy for the test above: with no fall-through, the caller reaches the later
/// alternative by unregistering the earlier one. The same client that failed on its selected
/// alternative then attaches the fallback — and only the fallback, since the removed scheme's
/// credential must not ride along.
#[test]
fn removing_the_selected_credential_falls_through_to_a_later_alternative() {
    let mut core = core();
    core.set_credential(
        "primary",
        Credential::Provider(Arc::new(|| {
            Box::pin(async { Err(AuthError::new("refresh rejected")) }) as TokenFuture
        })),
    );
    core.set_credential("fallback", Credential::ApiKey(SecretString::from("k3y")));
    // Before removal the failing primary is selected, as the test above pins.
    assert!(poll_ready(attach_auth(&core, get(&core), FIRST_THEN_FALLBACK)).is_err());

    assert!(matches!(
        core.remove_credential("primary"),
        Some(Credential::Provider(_))
    ));
    let request = poll_ready(attach_auth(&core, get(&core), FIRST_THEN_FALLBACK))
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(request.url().query(), Some("api_key=k3y"));
    assert!(request.headers().get("authorization").is_none());

    // Removing every alternative's scheme leaves nothing satisfiable: the call fails as
    // `MissingCredential`, never by sending without credentials.
    core.remove_credential("fallback");
    let error = poll_ready(attach_auth(&core, get(&core), FIRST_THEN_FALLBACK)).unwrap_err();
    let Error::RequestConstruction(RequestError::MissingCredential { alternatives }) = error else {
        panic!("expected MissingCredential, got {error:?}");
    };
    assert_eq!(alternatives, [vec!["primary"], vec!["fallback"]]);
}

#[test]
fn api_key_header_is_sensitive() {
    let mut core = core();
    core.set_credential("key", Credential::ApiKey(SecretString::from("k3y")));
    let request = poll_ready(attach_auth(
        &core,
        get(&core),
        &[&[AuthScheme {
            name: "key",
            kind: AuthKind::ApiKeyHeader("X-Api-Key"),
        }]],
    ))
    .unwrap()
    .build()
    .unwrap();
    let value = &request.headers()["X-Api-Key"];
    assert_eq!(value, "k3y");
    assert!(value.is_sensitive());
}

/// `Credential`'s shipped documentation states which variant each kind of scheme accepts: both
/// static token variants under every token-carrying kind, and neither under `http basic`. The
/// provider's half of that table is pinned by the provider tests; this pins the static half,
/// so the documented rule and the attach code cannot drift apart unseen.
#[test]
fn both_static_token_variants_attach_under_every_token_kind_and_neither_under_basic() {
    for (credential, registered) in [
        (Credential::Bearer(SecretString::from("t0k")), "Bearer"),
        (Credential::ApiKey(SecretString::from("t0k")), "ApiKey"),
    ] {
        let mut core = core();
        core.set_credential("s", credential);
        let attach = |kind| {
            poll_ready(attach_auth(
                &core,
                get(&core),
                &[&[AuthScheme { name: "s", kind }]],
            ))
        };

        let bearer = attach(AuthKind::Bearer).unwrap().build().unwrap();
        assert_eq!(
            bearer.headers()[reqwest::header::AUTHORIZATION],
            "Bearer t0k",
            "{registered}"
        );
        let header = attach(AuthKind::ApiKeyHeader("X-Api-Key"))
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(header.headers()["X-Api-Key"], "t0k", "{registered}");
        let query = attach(AuthKind::ApiKeyQuery("api_key"))
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(query.url().query(), Some("api_key=t0k"), "{registered}");
        let cookie = attach(AuthKind::ApiKeyCookie("SESSION"))
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            cookie.headers()[reqwest::header::COOKIE],
            "SESSION=t0k",
            "{registered}"
        );

        let error = attach(AuthKind::Basic).unwrap_err();
        match error {
            Error::RequestConstruction(RequestError::CredentialMismatch {
                scheme: "s",
                required: "http basic",
                registered: reported,
            }) => assert_eq!(reported, registered),
            other => panic!("expected CredentialMismatch for {registered}, got {other:?}"),
        }
    }
}

#[test]
fn attaches_http_basic_credential() {
    // `Basic` was previously present only as the *wrong* credential for a bearer scheme, so
    // nothing asserted the header it is supposed to produce. RFC 7617: base64("u:p").
    let mut core = core();
    core.set_credential(
        "login",
        Credential::Basic {
            username: "aladdin".to_owned(),
            password: SecretString::from("open sesame"),
        },
    );
    let request = poll_ready(attach_auth(
        &core,
        get(&core),
        &[&[AuthScheme {
            name: "login",
            kind: AuthKind::Basic,
        }]],
    ))
    .unwrap()
    .build()
    .unwrap();
    assert_eq!(
        request.headers()[reqwest::header::AUTHORIZATION],
        "Basic YWxhZGRpbjpvcGVuIHNlc2FtZQ=="
    );
}

#[test]
fn a_bearer_credential_does_not_satisfy_a_basic_scheme() {
    let mut core = core();
    core.set_credential("login", Credential::Bearer(SecretString::from("t0k")));
    let error = poll_ready(attach_auth(
        &core,
        get(&core),
        &[&[AuthScheme {
            name: "login",
            kind: AuthKind::Basic,
        }]],
    ))
    .unwrap_err();
    assert!(
        matches!(
            error,
            Error::RequestConstruction(RequestError::CredentialMismatch {
                scheme: "login",
                required: "http basic",
                registered: "Bearer",
            })
        ),
        "{error:?}"
    );
}

/// The other direction: a static basic credential under a scheme that carries a token. It
/// yields no token, so every token-carrying kind reports the mismatch with its own name.
#[test]
fn a_basic_credential_does_not_satisfy_a_token_scheme() {
    let mut core = core();
    core.set_credential(
        "login",
        Credential::Basic {
            username: "aladdin".to_owned(),
            password: SecretString::from("open sesame"),
        },
    );
    for (kind, required) in [
        (AuthKind::Bearer, "bearer"),
        (AuthKind::ApiKeyHeader("X-Api-Key"), "apiKey"),
        (AuthKind::ApiKeyQuery("api_key"), "apiKey"),
        (AuthKind::ApiKeyCookie("SESSION"), "apiKey"),
    ] {
        let error = poll_ready(attach_auth(
            &core,
            get(&core),
            &[&[AuthScheme {
                name: "login",
                kind,
            }]],
        ))
        .unwrap_err();
        match error {
            Error::RequestConstruction(RequestError::CredentialMismatch {
                scheme,
                required: reported,
                registered,
            }) => {
                assert_eq!(scheme, "login");
                assert_eq!(reported, required, "{kind:?}");
                assert_eq!(registered, "Basic");
            }
            other => panic!("expected CredentialMismatch for {kind:?}, got {other:?}"),
        }
    }
}

#[test]
fn attaches_api_key_cookie_and_marks_it_sensitive() {
    let mut core = core();
    core.set_credential("session", Credential::ApiKey(SecretString::from("s3ss")));
    let request = poll_ready(attach_auth(
        &core,
        get(&core),
        &[&[AuthScheme {
            name: "session",
            kind: AuthKind::ApiKeyCookie("SESSION"),
        }]],
    ))
    .unwrap()
    .build()
    .unwrap();
    let value = &request.headers()[reqwest::header::COOKIE];
    assert_eq!(value, "SESSION=s3ss");
    assert!(value.is_sensitive());
}

#[test]
fn mutual_tls_is_satisfied_by_the_transport_without_a_registered_credential() {
    // The client certificate lives on the `reqwest::Client`, so a `mutualTLS` scheme must
    // neither demand a credential nor block its alternative from being selected — and it must
    // add no header of its own.
    let core = core();
    let request = poll_ready(attach_auth(
        &core,
        get(&core),
        &[&[AuthScheme {
            name: "mtls",
            kind: AuthKind::MutualTls,
        }]],
    ))
    .unwrap()
    .build()
    .unwrap();
    assert!(request.headers().is_empty(), "{:?}", request.headers());
}

#[test]
fn mutual_tls_does_not_block_a_paired_scheme_in_the_same_alternative() {
    let mut core = core();
    core.set_credential("token", Credential::Bearer(SecretString::from("t0k")));
    let request = poll_ready(attach_auth(
        &core,
        get(&core),
        &[&[
            AuthScheme {
                name: "mtls",
                kind: AuthKind::MutualTls,
            },
            AuthScheme {
                name: "token",
                kind: AuthKind::Bearer,
            },
        ]],
    ))
    .unwrap()
    .build()
    .unwrap();
    assert_eq!(
        request.headers()[reqwest::header::AUTHORIZATION],
        "Bearer t0k"
    );
}

#[test]
fn a_credential_registered_under_a_mutual_tls_scheme_never_reaches_a_token_provider() {
    // `set_credential` takes an untyped `&str`, so a consumer can register a provider under a
    // `mutualTLS` scheme's name. Awaiting it would be a live token fetch whose result the
    // `MutualTls` arm then throws away. Two guards keep that from happening: `attach_auth`
    // skips a `mutualTLS` scheme before `apply_credential` is called, and `takes_token` excludes
    // `MutualTls` should anything reach `apply_credential` another way. This pins the
    // behaviour they jointly hold rather than either line; drop both and the counter reaches 1.
    use std::sync::atomic::{AtomicUsize, Ordering};

    let calls = Arc::new(AtomicUsize::new(0));
    let provider_calls = Arc::clone(&calls);
    let mut core = core();
    core.set_credential(
        "mtls",
        Credential::Provider(Arc::new(move || {
            provider_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(SecretString::from("never-fetched")) }) as TokenFuture
        })),
    );
    let request = poll_ready(attach_auth(
        &core,
        get(&core),
        &[&[AuthScheme {
            name: "mtls",
            kind: AuthKind::MutualTls,
        }]],
    ))
    .unwrap()
    .build()
    .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a mutualTLS scheme must never invoke a token provider"
    );
    assert!(request.headers().is_empty(), "{:?}", request.headers());
    assert_eq!(request.url().query(), None, "{}", request.url());
}

use super::{build_url, build_url_on, build_url_with_query_string, is_dot_segment, StatusSpec};

fn core_at(base: &str) -> ClientCore {
    ClientCore::new(base).unwrap()
}

#[test]
fn build_url_collapses_double_slash_at_join() {
    let core = core_at("https://example.com/");
    let url = build_url(&core, "/foo", &[]).unwrap();
    // Trailing base slash + leading path slash collapse to a single separator.
    assert_eq!(url.path(), "/foo");
    // An empty query must not stamp a trailing `?` onto the serialized URL.
    assert_eq!(url.as_str(), "https://example.com/foo");
}

#[test]
fn build_url_preserves_base_path_prefix() {
    let core = core_at("https://example.com/api");
    let url = build_url(&core, "foo", &[]).unwrap();
    assert_eq!(url.path(), "/api/foo");
}

#[test]
fn build_url_empty_path_keeps_base_path() {
    let prefixed = core_at("https://example.com/api");
    assert_eq!(build_url(&prefixed, "", &[]).unwrap().path(), "/api");

    let root = core_at("https://example.com");
    assert_eq!(build_url(&root, "", &[]).unwrap().path(), "/");
}

#[test]
fn build_url_installs_pre_encoded_query_fragments_verbatim() {
    let core = core_at("https://example.com");
    // The caller has already encoded the data and left the style delimiter literal; the two
    // commas here must stay distinguishable all the way onto the wire.
    let url = build_url(
        &core,
        "/search",
        &["q=a%20b%26c".to_owned(), "tags=x%2Cy,z".to_owned()],
    )
    .unwrap();
    assert_eq!(url.query(), Some("q=a%20b%26c&tags=x%2Cy,z"));
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("q".to_owned(), "a b&c".to_owned()),
            ("tags".to_owned(), "x,y,z".to_owned()),
        ]
    );
}

#[test]
fn build_url_appends_to_a_query_already_on_the_base_url() {
    let core = core_at("https://example.com?tenant=acme");
    let url = build_url(&core, "/search", &["q=rust".to_owned()]).unwrap();
    assert_eq!(url.query(), Some("tenant=acme&q=rust"));
}

#[test]
fn build_url_does_not_join_onto_an_empty_query_on_the_base_url() {
    // `https://example.com/?` carries a query that is present but empty: there is nothing to
    // join onto, so no leading `&` may appear.
    let core = core_at("https://example.com/?");
    assert_eq!(core.base_url().query(), Some(""));
    let url = build_url(&core, "/search", &["q=rust".to_owned()]).unwrap();
    assert_eq!(url.query(), Some("q=rust"));
}

#[test]
fn build_url_keeps_matrix_and_label_prefixes_in_the_path() {
    let core = core_at("https://example.com");
    // `set_path` must not disturb `;`, `=`, `,` or an existing percent-triple.
    let url = build_url(&core, "/map/;position=B,150,R,100", &[]).unwrap();
    assert_eq!(url.path(), "/map/;position=B,150,R,100");
    let labelled = build_url(&core, "/files/.tar%2Egz", &[]).unwrap();
    assert_eq!(labelled.path(), "/files/.tar%2Egz");
}

/// Every spelling the URL Standard treats as a `.` or `..` path segment.
const DOT_SEGMENTS: [&str; 12] = [
    ".", "%2e", "%2E", "..", ".%2e", ".%2E", "%2e.", "%2E.", "%2e%2e", "%2E%2E", "%2e%2E", "%2E%2e",
];

/// Dot segments spelled with the ASCII tab, LF and CR the URL parser deletes before it reads
/// a segment, so `set_path` removes each of them as it does `.` or `..`.
const WHITESPACE_DOT_SEGMENTS: [&str; 9] = [
    ".\t.", "..\n", "\r..", ".\n", "\t.", "%2\tE", ".%2\re", "%\n2e%2E", "\t.\n.\r",
];

#[test]
fn dot_segments_spelled_with_tab_lf_or_cr_are_what_set_path_removes() {
    let mut url = reqwest::Url::parse("https://example.com/").unwrap();
    for segment in WHITESPACE_DOT_SEGMENTS {
        url.set_path(&format!("/a/{segment}/b"));
        assert!(
            url.path() == "/b" || url.path() == "/a/b",
            "{segment:?}: {}",
            url.path()
        );
        assert!(is_dot_segment(segment), "{segment:?}");
    }
    // Deleting the whitespace leaves no dot segment, so these are not refused.
    for segment in ["\t", ".\t..", "a\n.", "%2\tE%2E."] {
        assert!(!is_dot_segment(segment), "{segment:?}");
    }
}

#[test]
fn build_url_refuses_template_text_that_forms_a_dot_segment_once_whitespace_is_deleted() {
    let core = core_at("https://api.example.com/v1/");
    for segment in WHITESPACE_DOT_SEGMENTS {
        let error = build_url(&core, &format!("/users/{segment}/keys"), &[]).expect_err(segment);
        assert!(
            matches!(error, Error::RequestConstruction(RequestError::Other(_))),
            "{segment:?}: {error:?}"
        );
        build_url(&core, &format!("/users/{segment}"), &[]).expect_err(segment);
    }
}

#[test]
fn the_dot_segment_refusal_escapes_the_segments_tab_lf_and_cr() {
    // #452: the refused segment may hold the tab, LF or CR `is_dot_segment` ignores; the
    // message must show them escaped and stay on one line.
    let core = core_at("https://api.example.com/v1/");
    for (segment, escaped) in [
        (".\t.", r#"".\t.""#),
        ("..\n", r#""..\n""#),
        ("\r..", r#""\r..""#),
    ] {
        let error = build_url(&core, &format!("/users/{segment}/keys"), &[]).expect_err(segment);
        let cause = std::error::Error::source(&error)
            .map(ToString::to_string)
            .expect("a refusal carries its message");
        assert!(
            cause.contains(&format!("the dot segment {escaped},")),
            "{segment:?}: {cause:?}"
        );
        assert!(
            !cause.contains(['\t', '\n', '\r']),
            "{segment:?}: {cause:?}"
        );
    }
}

/// One path segment: never `/` or `\`, the separators the guard splits on, and drawn mostly
/// from dots, `%2E` in either case, near-misses of it, and the tab, LF and CR the URL parser
/// deletes.
fn segment() -> impl proptest::strategy::Strategy<Value = String> {
    use proptest::strategy::Strategy;
    proptest::string::string_regex("(\\.|%2[eE]|%2|%|2|[eE]|\\t|\\n|\\r|a| |\\?|#|;|\\PC){0,6}")
        .expect("a valid regex")
        .prop_filter("a single segment", |s| !s.contains(['/', '\\']))
}

proptest::proptest! {
    /// The guard agrees with `url` over the whole space of single segments, not only at the
    /// listed spellings: `is_dot_segment(s)` holds exactly when `set_path("/a/{s}/b")` stops
    /// being the three segments it was written as.
    #[test]
    fn is_dot_segment_holds_exactly_where_set_path_removes_the_segment(segment in segment()) {
        let mut url = reqwest::Url::parse("https://example.com/").unwrap();
        url.set_path(&format!("/a/{segment}/b"));
        let segments = url.path_segments().map_or(0, Iterator::count);
        proptest::prop_assert_eq!(
            is_dot_segment(&segment),
            segments != 3,
            "{:?} -> {}",
            segment,
            url.path()
        );
    }
}

#[test]
fn dot_segment_spellings_are_exactly_what_set_path_removes() {
    // Pins the guard's spelling list to `url`'s own normalization: each listed segment is
    // removed (or climbs) under `set_path`, and near-misses survive it untouched.
    let mut url = reqwest::Url::parse("https://example.com/").unwrap();
    for segment in DOT_SEGMENTS {
        url.set_path(&format!("/a/{segment}/b"));
        assert_ne!(url.path(), format!("/a/{segment}/b"), "{segment}");
        assert!(is_dot_segment(segment), "{segment}");
    }
    for segment in [
        "...",
        ".a",
        "a.",
        "%2e%2e%2e",
        "%2",
        "%2F..",
        ".%2",
        "",
        "a..b",
    ] {
        url.set_path(&format!("/a/{segment}/b"));
        assert_eq!(url.path(), format!("/a/{segment}/b"), "{segment}");
        assert!(!is_dot_segment(segment), "{segment}");
    }
}

#[test]
fn build_url_refuses_a_path_value_that_forms_a_dot_segment() {
    // #406: `/users/{p}/keys` with `p = ".."` must not be sent to `/v1/keys`.
    let core = core_at("https://api.example.com/v1/");
    for segment in DOT_SEGMENTS {
        let error = build_url(&core, &format!("/users/{segment}/keys"), &[]).expect_err(segment);
        assert!(
            matches!(error, Error::RequestConstruction(RequestError::Other(_))),
            "{segment}: {error:?}"
        );
        let cause = std::error::Error::source(&error).map(ToString::to_string);
        assert!(
            cause.as_deref().is_some_and(|c| c.contains("dot segment")),
            "{cause:?}"
        );
        // The trailing segment, and the whole request path, are segments too.
        build_url(&core, &format!("/users/{segment}"), &[]).expect_err(segment);
        build_url(&core, segment, &[]).expect_err(segment);
    }
    // A special-scheme URL treats `\` as a separator as well.
    build_url(&core, "/users\\..\\keys", &[]).expect_err("backslash");
    // Through every builder.
    build_url_on(&core, Some("/v2/"), "/users/../keys", &[]).expect_err("override");
    build_url_with_query_string(&core, "/users/%2E%2E/keys", &[], Some("a=b"))
        .expect_err("querystring");
}

#[test]
fn a_label_value_that_is_only_its_prefix_is_refused_as_a_whole_segment() {
    // The `label` undefined row is `.`; as a whole segment no encoding of it survives
    // `set_path`, so the request is refused rather than re-targeted.
    let rendered = crate::serialize_label(
        &serde_json::Value::Null,
        false,
        crate::PercentEncoding::Unreserved,
    )
    .unwrap();
    assert_eq!(rendered, ".");
    let core = core_at("https://example.com/v1");
    build_url(&core, &format!("/colors/{rendered}/shades"), &[]).expect_err("label");
}

#[test]
fn build_url_keeps_dots_that_do_not_form_a_whole_segment() {
    let core = core_at("https://api.example.com/v1/");
    for path in [
        "/users/.../keys",
        "/files/.tar.gz",
        "/v../x",
        "/a/.%2e%2e",
        "/x.",
    ] {
        let url = build_url(&core, path, &[]).unwrap();
        assert_eq!(url.path(), format!("/v1{path}"));
    }
}

#[test]
fn build_url_on_replaces_the_base_with_an_absolute_server_override() {
    let core = core_at("https://example.com/api");
    let url = build_url_on(&core, Some("https://files.example.net/v2"), "/blobs", &[]).unwrap();
    assert_eq!(url.as_str(), "https://files.example.net/v2/blobs");
}

#[test]
fn build_url_on_joins_a_relative_server_override_onto_the_base() {
    let core = core_at("https://example.com/api/");
    let url = build_url_on(&core, Some("../edge/"), "/blobs", &[]).unwrap();
    assert_eq!(url.as_str(), "https://example.com/edge/blobs");
}

#[test]
fn build_url_with_query_string_installs_a_whole_query_verbatim() {
    let core = core_at("https://example.com?stale=server-value");
    // The whole-query value arrives already encoded by the generated method.
    let url = build_url_with_query_string(
        &core,
        "/search",
        &[],
        Some("%7B%22numbers%22%3A%5B1%2C2%5D%7D"),
    )
    .unwrap();
    assert_eq!(url.query(), Some("%7B%22numbers%22%3A%5B1%2C2%5D%7D"));
}

#[test]
fn build_url_replaces_the_server_query_with_a_form_whole_query_string() {
    let core = core_at("https://example.com?stale=server-value");
    let url = build_url_with_query_string(&core, "/search", &["term=rust%20api".to_owned()], None)
        .unwrap();
    assert_eq!(url.query(), Some("term=rust%20api"));
}

/// Both arguments at once. Generated code never passes both — lowering rejects an `in: query`
/// parameter beside an `in: querystring` one (the 3.2 Parameter Locations rule forbids it),
/// and the one `querystring` parameter fills only one of them — but the entry point is public,
/// so what it does with both is still its contract: the fragments come first and the
/// whole-query value is joined after them, with no separator where the fragments rendered to
/// nothing.
#[test]
fn build_url_with_query_string_joins_the_whole_query_after_any_fragments() {
    let core = core_at("https://example.com?stale=server-value");
    let whole = Some("b=2");
    let joined = build_url_with_query_string(&core, "/search", &["a=1".to_owned()], whole).unwrap();
    assert_eq!(joined.query(), Some("a=1&b=2"));
    let after_empty =
        build_url_with_query_string(&core, "/search", &[String::new()], whole).unwrap();
    assert_eq!(after_empty.query(), Some("b=2"));
    let alone = build_url_with_query_string(&core, "/search", &[], whole).unwrap();
    assert_eq!(alone.query(), Some("b=2"));
}

#[test]
fn status_spec_matches_exact_range_and_any() {
    use reqwest::StatusCode;

    assert!(StatusSpec::Exact(404).matches(StatusCode::NOT_FOUND));
    assert!(!StatusSpec::Exact(404).matches(StatusCode::INTERNAL_SERVER_ERROR));

    assert!(StatusSpec::Range(5).matches(StatusCode::SERVICE_UNAVAILABLE));
    assert!(!StatusSpec::Range(5).matches(StatusCode::NOT_FOUND));

    assert!(StatusSpec::Any.matches(StatusCode::OK));
    assert!(StatusSpec::Any.matches(StatusCode::IM_A_TEAPOT));
}

use std::convert::Infallible;

use super::{
    classify_error_bytes, classify_error_text, decode_success_bytes, decode_success_text,
    decode_text_body, read_error_body, read_success_body,
};
use crate::{Error, ResponseValue};

/// Synthesize an in-memory `reqwest::Response` (no server, no runtime) so the body readers can be
/// driven with a poll-once noop waker.
fn json_response(status: u16, body: &str) -> reqwest::Response {
    reqwest::Response::from(
        http::Response::builder()
            .status(status)
            .body(body.to_owned())
            .expect("valid synthetic response"),
    )
}

fn raw_response(status: u16, body: impl Into<reqwest::Body>) -> reqwest::Response {
    reqwest::Response::from(
        http::Response::builder()
            .status(status)
            .body(body.into())
            .expect("valid synthetic response"),
    )
}

#[derive(serde::Deserialize, Debug, PartialEq)]
enum TextChoice {
    #[serde(rename = "ready")]
    Ready,
}

#[test]
fn textual_codec_uses_raw_utf8_as_a_typed_json_string() {
    assert_eq!(
        decode_text_body::<String>(b"not quoted").unwrap(),
        "not quoted"
    );
    assert_eq!(
        decode_text_body::<TextChoice>(b"ready").unwrap(),
        TextChoice::Ready
    );
    assert!(decode_text_body::<String>(&[0xff]).is_err());

    let value = poll_ready(decode_success_text::<String>(
        &core(),
        json_response(200, "<p>raw</p>"),
    ))
    .unwrap();
    assert_eq!(value.into_inner(), "<p>raw</p>");
}

/// An empty body under a status that documents a textual body is the zero-length text, so
/// `String` decodes it to `""` on purpose (#126) — while a typed text value (a string enum or
/// format) that has no empty member still fails with `Decode`, because the codec decodes the
/// empty text rather than special-casing it. A documented bodyless status never reaches this
/// codec: beside a documented body it is a unit variant of the response enum (#121, #204), and
/// otherwise `()` on the success side or `Error::UnexpectedStatus` on the error side.
#[test]
fn textual_codec_reads_an_empty_body_as_the_empty_string() {
    assert_eq!(decode_text_body::<String>(b"").unwrap(), "");
    assert!(decode_text_body::<TextChoice>(b"").is_err());

    let value = poll_ready(decode_success_text::<String>(
        &core(),
        json_response(200, ""),
    ))
    .unwrap();
    assert_eq!(value.into_inner(), "");

    match poll_ready(decode_success_text::<TextChoice>(
        &core(),
        json_response(200, ""),
    )) {
        Err(Error::Decode { status, body, .. }) => {
            assert_eq!(status.as_u16(), 200);
            assert!(body.is_empty());
        }
        other => panic!("expected a Decode error, got {other:?}"),
    }
}

#[test]
fn binary_codec_preserves_success_and_documented_error_bytes() {
    let success = poll_ready(decode_success_bytes(
        &core(),
        raw_response(200, bytes::Bytes::from_static(b"\0raw\xff")),
    ))
    .unwrap();
    assert_eq!(&success.into_inner()[..], b"\0raw\xff");

    let error = poll_ready(classify_error_bytes::<bytes::Bytes>(
        &core(),
        raw_response(400, bytes::Bytes::from_static(b"bad\0")),
        &[StatusSpec::Exact(400)],
    ));
    match error {
        Error::Api(response) => assert_eq!(&response.into_inner()[..], b"bad\0"),
        other => panic!("expected raw API error, got {other:?}"),
    }
}

#[test]
fn textual_error_codec_keeps_documented_status_semantics() {
    let error = poll_ready(classify_error_text::<String>(
        &core(),
        json_response(400, "plain failure"),
        &[StatusSpec::Exact(400)],
    ));
    match error {
        Error::Api(response) => assert_eq!(response.into_inner(), "plain failure"),
        other => panic!("expected textual API error, got {other:?}"),
    }
}

#[test]
fn read_success_body_returns_status_and_bytes() {
    let response = json_response(201, r#"{"ok":true}"#);
    let (status, _headers, body) = poll_ready(read_success_body(response)).unwrap();
    assert_eq!(status, reqwest::StatusCode::CREATED);
    assert_eq!(&body[..], br#"{"ok":true}"#);
}

/// Every runtime helper that raises `Decode` keeps the status and headers of the response it
/// failed to decode, so `Error::status()` answers it and a caller can still read, say, the
/// `Retry-After` of a documented `429` whose body a proxy replaced with HTML (#268). Each
/// response carries a status other than `200` and a header value of its own, so neither a
/// hard-coded status nor an empty or shared header map can pass.
#[test]
fn every_decode_helper_keeps_the_response_status_and_headers() {
    use reqwest::StatusCode;

    fn response(status: u16, retry_after: &'static str, body: &str) -> reqwest::Response {
        reqwest::Response::from(
            http::Response::builder()
                .status(status)
                .header("retry-after", retry_after)
                .body(body.to_owned())
                .expect("valid synthetic response"),
        )
    }

    fn assert_decode<E: std::fmt::Debug>(error: Error<E>, expected: u16, retry_after: &str) {
        match &error {
            Error::Decode {
                status, headers, ..
            } => {
                assert_eq!(status.as_u16(), expected);
                assert_eq!(
                    headers.get("retry-after").map(|value| value.as_bytes()),
                    Some(retry_after.as_bytes()),
                    "the Decode error for {expected} lost its response headers"
                );
            }
            other => panic!("expected a Decode error, got {other:?}"),
        }
        assert_eq!(error.status(), StatusCode::from_u16(expected).ok());
    }

    let success = poll_ready(super::decode_success::<Created>(
        &core(),
        response(203, "1", "not json"),
    ));
    assert_decode(success.unwrap_err(), 203, "1");

    let text = poll_ready(decode_success_text::<TextChoice>(
        &core(),
        response(206, "2", "not a choice"),
    ));
    assert_decode(text.unwrap_err(), 206, "2");

    let documented = [StatusSpec::Exact(429)];
    let error = poll_ready(super::classify_error::<Created>(
        &core(),
        response(429, "3", "<html>rate limited</html>"),
        &documented,
    ));
    assert_decode(error, 429, "3");

    let error = poll_ready(classify_error_text::<TextChoice>(
        &core(),
        response(429, "4", "not a choice"),
        &documented,
    ));
    assert_decode(error, 429, "4");
}

#[test]
fn a_decode_message_escapes_the_servers_lf_cr_and_tab() {
    // #457: serde quotes an unknown variant verbatim, so a server-sent `"x\ny\r\tz"` reaches
    // the message; it must show those characters escaped and stay on one line, on both the
    // success and the documented-error decode paths.
    #[derive(serde::Deserialize, Debug)]
    enum Problem {
        #[serde(rename = "gone")]
        Gone,
    }
    impl std::fmt::Display for Problem {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{self:?}")
        }
    }
    let body = r#""x\ny\r\tz""#;
    let success = poll_ready(super::decode_success::<TextChoice>(
        &core(),
        json_response(200, body),
    ))
    .unwrap_err();
    let error = poll_ready(super::classify_error::<Problem>(
        &core(),
        json_response(422, body),
        &[StatusSpec::Exact(422)],
    ));
    assert!(matches!(success, Error::Decode { .. }), "{success:?}");
    assert!(matches!(error, Error::Decode { .. }), "{error:?}");
    for message in [success.to_string(), error.to_string()] {
        assert!(message.contains(r"x\ny\r\tz"), "{message:?}");
        assert!(!message.contains(['\n', '\r', '\t']), "{message:?}");
    }
}

#[test]
fn read_error_body_truncates_at_cap() {
    let mut core = core();
    core.config_mut().max_error_body = 4;
    let response = json_response(500, "0123456789");
    let (status, _headers, body, truncated) =
        poll_ready(read_error_body::<std::convert::Infallible>(&core, response)).unwrap();
    assert_eq!(status, reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    assert!(truncated);
    assert_eq!(body.len(), 4);
    assert_eq!(&body[..], b"0123");
}

// Stand-ins for a generated multi-status SUCCESS enum: two success statuses, distinct bodies.
#[derive(serde::Deserialize, Debug, PartialEq)]
struct Created {
    id: u32,
}
#[derive(serde::Deserialize, Debug, PartialEq)]
struct Accepted {
    job: String,
}
#[derive(Debug, PartialEq)]
enum SuccessEnum {
    Status200(Created),
    Status202(Accepted),
}

/// A hand-written stand-in shaped like the generated per-status success dispatch: read once,
/// select the variant whose selector matches (arm order = precedence), decode into it, else an
/// undocumented-success error. Nothing ties it to the emitter's template: the tests over it pin
/// the runtime primitives it calls (`read_success_body`, `StatusSpec::matches`) and nothing
/// more. The emitted dispatch itself is driven over HTTP in `spargen/tests/e2e.rs`
/// (`success_dispatch_takes_the_exact_arm_before_an_overlapping_range`,
/// `a_bodyless_success_beside_one_body_is_its_own_variant`).
fn dispatch_success(
    response: reqwest::Response,
) -> Result<ResponseValue<SuccessEnum>, Error<Infallible>> {
    let (status, headers, body) = poll_ready(read_success_body(response))?;
    if StatusSpec::Exact(200).matches(status) {
        let value = match serde_json::from_slice::<Created>(&body) {
            Ok(value) => value,
            Err(error) => {
                return Err(Error::Decode {
                    status,
                    headers,
                    path: error.to_string(),
                    body,
                    truncated: false,
                })
            }
        };
        return Ok(ResponseValue::new(
            status,
            headers,
            SuccessEnum::Status200(value),
        ));
    }
    if StatusSpec::Exact(202).matches(status) {
        let value = match serde_json::from_slice::<Accepted>(&body) {
            Ok(value) => value,
            Err(error) => {
                return Err(Error::Decode {
                    status,
                    headers,
                    path: error.to_string(),
                    body,
                    truncated: false,
                })
            }
        };
        return Ok(ResponseValue::new(
            status,
            headers,
            SuccessEnum::Status202(value),
        ));
    }
    Err(Error::UnexpectedStatus {
        status,
        headers,
        body,
    })
}

#[test]
fn success_dispatch_selects_variant_per_status() {
    let created = dispatch_success(json_response(200, r#"{"id":7}"#)).unwrap();
    assert_eq!(*created.inner(), SuccessEnum::Status200(Created { id: 7 }));
    let accepted = dispatch_success(json_response(202, r#"{"job":"j"}"#)).unwrap();
    assert_eq!(
        *accepted.inner(),
        SuccessEnum::Status202(Accepted {
            job: "j".to_owned()
        })
    );
}

#[test]
fn success_dispatch_undocumented_status_has_no_untyped_fallback() {
    // 201 is a success status matching no documented variant → an unexpected-status error, never
    // a silent `serde_json::Value`.
    let error = dispatch_success(json_response(201, r#"{"id":1}"#)).unwrap_err();
    assert!(matches!(error, Error::UnexpectedStatus { .. }));
}

#[test]
fn success_dispatch_parse_failure_is_decode() {
    let error = dispatch_success(json_response(202, "not json")).unwrap_err();
    assert!(matches!(error, Error::Decode { .. }));
    assert_eq!(error.status(), Some(reqwest::StatusCode::ACCEPTED));
}

// Stand-ins for a generated multi-status ERROR enum: an exact status plus a range that would
// also cover it — precedence must prefer the exact selector (it is checked first).
#[derive(serde::Deserialize, Debug, PartialEq)]
struct Conflict {
    conflict: String,
}
#[derive(serde::Deserialize, Debug, PartialEq)]
struct ClientError {
    message: String,
}
#[derive(Debug, PartialEq)]
enum ApiError {
    Status409(Conflict),
    Status4xx(ClientError),
}

/// A hand-written stand-in shaped like the generated per-status error classification: read
/// capped, select by status (exact before range), decode → `Api`; a parse failure → `Decode`;
/// an undocumented status → `UnexpectedStatus`. Like `dispatch_success`, it pins the runtime
/// primitives (`read_error_body`, `StatusSpec::matches`), not the emitter; the emitted
/// classification is driven over HTTP in `spargen/tests/e2e.rs`
/// (`error_dispatch_takes_the_exact_arm_before_an_overlapping_range`).
fn dispatch_error(response: reqwest::Response) -> Error<ApiError> {
    let core = core();
    let (status, headers, body, truncated) =
        match poll_ready(read_error_body::<ApiError>(&core, response)) {
            Ok(parts) => parts,
            Err(error) => return error,
        };
    if StatusSpec::Exact(409).matches(status) {
        return match serde_json::from_slice::<Conflict>(&body) {
            Ok(value) => Error::Api(ResponseValue::new(
                status,
                headers,
                ApiError::Status409(value),
            )),
            Err(error) => Error::Decode {
                status,
                headers,
                path: error.to_string(),
                body,
                truncated,
            },
        };
    }
    if StatusSpec::Range(4).matches(status) {
        return match serde_json::from_slice::<ClientError>(&body) {
            Ok(value) => Error::Api(ResponseValue::new(
                status,
                headers,
                ApiError::Status4xx(value),
            )),
            Err(error) => Error::Decode {
                status,
                headers,
                path: error.to_string(),
                body,
                truncated,
            },
        };
    }
    Error::UnexpectedStatus {
        status,
        headers,
        body,
    }
}

#[test]
fn error_dispatch_exact_selector_beats_range() {
    // 409 matches both `Exact(409)` and `Range(4)`; the exact variant wins because it is tried
    // first, preserving spec precedence.
    match dispatch_error(json_response(409, r#"{"conflict":"dup"}"#)) {
        Error::Api(value) => assert_eq!(
            *value.inner(),
            ApiError::Status409(Conflict {
                conflict: "dup".to_owned()
            })
        ),
        other => panic!("expected Api(Status409), got {other:?}"),
    }
}

#[test]
fn error_dispatch_range_matches_other_4xx() {
    match dispatch_error(json_response(404, r#"{"message":"nope"}"#)) {
        Error::Api(value) => assert_eq!(
            *value.inner(),
            ApiError::Status4xx(ClientError {
                message: "nope".to_owned()
            })
        ),
        other => panic!("expected Api(Status4xx), got {other:?}"),
    }
}

#[test]
fn error_dispatch_undocumented_status_is_unexpected() {
    let error = dispatch_error(json_response(500, r#"{}"#));
    assert!(matches!(error, Error::UnexpectedStatus { .. }));
}

#[test]
fn error_dispatch_parse_failure_is_decode() {
    let error = dispatch_error(json_response(409, "not json"));
    assert!(matches!(error, Error::Decode { .. }));
    assert_eq!(error.status(), Some(reqwest::StatusCode::CONFLICT));
}

// --- error-body cap fixtures -------------------------------------------------------------
//
// `max_error_body` is documented as a cap on RETENTION (README, `config.rs`, and
// `Error::Decode`'s own field docs). These pin all three ways that contract can be broken:
// retaining a view into an oversized buffer, reading an unbounded body before capping, and
// the success-decode paths ignoring the cap outright.

/// A body far larger than the cap must not leave the original allocation reachable through the
/// retained `Bytes`. `Bytes::slice` returns a refcounted view whose parent stays alive; a real
/// copy is detached, which `try_into_mut` (unique ownership) reports through its capacity.
#[test]
fn capped_error_body_does_not_retain_the_oversized_allocation() {
    let mut core = core();
    core.config_mut().max_error_body = 8;
    let response = json_response(500, &"x".repeat(64 * 1024));
    let (_status, _headers, body, truncated) =
        poll_ready(read_error_body::<std::convert::Infallible>(&core, response)).unwrap();
    assert!(truncated);
    assert_eq!(body.len(), 8);
    let owned = body
        .try_into_mut()
        .expect("retained body should be uniquely owned");
    assert_eq!(
        owned.capacity(),
        8,
        "retained body still points into the full response allocation"
    );
}

/// The cap must bound PEAK memory too: an oversized body has to stop being pulled once enough
/// bytes are in hand, rather than being buffered whole and trimmed afterwards.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn oversized_error_body_stops_being_read_at_the_cap() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 1 KiB chunks against the 4 KiB cap below: four pulls reach the cap exactly, and the
    /// fifth is the one that proves the remainder is being dropped.
    const PULLS_FOR_4K_CAP: usize = 5;

    // A stream of 1 KiB chunks that counts how many were actually pulled. Always immediately
    // ready, so the poll-once waker is enough.
    struct Counting {
        remaining: usize,
        pulled: Arc<AtomicUsize>,
    }
    impl futures_core::Stream for Counting {
        type Item = Result<Bytes, std::io::Error>;
        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            if self.remaining == 0 {
                return Poll::Ready(None);
            }
            self.remaining -= 1;
            self.pulled.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Some(Ok(Bytes::from_static(&[b'x'; 1024]))))
        }
    }

    let pulled = Arc::new(AtomicUsize::new(0));
    let body = reqwest::Body::wrap_stream(Counting {
        remaining: 1024, // 1 MiB available
        pulled: Arc::clone(&pulled),
    });

    let mut core = core();
    core.config_mut().max_error_body = 4096; // 4 KiB cap

    let (_status, _headers, retained, truncated) = poll_ready(read_error_body::<
        std::convert::Infallible,
    >(&core, raw_response(500, body)))
    .unwrap();

    assert!(truncated);
    assert_eq!(retained.len(), 4096);
    let pulled = pulled.load(Ordering::SeqCst);
    // Derived, not fitted: the loop exits at the first pull that puts the buffer past the cap,
    // so 1 KiB chunks against a 4 KiB cap is exactly five. Asserting the tight value is what
    // pins "stops at the first over-cap chunk" rather than merely "stops somewhere early".
    assert_eq!(
        pulled, PULLS_FOR_4K_CAP,
        "read {pulled} KiB chunks for a 4 KiB cap - the whole body was buffered"
    );
}

/// An UNDER-cap body must not pin the read buffer either. The incremental read grows a
/// `BytesMut` by doubling and `freeze` hands it over at full capacity, so 40 KiB arriving in
/// chunks under the 64 KiB default retains 64 KiB — measured, not hypothetical. Same contract
/// violation as the oversized case, just bounded by the cap instead of by the server.
///
/// The body has to arrive in MULTIPLE chunks to reproduce: a single-chunk body reserves
/// exactly once and lands on an exact-size allocation, so it hides the defect.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn an_under_cap_body_does_not_retain_the_read_buffer() {
    let mut core = core();
    core.config_mut().max_error_body = 64 * 1024;
    // 40 x 1 KiB, all under the cap, so the read runs to EOF and nothing is truncated.
    // `futures-core` is the only stream dependency here and carries no combinators, so the
    // stream is hand-rolled; it is always immediately ready, which the poll-once waker needs.
    struct Chunks(usize);
    impl futures_core::Stream for Chunks {
        type Item = Result<Bytes, std::io::Error>;
        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            if self.0 == 0 {
                return Poll::Ready(None);
            }
            self.0 -= 1;
            Poll::Ready(Some(Ok(Bytes::from_static(&[b'x'; 1024]))))
        }
    }

    let body = reqwest::Body::wrap_stream(Chunks(40));
    let (_status, _headers, body, truncated) = poll_ready(read_error_body::<
        std::convert::Infallible,
    >(&core, raw_response(500, body)))
    .unwrap();
    assert!(!truncated);
    assert_eq!(body.len(), 40 * 1024);
    let owned = body
        .try_into_mut()
        .expect("retained body should be uniquely owned");
    assert_eq!(
        owned.capacity(),
        40 * 1024,
        "under-cap body still pins the doubled read buffer"
    );
}

/// A body of exactly `cap` bytes is not truncated. This pins `cap_body`'s truncation test:
/// widening `body.len() > cap` to `>=` reports an exact-fit body as truncated. Verified by
/// regressing it — that mutation fails this fixture and `a_zero_cap_retains_nothing`, whose
/// empty body under a zero cap is the same `len == cap` case, and nothing else.
#[test]
fn a_body_of_exactly_the_cap_is_not_truncated() {
    let mut core = core();
    core.config_mut().max_error_body = 32;
    let response = json_response(500, &"x".repeat(32));
    let (_status, _headers, body, truncated) =
        poll_ready(read_error_body::<std::convert::Infallible>(&core, response)).unwrap();
    assert!(!truncated, "an exact-fit body must not report truncation");
    assert_eq!(body.len(), 32);
}

/// A cap of zero retains nothing and still terminates. This is the only fixture that pins the
/// read loop's own `<= cap` bound: narrowing it to `<` never enters the loop at all, so an
/// over-cap body is silently reported as complete. It also covers `cap_body`'s truncation test
/// from the other side, since an empty body under a zero cap is `len == cap`. Both verified by
/// regressing each comparison.
#[test]
fn a_zero_cap_retains_nothing() {
    let mut core = core();
    core.config_mut().max_error_body = 0;

    let (_status, _headers, body, truncated) = poll_ready(read_error_body::<
        std::convert::Infallible,
    >(&core, json_response(500, "body")))
    .unwrap();
    assert!(truncated);
    assert!(body.is_empty());

    // An empty body under a zero cap is the one case that must NOT report truncation: nothing
    // was dropped.
    let (_status, _headers, body, truncated) = poll_ready(read_error_body::<
        std::convert::Infallible,
    >(&core, json_response(500, "")))
    .unwrap();
    assert!(!truncated, "nothing was dropped, so nothing was truncated");
    assert!(body.is_empty());
}

/// A SUCCESS response that fails to deserialize must also honour the cap: `Error::Decode`
/// documents its body as retained "up to the configured cap", with `truncated` saying so.
#[test]
fn decode_failure_on_success_body_honours_the_cap() {
    let mut core = core();
    core.config_mut().max_error_body = 16;
    // Valid UTF-8, not valid JSON for `Created`, and far larger than the cap.
    let response = json_response(200, &"n".repeat(32 * 1024));
    match poll_ready(super::decode_success::<Created>(&core, response)) {
        Err(Error::Decode {
            body, truncated, ..
        }) => {
            assert!(truncated, "oversized decode body reported as untruncated");
            assert_eq!(body.len(), 16);
        }
        other => panic!("expected a capped Decode error, got {other:?}"),
    }
}

/// The textual codec shares the cap.
#[test]
fn textual_decode_failure_honours_the_cap() {
    let mut core = core();
    core.config_mut().max_error_body = 16;
    let response = json_response(200, &"n".repeat(32 * 1024));
    match poll_ready(super::decode_success_text::<TextChoice>(&core, response)) {
        Err(Error::Decode {
            body, truncated, ..
        }) => {
            assert!(truncated);
            assert_eq!(body.len(), 16);
        }
        other => panic!("expected a capped Decode error, got {other:?}"),
    }
}

/// Assert `result` is a `Decode` carrying the zero-length body of the `status` it failed on.
fn assert_empty_body_decode<T: std::fmt::Debug>(
    result: Result<ResponseValue<T>, Error<Infallible>>,
    status: u16,
) {
    match result {
        Err(Error::Decode {
            status: got,
            headers: _,
            path,
            body,
            truncated,
        }) => {
            assert_eq!(got.as_u16(), status);
            assert!(!path.is_empty(), "a Decode error must say why");
            assert!(body.is_empty(), "an empty body retained as {body:?}");
            assert!(!truncated, "an empty body reported as truncated");
        }
        other => panic!("expected a Decode error for an empty body, got {other:?}"),
    }
}

/// A zero-length success body is not a JSON value, so the JSON codec rejects it as `Decode`
/// whatever `T` is — even a `T` (`String`, `Value`) that would accept an empty *text* body.
#[test]
fn json_codec_rejects_a_zero_length_success_body() {
    assert_empty_body_decode(
        poll_ready(super::decode_success::<Created>(
            &core(),
            raw_response(200, ""),
        )),
        200,
    );
    assert_empty_body_decode(
        poll_ready(super::decode_success::<String>(
            &core(),
            raw_response(204, ""),
        )),
        204,
    );
    assert_empty_body_decode(
        poll_ready(super::decode_success::<serde_json::Value>(
            &core(),
            raw_response(204, ""),
        )),
        204,
    );
}

/// A zero-length binary body is an empty `Bytes`: every byte sequence is a valid binary body.
#[test]
fn binary_codec_returns_a_zero_length_success_body_as_empty_bytes() {
    let value = poll_ready(decode_success_bytes(&core(), raw_response(204, ""))).unwrap();
    assert_eq!(value.status().as_u16(), 204);
    assert!(value.into_inner().is_empty());
}

#[derive(serde::Deserialize, Debug, PartialEq)]
enum TextChoiceWithEmpty {
    #[serde(rename = "")]
    Empty,
    #[serde(rename = "ready")]
    Ready,
}

/// The text codec decodes a zero-length body as the string `""`, so whether it succeeds is
/// `T`'s decision, not the codec's. Pinned for each textual `T` the generator can emit, through
/// both the single-body codec and the per-arm `decode_text_body` a multi-status enum uses.
#[test]
fn text_codec_decodes_a_zero_length_success_body_as_the_empty_string() {
    // Accepts `""`: `String` — also what `format: uuid`/`date-time`/`date` lower to with the
    // `uuid`/`time` features off — an untyped `{}` schema, and an enum with an empty variant.
    assert_eq!(decode_text_body::<String>(b"").unwrap(), "");
    assert_eq!(
        decode_text_body::<serde_json::Value>(b"").unwrap(),
        serde_json::Value::String(String::new())
    );
    assert_eq!(
        decode_text_body::<TextChoiceWithEmpty>(b"").unwrap(),
        TextChoiceWithEmpty::Empty
    );
    let value = poll_ready(decode_success_text::<String>(
        &core(),
        raw_response(204, ""),
    ))
    .unwrap();
    assert_eq!(value.status().as_u16(), 204);
    assert_eq!(value.into_inner(), "");
    let value = poll_ready(decode_success_text::<serde_json::Value>(
        &core(),
        raw_response(200, ""),
    ))
    .unwrap();
    assert_eq!(value.into_inner(), serde_json::Value::String(String::new()));

    // Rejects `""`: an enum with no empty variant.
    assert!(decode_text_body::<TextChoice>(b"").is_err());
    assert_empty_body_decode(
        poll_ready(decode_success_text::<TextChoice>(
            &core(),
            raw_response(204, ""),
        )),
        204,
    );
}

/// With the `time` feature on, `format: date-time`/`date` lower to the RFC 3339 newtypes, which
/// reject `""` — the same schemas that decode an empty body as `""` with the feature off.
#[cfg(feature = "time")]
#[test]
fn text_codec_rejects_a_zero_length_body_for_the_rfc3339_newtypes() {
    assert!(decode_text_body::<crate::DateTime>(b"").is_err());
    assert!(decode_text_body::<crate::Date>(b"").is_err());
    assert_empty_body_decode(
        poll_ready(decode_success_text::<crate::DateTime>(
            &core(),
            raw_response(204, ""),
        )),
        204,
    );
    assert_empty_body_decode(
        poll_ready(decode_success_text::<crate::Date>(
            &core(),
            raw_response(200, ""),
        )),
        200,
    );
}

/// Every message in `error`'s `source()` chain, outermost first.
fn messages(error: &(dyn std::error::Error + 'static)) -> Vec<String> {
    std::iter::successors(Some(error), |error| error.source())
        .map(ToString::to_string)
        .collect()
}

/// Whether every message in `error`'s chain is free of control characters, so none can break
/// the line it is logged on or forge another.
fn single_line(error: &(dyn std::error::Error + 'static)) -> Result<(), String> {
    match messages(error)
        .into_iter()
        .find(|message| message.contains(char::is_control))
    {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

/// Text biased to control characters: the C0 set, DEL and the C1 set, beside dots, `%2E` and
/// ordinary characters.
fn control_biased() -> impl proptest::strategy::Strategy<Value = String> {
    proptest::string::string_regex(
        "([\\x00-\\x1f]|\\x7f|[\\u{80}-\\u{9f}]|\\.|%2[eE]|a|\"|\\PC){0,8}",
    )
    .expect("a valid regex")
}

/// A documented string enum: serde's message for a value it does not list quotes the value.
/// It is an error body too, so the classifiers' `Error<Listed>` has a `source()` chain.
#[derive(Debug, serde::Deserialize)]
enum Listed {
    Listed,
}

impl std::fmt::Display for Listed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Listed::Listed => formatter.write_str("listed"),
        }
    }
}

impl std::error::Error for Listed {}

proptest::proptest! {
    /// `build_url` refuses a path value that forms a dot segment by quoting the segment, and
    /// a server override that does not parse through `url`'s error: whatever the text holds,
    /// every message in the refusal's chain stays on one line (#452).
    #[test]
    fn a_url_refusal_message_is_a_single_line(
        segment in control_biased(),
        server in control_biased(),
    ) {
        let core = core_at("https://api.example.com/v1/");
        let path = format!("/users/{segment}/keys");
        let refusals = [
            build_url(&core, &path, &[]).err(),
            build_url(&core, &segment, &[]).err(),
            build_url_on(&core, Some(&server), &path, &[]).err(),
            build_url_with_query_string(&core, &path, &[], Some(&segment)).err(),
        ];
        for error in refusals.iter().flatten() {
            proptest::prop_assert_eq!(single_line(error), Ok(()));
        }
    }

    /// `Error::Decode` carries serde's message, which quotes the server-supplied value it
    /// failed on; through the per-status dispatch, the JSON and text success decoders and
    /// both error classifiers, every message in its chain stays on one line (#457).
    #[test]
    fn a_decode_failure_message_is_a_single_line(text in control_biased()) {
        let json = serde_json::Value::String(text.clone()).to_string();
        let core = core();
        let mut errors: Vec<Error<Listed>> = Vec::new();
        for body in [format!(r#"{{"id":{json}}}"#), json.clone(), text.clone()] {
            if let Err(error) = dispatch_success(json_response(200, &body)) {
                errors.push(error.widen());
            }
            if let Err(error) = dispatch_success(json_response(202, &format!(r#"{{"job":{body}}}"#))) {
                errors.push(error.widen());
            }
            if let Err(error) = poll_ready(super::decode_success::<Listed>(&core, json_response(200, &body))) {
                errors.push(error.widen());
            }
            if let Err(error) = poll_ready(decode_success_text::<Listed>(&core, json_response(200, &body))) {
                errors.push(error.widen());
            }
            errors.push(poll_ready(super::classify_error::<Listed>(
                &core,
                json_response(400, &body),
                &[StatusSpec::Any],
            )));
            errors.push(poll_ready(classify_error_text::<Listed>(
                &core,
                json_response(400, &body),
                &[StatusSpec::Any],
            )));
        }
        proptest::prop_assert!(
            errors.iter().any(|error| matches!(error, Error::Decode { .. })),
            "no decode failure was produced"
        );
        for error in &errors {
            proptest::prop_assert_eq!(single_line(error), Ok(()));
        }
    }
}
