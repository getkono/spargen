//! The opt-in `testing` module (`Spec::testing`): a typed test double for the generated client.
//!
//! `testing::MockBackend` is an `HttpBackend` that routes each request by the `ops::Operation` its
//! operation method attaches, never by path, to the handler registered with that operation's
//! `on_{method}`. The handler reads the recorded `testing::Call` and answers with the operation's
//! own `testing::{method}::Reply`, whose constructors, one per documented response, encode a typed
//! body with the codec the client's decoder reads that status with. A request no handler is
//! registered for fails with a `testing::Unrouted` error naming its operation, method and path.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::ir::{Api, Operation, Response, StatusSpec};
use crate::name::Names;

use super::dispatch::BodyCodec;
use super::docs::normalize_rustdoc;
use super::operation::ops_const_ident;
use super::ty::ty_tokens;

/// Emit the `testing` module: the operation-independent `Call`, `Reply`, `Unrouted` and
/// `MockBackend`, one `on_{method}` registration per operation, and one `{method}` module per
/// operation holding its typed `Reply`.
///
/// Every crate path in the module is written from the extern prelude (`::http`, `::bytes`,
/// `::std`, `::serde`, `::serde_json`, `::http_body`): a `{method}` module of the same name sits
/// beside it and would shadow the bare spelling.
pub(super) fn emit_testing(api: &Api, names: &Names) -> TokenStream {
    let registrations = api
        .operations
        .iter()
        .map(|operation| registration(operation, names));
    let reply_modules = api
        .operations
        .iter()
        .map(|operation| reply_module(operation, api, names));
    quote! {
        /// A typed test double for `Client`, emitted under `Spec::testing`: build a client on a
        /// `MockBackend` with `Client::with_backend`, register a handler per operation, and read
        /// back the calls it recorded.
        #[allow(dead_code, clippy::result_large_err)]
        pub mod testing {
            /// One request the `MockBackend` received, recorded whole: its operation, method, URI,
            /// headers and body.
            #[derive(Debug, Clone)]
            pub struct Call {
                operation: Option<super::ops::Operation>,
                method: ::http::Method,
                uri: ::http::Uri,
                headers: ::http::HeaderMap,
                body: ::bytes::Bytes,
            }

            impl Call {
                /// The operation that built the request, read from its extensions; `None` for a
                /// request no operation method or request builder built.
                pub fn operation(&self) -> Option<super::ops::Operation> {
                    self.operation
                }

                /// The request's method.
                pub fn method(&self) -> &::http::Method {
                    &self.method
                }

                /// The request's absolute URI.
                pub fn uri(&self) -> &::http::Uri {
                    &self.uri
                }

                /// The request's path, percent-encoded as sent.
                pub fn path(&self) -> &str {
                    self.uri.path()
                }

                /// The request's query string, percent-encoded as sent, without its `?`.
                pub fn query(&self) -> Option<&str> {
                    self.uri.query()
                }

                /// The request's headers.
                pub fn headers(&self) -> &::http::HeaderMap {
                    &self.headers
                }

                /// The first value of the header `name`, where it is present and visible ASCII.
                pub fn header(&self, name: &str) -> Option<&str> {
                    self.headers.get(name).and_then(|value| value.to_str().ok())
                }

                /// The request body, read whole; empty for a request without one.
                pub fn body(&self) -> &[u8] {
                    &self.body
                }

                /// The request body decoded from JSON into `T`, as the server would read it.
                pub fn json<T: ::serde::de::DeserializeOwned>(&self) -> Result<T, ::serde_json::Error> {
                    ::serde_json::from_slice(&self.body)
                }
            }

            /// A response the `MockBackend` answers with: a status, headers, and a body held in
            /// memory. Each operation's typed `Reply` builds one for each response the operation
            /// documents; this is the untyped form those wrap, for a response the description
            /// does not document.
            #[derive(Debug, Clone)]
            pub struct Reply {
                status: ::http::StatusCode,
                headers: ::http::HeaderMap,
                body: ::bytes::Bytes,
            }

            impl Reply {
                /// A response with the status `status`, no headers, and an empty body.
                ///
                /// # Panics
                ///
                /// When `status` is not a status code (`100..=999`).
                pub fn new(status: u16) -> Self {
                    let Ok(status) = ::http::StatusCode::from_u16(status) else {
                        panic!("testing::Reply: {status} is not an HTTP status code");
                    };
                    Self {
                        status,
                        headers: ::http::HeaderMap::new(),
                        body: ::bytes::Bytes::new(),
                    }
                }

                /// Add the header `name: value`, beside any already added under that name.
                ///
                /// # Panics
                ///
                /// When `name` is not a header name or `value` not a header value.
                #[must_use]
                pub fn header(mut self, name: &str, value: &str) -> Self {
                    let Ok(name) = ::http::header::HeaderName::from_bytes(name.as_bytes()) else {
                        panic!("testing::Reply: `{name}` is not a header name");
                    };
                    let Ok(value) = ::http::HeaderValue::from_str(value) else {
                        panic!("testing::Reply: `{value}` is not a header value");
                    };
                    self.headers.append(name, value);
                    self
                }

                /// Replace the body with `body`, as given; the content type is the caller's to set
                /// with [`Self::header`].
                #[must_use]
                pub fn body(mut self, body: impl Into<::bytes::Bytes>) -> Self {
                    self.body = body.into();
                    self
                }

                /// The response's status.
                pub fn status(&self) -> ::http::StatusCode {
                    self.status
                }

                /// A response with the status `status` whose body is `body`, already encoded and
                /// announced with `content_type`.
                fn encoded(status: u16, content_type: &'static str, body: ::bytes::Bytes) -> Self {
                    Self::new(status)
                        .header("content-type", content_type)
                        .body(body)
                }

                /// `body` as JSON.
                fn json<T: ::serde::Serialize + ?Sized>(status: u16, body: &T) -> Self {
                    match ::serde_json::to_vec(body) {
                        Ok(bytes) => Self::encoded(status, "application/json", bytes.into()),
                        Err(error) => panic!("testing::Reply: the body has no JSON form: {error}"),
                    }
                }

                /// `body` as `text/plain`, read back by the client as the JSON string it is.
                fn text<T: ::serde::Serialize + ?Sized>(status: u16, body: &T) -> Self {
                    match ::serde_json::to_value(body) {
                        Ok(::serde_json::Value::String(text)) => {
                            Self::encoded(status, "text/plain; charset=utf-8", text.into())
                        }
                        Ok(other) => panic!(
                            "testing::Reply: a text body must serialize to a string, not {other}"
                        ),
                        Err(error) => panic!("testing::Reply: the body has no text form: {error}"),
                    }
                }

                /// `body` as raw octets.
                fn octets(status: u16, body: impl Into<::bytes::Bytes>) -> Self {
                    Self::encoded(status, "application/octet-stream", body.into())
                }

                /// Panic unless `status` falls in the `prefix`XX range a reply constructor stands
                /// for.
                fn in_range(status: u16, prefix: u16) -> u16 {
                    assert!(
                        status / 100 == prefix,
                        "testing::Reply: {status} is not a {prefix}XX status"
                    );
                    status
                }

                /// The `http::Response` the backend hands the client.
                fn into_response(self) -> ::http::Response<super::support::HttpBody> {
                    let mut response = ::http::Response::new(super::support::HttpBody::from(self.body));
                    *response.status_mut() = self.status;
                    *response.headers_mut() = self.headers;
                    response
                }
            }

            /// The error a request no handler is registered for fails with, as the source of the
            /// `TransportError::request` the `MockBackend` returns; the client reports it as
            /// `Error::RequestConstruction`. It names the operation the request was built by,
            /// where any was, and its method and path.
            #[derive(Debug, Clone)]
            pub struct Unrouted {
                call: Call,
            }

            impl Unrouted {
                /// The request that found no handler, recorded like every other.
                pub fn call(&self) -> &Call {
                    &self.call
                }
            }

            impl ::std::fmt::Display for Unrouted {
                fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                    match self.call.operation {
                        Some(operation) => write!(
                            f,
                            "testing::MockBackend has no handler for operation `{}` ({} {}), \
                             which received {} {}",
                            operation.id,
                            operation.method,
                            operation.path_template,
                            self.call.method,
                            self.call.uri,
                        ),
                        None => write!(
                            f,
                            "testing::MockBackend received {} {}, which no operation built",
                            self.call.method, self.call.uri,
                        ),
                    }
                }
            }

            impl ::std::error::Error for Unrouted {}

            /// A registered handler.
            type Handler = ::std::sync::Arc<dyn Fn(&Call) -> Reply + Send + Sync>;

            /// What the clones of one `MockBackend` share.
            #[derive(Default)]
            struct State {
                /// The handler per operation id, the latest registration winning.
                handlers: ::std::collections::HashMap<&'static str, Handler>,
                /// Every request received, in arrival order.
                calls: Vec<Call>,
            }

            /// A typed test double for the transport: hand it to `Client::with_backend` and
            /// register one handler per operation the code under test calls.
            ///
            /// It routes each request by the `ops::Operation` among its extensions, which every
            /// operation method and request builder attaches, never by its path: the handler
            /// registered with that operation's `on_{method}` answers it with the operation's own
            /// `Reply`. Every request is recorded first, so `calls` reads back what was sent,
            /// unrouted ones included. A request with no handler fails with an `Unrouted` error
            /// instead of any response.
            ///
            /// Clones share their handlers and recorded calls, so keep one to read the calls back
            /// after handing another to the client.
            #[derive(Clone, Default)]
            pub struct MockBackend {
                state: ::std::sync::Arc<::std::sync::Mutex<State>>,
            }

            impl ::std::fmt::Debug for MockBackend {
                fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                    let state = self.lock();
                    let mut routed: Vec<&str> = state.handlers.keys().copied().collect();
                    routed.sort_unstable();
                    f.debug_struct("MockBackend")
                        .field("routed", &routed)
                        .field("calls", &state.calls.len())
                        .finish()
                }
            }

            impl MockBackend {
                /// A mock with no handler registered and no call recorded.
                pub fn new() -> Self {
                    Self::default()
                }

                /// Every request received for `operation`, in arrival order.
                pub fn calls(&self, operation: super::ops::Operation) -> Vec<Call> {
                    self.lock()
                        .calls
                        .iter()
                        .filter(|call| call.operation == Some(operation))
                        .cloned()
                        .collect()
                }

                /// Every request received, in arrival order, whichever operation built it.
                pub fn all_calls(&self) -> Vec<Call> {
                    self.lock().calls.clone()
                }

                /// The shared state; a handler that panicked while the lock was held poisons
                /// nothing a test still reads.
                fn lock(&self) -> ::std::sync::MutexGuard<'_, State> {
                    self.state
                        .lock()
                        .unwrap_or_else(::std::sync::PoisonError::into_inner)
                }

                /// Register `handler` for the operation `id`, replacing any registered before.
                fn route(self, id: &'static str, handler: Handler) -> Self {
                    self.lock().handlers.insert(id, handler);
                    self
                }

                #(#registrations)*
            }

            impl super::support::HttpBackend for MockBackend {
                fn execute(
                    &self,
                    request: ::http::Request<super::support::HttpBody>,
                ) -> super::support::ExecuteFuture<'_> {
                    Box::pin(async move {
                        let (parts, mut body) = request.into_parts();
                        let mut bytes = Vec::new();
                        while let Some(frame) = ::std::future::poll_fn(|cx| {
                            ::http_body::Body::poll_frame(::std::pin::Pin::new(&mut body), cx)
                        })
                        .await
                        {
                            if let Ok(data) = frame?.into_data() {
                                bytes.extend_from_slice(&data);
                            }
                        }
                        let call = Call {
                            operation: parts.extensions.get::<super::ops::Operation>().copied(),
                            method: parts.method,
                            uri: parts.uri,
                            headers: parts.headers,
                            body: bytes.into(),
                        };
                        let handler = {
                            let mut state = self.lock();
                            state.calls.push(call.clone());
                            call.operation
                                .and_then(|operation| state.handlers.get(operation.id).cloned())
                        };
                        match handler {
                            Some(handler) => Ok(handler(&call).into_response()),
                            None => Err(super::support::TransportError::request(Unrouted { call })),
                        }
                    })
                }
            }

            #(#reply_modules)*
        }
    }
}

