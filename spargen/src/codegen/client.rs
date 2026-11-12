//! The async `Client` and the items emitted beside it: each operation's parameter struct, typed
//! error, success enum, and header structs, and the `servers` module.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::Api;
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

/// The rustdoc of `with_credential`, which `Client` and `BlockingClient` both carry: what
/// registration checks and how an operation fails on it, followed by the declared schemes.
pub(super) fn with_credential_docs(api: &Api, options: &CodegenOptions) -> TokenStream {
    let scheme_docs = scheme_doc_lines(api, options);
    quote! {
        /// Register a credential for a named security scheme (a `securitySchemes` key).
        ///
        /// Registration checks nothing; each operation matches its `security` requirement
        /// against the registered credentials when it is called, and fails before anything is
        /// sent, as `Error::RequestConstruction`, when no alternative has every scheme
        /// registered (`RequestError::MissingCredential`), when the selected alternative has a
        /// credential its scheme cannot carry (`RequestError::CredentialMismatch` — for
        /// instance a `Credential::Provider` under an `http basic` scheme, which accepts only
        /// `Credential::Basic`), or when its token provider fails
        /// (`RequestError::CredentialProvider`). `Credential`'s documentation lists which
        /// variant each kind of scheme accepts.
        #(#scheme_docs)*
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
        /// declares it optional leaves it off. Binding checks nothing; a name no operation sends
        /// is held and never read.
        ///
        #[doc = #headers_doc]
        #[must_use]
        pub fn with_bound_header(mut self, name: &str, value: impl Into<String>) -> Self {
            #body
            self
        }
    })
}

