//! The embedded `support` runtime module, and the names the generated root re-exports from it.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

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
    let blocking_module = quote! {
        #[cfg(all(feature = "blocking", not(target_arch = "wasm32")))]
        #blocking_inner
    };
    let blocking_reexport = quote! {
        #[cfg(all(feature = "blocking", not(target_arch = "wasm32")))]
        pub use blocking::BlockingRuntime;
    };
    quote! {
        /// The freestanding runtime embedded verbatim into this output; no spargen crate exists
        /// at runtime.
        #[forbid(unsafe_code)]
        #[allow(dead_code, unexpected_cfgs, unused_imports, clippy::result_large_err)]
        mod support {
            #(#modules)*
            #stream_module
            #xml_module
            #datetime_module
            #blocking_module

            pub use auth::{AuthError, AuthKind, AuthScheme, Credential, ExposeSecret, SecretString, TokenFuture, TokenProvider};
            pub use client::{ClientConfig, ClientCore};
            pub use dispatch::{attach_auth, build_url, build_url_on, build_url_with_query_string, build_url_with_query_string_on, classify_error, classify_error_bytes, classify_error_text, decode_success, decode_success_bytes, decode_success_text, decode_text_body, read_error_body, read_success_body, send, unexpected_status, StatusSpec};
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

/// The names every generated module re-exports from the embedded runtime into its root, in the
/// order `generate` emits them.
///
/// The embedded `support` module is private, so this list and its two conditional siblings are the
/// whole nameable runtime surface of a generated client, and so part of its semver surface. It has
/// to cover every type that appears in a signature the output emits: `HeaderError` is the return
/// type of each `…Headers::from_headers`, `RetryWait` is what a caller's `RetryPolicy` must return,
/// `ClientCore` is what `Client::core` hands back, and the taxonomy's payload types are matched on.
/// Anything short of that leaves a generated signature a caller can call but cannot write down.
/// `ApiErrorBody` is the bound `Error::api_body` needs, implemented by the uniform-body error enum,
/// the single-body newtype, and the uninhabited shape (an enum whose bodies are different generated
/// types gets none). `ApiErrorProblem` is the bound `Error::problem` needs, implemented by every
/// error shape, and `ProblemDetails` is what that reader returns.
///
/// `generate` emits the root `pub use` from these lists and [`error_type_ident`] steers clear of
/// them, so the two read one source. `spargen/tests/reexport_lists.rs` holds each name to the
/// embedded `support` module's own re-exports, and pins the emitted root surface as a golden file.
pub(super) const ROOT_REEXPORTS: &[&str] = &[
    "ApiErrorBody",
    "ApiErrorProblem",
    "AuthError",
    "ClientConfig",
    "ClientCore",
    "Credential",
    "Error",
    "ExecuteFuture",
    "ExposeSecret",
    "HeaderError",
    "HeaderShape",
    "HttpBackend",
    "LinkPaginator",
    "Middleware",
    "MiddlewareBackend",
    "Next",
    "ProblemDetails",
    "ProtocolError",
    "RedirectError",
    "RequestCause",
    "RequestError",
    "ReqwestBackend",
    "ResponseValue",
    "RetryBackend",
    "RetryOutcome",
    "RetryPolicy",
    "RetryWait",
    "SecretString",
    "TimeoutKind",
    "TokenFuture",
    "TokenProvider",
    "TransportError",
    "exponential_backoff",
    "next_link",
];

/// The root re-exports emitted only when the API has a sequential (streaming) response.
pub(super) const STREAM_ROOT_REEXPORTS: &[&str] = &[
    "EventStream",
    "ReconnectPolicy",
    "ReconnectReason",
    "ReconnectWait",
    "StreamError",
];

/// The root re-exports emitted only when a date-typed primitive survives lowering with the `time`
/// mapping on. `DateParseError` is the `FromStr` error of the other two, so `"…".parse::<Date>()`
/// yields a `Result` whose error type a caller can write down.
pub(super) const DATETIME_ROOT_REEXPORTS: &[&str] = &["Date", "DateParseError", "DateTime"];

/// The name of an operation's error type: `{Operation}Error`, widened to
/// `{Operation}OperationError` when the first form would shadow a runtime re-export.
///
/// An `operationId` of `request`, `transport`, or `header` otherwise produces `RequestError`,
/// `TransportError`, or `HeaderError` beside the `pub use` of the same name, and the emitted module
/// fails to compile. Operation IDs are unique, so both forms stay unique across operations.
///
/// The check reads the *union* of the root re-exports, the conditional ones included, so an
/// operation's type name never changes because an unrelated part of the spec started or stopped
/// using streams or dates.
pub(super) fn error_type_ident(method_ident: &str) -> proc_macro2::Ident {
    let base = to_pascal(method_ident);
    let name = format!("{base}Error");
    let shadows_reexport = [
        ROOT_REEXPORTS,
        STREAM_ROOT_REEXPORTS,
        DATETIME_ROOT_REEXPORTS,
    ]
    .iter()
    .any(|names| names.contains(&name.as_str()));
    if shadows_reexport {
        format_ident!("{}OperationError", base)
    } else {
        format_ident!("{}", name)
    }
}

/// The `PascalCase` form of an allocated method identifier, raw-identifier prefix dropped.
pub(super) fn to_pascal(value: &str) -> String {
    crate::name::to_pascal_case(value.trim_start_matches("r#"))
}
