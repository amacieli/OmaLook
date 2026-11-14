//! The one process-wide tokio runtime.
//!
//! Every synchronous entry point (HTTP handlers on their connection thread,
//! daemon loops on their own OS thread, tests) that needs to drive an async
//! provider call goes through [`block_on`]. Nothing else in the crate may build
//! a tokio runtime: a runtime per call costs a thread pool per request and
//! dropping one inside an async context panics.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::{Builder, Handle, Runtime, RuntimeFlavor};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .thread_name("omalook-rt")
            .build()
            .expect("omalook: cannot start the shared tokio runtime")
    })
}

/// Handle to the shared runtime. Builds it on first use.
pub fn handle() -> Handle {
    runtime().handle().clone()
}

/// Drive `f` to completion from synchronous code.
///
/// * Not inside a runtime (the normal case: HTTP connection threads, daemon
///   threads, plain tests): `Handle::block_on` on the shared runtime. Many
///   threads may call it at once. `f` need not be `Send`.
/// * Inside the shared multi-thread runtime (a worker thread): `block_in_place`
///   hands the worker's core away so `Handle::block_on` may run the future
///   without stalling the pool. `f` need not be `Send`.
/// * Inside a current-thread runtime: blocking that thread would stall the
///   caller's own runtime, and `block_in_place` panics there. Building a
///   runtime there panicked before this module existed too, so this is not a
///   regression. Use [`block_on_send`] there instead.
///
/// Panics (with a message naming `block_on_send`) only in the third case.
pub fn block_on<F: Future>(f: F) -> F::Output {
    match Handle::try_current() {
        Err(_) => handle().block_on(f),
        Ok(current) => match current.runtime_flavor() {
            RuntimeFlavor::MultiThread => tokio::task::block_in_place(|| current.block_on(f)),
            _ => panic!("runtime::block_on called inside a current-thread runtime; use runtime::block_on_send"),
        },
    }
}

/// Like [`block_on`], but also works inside a current-thread runtime.
///
/// That case cannot block the calling thread (it would stall its own runtime),
/// so `f` runs on a fresh scoped OS thread that is joined before returning.
/// This is why `F` must be `Send`. Use it only from tests or code that is
/// known to run inside a current-thread runtime; otherwise prefer [`block_on`].
pub fn block_on_send<F>(f: F) -> F::Output
where
    F: Future + Send,
    F::Output: Send,
{
    match Handle::try_current() {
        Ok(current) if current.runtime_flavor() == RuntimeFlavor::CurrentThread => {
            std::thread::scope(|scope| {
                let worker = scope.spawn(|| handle().block_on(f));
                match worker.join() {
                    Ok(out) => out,
                    Err(panic) => std::panic::resume_unwind(panic),
                }
            })
        }
        _ => block_on(f),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn thirty_two_threads_block_on_concurrently_and_all_finish() {
        let done = std::sync::Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..32)
            .map(|i| {
                let done = done.clone();
                std::thread::spawn(move || {
                    let v = block_on(async move {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        i * 2
                    });
                    assert_eq!(v, i * 2);
                    done.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();
        for t in threads {
            t.join().expect("a block_on thread panicked");
        }
        assert_eq!(done.load(Ordering::SeqCst), 32);
    }

    #[test]
    fn nested_block_on_inside_the_shared_runtime_does_not_panic() {
        let out = handle().block_on(async {
            // We are inside a runtime context now; this must not panic.
            block_on(async { tokio::time::sleep(Duration::from_millis(1)).await; 7 })
        });
        assert_eq!(out, 7);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn block_on_send_inside_a_current_thread_runtime_does_not_panic() {
        let out = block_on_send(async { 11 });
        assert_eq!(out, 11);
    }

    #[test]
    fn block_on_on_a_shared_worker_thread_does_not_panic() {
        // A future spawned onto the shared runtime runs on a worker thread,
        // where block_in_place is the path taken.
        let got = handle().block_on(async {
            handle()
                .spawn(async { block_on(async { 5 }) })
                .await
                .expect("spawned task panicked")
        });
        assert_eq!(got, 5);
    }
}
