//! Task spawning helpers that set up observer instrumentation.
//!
//! Use [`spawn`] or [`spawn_named`] instead of `tokio::spawn` to ensure
//! each task has an initialised held-lock stack and task-local metadata.
//! [`named`] wraps an existing future with instrumentation without spawning.
use crate::observer::instrument;
use std::fmt::Display;
use std::panic::Location;

/// Identifies a task for diagnostics — either a human-provided name or
/// an auto-captured spawn location + tokio task id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum TaskId {
    /// A human-provided name (via [`spawn_named`] or [`named`]).
    Named(&'static str),
    /// Auto-captured from the spawn call site and/or tokio task id.
    Unnamed {
        /// The source location where the task was spawned.
        location: Option<&'static Location<'static>>,
        /// The tokio-assigned task id, if available.
        id: Option<tokio::task::Id>,
    },
    /// No task context (e.g. called outside a spawned task).
    None,
}

impl TaskId {
    /// Create a named task id.
    pub fn named(name: &'static str) -> Self {
        TaskId::Named(name)
    }

    /// Create a task id from a source location (used by [`spawn`]).
    pub fn created_at(loc: &'static Location<'static>) -> Self {
        TaskId::Unnamed {
            location: Some(loc),
            id: tokio::task::try_id(),
        }
    }

    /// Create an unnamed task id from the current tokio task, if any.
    pub fn unnamed() -> Self {
        match tokio::task::try_id() {
            Some(id) => TaskId::Unnamed {
                location: None,
                id: Some(id),
            },
            None => TaskId::None,
        }
    }
}

impl Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskId::Named(name) => name.fmt(f),
            TaskId::Unnamed { location, id } => {
                write!(f, "task")?;
                if let Some(id) = id {
                    write!(f, " id={}", id)?;
                }
                if let Some(loc) = location {
                    write!(f, " created at {}", loc)?;
                }
                Ok(())
            }
            TaskId::None => f.write_str("root task"),
        }
    }
}

impl From<&'static str> for TaskId {
    fn from(name: &'static str) -> Self {
        TaskId::Named(name)
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::unnamed()
    }
}

tokio::task_local! {
    /// Metadata of the currently executing task (set by `spawn_named`, `named`, or `spawn`).
    static CURRENT_TASK: TaskId;
}

/// Returns the [`TaskId`] of the current instrumentation scope, or
/// `TaskId::None` if called outside any scope.
pub fn current_task() -> TaskId {
    CURRENT_TASK.try_with(|n| *n).unwrap_or_default()
}

/// Wrap a future with observer instrumentation under a given name,
/// without spawning a new tokio task.
pub fn named<F, R>(name: &'static str, f: F) -> impl Future<Output = R>
where
    F: Future<Output = R>,
{
    CURRENT_TASK.scope(TaskId::Named(name), instrument(f))
}

/// Spawn a named tokio task with observer instrumentation.
pub fn spawn_named<F, R>(name: &'static str, f: F) -> tokio::task::JoinHandle<R>
where
    F: Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    let id = TaskId::Named(name);
    tokio::task::Builder::new()
        .name(name)
        .spawn(CURRENT_TASK.scope(id, instrument(f)))
        .expect("failed to spawn task")
}

/// Spawn a tokio task with observer instrumentation, identified by call site.
#[track_caller]
pub fn spawn<F, R>(f: F) -> tokio::task::JoinHandle<R>
where
    F: Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    let caller = Location::caller();
    tokio::task::spawn(async move {
        let id = TaskId::created_at(caller);
        CURRENT_TASK.scope(id, instrument(f)).await
    })
}
