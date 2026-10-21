//! The embedded `support` runtime module. The names the generated root re-exports from it are
//! [`crate::name::ROOT_REEXPORTS`] and its two conditional siblings.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::blocking::blocking_cfg;

/// Emit the private `support` module, one submodule per freestanding runtime source: each file's
/// text up to its `#[cfg(test)]` module, with `crate::` rewritten to `super::`, re-parsed as
/// tokens. The module carries an outer `#[forbid(unsafe_code)]` (an inner `#![…]` would break
/// `include!`). When `uses_xml` is set (the API has an `application/xml` / `text/xml` body), the
/// XML codec module is embedded and its helpers re-exported; otherwise it is omitted entirely, so
/// a non-XML output carries no `quick-xml` reference. `uses_streams` embeds the sequential-response
/// `stream` module, and `uses_time` the RFC 3339 `DateTime`/`Date` newtypes, on the same terms.
pub(super) fn emit_support(uses_xml: bool, uses_streams: bool, uses_time: bool) -> TokenStream {
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
    // The RFC 3339 newtypes are embedded only when a date-typed primitive survives lowering with the
    // `time` mapping enabled; only then does the audit require `time` of the consumer.
    let datetime_module = uses_time.then(|| embed(&crate::support::datetime_runtime_file()));
    let datetime_reexport = uses_time.then(|| {
        quote! { pub use datetime::{Date, DateParseError, DateTime}; }
    });
    // The blocking facade (`BlockingRuntime`) is embedded unconditionally but gated on the
    // `blocking` feature AND `not(target_arch = "wasm32")` at the module level: the tokio-dependent
    // code compiles only when a consumer opts in on a native target, so a default build carries no
    // tokio reference and a wasm build never pulls tokio even with the feature on (tokio's blocking
    // runtime cannot run on the single-threaded browser). A consumer opts in by declaring its own
    // `blocking` feature wired to an optional, non-wasm tokio dependency — which is what
    // `spargen deps` prints, commented out, and what the audit then requires.
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
            #stream_module
            #xml_module
            #datetime_module
            #blocking_module

            pub use auth::{attach_auth, AuthError, AuthKind, AuthScheme, Credential, ExposeSecret, SecretString, TokenFuture, TokenProvider};
            pub use client::{ClientConfig, ClientCore};
            pub use dispatch::{build_url, build_url_on, build_url_with_query_string, build_url_with_query_string_on, classify_error, classify_error_bytes, classify_error_text, decode_success, decode_success_bytes, decode_success_text, decode_success_with, decode_text_body, read_error_body, send, unexpected_status, StatusSpec};
            pub use error::{ApiErrorBody, ApiErrorProblem, Error, ProblemDetails, ProtocolError, RedirectError, RequestCause, RequestError, TimeoutKind, TransportError};
            pub use middleware::{Middleware, MiddlewareBackend, Next};
            pub use header::{parse_header, require_header, HeaderError, HeaderShape};
            pub use parameter::{encode, serialize_deep_object, serialize_delimited, serialize_form, serialize_form_body, serialize_label, serialize_matrix, serialize_multipart_values, serialize_simple, Delimiter, FormMode, FormProperty, FormStyle, ParameterError, PercentEncoding};
            pub use paginate::{next_link, LinkPaginator};
            pub use response::ResponseValue;
            pub use retry::{exponential_backoff, RetryBackend, RetryOutcome, RetryPolicy, RetryWait};
            pub use transport::{ExecuteFuture, HttpBackend, ReqwestBackend};
            pub use wasm::{MaybeSend, MaybeSync};
            #stream_reexport
            #xml_reexport
            #datetime_reexport
            #blocking_reexport
        }
    }
}
