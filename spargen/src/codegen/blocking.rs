//! The `BlockingClient`: the synchronous facade over the async `Client`.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Api, Operation};
use crate::name::Names;

use super::docs::doc_tokens;
use super::operation::{operation_args, operation_return_ty, param_default_docs_tokens};

/// Emit the `BlockingClient`: a synchronous facade that owns the async `Client` plus a
/// current-thread tokio runtime and drives each async operation to completion with `block_on`. It
/// is compiled only under the generated crate's `blocking` feature on a non-`wasm32` target
/// (`cfg(all(feature = "blocking", not(target_arch = "wasm32")))`): tokio's blocking runtime cannot
/// run on the single-threaded browser, so a wasm build never has one, feature or not. It reuses the
/// whole async dispatch — every method is a thin shim — so there is zero logic duplication.
/// Constructors mirror the async client's.
///
/// A `BlockingClient` must not be built or used from inside another async runtime (tokio's
/// `block_on` panics when nested); the constructor building its own current-thread runtime is the
/// standard shape for a non-async caller.
pub(super) fn emit_blocking_client(api: &Api, names: &Names) -> TokenStream {
    let methods = api
        .operations
        .iter()
        .map(|operation| emit_blocking_operation(operation, api, names));
    let doc = "A synchronous client: owns the async `Client` plus a current-thread tokio runtime \
        and `block_on`s each operation. Enable the crate's `blocking` feature to use it.\n\n\
        Must NOT be constructed or called from inside another async runtime — tokio's `block_on` \
        panics when nested. Build one on a plain thread (e.g. `std::thread` or \
        `tokio::task::spawn_blocking`).";
    quote! {
        // In module/include!/macro output, `feature = "blocking"` resolves against the consumer
        // crate. Keep the cfgs inside an ungated lexical lint scope so crates that do not declare
        // that optional feature stay warning-free under `unexpected_cfgs` (including `-D warnings`).
        #[allow(unexpected_cfgs, unused_imports)]
        mod __spargen_blocking {
            use super::*;

            // `Debug` only: the owned current-thread `tokio::runtime::Runtime` is not `Clone`,
            // and cloning a blocking client would have to mean sharing or duplicating a reactor.
            #[cfg(all(feature = "blocking", not(target_arch = "wasm32")))]
            #[doc = #doc]
            #[derive(Debug)]
            #[allow(dead_code)]
            pub struct BlockingClient {
                inner: Client,
                runtime: support::BlockingRuntime,
            }

            #[cfg(all(feature = "blocking", not(target_arch = "wasm32")))]
            #[forbid(unsafe_code)]
            #[allow(dead_code, unused_mut, unused_variables, clippy::result_large_err)]
            impl BlockingClient {
            /// Build a blocking client over a fresh default `reqwest::Client`.
            pub fn new(base_url: &str) -> Result<Self, support::Error<std::convert::Infallible>> {
                let inner = Client::new(base_url)?;
                let runtime = support::BlockingRuntime::new()
                    .map_err(support::Error::request_construction)?;
                Ok(Self { inner, runtime })
            }

            /// Build a blocking client over a caller-supplied `reqwest::Client`.
            pub fn with_client(
                client: reqwest::Client,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let inner = Client::with_client(client, base_url)?;
                let runtime = support::BlockingRuntime::new()
                    .map_err(support::Error::request_construction)?;
                Ok(Self { inner, runtime })
            }

            /// Build a blocking client over a caller-supplied transport backend.
            pub fn with_backend(
                backend: std::sync::Arc<dyn support::HttpBackend>,
                base_url: &str,
            ) -> Result<Self, support::Error<std::convert::Infallible>> {
                let inner = Client::with_backend(backend, base_url)?;
                let runtime = support::BlockingRuntime::new()
                    .map_err(support::Error::request_construction)?;
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

            /// Register a credential for a named security scheme (mirrors the async client).
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

                #(#methods)*
            }
        }

        #[allow(unused_imports)]
        pub use __spargen_blocking::*;
    }
}

/// Emit one blocking operation method: the async method's signature minus `async`, whose body drives
/// the async method to completion on the owned runtime. Same docs, deprecation, argument list, and
/// return types as the async method (all built from the shared signature helpers).
fn emit_blocking_operation(operation: &Operation, api: &Api, names: &Names) -> TokenStream {
    let method_ident = names
        .operations
        .get(&operation.id)
        .expect("operation name allocated");
    let docs = doc_tokens(&operation.docs);
    let param_default_docs = param_default_docs_tokens(operation);
    let deprecated = operation.deprecated.then(|| quote! { #[deprecated] });
    let (args, forwards) = operation_args(operation, api, names);
    let (return_ok_ty, error_ty) = operation_return_ty(operation, names);
    quote! {
        #docs
        #(#param_default_docs)*
        #deprecated
        // A deprecated blocking wrapper intentionally forwards to its deprecated async twin. This
        // suppresses only that internal call; `#deprecated` still warns consumers of this method.
        #[allow(deprecated)]
        #[inline]
        pub fn #method_ident(
            &self,
            #(#args),*
        ) -> Result<#return_ok_ty, support::Error<#error_ty>> {
            self.runtime.block_on(self.inner.#method_ident(#(#forwards),*))
        }
    }
}
