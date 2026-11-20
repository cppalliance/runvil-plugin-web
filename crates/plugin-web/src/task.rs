//! A request as a task on the Plugin's runtime: [`run`] spawns it there,
//! lets the call that made it wait for it, and aborts it when that call is
//! dropped.
//!
//! The Host polls a call on a thread of its own, which has no context of
//! the Plugin's runtime, and an HTTP client needs that context for its
//! sockets, its timers, and its host lookups. So a call sends no request
//! itself. It hands the request to [`run`] and awaits the task's handle,
//! which needs no context and wakes the call when the task ends.

use std::future::Future;

use tokio::runtime::Handle;
use tokio::task::JoinHandle;

/// Runs `request` as a task on `runtime` and returns what it returns, or
/// `None` when the task did not come to its end: it panicked, or the
/// runtime shut down first.
///
/// The call that awaits this is parked until the task ends, which wakes
/// it. Dropping the returned future aborts the task, so the request of a
/// call the Host abandons stops, and its connection closes with it.
pub(crate) async fn run<T: Send + 'static>(
    runtime: &Handle,
    request: impl Future<Output = T> + Send + 'static,
) -> Option<T> {
    let mut task = AbortOnDrop(runtime.spawn(request));
    (&mut task.0).await.ok()
}

/// A spawned task, aborted when the call awaiting it is dropped.
struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::runtime::Handle;
    use tokio::sync::oneshot;

    use super::run;

    /// A request that panics partway, before it answers.
    async fn panics() -> u8 {
        tokio::task::yield_now().await;
        panic!("the request panics")
    }

    #[tokio::test]
    async fn a_task_answers_what_its_request_returns() {
        assert_eq!(run(&Handle::current(), async { 7 }).await, Some(7));
    }

    #[tokio::test]
    async fn a_task_that_panics_is_none() {
        assert_eq!(run(&Handle::current(), panics()).await, None);
    }

    /// A runtime that has shut down drops a task as it is spawned, so the
    /// task never runs.
    #[tokio::test]
    async fn a_task_on_a_runtime_that_has_shut_down_is_none() {
        let gone = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime builds");
        let handle = gone.handle().clone();
        gone.shutdown_background();
        assert_eq!(run(&handle, async { 7 }).await, None);
    }

    #[tokio::test]
    async fn dropping_the_wait_for_a_task_aborts_it() {
        let (started, running) = oneshot::channel();
        // The task holds `held` until the task itself is dropped, which
        // only an abort does, because the task never ends.
        let (held, dropped) = oneshot::channel::<()>();
        let runtime = Handle::current();
        let wait = run(&runtime, async move {
            let _held = held;
            let _ = started.send(());
            std::future::pending::<()>().await;
        });
        tokio::select! {
            ended = wait => panic!("a task that never ends has no result: {ended:?}"),
            started = running => started.expect("the task runs"),
        }
        tokio::time::timeout(Duration::from_secs(5), dropped)
            .await
            .expect("dropping the wait aborts the task, which drops what it held")
            .expect_err("the task sent nothing before it was dropped");
    }
}