/// The `on_{method}` registration of one operation on `MockBackend`.
fn registration(operation: &Operation, names: &Names) -> TokenStream {
    let operation_names = names.operation(&operation.id);
    let method = &operation_names.method;
    let register = format_ident!("on_{}", method.unraw());
    let constant = ops_const_ident(operation_names);
    let doc = normalize_rustdoc(&format!(
        "Answer every request `{}` sends (`ops::{constant}`) with `handler`, replacing any handler \
         registered for it before.",
        method.unraw()
    ));
    quote! {
        #[doc = #doc]
        #[must_use]
        pub fn #register(
            self,
            handler: impl Fn(&Call) -> #method::Reply + Send + Sync + 'static,
        ) -> Self {
            self.route(
                super::ops::#constant.id,
                ::std::sync::Arc::new(move |call: &Call| handler(call).0),
            )
        }
    }
}

/// The `{method}` module of one operation: its typed `Reply`, with one constructor per documented
/// response.
fn reply_module(operation: &Operation, api: &Api, names: &Names) -> TokenStream {
    let method = &names.operation(&operation.id).method;
    let constructors = operation
        .responses
        .entries()
        .map(|(spec, response)| constructor(spec, response, api, names));
    let module_doc = normalize_rustdoc(&format!(
        "The replies `MockBackend::on_{}` handlers answer with.",
        method.unraw()
    ));
    let reply_doc = normalize_rustdoc(&format!(
        "A response to `{}`: one constructor per response it documents, each encoding its body \
         with the codec the client decodes that status with, and `from_reply` for any other.",
        method.unraw()
    ));
    let root = generated_root();
    quote! {
        #[doc = #module_doc]
        pub mod #method {
            #[allow(unused_imports)]
            use #root::*;

            #[doc = #reply_doc]
            #[derive(Debug, Clone)]
            pub struct Reply(pub(super) super::Reply);

            impl Reply {
                /// Answer with `reply` as built, for a response the description does not document
                /// or a body its constructor cannot express.
                pub fn from_reply(reply: super::Reply) -> Self {
                    Self(reply)
                }

                #(#constructors)*
            }

            impl From<Reply> for super::Reply {
                fn from(reply: Reply) -> Self {
                    reply.0
                }
            }
        }
    }
}

