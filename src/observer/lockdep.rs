//! **lockdep** — static lock-ordering validator, inspired by the Linux kernel's lockdep.
//!
//! Every time a task acquires lock **B** while already holding lock **A**, the
//! dependency edge **A → B** is recorded (together with the call-sites where each
//! was acquired).  If adding a new edge would create a cycle in the global
//! ordering graph, a warning is emitted *immediately* — even if the current
//! execution would not actually deadlock.
//!
//! This catches *potential* deadlocks that depend on scheduling order, without
//! requiring them to manifest at runtime.
use crate::observer::{Resource, ResourceObserver};
use crate::task::TaskId;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter, Write};
use std::future::Future;
use std::panic::Location;
use std::sync::LazyLock;

type CallSite = &'static Location<'static>;

// ─── Per-task held-lock stack ───────────────────────────────────────────────
tokio::task_local! {
    /// The locks currently held by this task, most-recently-acquired last.
    /// Each entry is `(resource, call_site_where_acquired)`.
    static HELD_LOCKS: std::cell::RefCell<Vec<(Resource, CallSite)>>;
}
/// Run `f` with an initialised held-lock stack.
/// If the stack is already initialised (nested call), just run `f` directly.
fn with_held_locks<R>(
    f: impl FnOnce(&std::cell::RefCell<Vec<(Resource, CallSite)>>) -> R,
) -> Option<R> {
    HELD_LOCKS.try_with(|cell| f(cell)).ok()
}
// ─── Dependency edge ────────────────────────────────────────────────────────

/// How an ordering edge was established.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CallChainLink {
    /// An implicit edge added by the framework (e.g. bounded-channel back-pressure).
    Implicit,
    /// An edge witnessed at runtime when a task held one resource and acquired another.
    Explicit {
        /// Where `from` was acquired.
        from_site: CallSite,
        /// Where `to` was acquired.
        to_site: CallSite,
        /// The task that first witnessed this ordering.
        task: TaskId,
    },
}

/// A recorded ordering: lock `from` was held (at `from_site`) when lock `to`
/// was acquired (at `to_site`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DepEdge {
    /// The resource that was already held.
    pub from: Resource,
    /// The resource that was being acquired.
    pub to: Resource,
    /// Evidence of how the edge was established.
    pub link: CallChainLink,
}

impl Display for DepEdge {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.link {
            CallChainLink::Implicit => write!(f, "implicit"),
            CallChainLink::Explicit {
                from_site: from,
                to_site: to,
                task,
            } => {
                write!(
                    f,
                    "{}\n  held {} at {}\n  acquired {} at {}",
                    task, self.from, from, self.to, to
                )
            }
        }
    }
}

