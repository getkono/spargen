use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use secrecy::{ExposeSecret, SecretString};

use crate::{ClientCore, Error, RequestError};

use super::{
    attach_auth, attach_auth_replayable, replay_rejected, token_provider, AuthError, AuthKind,
    AuthScheme, Credential, TokenFuture,
};
use crate::tests::{core, poll_ready, Reply, SequenceBackend};

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

/// RFC 6265 §5.4: a user agent sends at most one `Cookie` header. Two cookie apiKeys in one
/// alternative therefore share it, joined by `; ` in the alternative's order, rather than each
/// appending a header of its own.
#[test]
fn two_cookie_api_keys_in_one_alternative_share_one_cookie_header() {
    let mut core = core();
    core.set_credential("session", Credential::ApiKey(SecretString::from("s3ss")));
    core.set_credential("csrf", Credential::ApiKey(SecretString::from("t0k")));
    let request = poll_ready(attach_auth(
        &core,
        get(&core),
        &[&[
            AuthScheme {
                name: "session",
                kind: AuthKind::ApiKeyCookie("SESSION"),
            },
            AuthScheme {
                name: "csrf",
                kind: AuthKind::ApiKeyCookie("CSRF"),
            },
        ]],
    ))
    .unwrap()
    .build()
    .unwrap();
    let cookies: Vec<_> = request
        .headers()
        .get_all(reqwest::header::COOKIE)
        .iter()
        .collect();
    assert_eq!(cookies, ["SESSION=s3ss; CSRF=t0k"]);
    assert!(cookies[0].is_sensitive());
}

/// A generated operation sets its cookie parameters' `Cookie` header before it attaches
/// credentials, so a cookie apiKey joins that header after the parameters instead of sending a
/// second one, and the merged header, which now carries the secret, is marked sensitive.
#[test]
fn a_cookie_api_key_joins_the_cookie_header_the_request_already_carries() {
    let mut core = core();
    core.set_credential("key", Credential::ApiKey(SecretString::from("k3y")));
    let request = get(&core).header(reqwest::header::COOKIE, "theme=dark; lang=en");
    let request = poll_ready(attach_auth(
        &core,
        request,
        &[&[AuthScheme {
            name: "key",
            kind: AuthKind::ApiKeyCookie("SESSION"),
        }]],
    ))
    .unwrap()
    .build()
    .unwrap();
    let cookies: Vec<_> = request
        .headers()
        .get_all(reqwest::header::COOKIE)
        .iter()
        .collect();
    assert_eq!(cookies, ["theme=dark; lang=en; SESSION=k3y"]);
    assert!(cookies[0].is_sensitive());
}