/// The path from a module nested in `testing` up to the generated module's root, whose `types`,
/// `support` and re-exported names a reply's body types are spelled with. It is assembled from
/// `super` tokens rather than written out: it climbs the generated module, not spargen's, and the
/// layering test reads a written-out double `super` in this file as a path into spargen's own
/// crate root.
fn generated_root() -> TokenStream {
    let up = <syn::Token![super]>::default();
    quote! { #up::#up }
}

/// The constructor of the reply to one documented response: named for its status selector, taking
/// the status where the selector is a range or `default`, and the body where the response has one.
/// A streamed body has no constructor: its framing is `from_reply`'s to write.
fn constructor(spec: StatusSpec, response: &Response, api: &Api, names: &Names) -> TokenStream {
    let (ident, status_param, status, selector) = match spec {
        StatusSpec::Exact(code) => (
            format_ident!("status_{code}"),
            None,
            quote! { #code },
            format!("`{code}`"),
        ),
        StatusSpec::Range(prefix) => {
            let prefix = u16::from(prefix);
            (
                format_ident!("status_{prefix}xx"),
                Some(quote! { status: u16, }),
                quote! { super::Reply::in_range(status, #prefix) },
                format!("`{prefix}XX` (`status`, which must be in it)"),
            )
        }
        StatusSpec::Default => (
            format_ident!("default"),
            Some(quote! { status: u16, }),
            quote! { status },
            "`default` (`status`)".to_owned(),
        ),
    };
    let body = match (response.body, response.stream) {
        (Some(ty), None) => Some(ty),
        (Some(_), Some(_)) => return TokenStream::new(),
        (None, _) => None,
    };
    let Some(ty) = body else {
        let doc = normalize_rustdoc(&format!("The bodyless response {selector}."));
        return quote! {
            #[doc = #doc]
            pub fn #ident(#status_param) -> Self {
                Self(super::Reply::new(#status))
            }
        };
    };
    let (param, encode, codec) = match BodyCodec::of(api, ty, response.media) {
        BodyCodec::Bytes => (
            quote! { body: impl Into<::bytes::Bytes> },
            quote! { super::Reply::octets(#status, body) },
            "raw octets",
        ),
        BodyCodec::Xml => {
            let ty = ty_tokens(ty, names, true);
            (
                quote! { body: &#ty },
                quote! {
                    match support::to_xml(body) {
                        Ok(xml) => super::Reply::encoded(#status, "application/xml", xml.into()),
                        Err(error) => panic!("testing::Reply: the body has no XML form: {error}"),
                    }
                },
                "XML",
            )
        }
        BodyCodec::Text => {
            let ty = ty_tokens(ty, names, true);
            (
                quote! { body: &#ty },
                quote! { super::Reply::text(#status, body) },
                "text",
            )
        }
        BodyCodec::Json => {
            let ty = ty_tokens(ty, names, true);
            (
                quote! { body: &#ty },
                quote! { super::Reply::json(#status, body) },
                "JSON",
            )
        }
    };
    let doc = normalize_rustdoc(&format!(
        "The response {selector} carrying `body`, encoded as {codec}."
    ));
    quote! {
        #[doc = #doc]
        pub fn #ident(#status_param #param) -> Self {
            Self(#encode)
        }
    }
}

#[cfg(test)]
mod tests {
    /// One operation per reply shape: an exact JSON body beside a `4XX` text range and a bodyless
    /// `default` (`getItem`), an XML body (`getXml`), raw octets (`getRaw`), and a streamed body
    /// (`watch`).
    const REPLIES: &str = r##"openapi: 3.1.0
info: {title: T, version: '1'}
components:
  schemas:
    Item: {type: object, properties: {name: {type: string}}}
paths:
  /items:
    get:
      operationId: getItem
      responses:
        '200': {description: ok, content: {application/json: {schema: {$ref: '#/components/schemas/Item'}}}}
        '4XX': {description: refused, content: {text/plain: {schema: {type: string}}}}
        default: {description: failed}
  /xml:
    get:
      operationId: getXml
      responses:
        '200': {description: ok, content: {application/xml: {schema: {$ref: '#/components/schemas/Item'}}}}
  /raw:
    get:
      operationId: getRaw
      responses:
        '200': {description: ok, content: {application/octet-stream: {schema: {type: string, format: binary}}}}
  /watch:
    get:
      operationId: watch
      responses:
        '200': {description: ok, content: {application/x-ndjson: {schema: {$ref: '#/components/schemas/Item'}}}}
"##;

    /// The module `REPLIES` generates, with or without `testing`, with every run of whitespace
    /// removed so an assertion reads the tokens rather than `prettyplease`'s line breaking.
    fn generated(testing: bool) -> String {
        let directory = tempfile::tempdir().unwrap();
        let path =
            camino::Utf8PathBuf::from_path_buf(directory.path().join("openapi.yaml")).unwrap();
        std::fs::write(&path, REPLIES).unwrap();
        let preview = crate::__private::preview(&crate::Spec::new(path).testing(testing));
        let contents = preview
            .contents
            .unwrap_or_else(|| panic!("the spec generates: {:?}", preview.report.diagnostics));
        contents.split_whitespace().collect()
    }

    /// Each status selector names its constructor and takes the status where it is not exact, and
    /// each codec encodes the body the client decodes that status with; a streamed body has no
    /// constructor, and every operation has its registration and its `from_reply`.
    #[test]
    fn each_selector_and_codec_has_its_reply_constructor() {
        let generated = generated(true);
        for expected in [
            "pubfnstatus_200(body:&types::Item)->Self{Self(super::Reply::json(200u16,body))}",
            "pubfnstatus_4xx(status:u16,body:&types::ResponseBody)->Self{Self(super::Reply::text(super::Reply::in_range(status,4u16),body))}",
            "pubfndefault(status:u16)->Self{Self(super::Reply::new(status))}",
            "matchsupport::to_xml(body){Ok(xml)=>{super::Reply::encoded(200u16,\"application/xml\",xml.into())}",
            "pubfnstatus_200(body:implInto<::bytes::Bytes>)->Self{Self(super::Reply::octets(200u16,body))}",
            "pubfnon_get_item(self,handler:implFn(&Call)->get_item::Reply+Send+Sync+'static,)->Self{self.route(super::ops::GET_ITEM.id,",
            "pubfnon_watch(self,handler:implFn(&Call)->watch::Reply+Send+Sync+'static,)->Self",
        ] {
            assert!(generated.contains(expected), "missing `{expected}`");
        }
        let watch = &generated[generated.find("pubmodwatch{").expect("the watch replies")..];
        let watch = &watch[..watch.find("implFrom<Reply>").expect("its conversion")];
        assert!(watch.contains("pubfnfrom_reply("), "{watch}");
        assert!(!watch.contains("pubfnstatus_200"), "{watch}");
        assert_eq!(generated.matches("pubfnfrom_reply(").count(), 4);
    }

    /// Without the switch, nothing of the module is emitted.
    #[test]
    fn the_module_is_emitted_only_under_the_switch() {
        assert!(generated(true).contains("pubmodtesting{"));
        let off = generated(false);
        assert!(!off.contains("pubmodtesting{"));
        assert!(!off.contains("MockBackend"));
    }
}