// ─── Global ordering graph ─────────────────────────────────────────────────
/// The accumulated lock-ordering graph.
pub struct LockDepGraph {
    /// All observed ordering edges.  The key is `(from, to)` for quick
    /// adjacency lookup; the value stores call-site evidence.
    edges: std::sync::Mutex<BTreeMap<(Resource, Resource), DepEdge>>,
    /// Implicit edges (e.g. bounded channel tx depends on rx draining).
    implicit: std::sync::Mutex<Vec<(Resource, Resource, &'static str)>>,
}

impl LockDepGraph {
    /// Create an empty graph.
    pub fn new() -> Self {
        Self {
            edges: std::sync::Mutex::new(BTreeMap::new()),
            implicit: std::sync::Mutex::new(Vec::new()),
        }
    }
    /// Register an implicit ordering edge (e.g. bounded-channel tx → rx).
    pub fn add_implicit_edge(&self, from: Resource, to: Resource, label: &'static str) {
        self.implicit.lock().unwrap().push((from, to, label));
    }
    /// Record an explicit ordering edge and check for cycles.
    ///
    /// Returns `Some(path)` if the new edge closes a cycle.
    pub fn record_and_check(
        &self,
        from: Resource,
        to: Resource,
        from_site: &'static Location<'static>,
        to_site: &'static Location<'static>,
        task: &TaskId,
    ) -> Option<Vec<DepEdge>> {
        let mut edges = self.edges.lock().unwrap();
        edges.entry((from, to)).or_insert_with(|| DepEdge {
            from,
            to,
            link: CallChainLink::Explicit {
                from_site,
                to_site,
                task: *task,
            },
        });
        let implicit = self.implicit.lock().unwrap();
        let mut all_edges = edges.clone();
        for &(ifrom, ito, _label) in implicit.iter() {
            all_edges.entry((ifrom, ito)).or_insert_with(|| DepEdge {
                from: ifrom,
                to: ito,
                link: CallChainLink::Implicit,
            });
        }
        Self::find_cycle(&all_edges, from, to)
    }
    fn find_cycle(
        edges: &BTreeMap<(Resource, Resource), DepEdge>,
        target: Resource,
        start: Resource,
    ) -> Option<Vec<DepEdge>> {
        let mut adj: BTreeMap<Resource, Vec<(&Resource, &DepEdge)>> = BTreeMap::new();
        for ((from, to), edge) in edges {
            adj.entry(*from).or_default().push((to, edge));
        }
        let mut stack: Vec<(&Resource, Vec<DepEdge>)> = vec![(&start, Vec::new())];
        let mut visited = BTreeSet::new();
        while let Some((node, path)) = stack.pop() {
            if *node == target && !path.is_empty() {
                return Some(path);
            }
            if !visited.insert(node) {
                continue;
            }
            if let Some(neighbours) = adj.get(node) {
                for &(next, edge) in neighbours {
                    let mut new_path = path.clone();
                    new_path.push(edge.clone());
                    stack.push((next, new_path));
                }
            }
        }
        None
    }
    /// Return a snapshot of all explicit edges.
    pub fn edges(&self) -> Vec<DepEdge> {
        self.edges.lock().unwrap().values().cloned().collect()
    }
    /// Render the full ordering graph (explicit + implicit) as a Graphviz DOT string.
    pub fn to_dot(&self) -> String {
        let edges = self.edges.lock().unwrap();
        let implicit = self.implicit.lock().unwrap();
        let mut all_edges = edges.clone();
        let mut implicit_labels: BTreeMap<(Resource, Resource), &str> = BTreeMap::new();
        for &(ifrom, ito, label) in implicit.iter() {
            all_edges.entry((ifrom, ito)).or_insert_with(|| DepEdge {
                from: ifrom,
                to: ito,
                link: CallChainLink::Implicit,
            });
            implicit_labels.insert((ifrom, ito), label);
        }
        let mut resources = BTreeSet::new();
        for (from, to) in all_edges.keys() {
            resources.insert(*from);
            resources.insert(*to);
        }
        let mut cycle_pairs: BTreeSet<(Resource, Resource)> = BTreeSet::new();
        for &(from, to) in all_edges.keys() {
            if let Some(path) = Self::find_cycle(&all_edges, from, to) {
                for e in &path {
                    cycle_pairs.insert((e.from, e.to));
                }
                cycle_pairs.insert((from, to));
            }
        }
        let has_cycle = !cycle_pairs.is_empty();
        let mut dot = String::from("digraph lockdep {\n");
        let _ = writeln!(dot, "    rankdir=LR;");
        let _ = writeln!(
            dot,
            "    node [shape=ellipse, style=filled, fillcolor=\"#ffffcc\"];"
        );
        let _ = writeln!(dot);
        if has_cycle {
            let _ = writeln!(
                dot,
                "    label=\"⚠ POTENTIAL DEADLOCK (resource ordering cycle)\";"
            );
            let _ = writeln!(dot, "    labelloc=t;");
            let _ = writeln!(dot, "    fontcolor=red;");
            let _ = writeln!(dot, "    fontsize=18;");
            let _ = writeln!(dot);
        }
        for r in &resources {
            let _ = writeln!(dot, "    \"{r}\";");
        }
        let _ = writeln!(dot);
        for edge in all_edges.values() {
            let in_cycle = cycle_pairs.contains(&(edge.from, edge.to));
            let color = if in_cycle { "red" } else { "\"#333333\"" };
            let penwidth = if in_cycle { "2.5" } else { "1.0" };

            let mut label = edge.to_string();
            if let Some(implicit_label) = implicit_labels.get(&(edge.from, edge.to)) {
                label = format!("{}\n({})", label, implicit_label);
            }

            label = label.replace("\n", "\\n");
            let _ = writeln!(
                dot,
                "    \"{}\" -> \"{}\" [label=\"{}\", color={color}, fontcolor={color}, fontsize=9, penwidth={penwidth}];",
                edge.from, edge.to, label,
            );
        }
        dot.push_str("}\n");
        dot
    }
}
// ─── Observer ───────────────────────────────────────────────────────────────
static LOCKDEP_GRAPH: LazyLock<LockDepGraph> = LazyLock::new(LockDepGraph::new);

// ─── Helper: run a future either inside a new HELD_LOCKS scope or directly ──
use std::pin::Pin;
use std::task::{Context, Poll};

/// Returned by `LockDepObserver::instrument`.  When `already_instrumented` is
/// `true` the inner future is polled directly; otherwise it is wrapped in a
/// fresh `HELD_LOCKS` task-local scope so the held-lock stack is initialised.
#[pin_project::pin_project(project = EitherProj)]
enum EitherFuture<F: Future> {
    /// Task already has a HELD_LOCKS scope — poll the future directly.
    Pass(#[pin] F),
    /// Fresh scope — poll through the tokio task-local scope future.
    Scoped(
        #[pin]
        tokio::task::futures::TaskLocalFuture<std::cell::RefCell<Vec<(Resource, CallSite)>>, F>,
    ),
}

impl<F: Future> EitherFuture<F> {
    fn new(already_instrumented: bool, fut: F) -> Self {
        if already_instrumented {
            EitherFuture::Pass(fut)
        } else {
            EitherFuture::Scoped(HELD_LOCKS.scope(std::cell::RefCell::new(Vec::new()), fut))
        }
    }
}

impl<F: Future> Future for EitherFuture<F> {
    type Output = F::Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project() {
            EitherProj::Pass(f) => f.poll(cx),
            EitherProj::Scoped(f) => f.poll(cx),
        }
    }
}
/// Lock-ordering observer inspired by the Linux kernel's lockdep.
///
/// Records `A → B` edges whenever a task acquires `B` while holding `A`.
/// Logs an error and writes a `.dot` file when a cycle is detected.
#[derive(Clone, Copy, Default)]
pub struct LockDepObserver;
impl LockDepObserver {
    /// Access the global lock-ordering graph.
    pub fn graph() -> &'static LockDepGraph {
        &LOCKDEP_GRAPH
    }
    /// Register an implicit `tx → rx` edge for a bounded channel.
    pub fn register_channel(tx_id: Resource, rx_id: Resource) {
        LOCKDEP_GRAPH.add_implicit_edge(tx_id, rx_id, "bounded channel");
    }
    /// Register an implicit `rx → tx` edge for a oneshot channel.
    pub fn register_oneshot(rx_id: Resource, tx_id: Resource) {
        LOCKDEP_GRAPH.add_implicit_edge(rx_id, tx_id, "oneshot");
    }
}
impl ResourceObserver for LockDepObserver {
    fn instrument<F>(fut: F) -> impl Future<Output = F::Output>
    where
        F: Future,
    {
        // If HELD_LOCKS is already set (nested instrumentation), run the
        // future as-is so we don't shadow the existing held-lock stack.
        let already_instrumented = HELD_LOCKS.try_with(|_| ()).is_ok();
        tracing::info!(
            "Instrumenting future with LockDepObserver, nested: {}",
            already_instrumented
        );
        EitherFuture::new(already_instrumented, fut)
    }
    fn on_waiting(&self, resource: &Resource, caller: &'static Location<'static>) {
        with_held_locks(|cell| {
            let task = crate::task::current_task();
            let held = cell.borrow();
            for &(held_name, held_site) in held.iter() {
                if let Some(cycle) =
                    LOCKDEP_GRAPH.record_and_check(held_name, *resource, held_site, caller, &task)
                {
                    let chain: Vec<String> = cycle.iter().map(|e| e.to_string()).collect();

                    let new_edge = DepEdge {
                        from: held_name,
                        to: *resource,
                        link: CallChainLink::Explicit {
                            from_site: held_site,
                            to_site: caller,
                            task,
                        },
                    };

                    tracing::error!(
                        resource = %resource,
                        task = %task,
                        caller_file = %caller.file(),
                        caller_line = %caller.line(),
                        caller_col = %caller.column(),
                        "⚠ POTENTIAL DEADLOCK: lock ordering cycle detected!\n\
                         Existing ordering:\n* {}\n\
                         New edge that closes the cycle:\n* \
                         {new_edge}\n",
                        chain.join("\n* "),
                    );
                    let mut cycle: Vec<Resource> = cycle
                        .iter()
                        .flat_map(|e| [e.from, e.to])
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect();
                    if !cycle.contains(resource) {
                        cycle.push(*resource);
                    }

                    let mut filename = String::from("lockdep-cycle");
                    for resource in cycle.iter() {
                        filename.push('-');
                        filename.push_str(resource.slug().as_str());
                    }
                    filename.push_str(".dot");

                    let dot = LOCKDEP_GRAPH.to_dot();
                    match std::fs::write(&filename, &dot) {
                        Ok(()) => tracing::info!(filename, "wrote lock-ordering graph"),
                        Err(e) => {
                            tracing::error!(filename, "failed to write lock-ordering graph: {e}")
                        }
                    }
                }
            }
        });
    }
    fn on_acquired(&self, resource: &Resource, caller: &'static Location<'static>) {
        with_held_locks(|cell| {
            cell.borrow_mut().push((*resource, caller));
        });
    }
    fn on_released(&self, resource: &Resource, _caller: &'static Location<'static>) {
        with_held_locks(|cell| {
            let mut held = cell.borrow_mut();
            if let Some(pos) = held.iter().position(|(name, _)| *name == *resource) {
                held.remove(pos);
            }
        });
    }
}
