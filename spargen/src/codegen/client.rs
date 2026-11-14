//! The async `Client` and the items emitted beside it: each operation's parameter struct, typed
//! error, success enum, and header structs, and the `servers` module.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Api, ApiKeyLoc, CredentialShape, SecurityScheme, SecuritySchemeDef};
use crate::name::Names;

use super::docs::{carried, normalize_rustdoc};
use super::operation::{emit_operation, OperationSig};
use super::params::emit_params_struct;
use super::responses::{emit_error_enum, emit_response_enum, emit_response_headers};
use super::CodegenOptions;

/// The rustdoc of `with_default_server`, which `Client` and `BlockingClient` both emit when the
/// specification declares a server.
pub(super) fn default_server_docs() -> TokenStream {
    quote! {
        /// Build a client on the first server the specification declares, with every server
        /// variable at its declared default.
    }
}

/// The typed credential setters of every declared security scheme that takes a credential, in
/// declaration order, under the names [`Names::credential_setters`] holds: `with_<scheme>` (a
/// static token, or a username and password for `http basic`), `with_<scheme>_provider` and
/// `with_<scheme>_refreshable` for a scheme taking a token, and `without_<scheme>`. `Client`'s
/// (`forward` is `None`) register on its core under the scheme's key; `BlockingClient`'s forward
/// each call to the async client it wraps, the field `forward` names, and carry the same rustdoc.
pub(super) fn credential_setter_tokens(
    api: &Api,
    names: &Names,
    options: &CodegenOptions,
    forward: Option<TokenStream>,
) -> TokenStream {
    let setters = api.security_schemes.iter().filter_map(|(id, def)| {
        let shape = def.kind.credential_shape()?;
        let setter = names
            .credential_setters
            .get(id)
            .expect("credential setters are allocated for every scheme taking a credential");
        let key = id.0.as_str();
        let scheme_docs = scheme_doc_lines(def, options);
        let register = |credential: TokenStream, method: &crate::name::Ident, args: TokenStream| {
            match &forward {
                None => quote! { self.core.set_credential(#key, #credential); },
                Some(inner) => quote! { self.#inner = self.#inner.#method(#args); },
            }
        };
        let with = &setter.with;
        let selection = format!(
            "An operation attaches the first alternative of its `security` whose schemes all have \
             a credential registered, and fails before anything is sent, as \
             `Error::RequestConstruction` with `RequestError::MissingCredential`, when none has. \
             It never falls through past the alternative it chose: a credential that fails to \
             attach fails the call, and `{}` is how a caller reaches a later alternative.",
            setter.without.as_str()
        );
        let with_setter = match shape {
            CredentialShape::Token => {
                let (credential_variant, carrier) = token_carrier(&def.kind);
                let summary = format!(
                    "Register a static `token` for the security scheme `{key}`, {carrier}, \
                     replacing any credential registered for it."
                );
                let body = register(
                    quote! { support::Credential::#credential_variant(token) },
                    with,
                    quote! { token },
                );
                quote! {
                    #[doc = #summary]
                    ///
                    #[doc = #selection]
                    #(#scheme_docs)*
                    #[must_use]
                    pub fn #with(mut self, token: support::SecretString) -> Self {
                        #body
                        self
                    }
                }
            }
            CredentialShape::Basic => {
                let summary = format!(
                    "Register `username` and `password` for the `http basic` security scheme \
                     `{key}`, sent as `Authorization: Basic <base64>`, replacing any credential \
                     registered for it."
                );
                let body = register(
                    quote! {
                        support::Credential::Basic { username: username.into(), password }
                    },
                    with,
                    quote! { username, password },
                );
                quote! {
                    #[doc = #summary]
                    ///
                    #[doc = #selection]
                    #(#scheme_docs)*
                    #[must_use]
                    pub fn #with(
                        mut self,
                        username: impl Into<String>,
                        password: support::SecretString,
                    ) -> Self {
                        #body
                        self
                    }
                }
            }
        };
        let provider_setter = setter.provider.as_ref().map(|provider| {
            let summary = format!(
                "Register `provider` for the security scheme `{key}`, replacing any credential \
                 registered for it: it is asked for a fresh token each time a call attaches the \
                 scheme, and a call whose provider fails fails before anything is sent, as \
                 `RequestError::CredentialProvider`. `token_provider` builds one from an async \
                 closure."
            );
            let body = register(
                quote! { support::Credential::Provider(provider) },
                provider,
                quote! { provider },
            );
            quote! {
                #[doc = #summary]
                #[must_use]
                pub fn #provider(mut self, provider: support::TokenProvider) -> Self {
                    #body
                    self
                }
            }
        });
        let refreshable_setter = setter.refreshable.as_ref().map(|refreshable| {
            let summary = format!(
                "Register a refreshable token for the security scheme `{key}`, replacing any \
                 credential registered for it: `token` yields the token each call attaches, and \
                 when a call's failed response is one `is_rejection` reads as a rejection of that \
                 token, `on_rejected` is given it and yields a fresh one, and the call is sent \
                 once more with it. `Credential::Refreshable`'s documentation states when that \
                 replay happens and what the call returns."
            );
            let body = register(
                quote! {
                    support::Credential::refreshable(token, is_rejection, on_rejected)
                },
                refreshable,
                quote! { token, is_rejection, on_rejected },
            );
            quote! {
                #[doc = #summary]
                #[must_use]
                pub fn #refreshable<P, R, Fut>(
                    mut self,
                    token: support::TokenProvider,
                    is_rejection: P,
                    on_rejected: R,
                ) -> Self
                where
                    P: Fn(&http::Response<bytes::Bytes>) -> bool
                        + support::MaybeSend
                        + support::MaybeSync
                        + 'static,
                    R: Fn(support::SecretString) -> Fut
                        + support::MaybeSend
                        + support::MaybeSync
                        + 'static,
                    Fut: std::future::Future<
                            Output = Result<support::SecretString, support::AuthError>,
                        > + support::MaybeSend
                        + 'static,
                {
                    #body
                    self
                }
            }
        });
        let without = &setter.without;
        let without_summary = format!(
            "Unregister the credential for the security scheme `{key}`; one never registered is \
             left as it is. Operations pick the first `security` alternative whose schemes are \
             all registered and never fall through past it when attaching fails, so this is how \
             a caller reaches a later alternative: the client this returns no longer selects \
             one listing `{key}`."
        );
        let without_body = match &forward {
            None => quote! { self.core.remove_credential(#key); },
            Some(inner) => quote! { self.#inner = self.#inner.#without(); },
        };
        Some(quote! {
            #with_setter
            #provider_setter
            #refreshable_setter
            #[doc = #without_summary]
            #[must_use]
            pub fn #without(mut self) -> Self {
                #without_body
                self
            }
        })
    });
    quote! { #(#setters)* }
}

/// The `Credential` variant a static token registers as under a scheme taking a token, and how
/// the scheme carries it, as the setter's rustdoc states it.
fn token_carrier(scheme: &SecurityScheme) -> (TokenStream, String) {
    match scheme {
        SecurityScheme::ApiKey { location, name } => {
            let place = match location {
                ApiKeyLoc::Header => "header",
                ApiKeyLoc::Query => "query parameter",
                ApiKeyLoc::Cookie => "cookie",
            };
            (
                quote! { ApiKey },
                format!("an `apiKey` sent as the {place} `{name}`"),
            )
        }
        SecurityScheme::OAuth2 => (
            quote! { Bearer },
            "an `oauth2` scheme whose token is sent as `Authorization: Bearer <token>`".to_owned(),
        ),
        SecurityScheme::OpenIdConnect => (
            quote! { Bearer },
            "an `openIdConnect` scheme whose token is sent as `Authorization: Bearer <token>`"
                .to_owned(),
        ),
        SecurityScheme::Http(_) | SecurityScheme::MutualTls => (
            quote! { Bearer },
            "sent as `Authorization: Bearer <token>`".to_owned(),
        ),
    }
}

/// `with_bound_header`, which `Client` emits, and `BlockingClient` too with `body` forwarding to
/// it, when the build bound a header ([`Api::binds_headers`]); `None` otherwise. Its rustdoc names
/// each bound header in the spelling the first operation declaring it uses.
pub(super) fn with_bound_header_tokens(api: &Api, body: TokenStream) -> Option<TokenStream> {
    if !api.binds_headers() {
        return None;
    }
    let mut bound: Vec<&str> = Vec::new();
    for header in api
        .operations
        .iter()
        .flat_map(|operation| &operation.bound_headers)
    {
        if !bound
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&header.name))
        {
            bound.push(&header.name);
        }
    }
    let list = bound
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let headers_doc = normalize_rustdoc(&format!("The bound headers: {list}."));
    Some(quote! {
        /// Bind `value` to the header `name`, matched in any letter case, on this client value,
        /// replacing any value bound to it. The build bound these headers once on the client, so
        /// no operation takes an argument for one: each operation declaring it sends this value
        /// as written. An operation that requires a bound header fails before anything is sent,
        /// with `RequestError::MissingBoundParameter`, while no value is bound to it; one that
        /// declares it optional leaves it off. A name no operation sends is held and never read.
        /// Fails with `Error::RequestConstruction`, which does not quote `value`, if `value` is
        /// not a valid header value: one holding a control character other than a tab, such as
        /// CR, LF, NUL or DEL.
        ///
        #[doc = #headers_doc]
        pub fn with_bound_header(
            mut self,
            name: &str,
            value: impl Into<String>,
        ) -> Result<Self, support::Error<std::convert::Infallible>> {
            #body
            Ok(self)
        }
    })
}

/// One declared security scheme's documentation, rendered as rustdoc for its `with_<scheme>`
/// setter, for the reason [`crate::ir::SecuritySchemeDef`] gives. The lines carry the spec's
/// scheme and scope descriptions, so they go through [`carried`] and then [`normalize_rustdoc`]
/// together, as the one Markdown text rustdoc joins them into.
fn scheme_doc_lines(scheme: &SecuritySchemeDef, options: &CodegenOptions) -> Vec<TokenStream> {
    if scheme.docs.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![String::new(), "# Scheme".to_owned(), String::new()];
    lines.extend(
        normalize_rustdoc(&carried(&scheme.docs.join("\n"), options))
            .split('\n')
            .map(str::to_owned),
    );
    lines
        .into_iter()
        .map(|line| quote! { #[doc = #line] })
        .collect()
}

/// Emit everything the generated root holds beside `types`: each operation's `…Params`
/// struct, typed error, multi-status success enum, and response-header structs, the `servers`
/// module, and the `Client` itself — its constructors (`new`, `with_default_server` when a server
/// is declared, `with_client`, `with_backend`), `core`, the typed credential setters of each
/// security scheme ([`credential_setter_tokens`]), `with_timeout`, `with_options` and `with_base_url`, and
/// one async method per operation, declared from its entry in `signatures`, which runs parallel to
/// `api.operations`.
pub(super) fn emit_client(
    api: &Api,
    signatures: &[OperationSig],
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
    let credential_setters = credential_setter_tokens(api, names, options, None);
    let params = api
        .operations
        .iter()
        .filter(|operation| operation.takes_params())
        .map(|operation| emit_params_struct(operation, api, names));
    let errors = api
        .operations
        .iter()
        .map(|operation| emit_error_enum(operation, api, names));
    let response_enums = api
        .operations
        .iter()
        .map(|operation| emit_response_enum(operation, api, names));
    let methods = api
        .operations
        .iter()
        .zip(signatures)
        .map(|(operation, sig)| emit_operation(operation, sig, api, names));
    let response_headers = api
        .operations
        .iter()
        .map(|operation| emit_response_headers(operation, names, options));
    let client_docs = client_doc_tokens(api, options);
    let with_bound_header =
        with_bound_header_tokens(api, quote! { self.core.bind_header(name, value)?; });
    let error_body_cap = options.error_body_cap;
    let servers = emit_servers(api, names, options);
    let default_server = (!api.servers.is_empty()).then(|| {
        let docs = default_server_docs();
        quote! {
            #docs
            pub fn with_default_server() -> Result<Self, support::Error<std::convert::Infallible>> {
                Self::new(&servers::default_url())
            }
        }
    });
    quote! {
        #(#params)*
        #(#errors)*
        #(#response_enums)*
        #(#response_headers)*
        #servers

        #client_docs
        // `ClientCore` is `Debug + Clone` (its backend is a handle type, and
        // `Credential`'s `Debug` redacts every secret), so both derives are free here and both are
        // routinely wanted: `Clone` to hand the client to several tasks without an `Arc`, `Debug`
        // to put it in a struct that derives `Debug`.
        #[derive(Debug, Clone)]
        #[allow(dead_code)]
        pub struct Client {
            core: support::ClientCore,
        }

        // `deprecated`: the module names its own deprecated components in signatures and decode
        // paths, as `types`, the parameter structs, the response and error types, and the
        // blocking twin do. A lint level is read at the use site, so a consumer naming a
        // deprecated item is still warned.
        #[forbid(unsafe_code)]
        #[allow(dead_code, deprecated, unused_mut, unused_variables, clippy::result_large_err)]
        impl Client {
            /// Build a client on the default `ClientConfig` and `TransportConfig`: a 30 s connect
            /// timeout, a 5 min read (stall) timeout, and no total timeout.
            /// [`Self::with_config`] changes any of them. Fails with `Error::RequestConstruction`
            /// if `base_url` is not an `http` or `https` URL.
            pub fn new(base_url: &str) -> Result<Self, support::Error<std::convert::Infallible>> {
                Self::with_config(base_url, |_, _| {})
            }

            /// Build a client whose `ClientConfig` and `TransportConfig` are the default ones as
            /// `configure` leaves them: the total timeout and body limits on the first, the
            /// connect and read timeouts on the second. The `reqwest::Client` its default backend
            /// runs on is built from the `TransportConfig` as it is when `configure` returns,
            /// which is the one place it is read. Fails with `Error::RequestConstruction` if
            /// `base_url` is not an `http` or `https` URL.
            pub fn with_config(
                base_url: &str,
                configure: impl FnOnce(&mut support::ClientConfig, &mut support::TransportConfig),
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let mut config = support::ClientConfig {
                    max_error_body: #error_body_cap,
                    ..support::ClientConfig::default()
                };
                let mut transport = support::TransportConfig::default();
                configure(&mut config, &mut transport);
                let core = support::ClientCore::with_config(base_url, config, transport)?;
                Ok(Self { core })
            }

            #default_server

            /// Build a client that sends its requests on a caller-supplied `reqwest::Client`, as
            /// `with_backend` does with `ReqwestBackend::new(client)`: your client's timeouts
            /// apply instead of a `TransportConfig`'s connect and read timeouts.
            pub fn with_client(
                client: reqwest::Client,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let mut core = support::ClientCore::with_client(client, base_url)?;
                core.config_mut().max_error_body = #error_body_cap;
                Ok(Self { core })
            }

            /// Build a client over a caller-supplied transport backend — the injection point for
            /// retry, middleware, or a non-reqwest transport — which executes every request; the
            /// client builds no HTTP client of its own, so many clients can share one backend.
            /// Connecting and reading are bounded however the backend bounds them, and the total
            /// timeout travels on each request as a `TotalTimeout` extension for it to enforce.
            pub fn with_backend(
                backend: std::sync::Arc<dyn support::HttpBackend>,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let mut core = support::ClientCore::with_backend(backend, base_url)?;
                core.config_mut().max_error_body = #error_body_cap;
                Ok(Self { core })
            }

            pub fn core(&self) -> &support::ClientCore {
                &self.core
            }

            #credential_setters

            #with_bound_header

            /// Set the total timeout of every call made through this client value, replacing
            /// `ClientConfig::timeout` from `with_config` (or the default, none): a call that has
            /// not read its whole response within it fails with
            /// `Error::Timeout(TimeoutKind::Total)`. `None` sets no total bound of this client's
            /// own; the connect and read timeouts still apply, and so does a total timeout set on
            /// a `reqwest::Client` injected with `with_client`, which a request cannot lift. An
            /// operation returning an `EventStream` is exempt, as it is from
            /// `ClientConfig::timeout`, since a stream may rightly stay open indefinitely. Derive
            /// a client for a different deadline: `client.clone().with_timeout(Some(duration))`.
            #[must_use]
            pub fn with_timeout(mut self, timeout: Option<std::time::Duration>) -> Self {
                self.core.config_mut().timeout = timeout;
                self
            }

            /// Set the `RequestOptions` every call made through this client value carries,
            /// replacing the ones it had (none, until this is called): `max_body` bounds a
            /// non-streaming call's response body, failing it with `Error::BodyTooLarge` past
            /// the limit; `extensions` are copied onto each request, for a transport backend to
            /// read; `headers` are added to each request that does not already carry them.
            /// Derive a client for one call's options:
            /// `client.clone().with_options(options).operation(..)`.
            #[must_use]
            pub fn with_options(mut self, options: support::RequestOptions) -> Self {
                self.core.set_options(options);
                self
            }

            /// Repoint this client value at another base URL, keeping its transport backend,
            /// configuration and credentials, so nothing is rebuilt: derive one
            /// with `client.clone().with_base_url(url)?`. Fails with `Error::RequestConstruction`
            /// if `base_url` is not an `http` or `https` URL.
            pub fn with_base_url(
                mut self,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                self.core.set_base_url(base_url)?;
                Ok(self)
            }

            #(#methods)*
        }
    }
}

/// Emit the `servers` module: one builder per declared server, plus the default base URL.
///
/// A Server Variable `default` is sent when the caller supplies no alternative, so every server
/// resolves to a concrete URL with no arguments; a variable that declares an `enum` gets a typed
/// enum so an illegal value cannot be constructed at all.
fn emit_servers(api: &Api, names: &Names, options: &CodegenOptions) -> TokenStream {
    if api.servers.is_empty() {
        return quote! {};
    }
    let builders = api.servers.iter().enumerate().map(|(index, server)| {
        let ident = names.servers.get(index).expect("server name allocated");
        let mut docs = vec![format!("Server `{}`.", server.url)];
        if let Some(description) = &server.description {
            docs.push(normalize_rustdoc(&carried(description, options)));
        }
        let doc_attrs = docs.iter().map(|line| quote! { #[doc = #line] });

        // Every variable has a builder field; only one that declares an `enum` has a typed enum,
        // so that lookup is the variable's shape rather than an allocation to expect.
        let field_ident = |name: &String| {
            names
                .server_variable_fields
                .get(&(index, name.clone()))
                .expect("server variable field allocated")
        };
        let enum_ident = |name: &String| names.server_variable_enums.get(&(index, name.clone()));
        let variant_ident = |name: &String, value: &String| {
            names
                .server_variable_variants
                .get(&(index, name.clone(), value.clone()))
                .expect("server variable variant allocated")
        };

        let variable_enums = server.variables.iter().filter_map(|(name, variable)| {
            let enum_ident = enum_ident(name)?;
            let variants = variable.enum_values.iter().map(|value| {
                let variant = variant_ident(name, value);
                let default = (value == &variable.default).then(|| quote! { #[default] });
                quote! { #default #variant }
            });
            let arms = variable.enum_values.iter().map(|value| {
                let variant = variant_ident(name, value);
                quote! { Self::#variant => #value }
            });
            let doc = format!("Permitted values of the `{name}` server variable.");
            Some(quote! {
                #[doc = #doc]
                #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
                pub enum #enum_ident {
                    #(#variants),*
                }

                impl #enum_ident {
                    /// The value substituted into the server URL.
                    pub fn as_str(self) -> &'static str {
                        match self {
                            #(#arms),*
                        }
                    }
                }
            })
        });

        let fields = server.variables.keys().map(|name| {
            let field = field_ident(name);
            match enum_ident(name) {
                Some(enum_ident) => {
                    quote! { #field: #enum_ident }
                }
                None => quote! { #field: String },
            }
        });

        let defaults = server.variables.iter().map(|(name, variable)| {
            let field = field_ident(name);
            match enum_ident(name) {
                // The `#[default]` variant is the declared default, so `Default` is exact.
                Some(enum_ident) => {
                    quote! { #field: <#enum_ident as Default>::default() }
                }
                None => {
                    let default = variable.default.clone();
                    quote! { #field: #default.to_owned() }
                }
            }
        });

        let setters = server.variables.iter().map(|(name, variable)| {
            let field = field_ident(name);
            let mut doc = format!("Set the `{name}` server variable.");
            if let Some(description) = &variable.description {
                doc.push(' ');
                doc.push_str(&carried(description, options));
            }
            let doc = normalize_rustdoc(&doc);
            match enum_ident(name) {
                Some(enum_ident) => {
                    quote! {
                        #[doc = #doc]
                        #[must_use]
                        pub fn #field(mut self, value: #enum_ident) -> Self {
                            self.#field = value;
                            self
                        }
                    }
                }
                None => quote! {
                    #[doc = #doc]
                    #[must_use]
                    pub fn #field(mut self, value: impl Into<String>) -> Self {
                        self.#field = value.into();
                        self
                    }
                },
            }
        });

        let pieces = server.segments.iter().map(|segment| match segment {
            // A one-character literal goes through `push`: `push_str` with a single-char literal
            // trips `clippy::single_char_add_str`, and generated code must pass `-D warnings` in
            // the consuming crate.
            crate::ir::UrlSegment::Literal(text) => match text.chars().count() {
                1 => {
                    let character = text.chars().next().expect("one character");
                    quote! { url.push(#character); }
                }
                _ => quote! { url.push_str(#text); },
            },
            crate::ir::UrlSegment::Variable(name) => {
                let field = field_ident(name);
                match enum_ident(name) {
                    Some(_) => quote! { url.push_str(self.#field.as_str()); },
                    None => quote! { url.push_str(&self.#field); },
                }
            }
        });

        // `Default` is derivable exactly when every field's declared default is what the derive
        // would produce: an enum variable pins its default with `#[default]`, and a server with no
        // variables has nothing to default. A free-form variable carries a spec-declared string,
        // which the derive would replace with `""`.
        let derivable_default = server
            .variables
            .keys()
            .all(|name| enum_ident(name).is_some());
        let (derive_default, default_impl) = if derivable_default {
            (quote! { , Default }, quote! {})
        } else {
            (
                quote! {},
                quote! {
                    impl Default for #ident {
                        fn default() -> Self {
                            Self { #(#defaults),* }
                        }
                    }
                },
            )
        };
        quote! {
            #(#variable_enums)*

            #(#doc_attrs)*
            #[derive(Debug, Clone #derive_default)]
            pub struct #ident {
                #(#fields),*
            }

            #default_impl

            impl #ident {
                /// Every server variable at its declared default.
                pub fn new() -> Self {
                    Self::default()
                }

                #(#setters)*

                /// The server URL with every variable substituted.
                pub fn url(&self) -> String {
                    let mut url = String::new();
                    #(#pieces)*
                    url
                }
            }
        }
    });
    let first = names.servers.first().expect("at least one server");
    quote! {
        /// Base URLs declared by the API description.
        #[allow(dead_code)]
        pub mod servers {
            #(#builders)*

            /// The first declared server, with every server variable at its declared default.
            pub fn default_url() -> String {
                #first::new().url()
            }
        }
    }
}

/// Document the generated `Client` with the API identity and its declared servers. The title,
/// version and descriptions are the spec's, so each goes through [`carried`].
fn client_doc_tokens(api: &Api, options: &CodegenOptions) -> TokenStream {
    let mut text = format!(
        "Client for {} v{}.",
        carried(&api.info.title, options),
        carried(&api.info.version, options)
    );
    if let Some(description) = api
        .info
        .description
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        text.push_str("\n\n");
        text.push_str(&carried(description, options));
    }
    if !api.servers.is_empty() {
        text.push_str("\n\nServers declared by the spec:");
        for server in &api.servers {
            text.push_str("\n- `");
            text.push_str(&server.url);
            text.push('`');
            if let Some(description) = server
                .description
                .as_deref()
                .filter(|text| !text.trim().is_empty())
            {
                text.push_str(" — ");
                text.push_str(&carried(description, options));
            }
        }
    }
    let text = normalize_rustdoc(&text);
    quote! { #[doc = #text] }
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::CodegenOptions;
    use crate::diag::{JsonPointer, Provenance};
    use crate::ir::{
        Api, BoundHeader, Docs, HttpScheme, Info, Method, Operation, OperationId, PathSegment,
        PathTemplate, Response, Responses, SchemeId, SecurityScheme, SecuritySchemeDef, Server,
        StatusSpec, TypeGraph, UrlSegment,
    };

    /// The name of every `fn` in each inherent `impl <self_ty>` block among `items`, nested modules
    /// included, appended to `into`.
    fn inherent_methods(items: &[syn::Item], self_ty: &str, into: &mut Vec<String>) {
        for item in items {
            match item {
                syn::Item::Impl(block) if block.trait_.is_none() => {
                    let syn::Type::Path(path) = &*block.self_ty else {
                        continue;
                    };
                    if !path.path.is_ident(self_ty) {
                        continue;
                    }
                    into.extend(block.items.iter().filter_map(|item| match item {
                        syn::ImplItem::Fn(method) => Some(method.sig.ident.to_string()),
                        _ => None,
                    }));
                }
                syn::Item::Mod(module) => {
                    if let Some((_, items)) = &module.content {
                        inherent_methods(items, self_ty, into);
                    }
                }
                _ => {}
            }
        }
    }

    /// Issue #286: operation methods share `impl Client` and `impl BlockingClient` with the fixed
    /// constructors and accessors, so `name::CLIENT_METHODS` must be exactly the fixed methods
    /// emitted there, and an operation spelling each of them must yield rather than duplicate it.
    /// The API declares a server so `with_default_server` is emitted, and one operation sends a
    /// bound header so `with_bound_header` is.
    #[test]
    fn client_methods_are_exactly_the_fixed_methods_and_operations_yield_to_them() {
        let operation = |id: &str| Operation {
            id: OperationId(id.to_owned()),
            method: Method::Get,
            path: PathTemplate {
                raw: format!("/{id}"),
                segments: vec![PathSegment::Literal(format!("/{id}"))],
            },
            params: Vec::new(),
            request_body: None,
            responses: Responses {
                by_status: vec![(
                    StatusSpec::Exact(204),
                    Response {
                        body: None,
                        media: None,
                        stream: None,
                        accept: None,
                        headers: Vec::new(),
                    },
                )],
                default: None,
            },
            security: Vec::new(),
            deprecated: false,
            docs: Docs::default(),
            server: None,
            provenance: Provenance::new(JsonPointer::root().push(id), None),
            bound_headers: Vec::new(),
        };
        let mut api = Api {
            info: Info {
                title: "T".to_owned(),
                version: "1".to_owned(),
                description: None,
            },
            servers: vec![Server {
                name: None,
                url: "https://api.example.com".to_owned(),
                segments: vec![UrlSegment::Literal("https://api.example.com".to_owned())],
                variables: IndexMap::new(),
                description: None,
            }],
            operations: crate::name::CLIENT_METHODS
                .iter()
                .map(|method| operation(method))
                .collect(),
            types: TypeGraph::default(),
            // Issue #1337: schemes spelling fixed methods (`with_timeout`, `with_config`) and an
            // operation's (`with_new`), and a `mutualTLS` scheme, which takes no credential.
            security_schemes: [
                ("timeout", SecurityScheme::Http(HttpScheme::Bearer)),
                ("config", SecurityScheme::Http(HttpScheme::Basic)),
                ("new", SecurityScheme::OAuth2),
                ("mtls", SecurityScheme::MutualTls),
            ]
            .into_iter()
            .map(|(key, kind)| {
                (
                    SchemeId(key.to_owned()),
                    SecuritySchemeDef {
                        kind,
                        docs: Vec::new(),
                    },
                )
            })
            .collect(),
        };
        api.operations.push(operation("withNew"));
        api.operations[0].bound_headers.push(BoundHeader {
            name: "X-Api-Version".to_owned(),
            required: true,
        });
        // One operation reads a raw binary success, so it has a `_stream` twin, with its request
        // builder and decoder, on `Client` alone.
        let bytes = api.types.insert(crate::ir::TypeDef {
            name_hint: "Blob".to_owned(),
            kind: crate::ir::TypeKind::Bytes(crate::ir::BytesEncoding::Raw),
            docs: Docs::default(),
            provenance: Provenance::new(JsonPointer::root(), None),
            document: String::new(),
        });
        api.operations[1].responses.by_status = vec![(
            StatusSpec::Exact(200),
            Response {
                body: Some(crate::ir::Ty {
                    id: bytes,
                    nullable: false,
                    boxed: false,
                }),
                media: Some(crate::ir::MediaType::OctetStream),
                stream: None,
                accept: None,
                headers: Vec::new(),
            },
        )];
        let names = crate::name::allocate(&api);
        let options = CodegenOptions::default();
        let signatures = super::super::operation::signatures(&api, &names, &options);
        let client: syn::File =
            syn::parse2(super::emit_client(&api, &signatures, &names, &options))
                .expect("the emitted client parses");
        let blocking: syn::File = syn::parse2(super::super::blocking::emit_blocking_client(
            &api,
            &signatures,
            &names,
            &options,
        ))
        .expect("the emitted blocking client parses");

        // Each operation's method, its URL accessor, request builder and decoder, which both
        // clients emit, and the preparer, which only `Client` does.
        let operation_methods: std::collections::BTreeSet<String> = api
            .operations
            .iter()
            .map(|operation| names.operation(&operation.id))
            .flat_map(|names| {
                [
                    &names.method,
                    &names.url_accessor,
                    &names.request_builder,
                    &names.decoder,
                ]
            })
            .map(|ident| ident.as_str().to_owned())
            .collect();
        // The preparers, and each `_stream` twin with its request builder and decoder, which only
        // `Client` defines.
        let preparers: std::collections::BTreeSet<String> =
            api.operations
                .iter()
                .map(|operation| names.operation(&operation.id))
                .flat_map(|names| {
                    std::iter::once(&names.preparer).chain(names.byte_stream.iter().flat_map(
                        |stream| [&stream.twin, &stream.request_builder, &stream.decoder],
                    ))
                })
                .map(|ident| ident.as_str().to_owned())
                .collect();
        assert_eq!(
            preparers.len(),
            api.operations.len() + 3,
            "one operation has a twin family: {preparers:?}"
        );
        // Each scheme taking a credential's typed setters, which both clients emit: four for a
        // token scheme, two for `http basic`, none for `mutualTLS`.
        let setters: std::collections::BTreeSet<String> = names
            .credential_setters
            .values()
            .flat_map(|setter| {
                [
                    Some(&setter.with),
                    setter.provider.as_ref(),
                    setter.refreshable.as_ref(),
                    Some(&setter.without),
                ]
            })
            .flatten()
            .map(|ident| ident.as_str().to_owned())
            .collect();
        assert_eq!(setters.len(), 4 + 2 + 4, "{setters:?}");
        assert!(
            !names
                .credential_setters
                .contains_key(&SchemeId("mtls".to_owned())),
            "a `mutualTLS` scheme has no setter"
        );
        for setter in &setters {
            assert!(
                !operation_methods.contains(setter) && !preparers.contains(setter),
                "credential setter `{setter}` took an operation's spelling"
            );
        }
        let reserved: std::collections::BTreeSet<String> = crate::name::CLIENT_METHODS
            .iter()
            .map(|method| (*method).to_owned())
            .collect();
        // Issue #1119: each client emits every reserved method but the other client's own, so
        // `BlockingClient` carries `with_default_server` too; only `inner` is `BlockingClient`'s.
        let mut client_only = reserved.clone();
        client_only.remove("inner");
        for (file, self_ty, expected) in [
            (&client, "Client", &client_only),
            (&blocking, "BlockingClient", &reserved),
        ] {
            let mut methods = Vec::new();
            inherent_methods(&file.items, self_ty, &mut methods);
            let unique: std::collections::BTreeSet<&String> = methods.iter().collect();
            assert_eq!(
                unique.len(),
                methods.len(),
                "`impl {self_ty}` defines a method twice: {methods:?}"
            );
            for method in &operation_methods {
                assert!(
                    methods.contains(method),
                    "`impl {self_ty}` lacks operation method `{method}`: {methods:?}"
                );
            }
            for setter in &setters {
                assert!(
                    methods.contains(setter),
                    "`impl {self_ty}` lacks credential setter `{setter}`: {methods:?}"
                );
            }
            for preparer in &preparers {
                assert_eq!(
                    methods.contains(preparer),
                    self_ty == "Client",
                    "only `impl Client` defines `{preparer}`: {methods:?}"
                );
            }
            let fixed: std::collections::BTreeSet<String> = methods
                .into_iter()
                .filter(|method| {
                    !operation_methods.contains(method)
                        && !preparers.contains(method)
                        && !setters.contains(method)
                })
                .collect();
            assert_eq!(
                &fixed, expected,
                "`impl {self_ty}`'s fixed methods must be `name::CLIENT_METHODS`, less `inner` on \
                 `Client`"
            );
        }
        for method in crate::name::CLIENT_METHODS {
            assert!(
                !operation_methods.contains(*method)
                    && !preparers.contains(*method)
                    && !setters.contains(*method),
                "an operation or a credential setter kept the bare spelling of the fixed client \
                 method `{method}`"
            );
        }
        assert!(
            !setters.contains("with_new") && setters.contains("without_new"),
            "the scheme `new` yields `with_new` to the operation `withNew`: {setters:?}"
        );
    }

    /// Issue #1324: a server builder's inherent methods are `new` and `url` beside one setter per
    /// variable, both in `name::SERVER_METHODS`, and a variable spelling any method of that list
    /// takes a setter of another spelling instead of duplicating or shadowing it.
    #[test]
    fn server_builder_methods_are_reserved_and_variables_yield_to_them() {
        let variables: IndexMap<String, crate::ir::ServerVariable> = crate::name::SERVER_METHODS
            .iter()
            .map(|name| {
                (
                    (*name).to_owned(),
                    crate::ir::ServerVariable {
                        default: "v".to_owned(),
                        enum_values: Vec::new(),
                        description: None,
                    },
                )
            })
            .collect();
        let api = Api {
            info: Info {
                title: "T".to_owned(),
                version: "1".to_owned(),
                description: None,
            },
            servers: vec![Server {
                name: None,
                url: "https://{clone}.{default}.{new}.{url}".to_owned(),
                segments: variables
                    .keys()
                    .map(|name| UrlSegment::Variable(name.clone()))
                    .collect(),
                variables,
                description: None,
            }],
            operations: Vec::new(),
            types: TypeGraph::default(),
            security_schemes: IndexMap::new(),
        };
        let names = crate::name::allocate(&api);
        let file: syn::File = syn::parse2(super::emit_servers(
            &api,
            &names,
            &CodegenOptions::default(),
        ))
        .expect("the emitted servers module parses");
        let mut methods = Vec::new();
        inherent_methods(&file.items, names.servers[0].as_str(), &mut methods);
        let unique: std::collections::BTreeSet<&String> = methods.iter().collect();
        assert_eq!(unique.len(), methods.len(), "a method twice: {methods:?}");
        let setters: std::collections::BTreeSet<String> = api.servers[0]
            .variables
            .keys()
            .map(|name| {
                names.server_variable_fields[&(0, name.clone())]
                    .as_str()
                    .to_owned()
            })
            .collect();
        let fixed: std::collections::BTreeSet<&str> = methods
            .iter()
            .map(String::as_str)
            .filter(|method| !setters.contains(*method))
            .collect();
        assert_eq!(
            fixed,
            ["new", "url"].into_iter().collect(),
            "the builder's fixed methods: {methods:?}"
        );
        for method in fixed {
            assert!(crate::name::SERVER_METHODS.contains(&method), "{method}");
        }
        for method in crate::name::SERVER_METHODS {
            assert!(
                !setters.contains(*method),
                "variable `{method}` kept the bare spelling of a builder method: {setters:?}"
            );
        }
    }

    /// The `#[doc]` text of `method` in the inherent `impl <self_ty>` among `items`, nested modules
    /// included, or `None` where that impl does not define it.
    fn method_docs(items: &[syn::Item], self_ty: &str, method: &str) -> Option<Vec<String>> {
        items.iter().find_map(|item| match item {
            syn::Item::Impl(block) if block.trait_.is_none() => {
                let syn::Type::Path(path) = &*block.self_ty else {
                    return None;
                };
                if !path.path.is_ident(self_ty) {
                    return None;
                }
                block.items.iter().find_map(|item| match item {
                    syn::ImplItem::Fn(function) if function.sig.ident == method => Some(
                        function
                            .attrs
                            .iter()
                            .filter(|attr| attr.path().is_ident("doc"))
                            .map(|attr| quote::ToTokens::to_token_stream(&attr.meta).to_string())
                            .collect(),
                    ),
                    _ => None,
                })
            }
            syn::Item::Mod(module) => module
                .content
                .as_ref()
                .and_then(|(_, items)| method_docs(items, self_ty, method)),
            _ => None,
        })
    }

    /// Issue #1119: `BlockingClient` emits `with_default_server` exactly when `Client` does (a
    /// declared server), and its credential setters carry `Client`'s documentation, the scheme's
    /// included on `with_<scheme>` (issue #1337), as does its `with_default_server`.
    #[test]
    fn blocking_client_mirrors_default_server_and_credential_docs() {
        for declares_server in [false, true] {
            let servers = if declares_server {
                vec![Server {
                    name: None,
                    url: "https://api.example.com".to_owned(),
                    segments: vec![UrlSegment::Literal("https://api.example.com".to_owned())],
                    variables: IndexMap::new(),
                    description: None,
                }]
            } else {
                Vec::new()
            };
            let api = Api {
                info: Info {
                    title: "T".to_owned(),
                    version: "1".to_owned(),
                    description: None,
                },
                servers,
                operations: Vec::new(),
                types: TypeGraph::default(),
                security_schemes: IndexMap::from([(
                    SchemeId("bearerAuth".to_owned()),
                    SecuritySchemeDef {
                        kind: SecurityScheme::Http(HttpScheme::Bearer),
                        docs: vec!["`bearerAuth`: a short-lived service token.".to_owned()],
                    },
                )]),
            };
            let names = crate::name::allocate(&api);
            let options = CodegenOptions::default();
            let signatures = super::super::operation::signatures(&api, &names, &options);
            let client: syn::File =
                syn::parse2(super::emit_client(&api, &signatures, &names, &options))
                    .expect("the emitted client parses");
            let blocking: syn::File = syn::parse2(super::super::blocking::emit_blocking_client(
                &api,
                &signatures,
                &names,
                &options,
            ))
            .expect("the emitted blocking client parses");

            let credential = method_docs(&client.items, "Client", "with_bearer_auth")
                .expect("`Client::with_bearer_auth` is emitted");
            assert!(
                credential
                    .iter()
                    .any(|doc| doc.contains("a short-lived service token")),
                "`Client::with_bearer_auth` carries its scheme's documentation: {credential:?}"
            );
            for setter in [
                "with_bearer_auth",
                "with_bearer_auth_provider",
                "with_bearer_auth_refreshable",
                "without_bearer_auth",
            ] {
                let docs = method_docs(&client.items, "Client", setter);
                assert!(docs.is_some(), "`Client::{setter}` is emitted");
                assert_eq!(
                    method_docs(&blocking.items, "BlockingClient", setter),
                    docs,
                    "`BlockingClient::{setter}` carries `Client`'s documentation"
                );
            }

            let default_server = method_docs(&client.items, "Client", "with_default_server");
            assert_eq!(
                default_server.is_some(),
                declares_server,
                "`Client::with_default_server` is emitted exactly when a server is declared"
            );
            assert_eq!(
                method_docs(&blocking.items, "BlockingClient", "with_default_server"),
                default_server,
                "`BlockingClient::with_default_server` is emitted exactly when `Client`'s is, \
                 with its documentation"
            );
        }
    }
}