/// A request builder that already failed carries its error to the caller as the
/// request-construction error `build` would have raised, rather than losing it while the cookie
/// is merged.
#[test]
fn a_cookie_api_key_on_a_failed_builder_reports_request_construction() {
    let mut core = core();
    core.set_credential("key", Credential::ApiKey(SecretString::from("k3y")));
    let request = core.http().request(reqwest::Method::GET, "not a url");
    let error = poll_ready(attach_auth(
        &core,
        request,
        &[&[AuthScheme {
            name: "key",
            kind: AuthKind::ApiKeyCookie("SESSION"),
        }]],
    ))
    .unwrap_err();
    assert!(
        matches!(error, Error::RequestConstruction(RequestError::Other(_))),
        "{error:?}"
    );
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

/// The key joins the query the operation already built, form-encoded as reqwest's
/// `RequestBuilder::query` encoded it.
#[test]
fn a_query_api_key_joins_the_existing_query_form_encoded() {
    let mut core = core();
    core.set_credential("key", Credential::ApiKey(SecretString::from("a b&c=d+é")));
    let request = core
        .http()
        .request(reqwest::Method::GET, "https://example.com/op?page=2");
    let request = poll_ready(attach_auth(
        &core,
        request,
        &[&[AuthScheme {
            name: "key",
            kind: AuthKind::ApiKeyQuery("api key"),
        }]],
    ))
    .unwrap()
    .build()
    .unwrap();
    assert_eq!(
        request.url().query(),
        Some("page=2&api+key=a+b%26c%3Dd%2B%C3%A9")
    );
}

/// As for a cookie apiKey: a failed builder's error reaches the caller as a request-construction
/// error rather than being lost while the query is appended.
#[test]
fn a_query_api_key_on_a_failed_builder_reports_request_construction() {
    let mut core = core();
    core.set_credential("key", Credential::ApiKey(SecretString::from("k3y")));
    let request = core.http().request(reqwest::Method::GET, "not a url");
    let error = poll_ready(attach_auth(
        &core,
        request,
        &[&[AuthScheme {
            name: "key",
            kind: AuthKind::ApiKeyQuery("api_key"),
        }]],
    ))
    .unwrap_err();
    assert!(
        matches!(error, Error::RequestConstruction(RequestError::Other(_))),
        "{error:?}"
    );
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
/// schemes — the degenerate rendering `Display` was made total for. Generated output cannot reach
/// this (`codegen/dispatch.rs` omits the call when an operation declares no `security`), but the
/// function is public.
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

/// `Credential`'s shipped documentation states which variant each kind of scheme accepts: both
/// static token variants under every token-carrying kind, and neither under `http basic`. The
/// provider's half of that table is pinned by the provider tests; this pins the static half,
/// so the documented rule and the attach code cannot drift apart unseen. An apiKey header or
/// cookie the secret travels in is marked sensitive, so it is redacted wherever it is printed.
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
        assert!(header.headers()["X-Api-Key"].is_sensitive(), "{registered}");
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
        assert!(
            cookie.headers()[reqwest::header::COOKIE].is_sensitive(),
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
    // The header a `Basic` credential produces under an `http basic` scheme. RFC 7617:
    // base64("u:p").
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

/// An apiKey whose text a header value refuses (a line break here) fails the call as
/// `RequestConstruction` before anything is sent, under the two kinds that put it in a header.
/// It is not attached in some other form, and not dropped.
#[test]
fn an_api_key_that_is_not_a_header_value_fails_as_request_construction() {
    let mut core = core();
    core.set_credential(
        "key",
        Credential::ApiKey(SecretString::from("k3y\nX-Evil: 1")),
    );
    for kind in [
        AuthKind::ApiKeyHeader("X-Api-Key"),
        AuthKind::ApiKeyCookie("SESSION"),
    ] {
        let error = poll_ready(attach_auth(
            &core,
            get(&core),
            &[&[AuthScheme { name: "key", kind }]],
        ))
        .unwrap_err();
        match error {
            Error::RequestConstruction(RequestError::Other(_)) => {}
            other => panic!("{kind:?}: expected RequestConstruction, got {other:?}"),
        }
    }
}

/// A bearer token a header value refuses is handed to reqwest's `bearer_auth`, which records the
/// failure in the builder: `attach_auth` succeeds, and building the request fails, so it is
/// still never sent.
#[test]
fn a_bearer_token_that_is_not_a_header_value_fails_at_build() {
    let mut core = core();
    core.set_credential(
        "token",
        Credential::Bearer(SecretString::from("t0k\nX-Evil: 1")),
    );
    let builder = poll_ready(attach_auth(&core, get(&core), &[BEARER])).unwrap();
    let error = builder.build().unwrap_err();
    assert!(error.is_builder(), "{error:?}");
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

/// `Debug` is hand-written so a credential cannot reach a log. Every arm is pinned, `Basic`
/// included: it holds a username in the clear alongside the secret, so it is the arm where a
/// naive derive would leak most.
#[test]
fn every_credential_kind_redacts_its_secret_and_names_its_kind() {
    let cases: Vec<(Credential, &str)> = vec![
        (Credential::Bearer(SecretString::from("s3cr3t")), "Bearer"),
        (
            Credential::Basic {
                username: "aladdin".to_owned(),
                password: SecretString::from("s3cr3t"),
            },
            "Basic",
        ),
        (Credential::ApiKey(SecretString::from("s3cr3t")), "ApiKey"),
        (
            Credential::Provider(Arc::new(|| {
                Box::pin(async { Ok(SecretString::from("s3cr3t")) }) as TokenFuture
            })),
            "Provider",
        ),
        (
            Credential::refreshable(
                token_provider(|| async { Ok(SecretString::from("s3cr3t")) }),
                |_| true,
                |_| async { Ok(SecretString::from("s3cr3t")) },
            ),
            "Refreshable",
        ),
    ];

    for (credential, kind) in cases {
        let rendered = format!("{credential:?}");
        assert_eq!(rendered, format!("Credential::{kind}(***)"));
        assert!(!rendered.contains("s3cr3t"), "{rendered} leaks the secret");
        // The username is not a secret, but it is still an identity: `Debug` prints neither.
        assert!(
            !rendered.contains("aladdin"),
            "{rendered} leaks the username"
        );
    }
}

#[test]
fn an_auth_error_displays_and_chains_as_an_error() {
    let error = AuthError::new("token endpoint returned 503");
    assert_eq!(error.to_string(), "token endpoint returned 503");
    assert!(std::error::Error::source(&error).is_none());
}

#[test]
fn token_provider_wraps_an_async_closure() {
    let provider = token_provider(|| async { Ok(SecretString::from("t0k")) });
    assert_eq!(poll_ready(provider()).unwrap().expose_secret(), "t0k");
    let failing = token_provider(|| async { Err(AuthError::new("no session")) });
    assert_eq!(poll_ready(failing()).unwrap_err().to_string(), "no session");
}

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
    request: reqwest::RequestBuilder,
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
#[cfg(feature = "stream")]
#[test]
fn a_request_whose_body_cannot_be_cloned_is_not_replayed() {
    use crate::tests::{Chunk, Chunks};

    let backend = SequenceBackend::new([Reply::status(401, "expired"), Reply::status(200, "ok")]);
    let mut core = core_over(&backend);
    let calls = Arc::new(Calls::default());
    core.set_credential("token", refreshable(&calls, Some("fresh")));
    let upload = core
        .http()
        .request(reqwest::Method::POST, "https://example.com/op")
        .body(reqwest::Body::wrap_stream(Chunks::new([Chunk::data("ab")])));

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
                registered: "Refreshable",
                required: "http basic",
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
