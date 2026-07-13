//! A synchronous facade for callers without an async runtime.
//!
//! reqwest's async `Client` needs a running reactor to drive its IO, so a hand-rolled `block_on`
//! cannot execute a request. [`BlockingRuntime`] instead owns a real current-thread tokio runtime
//! and drives the generated async operation futures to completion on it — the blocking client is a
//! thin shim that reuses every line of the async dispatch, adding no logic of its own.
//!
//! This module is compiled only under the `blocking` feature, which pulls in `tokio` with just the
//! `rt` feature. The default runtime dependency set (reqwest/serde/serde_json/bytes/secrecy) is
//! unchanged — a client built without the feature carries no tokio direct dependency and no
//! blocking client at all.
//!
//! # Nested-runtime caveat
//!
//! A [`BlockingRuntime`] must NOT be constructed or used from inside another async runtime:
//! tokio's `block_on` panics when called from within a runtime's context. Build and drive one on a
//! plain (non-async) thread — for example a `std::thread`, or `tokio::task::spawn_blocking` when
//! you are already inside an async context.

/// A current-thread tokio runtime used to drive async operation futures to completion
/// synchronously. Held by the generated blocking client alongside the async client it wraps.
#[derive(Debug)]
pub struct BlockingRuntime {
    runtime: tokio::runtime::Runtime,
}

impl BlockingRuntime {
    /// Build a fresh current-thread runtime with all available drivers enabled. Returns the
    /// underlying I/O error if the OS refuses to create the runtime's resources.
    pub fn new() -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(Self { runtime })
    }

    /// Drive a future to completion on the owned runtime, blocking the current thread until it
    /// resolves.
    ///
    /// # Panics
    ///
    /// Panics if called from within another async runtime — tokio forbids nested `block_on`.
    pub fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }
}

#[cfg(test)]
mod tests {
    use super::BlockingRuntime;

    #[test]
    fn block_on_drives_a_future_to_completion() {
        let runtime = BlockingRuntime::new().expect("current-thread runtime builds");
        let value = runtime.block_on(async { 2 + 2 });
        assert_eq!(value, 4);
    }

    #[test]
    fn block_on_is_reusable_across_calls() {
        // One owned runtime drives many operation futures over the client's lifetime.
        let runtime = BlockingRuntime::new().expect("current-thread runtime builds");
        assert_eq!(runtime.block_on(async { 1 }), 1);
        assert_eq!(runtime.block_on(async { 2 }), 2);
    }
}
