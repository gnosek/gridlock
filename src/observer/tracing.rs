use crate::observer::{Resource, ResourceObserver};
use std::future::Future;
use std::panic::Location;

/// Emits `tracing::trace!` events for every resource lifecycle event.
#[derive(Clone, Copy)]
pub struct TracingObserver;
impl ResourceObserver for TracingObserver {
    #[inline(always)]
    fn instrument<F>(fut: F) -> impl Future<Output = F::Output>
    where
        F: Future,
    {
        async move {
            let name = crate::task::current_task();
            let id = tokio::task::try_id().map(tracing::field::display);
            tracing::trace!(task = %name, task_id = id, "starting");
            let res = fut.await;
            tracing::trace!(task = %name, task_id = id, "finished");
            res
        }
    }

    fn on_waiting(&self, resource: &Resource, caller: &'static Location<'static>) {
        let task = crate::task::current_task();
        tracing::trace!(
            resource = %resource,
            task = %task,
            caller = %caller,
            "acquiring",
        );
    }
    fn on_acquired(&self, resource: &Resource, caller: &'static Location<'static>) {
        let task = crate::task::current_task();
        tracing::trace!(
            resource = %resource,
            task = %task,
            caller = %caller,
            "acquired",
        );
    }

    fn on_released(&self, resource: &Resource, caller: &'static Location<'static>) {
        let task = crate::task::current_task();
        tracing::trace!(
            resource = %resource,
            released_by = %task,
            caller = %caller,
            "released",
        );
    }
}
