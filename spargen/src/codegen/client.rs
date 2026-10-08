//! The async `Client` and the items emitted beside it: each operation's parameter struct, typed
//! error, success enum, and header structs, and the `servers` module.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::Api;
use crate::name::Names;

use super::docs::normalize_rustdoc;
use super::operation::emit_operation;
use super::params::emit_params_struct;
use super::responses::{emit_error_enum, emit_response_enum, emit_response_headers};
use super::CodegenOptions;

/// The declared security schemes, rendered as rustdoc for `with_credential`.
///
/// This is the one place a caller chooses what to register, so it is where the scheme's own
/// documentation belongs: the bearer format, the flows that mint a token, the OpenID Connect
/// discovery URL, and whether the scheme is deprecated.
fn scheme_doc_lines(api: &Api) -> Vec<TokenStream> {
    let documented: Vec<&String> = api
        .security_schemes
        .values()
        .flat_map(|scheme| scheme.docs.iter())
        .collect();
    if documented.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![
        String::new(),
        "# Declared schemes".to_owned(),
        String::new(),
    ];
    lines.extend(documented.into_iter().cloned());
    lines
        .into_iter()
        .map(|line| quote! { #[doc = #line] })
        .collect()
}

/// Emit everything the generated root holds beside `types`: each operation's optional-parameters
/// struct, typed error, multi-status success enum, and response-header structs, the `servers`
/// module, and the `Client` itself — its constructors (`new`, `with_default_server` when a server
/// is declared, `with_client`, `with_backend`), `core`, the credential registration methods, and
/// one async method per operation.
pub(super) fn emit_client(api: &Api, names: &Names, options: &CodegenOptions) -> TokenStream {
    let scheme_docs = scheme_doc_lines(api);
    let params = api
        .operations
        .iter()
        .filter(|operation| operation.params.iter().any(|param| !param.required))
        .map(|operation| emit_params_struct(operation, names));
    let errors = api
        .operations
        .iter()
        .map(|operation| emit_error_enum(operation, api, names));
    let response_enums = api
        .operations
        .iter()
        .map(|operation| emit_response_enum(operation, names));
    let methods = api
        .operations
        .iter()
        .map(|operation| emit_operation(operation, api, names));
    let response_headers = api
        .operations
        .iter()
        .map(|operation| emit_response_headers(operation, names));
    let client_docs = client_doc_tokens(api);
    let error_body_cap = options.error_body_cap;
    let servers = emit_servers(api, names);
    let default_server = (!api.servers.is_empty()).then(|| {
        quote! {
            /// Build a client on the first server the specification declares, with every server
            /// variable at its declared default.
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

        #[forbid(unsafe_code)]
        #[allow(dead_code, unused_mut, unused_variables, clippy::result_large_err)]
        impl Client {
            pub fn new(base_url: &str) -> Result<Self, support::Error<std::convert::Infallible>> {
                Self::with_client(reqwest::Client::new(), base_url)
            }

            #default_server

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
            /// `reqwest::Client`; only the execute step goes through the backend.
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

            /// Register a credential for a named security scheme (a `securitySchemes` key).
            ///
            /// Registration checks nothing; each operation matches its `security` requirement
            /// against the registered credentials when it is called, and fails before anything is
            /// sent, as `support::Error::RequestConstruction`, when no alternative has every scheme
            /// registered (`RequestError::MissingCredential`), when the selected alternative has a
            /// credential its scheme cannot carry (`RequestError::CredentialMismatch` — for
            /// instance a `Credential::Provider` under an `http basic` scheme, which accepts only
            /// `Credential::Basic`), or when its token provider fails
            /// (`RequestError::CredentialProvider`). `support::Credential`'s documentation lists
            /// which variant each kind of scheme accepts.
            #(#scheme_docs)*
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

            #(#methods)*
        }
    }
}

/// Emit the `servers` module: one builder per declared server, plus the default base URL.
///
/// A Server Variable `default` is sent when the caller supplies no alternative, so every server
/// resolves to a concrete URL with no arguments; a variable that declares an `enum` gets a typed
/// enum so an illegal value cannot be constructed at all.
fn emit_servers(api: &Api, names: &Names) -> TokenStream {
    if api.servers.is_empty() {
        return quote! {};
    }
    let builders = api.servers.iter().enumerate().map(|(index, server)| {
        let ident = names
            .servers
            .get(index)
            .expect("server name allocated")
            .clone();
        let mut docs = vec![format!("Server `{}`.", server.url)];
        if let Some(description) = &server.description {
            docs.push(description.clone());
        }
        let doc_attrs = docs.iter().map(|line| quote! { #[doc = #line] });

        let variable_enums = server.variables.iter().filter_map(|(name, variable)| {
            let enum_ident = names.server_variable_enums.get(&(index, name.clone()))?;
            let variants = variable.enum_values.iter().map(|value| {
                let variant = names
                    .server_variable_variants
                    .get(&(index, name.clone(), value.clone()))
                    .expect("server variable variant allocated");
                let default = (value == &variable.default).then(|| quote! { #[default] });
                quote! { #default #variant }
            });
            let arms = variable.enum_values.iter().map(|value| {
                let variant = names
                    .server_variable_variants
                    .get(&(index, name.clone(), value.clone()))
                    .expect("server variable variant allocated");
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
            let field = names
                .server_variable_fields
                .get(&(index, name.clone()))
                .expect("server variable field allocated");
            match names.server_variable_enums.get(&(index, name.clone())) {
                Some(enum_ident) => {
                    quote! { #field: #enum_ident }
                }
                None => quote! { #field: String },
            }
        });

        let defaults = server.variables.iter().map(|(name, variable)| {
            let field = names
                .server_variable_fields
                .get(&(index, name.clone()))
                .expect("server variable field allocated");
            match names.server_variable_enums.get(&(index, name.clone())) {
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
            let field = names
                .server_variable_fields
                .get(&(index, name.clone()))
                .expect("server variable field allocated");
            let mut doc = format!("Set the `{name}` server variable.");
            if let Some(description) = &variable.description {
                doc.push(' ');
                doc.push_str(description);
            }
            match names.server_variable_enums.get(&(index, name.clone())) {
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
                let field = names
                    .server_variable_fields
                    .get(&(index, name.clone()))
                    .expect("server variable field allocated");
                match names.server_variable_enums.get(&(index, name.clone())) {
                    Some(_) => quote! { url.push_str(self.#field.as_str()); },
                    None => quote! { url.push_str(&self.#field); },
                }
            }
        });

        // `Default` is derivable exactly when every field's declared default is what the derive
        // would produce: an enum variable pins its default with `#[default]`, and a server with no
        // variables has nothing to default. A free-form variable carries a spec-declared string,
        // which the derive would replace with `""`.
        let derivable_default = server.variables.iter().all(|(name, _)| {
            names
                .server_variable_enums
                .contains_key(&(index, name.clone()))
        });
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
    let first = names.servers.first().expect("at least one server").clone();
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

/// Document the generated `Client` with the API identity and its declared servers.
fn client_doc_tokens(api: &Api) -> TokenStream {
    let mut text = format!("Client for {} v{}.", api.info.title, api.info.version);
    if let Some(description) = api
        .info
        .description
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        text.push_str("\n\n");
        text.push_str(description);
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
                text.push_str(description);
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
    use crate::diag::{Diagnostics, JsonPointer, Provenance};
    use crate::ir::{
        Api, Docs, Info, Method, Operation, OperationId, PathSegment, PathTemplate, Response,
        Responses, Server, StatusSpec, TypeGraph, UrlSegment,
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
    /// The API declares a server so `with_default_server` is emitted.
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
        };
        let api = Api {
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
        let names = crate::name::allocate(&api, &mut Diagnostics::default());
        let options = CodegenOptions::default();
        let client: syn::File = syn::parse2(super::emit_client(&api, &names, &options))
            .expect("the emitted client parses");
        let blocking: syn::File =
            syn::parse2(super::super::blocking::emit_blocking_client(&api, &names))
                .expect("the emitted blocking client parses");

        let operation_methods: std::collections::BTreeSet<String> = names
            .operations
            .values()
            .map(|ident| ident.as_str().to_owned())
            .collect();
        let mut fixed = std::collections::BTreeSet::new();
        for (file, self_ty) in [(&client, "Client"), (&blocking, "BlockingClient")] {
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
            fixed.extend(
                methods
                    .into_iter()
                    .filter(|method| !operation_methods.contains(method)),
            );
        }
        let reserved: std::collections::BTreeSet<String> = crate::name::CLIENT_METHODS
            .iter()
            .map(|method| (*method).to_owned())
            .collect();
        assert_eq!(
            fixed, reserved,
            "`name::CLIENT_METHODS` must list exactly the fixed methods of `Client` and \
             `BlockingClient`"
        );
        for method in crate::name::CLIENT_METHODS {
            assert!(
                !operation_methods.contains(*method),
                "operation `{method}` kept the bare spelling of a fixed client method"
            );
        }
    }
}
