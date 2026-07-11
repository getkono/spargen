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
        Self {
            status,
            headers,
            inner,
        }
    }

    /// The response status.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The response headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Consume the wrapper, yielding the decoded body.
    pub fn into_inner(self) -> T {
        self.inner
    }

    /// Borrow the decoded body.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Map the body while preserving status and headers.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> ResponseValue<U> {
        ResponseValue {
            status: self.status,
            headers: self.headers,
            inner: f(self.inner),
        }
    }
}

#[cfg(test)]
mod tests {
    use reqwest::header::HeaderMap;
    use reqwest::StatusCode;

    use super::ResponseValue;

    #[test]
    fn maps_body_without_losing_metadata() {
        let value = ResponseValue::new(StatusCode::CREATED, HeaderMap::new(), 41);
        let mapped = value.map(|value| value + 1);
        assert_eq!(mapped.status(), StatusCode::CREATED);
        assert_eq!(*mapped.inner(), 42);
    }

    #[test]
    fn accessors_expose_status_headers_and_inner() {
        let mut headers = HeaderMap::new();
        headers.insert("x-trace", "abc".parse().unwrap());
        let value = ResponseValue::new(StatusCode::OK, headers, "body".to_owned());

        assert_eq!(value.status(), StatusCode::OK);
        assert_eq!(value.headers()["x-trace"], "abc");
        assert_eq!(value.inner(), "body");
        assert_eq!(value.into_inner(), "body");
    }
}
