//! A synchronous facade for callers without an async runtime.
//!
//! reqwest's async `Client` needs a running reactor to drive its IO, so a hand-rolled `block_on`
//! cannot execute a request. [`BlockingRuntime`] instead owns a real current-thread tokio runtime
//! and drives the generated async operation futures to completion on it — the blocking client is a
//! thin shim that reuses every line of the async dispatch, adding no logic of its own.
//!
//! This module is compiled only under the `blocking` feature on a non-wasm32 target, and pulls in
//! `tokio` with just the `rt` feature. A client built without the feature, or for wasm32, carries
//! no tokio dependency and no blocking client at all.
//!
//! # Nested-runtime caveat
//!
//! A [`BlockingRuntime`] must NOT be *driven* from inside another async runtime: tokio's
//! `block_on` panics when called from within a runtime's context, and so does
//! [`BlockingRuntime::block_on`]; [`BlockingRuntime::try_block_on`] returns [`NestedRuntime`]
//! there instead. Drive one on a plain (non-async) thread — for example a `std::thread`, or
//! `tokio::task::spawn_blocking` when you are already inside an async context. Building one inside an async runtime is fine, and so is dropping one
//! there: the drop shuts the owned runtime down in the background rather than waiting on it, which
//! inside an async context would panic, so a blocking client held in async state can be dropped at
//! shutdown.

/// A current-thread tokio runtime used to drive async operation futures to completion
/// synchronously. Held by the generated blocking client alongside the async client it wraps.
///
/// Dropping it shuts the runtime down without waiting for its blocking-pool threads (tokio's
/// `Runtime::shutdown_background`), so the drop never blocks and never panics, whatever context
/// it runs in.
#[derive(Debug)]
pub struct BlockingRuntime {
    /// The owned runtime; `Some` until the drop takes it to shut it down.
    runtime: Option<tokio::runtime::Runtime>,
}

impl BlockingRuntime {
    /// Build a fresh current-thread runtime with all available drivers enabled. Returns the
    /// underlying I/O error if the OS refuses to create the runtime's resources.
    pub fn new() -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(Self {
            runtime: Some(runtime),
        })
    }

    /// Drive a future to completion on the owned runtime, blocking the current thread until it
    /// resolves.
    ///
    /// # Panics
    ///
    /// Panics if called from within another async runtime — tokio forbids nested `block_on`.
    /// [`try_block_on`](Self::try_block_on) returns [`NestedRuntime`] there instead.
    pub fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        // Only the drop takes the runtime, and nothing borrows `self` after it begins.
        let Some(runtime) = &self.runtime else {
            unreachable!("the runtime is taken only by the drop")
        };
        runtime.block_on(future)
    }

    /// Drive a future to completion on the owned runtime, as [`block_on`](Self::block_on) does,
    /// unless the current thread is inside a tokio runtime's context.
    ///
    /// # Errors
    ///
    /// Returns [`NestedRuntime`] without polling `future` when called from within a tokio runtime's
    /// context (an async task, or a thread that entered a runtime), where `block_on` would panic.
    pub fn try_block_on<F: std::future::Future>(
        &self,
        future: F,
    ) -> Result<F::Output, NestedRuntime> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(NestedRuntime { _private: () });
        }
        Ok(self.block_on(future))
    }
}

/// The error [`BlockingRuntime::try_block_on`] returns when called from within a tokio runtime's
/// context, where driving another runtime would panic. Drive the blocking client from a plain
/// thread instead, for example inside `tokio::task::spawn_blocking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NestedRuntime {
    /// Private: only `try_block_on` builds one.
    _private: (),
}

impl std::fmt::Display for NestedRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cannot block on a future from within an async runtime")
    }
}

impl std::error::Error for NestedRuntime {}

impl Drop for BlockingRuntime {
    /// Shut the runtime down without waiting: a plain drop of a tokio runtime waits on its
    /// blocking pool, which panics inside an async context.
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockingRuntime, NestedRuntime};

    #[test]
    fn try_block_on_drives_a_future_outside_a_runtime() {
        let runtime = BlockingRuntime::new().expect("current-thread runtime builds");
        assert_eq!(runtime.try_block_on(async { 1 }), Ok(1));
    }

    /// Inside another runtime's context `block_on` panics; `try_block_on` returns the typed
    /// error instead, without polling the future.
    #[test]
    fn try_block_on_refuses_inside_a_runtime_context() {
        let outer = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("current-thread runtime builds");
        let _entered = outer.enter();
        let runtime = BlockingRuntime::new().expect("current-thread runtime builds");
        let polled = std::cell::Cell::new(false);
        let result = runtime.try_block_on(async { polled.set(true) });
        assert_eq!(result, Err(NestedRuntime { _private: () }));
        assert!(!polled.get());
        let error: &dyn std::error::Error = &result.unwrap_err();
        assert_eq!(
            error.to_string(),
            "cannot block on a future from within an async runtime"
        );
    }

    /// The same refusal from inside an async task, the context a blocking client called from an
    /// async handler runs in.
    #[test]
    fn try_block_on_refuses_inside_an_async_task() {
        let outer = BlockingRuntime::new().expect("current-thread runtime builds");
        let inner = BlockingRuntime::new().expect("current-thread runtime builds");
        let result = outer.block_on(async { inner.try_block_on(async { 1 }) });
        assert!(result.is_err());
    }

    #[test]
    fn block_on_drives_a_future_to_completion() {
        let runtime = BlockingRuntime::new().expect("current-thread runtime builds");
        let value = runtime.block_on(async { 2 + 2 });
        assert_eq!(value, 4);
    }

    /// `enable_all` wires up the timer driver: a future that waits on tokio's clock completes,
    /// where a runtime without it would panic on the first poll.
    #[test]
    fn block_on_drives_a_timer_future() {
        let runtime = BlockingRuntime::new().expect("current-thread runtime builds");
        let wait = std::time::Duration::from_millis(5);
        let started = std::time::Instant::now();
        let value = runtime.block_on(async {
            tokio::time::sleep(wait).await;
            7
        });
        assert_eq!(value, 7);
        assert!(started.elapsed() >= wait);
    }

    #[test]
    fn block_on_is_reusable_across_calls() {
        // One owned runtime drives many operation futures over the client's lifetime.
        let runtime = BlockingRuntime::new().expect("current-thread runtime builds");
        assert_eq!(runtime.block_on(async { 1 }), 1);
        assert_eq!(runtime.block_on(async { 2 }), 2);
    }

    /// A blocking client held in async state is dropped inside that runtime's context, where
    /// blocking is not allowed; the drop must not wait on the owned runtime's blocking pool.
    #[test]
    fn dropping_inside_an_async_context_does_not_panic() {
        let inner = BlockingRuntime::new().expect("current-thread runtime builds");
        // A blocking task still running keeps a waiting shutdown from finishing; the drop below
        // returns without it.
        let (release, parked) = std::sync::mpsc::channel::<()>();
        inner.block_on(async {
            // Detached: the task outlives this future and is still parked at the drop.
            drop(tokio::task::spawn_blocking(move || {
                let _ = parked.recv();
            }));
        });
        let outer = BlockingRuntime::new().expect("current-thread runtime builds");
        outer.block_on(async move { drop(inner) });
        release.send(()).ok();
    }
}
