//! The embedded `support` runtime module. The names the generated root re-exports from it are
//! [`crate::name::ROOT_REEXPORTS`] and its two conditional siblings.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::blocking::blocking_cfg;
use crate::ir::DateTimeBackend;

/// Emit the private `support` module, one submodule per freestanding runtime source: each file's
/// text up to its `#[cfg(test)]` module, with `crate::` rewritten to `super::`, re-parsed as
/// tokens. The module carries an outer `#[forbid(unsafe_code)]`: an inner `#![…]` attribute would
/// make the output unusable via `include!`. When `features.xml` is set (the API has an
/// `application/xml` / `text/xml` body), the XML codec module is embedded and its helpers
/// re-exported; otherwise it is omitted entirely, so a non-XML output carries no `quick-xml`
/// reference. `features.chunked_reads` embeds the `chunked` module, the `wasm32` read of a body as
/// it arrives, `features.streams` the sequential-response `stream` module, which reads through it,
/// `features.datetime` the RFC 3339 `DateTime`/`Date` newtypes over the selected backend's crate
/// (with the shared `rfc3339` codec for `jiff` and `chrono`), and `features.base64` the
/// `contentEncoding: base64` codec, on the same terms.
pub(super) fn emit_support(features: crate::ir::RuntimeFeatures) -> TokenStream {
    let crate::ir::RuntimeFeatures {
        xml: uses_xml,
        streams: uses_streams,
        chunked_reads,
        datetime,
        base64: uses_base64,
    } = features;
    let embed = |file: &crate::support::SupportFile| {
        let stem = file.name.trim_end_matches(".rs");
        let ident = format_ident!("{}", stem);
        // Each runtime file keeps its `#[cfg(test)]` module last; strip it at embed time — the
        // runtime is tested in the support-runtime crate, and test-only `crate::` imports would
        // not survive the module renesting.
        let source = file
            .contents
            .split("#[cfg(test)]")
            .next()
            .expect("split yields at least one part")
            .replace("crate::", "super::");
        let tokens: TokenStream = source
            .parse()
            .expect("embedded support runtime parses as Rust tokens");
        quote! {
            mod #ident {
                #tokens
            }
        }
    };
    let modules = crate::support::runtime_files().iter().map(embed);
    // The chunked `wasm32` read is embedded for every reader of a body as it arrives, an
    // `EventStream` or a binary `_stream` twin's `ByteStream`; only then does the audit require
    // `futures-core` and reqwest's `stream` feature, through which it reads.
    let chunked_module = chunked_reads.then(|| embed(&crate::support::chunked_runtime_file()));
    let chunked_reexport = chunked_reads.then(|| quote! { pub use chunked::read_as_it_arrives; });
    let stream_module = uses_streams.then(|| embed(&crate::support::stream_runtime_file()));
    let stream_reexport = uses_streams.then(|| {
        quote! {
            pub use stream::{
                EventStream, Framing, ReconnectPolicy, ReconnectReason, ReconnectWait, StreamError,
            };
        }
    });
    // The XML codec module is embedded only when the API uses an XML body, and only then does the
    // dependency audit require `quick-xml` of the consumer. A non-XML output never references it.
    let xml_module = uses_xml.then(|| embed(&crate::support::xml_runtime_file()));
    let xml_reexport = uses_xml.then(|| {
        quote! { pub use xml::{classify_error_xml, decode_success_xml, decode_xml_body, to_xml}; }
    });
    // The RFC 3339 newtypes are embedded only when a date-typed primitive survives lowering with a
    // typed backend selected; only then does the audit require that backend's crate of the
    // consumer. Every backend's file is embedded as `datetime`, so the re-export is one spelling.
    let datetime_module = datetime.map(|backend| {
        let codec = || embed(&crate::support::rfc3339_runtime_file());
        match backend {
            DateTimeBackend::Jiff => {
                let codec = codec();
                let newtypes = embed(&crate::support::datetime_jiff_runtime_file());
                quote! { #newtypes #codec }
            }
            DateTimeBackend::Chrono => {
                let codec = codec();
                let newtypes = embed(&crate::support::datetime_chrono_runtime_file());
                quote! { #newtypes #codec }
            }
            // `runtime_features` never selects `String` (it embeds no newtypes), so the arm it
            // shares is `Time`'s.
            DateTimeBackend::Time | DateTimeBackend::String => {
                embed(&crate::support::datetime_runtime_file())
            }
        }
    });
    let datetime_reexport = datetime.is_some().then(|| {
        quote! { pub use datetime::{Date, DateParseError, DateTime}; }
    });
    // The base64 codec is embedded only when a `contentEncoding: base64` newtype survives lowering;
    // the newtype's serde impls are its only callers. It adds no dependency.
    let base64_module = uses_base64.then(|| embed(&crate::support::base64_runtime_file()));
    let base64_reexport = uses_base64.then(|| {
        quote! { pub use base64::{deserialize_base64, serialize_base64}; }
    });
    // The blocking runtime (`BlockingRuntime`) is embedded unconditionally under `blocking_cfg`. A
    // consumer opts in by declaring its own `blocking` feature wired to an optional, non-wasm tokio
    // dependency — which is what `spargen deps` prints, commented out, and what the audit then
    // requires.
    let blocking_inner = embed(&crate::support::blocking_runtime_file());
    let blocking_cfg = blocking_cfg();
    let blocking_module = quote! {
        #blocking_cfg
        #blocking_inner
    };
    let blocking_reexport = quote! {
        #blocking_cfg
        pub use blocking::BlockingRuntime;
    };
    quote! {
        /// The freestanding runtime embedded into this output; no spargen crate is a runtime
        /// dependency.
        #[forbid(unsafe_code)]
        #[allow(dead_code, unexpected_cfgs, unused_imports, clippy::result_large_err)]
        mod support {
            #(#modules)*
            #chunked_module
            #stream_module
            #xml_module
            #datetime_module
            #base64_module
            #blocking_module

            pub use auth::{attach_auth, attach_auth_replayable, replay_rejected, token_provider, AuthError, AuthKind, AuthScheme, Credential, ExposeSecret, RejectionCheck, Replay, SecretString, TokenFuture, TokenProvider, TokenRefresher};
            pub use client::{ClientConfig, ClientCore, RequestOptions};
            pub use dispatch::{build_url_on, build_url_with_query_string_on, classify_error, classify_error_bytes, classify_error_text, classify_with, decode_success, decode_success_bytes, decode_success_text, decode_success_with, decode_text_body, execute, prepare, send, send_streaming, unexpected_status, with_body_limits, StatusSpec};
            pub use error::{ApiErrorBody, ApiErrorProblem, Error, ProblemDetails, ProtocolError, RedirectError, RequestCause, RequestError, TimeoutKind, TransportError, UnknownVariant};
            pub use middleware::{Middleware, MiddlewareBackend, Next};
            pub use nullable::Nullable;
            pub use header::{parse_header, require_header, HeaderError, HeaderShape};
            pub use parameter::{encode, serialize_deep_object, serialize_delimited, serialize_form, serialize_form_body, serialize_label, serialize_matrix, serialize_multipart_values, serialize_simple, Delimiter, FormMode, FormProperty, FormStyle, ParameterError, PercentEncoding};
            pub use paginate::{next_link, LinkPaginator};
            pub use request::{MultipartForm, MultipartPart, RequestBuilder};
            pub use response::{stream_success_bytes, ByteStream, ResponseValue};
            pub use retry::{exponential_backoff, full_jitter, RetryBackend, RetryOutcome, RetryPolicy, RetryWait};
            pub use transport::{try_clone_request, ExecuteFuture, HttpBackend, HttpBody, ReqwestBackend, TotalTimeout};
            pub use wasm::{MaybeSend, MaybeSync};
            #chunked_reexport
            #stream_reexport
            #xml_reexport
            #datetime_reexport
            #base64_reexport
            #blocking_reexport
        }
    }
}
