//! The `BlockingClient`: the synchronous facade over the async `Client`.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Api, Operation};
use crate::name::Names;

use super::client::{default_server_docs, with_credential_docs};
use super::operation::{url_accessor_tokens, Arg, OperationSig};

/// The `cfg` every blocking item is emitted under, the facade here and the embedded
/// `BlockingRuntime` beside it: the consumer crate's `blocking` feature, on a non-`wasm32` target.
/// The items are emitted unconditionally and compiled out by this `cfg`, so a default build
/// carries no tokio reference, and a wasm build never has one, feature or not, because tokio's
/// blocking runtime cannot run on the single-threaded browser.
pub(super) fn blocking_cfg() -> TokenStream {
    quote! { #[cfg(all(feature = "blocking", not(target_arch = "wasm32")))] }
}

/// Emit the `BlockingClient`: a synchronous facade that owns the async `Client` plus a
/// current-thread tokio runtime and drives each async operation to completion with `block_on`,
/// compiled under [`blocking_cfg`]. Each method is a shim over the async client's method, so the
/// request building and dispatch are the async client's. Its constructors are the async client's
/// `new`, `with_config`, `with_default_server` (when a server is declared), `with_client` and
/// `with_backend`, and its `with_credential` carries the async client's documentation, declared
/// schemes included.
///
/// A `BlockingClient` must not be built or used from inside another async runtime (tokio's
/// `block_on` panics when nested); the constructor building its own current-thread runtime is the
/// standard shape for a non-async caller.
pub(super) fn emit_blocking_client(
    api: &Api,
    signatures: &[OperationSig],
    names: &Names,
) -> TokenStream {
    let methods = api
        .operations
        .iter()
        .zip(signatures)
        .map(|(operation, sig)| emit_blocking_operation(operation, sig, names));
    let doc = "A synchronous client: owns the async `Client` plus a current-thread tokio runtime \
        and `block_on`s each operation. Enable the crate's `blocking` feature to use it; it \
        compiles only on a non-wasm32 target.\n\n\
        Must NOT be constructed or called from inside another async runtime — tokio's `block_on` \
        panics when nested. Build one on a plain thread (e.g. `std::thread` or \
        `tokio::task::spawn_blocking`).";
    let cfg = blocking_cfg();
    let credential_docs = with_credential_docs(api);
    let default_server = (!api.servers.is_empty()).then(|| {
        let docs = default_server_docs();
        quote! {
            #docs
            pub fn with_default_server() -> Result<Self, support::Error<std::convert::Infallible>> {
                let inner = Client::with_default_server()?;
                let runtime = new_runtime()?;
                Ok(Self { inner, runtime })
            }
        }
    });
    quote! {
        // In module/include!/macro output, `feature = "blocking"` resolves against the consumer
        // crate. Keep the cfgs inside an ungated lexical lint scope so crates that do not declare
        // that optional feature stay warning-free under `unexpected_cfgs` (including `-D warnings`).
        #[allow(unexpected_cfgs, unused_imports)]
        mod __spargen_blocking {
            use super::*;

            // `Debug` only: the owned current-thread `tokio::runtime::Runtime` is not `Clone`,
            // and cloning a blocking client would have to mean sharing or duplicating a reactor.
            #cfg
            #[doc = #doc]
            #[derive(Debug)]
            #[allow(dead_code)]
            pub struct BlockingClient {
                inner: Client,
                runtime: support::BlockingRuntime,
            }

            /// Build the current-thread runtime every `BlockingClient` constructor owns, reporting a
            /// failure as the constructors' `RequestConstruction` error.
            #cfg
            #[allow(clippy::result_large_err)]
            fn new_runtime(
            ) -> Result<support::BlockingRuntime, support::Error<std::convert::Infallible>> {
                support::BlockingRuntime::new().map_err(support::Error::request_construction)
            }

            #cfg
            #[forbid(unsafe_code)]
            #[allow(dead_code, deprecated, unused_mut, unused_variables, clippy::result_large_err)]
            impl BlockingClient {
            /// Build a blocking client on the default `ClientConfig`, as `Client::new` does.
            pub fn new(base_url: &str) -> Result<Self, support::Error<std::convert::Infallible>> {
                let inner = Client::new(base_url)?;
                let runtime = new_runtime()?;
                Ok(Self { inner, runtime })
            }

            /// Build a blocking client whose `ClientConfig` is the default one as `configure`
            /// leaves it, as `Client::with_config` does.
            pub fn with_config(
                base_url: &str,
                configure: impl FnOnce(&mut support::ClientConfig),
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let inner = Client::with_config(base_url, configure)?;
                let runtime = new_runtime()?;
                Ok(Self { inner, runtime })
            }

            #default_server

            /// Build a blocking client over a caller-supplied `reqwest::Client`, whose own
            /// timeouts apply.
            pub fn with_client(
                client: reqwest::Client,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let inner = Client::with_client(client, base_url)?;
                let runtime = new_runtime()?;
                Ok(Self { inner, runtime })
            }

            /// Build a blocking client over a caller-supplied transport backend.
            pub fn with_backend(
                backend: std::sync::Arc<dyn support::HttpBackend>,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let inner = Client::with_backend(backend, base_url)?;
                let runtime = new_runtime()?;
                Ok(Self { inner, runtime })
            }

            /// Borrow the wrapped async client.
            pub fn inner(&self) -> &Client {
                &self.inner
            }

            /// Borrow the client's shared core (base URL, credentials, transport).
            pub fn core(&self) -> &support::ClientCore {
                self.inner.core()
            }

            #credential_docs
            #[must_use]
            pub fn with_credential(
                mut self,
                scheme: &str,
                credential: support::Credential,
            ) -> Self {
                self.inner = self.inner.with_credential(scheme, credential);
                self
            }

            /// Unregister the credential for a named security scheme (mirrors the async client).
            #[must_use]
            pub fn without_credential(mut self, scheme: &str) -> Self {
                self.inner = self.inner.without_credential(scheme);
                self
            }

            /// Set the total timeout of every call made through this client value (mirrors the
            /// async client's `with_timeout`): an elapsed one is
            /// `Error::Timeout(TimeoutKind::Total)`, `None` sets no total bound of its own, and
            /// an operation returning an `EventStream` is exempt.
            #[must_use]
            pub fn with_timeout(mut self, timeout: Option<std::time::Duration>) -> Self {
                self.inner = self.inner.with_timeout(timeout);
                self
            }

                #(#methods)*
            }
        }

        #[allow(unused_imports)]
        pub use __spargen_blocking::*;
    }
}

/// Emit one blocking operation method: the async method's signature minus `async`, whose body drives
/// the async method to completion on the owned runtime, declared from the async method's own
/// [`OperationSig`]. Its URL accessor follows it, forwarding to the async client's, which sends
/// nothing and needs no runtime.
fn emit_blocking_operation(
    operation: &Operation,
    sig: &OperationSig,
    names: &Names,
) -> TokenStream {
    let operation_names = names.operation(&operation.id);
    let method_ident = &operation_names.method;
    let url_ident = &operation_names.url_accessor;
    let attrs = sig.attrs();
    let decls = sig.decls();
    let forwards = sig.forwards();
    let (return_ok_ty, error_ty) = sig.return_tys();
    let url_forwards = sig.url_args().map(Arg::forward);
    let url_accessor = url_accessor_tokens(
        operation_names,
        sig,
        quote! { self.inner.#url_ident(#(#url_forwards),*) },
    );
    quote! {
        #attrs
        #[inline]
        pub fn #method_ident(
            &self,
            #(#decls),*
        ) -> Result<#return_ok_ty, support::Error<#error_ty>> {
            self.runtime.block_on(self.inner.#method_ident(#(#forwards),*))
        }

        #url_accessor
    }
}
