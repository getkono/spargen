use reqwest::header::HeaderMap;
use reqwest::StatusCode;

/// A successful response body paired with its status and headers (PRD FR3). Returned inside
/// `Result<ResponseValue<T>, Error<E>>` from every generated operation method.
#[derive(Debug, Clone)]
pub struct ResponseValue<T> {
    status: StatusCode,
    headers: HeaderMap,
    inner: T,
}

impl<T> ResponseValue<T> {
    /// Wrap a decoded body with its status and headers.
    pub fn new(status: StatusCode, headers: HeaderMap, inner: T) -> Self {
        todo!()
    }

    /// The response status.
    pub fn status(&self) -> StatusCode {
        todo!()
    }

    /// The response headers.
    pub fn headers(&self) -> &HeaderMap {
        todo!()
    }

    /// Consume the wrapper, yielding the decoded body.
    pub fn into_inner(self) -> T {
        todo!()
    }

    /// Map the body while preserving status and headers.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> ResponseValue<U> {
        todo!()
    }
}