/// The declared security schemes, rendered as rustdoc for `with_credential`, for the reason
/// [`crate::ir::SecuritySchemeDef`] gives. The lines carry the spec's scheme and scope
/// descriptions, so they go through [`carried`] together, as the one Markdown text rustdoc joins
/// them into.
fn scheme_doc_lines(api: &Api, options: &CodegenOptions) -> Vec<TokenStream> {
    let documented: Vec<&str> = api
        .security_schemes
        .values()
        .flat_map(|scheme| scheme.docs.iter().map(String::as_str))
        .collect();
    if documented.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![
        String::new(),
        "# Declared schemes".to_owned(),
        String::new(),
    ];
    lines.extend(
        carried(&documented.join("\n"), options)
            .split('\n')
            .map(str::to_owned),
    );
    lines
        .into_iter()
        .map(|line| quote! { #[doc = #line] })
        .collect()
}

/// Emit everything the generated root holds beside `types`: each operation's optional-parameters
/// struct, typed error, multi-status success enum, and response-header structs, the `servers`
/// module, and the `Client` itself — its constructors (`new`, `with_default_server` when a server
/// is declared, `with_client`, `with_backend`, `with_client_and_backend`), `core`, the credential
/// registration methods, `with_timeout`, `with_options` and `with_base_url`, and
/// one async method per operation, declared from its entry in `signatures`, which runs parallel to
/// `api.operations`.
pub(super) fn emit_client(
    api: &Api,
    signatures: &[OperationSig],
    names: &Names,
    options: &CodegenOptions,
) -> TokenStream {
    let credential_docs = with_credential_docs(api, options);
    let params = api
        .operations
        .iter()
        .filter(|operation| operation.has_optional_params())
        .map(|operation| emit_params_struct(operation, names));
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
        with_bound_header_tokens(api, quote! { self.core.bind_header(name, value); });
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
        // `ClientCore` is `Debug + Clone` (its `reqwest::Client` and backend are handle types, and
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
            /// Build a client on the default `ClientConfig`: a 30 s connect timeout, a 5 min read
            /// (stall) timeout, and no total timeout. [`Self::with_config`] changes any of them.
            pub fn new(base_url: &str) -> Result<Self, support::Error<std::convert::Infallible>> {
                Self::with_config(base_url, |_| {})
            }

            /// Build a client whose `ClientConfig` is the default one as `configure` leaves it —
            /// where the connect, read and total timeouts are tightened or loosened. Its
            /// `reqwest::Client` is built from the connect and read timeouts as they are when
            /// `configure` returns.
            pub fn with_config(
                base_url: &str,
                configure: impl FnOnce(&mut support::ClientConfig),
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let mut config = support::ClientConfig {
                    max_error_body: #error_body_cap,
                    ..support::ClientConfig::default()
                };
                configure(&mut config);
                let core = support::ClientCore::with_config(base_url, config)?;
                Ok(Self { core })
            }

            #default_server

            /// Build a client over a caller-supplied `reqwest::Client`, which both builds and
            /// sends requests: your client's timeouts apply instead of `ClientConfig`'s connect
            /// and read timeouts.
            pub fn with_client(
                client: reqwest::Client,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let mut core = support::ClientCore::with_client(client, base_url)?;
                core.config_mut().max_error_body = #error_body_cap;
                Ok(Self { core })
            }

            /// Build a client over a caller-supplied transport backend — the injection point for
            /// retry, middleware, or a non-reqwest transport. Requests are still built on a default
            /// `reqwest::Client`; only the execute step goes through the backend, so connecting
            /// and reading are bounded however the backend bounds them. Each call builds that
            /// `reqwest::Client`; [`Self::with_client_and_backend`] takes one you already have.
            pub fn with_backend(
                backend: std::sync::Arc<dyn support::HttpBackend>,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let mut core = support::ClientCore::with_backend(backend, base_url)?;
                core.config_mut().max_error_body = #error_body_cap;
                Ok(Self { core })
            }

            /// Build a client over both a caller-supplied `reqwest::Client`, which builds
            /// requests, and a caller-supplied transport backend, which executes them; nothing
            /// else is built, so many clients can share one of each. Connecting and reading are
            /// bounded however the backend bounds them.
            pub fn with_client_and_backend(
                client: reqwest::Client,
                backend: std::sync::Arc<dyn support::HttpBackend>,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let mut core = support::ClientCore::with_client_and_backend(client, backend, base_url)?;
                core.config_mut().max_error_body = #error_body_cap;
                Ok(Self { core })
            }

            pub fn core(&self) -> &support::ClientCore {
                &self.core
            }

            #credential_docs
            #[must_use]
            pub fn with_credential(
                mut self,
                scheme: &str,
                credential: support::Credential,
            ) -> Self {
                self.core.set_credential(scheme, credential);
                self
            }

            /// Unregister the credential for a named security scheme; a scheme never registered
            /// is left as it is. Operations pick the first `security` alternative whose schemes
            /// are all registered and never fall through past it when attaching fails, so this
            /// is how a caller reaches a later alternative: `client.clone().without_credential(..)`
            /// derives a client that no longer selects the earlier one.
            #[must_use]
            pub fn without_credential(mut self, scheme: &str) -> Self {
                self.core.remove_credential(scheme);
                self
            }

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

            /// Repoint this client value at another base URL, keeping its `reqwest::Client`,
            /// transport backend, configuration and credentials, so nothing is rebuilt: derive one
            /// with `client.clone().with_base_url(url)?`. Fails with `Error::RequestConstruction`
            /// if `base_url` is not a valid URL.
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
            docs.push(carried(description, options).into_owned());
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
            security_schemes: IndexMap::new(),
        };
        api.operations[0].bound_headers.push(BoundHeader {
            name: "X-Api-Version".to_owned(),
            required: true,
        });
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
        let preparers: std::collections::BTreeSet<String> = api
            .operations
            .iter()
            .map(|operation| names.operation(&operation.id).preparer.as_str().to_owned())
            .collect();
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
            for preparer in &preparers {
                assert_eq!(
                    methods.contains(preparer),
                    self_ty == "Client",
                    "only `impl Client` defines the preparer `{preparer}`: {methods:?}"
                );
            }
            let fixed: std::collections::BTreeSet<String> = methods
                .into_iter()
                .filter(|method| !operation_methods.contains(method) && !preparers.contains(method))
                .collect();
            assert_eq!(
                &fixed, expected,
                "`impl {self_ty}`'s fixed methods must be `name::CLIENT_METHODS`, less `inner` on \
                 `Client`"
            );
        }
        for method in crate::name::CLIENT_METHODS {
            assert!(
                !operation_methods.contains(*method) && !preparers.contains(*method),
                "operation `{method}` kept the bare spelling of a fixed client method"
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
    /// declared server), and its `with_credential` carries `Client`'s documentation, declared
    /// schemes included, as does its `with_default_server`.
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

            let credential = method_docs(&client.items, "Client", "with_credential")
                .expect("`Client::with_credential` is emitted");
            assert!(
                credential
                    .iter()
                    .any(|doc| doc.contains("a short-lived service token")),
                "`Client::with_credential` lists the declared schemes: {credential:?}"
            );
            assert_eq!(
                method_docs(&blocking.items, "BlockingClient", "with_credential"),
                Some(credential),
                "`BlockingClient::with_credential` carries `Client`'s documentation"
            );

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
