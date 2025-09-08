use anyhow::{Context, Error, Result};
use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap, HashSet, VecDeque},
    fmt::{self},
    future::Future,
    pin::Pin,
    sync::Arc,
};
use tokio::sync::mpsc;

type TaskFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
type TaskFn = Arc<dyn Fn() -> TaskFuture + Send + Sync>;

#[derive(Clone)]
pub struct GraphNode {
    pub priority: i32,
    pub dependencies: Vec<usize>,
    pub exec: TaskFn,
}

// Pretty-print without touching `exec`
impl fmt::Debug for GraphNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HashableGraphNode")
            .field("priority", &self.priority)
            .field("dependencies", &self.dependencies)
            .finish_non_exhaustive()
    }
}

impl GraphNode {
    pub fn new<F>(priority: i32, dependencies: Vec<usize>, exec: F) -> Self
    where
        F: Fn() -> TaskFuture + Send + Sync + 'static,
    {
        GraphNode {
            priority,
            dependencies,
            exec: Arc::new(exec),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HeapNode {
    priority: i32,
    index: usize,
}

// Max-heap: higher priority first; for ties, smaller index first (deterministic)
impl Ord for HeapNode {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority
            .cmp(&other.priority)
            .reverse()
            .then_with(|| self.index.cmp(&other.index))
    }
}

impl PartialOrd for HeapNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub struct TaskRunner {
    nodes: Vec<GraphNode>,
    reverse_dependencies: HashMap<usize, Vec<usize>>, // index -> Vec of dependent indices
    levels: HashMap<usize, i32>,                      // index -> level
    task_queue: BinaryHeap<HeapNode>,
    running_tasks: HashSet<usize>,   // indices of running tasks
    completed_tasks: HashSet<usize>, // indices of completed tasks
    waiting_tasks: HashSet<usize>,   // indices of tasks waiting for dependencies
    concurrency_limit: usize,        // max number of concurrent tasks
}

impl TaskRunner {
    pub fn new() -> Self {
        TaskRunner {
            nodes: Vec::new(),
            reverse_dependencies: HashMap::new(),
            levels: HashMap::new(),
            task_queue: BinaryHeap::new(),
            running_tasks: HashSet::new(),
            completed_tasks: HashSet::new(),
            waiting_tasks: HashSet::new(),
            concurrency_limit: 1, // default to 1 for simplicity
        }
    }

    pub fn with_concurrency(mut self, n: usize) -> Self {
        self.concurrency_limit = n.max(1);
        self
    }

    pub fn add_task(&mut self, node: GraphNode) -> Result<usize, Error> {
        let id = self.nodes.len();
        let priority = node.priority;

        // add to graph first

        let mut max_dep_level = -1;
        for &dep_index in &node.dependencies {
            if dep_index >= self.nodes.len() {
                anyhow::bail!("Dependency node not found in the graph");
            }

            if let Some(&dep_level) = self.levels.get(&dep_index) {
                max_dep_level = max_dep_level.max(dep_level);
            }
        }

        // All validations passed - now update the graph atomically
        self.nodes.push(node);
        self.levels.insert(id, max_dep_level + 1);
        self.reverse_dependencies.insert(id, Vec::new());

        for &dep_index in &self.nodes.get(id).unwrap().dependencies {
            self.reverse_dependencies
                .entry(dep_index)
                .or_default()
                .push(id);
        }

        // now it's safe to check deps (or compute from the node we just had)
        let ready = self.are_dependencies_met(&id);

        if ready {
            self.task_queue.push(HeapNode {
                priority,
                index: id,
            });
        } else {
            self.waiting_tasks.insert(id);
        }

        Ok(id)
    }

    fn are_dependencies_met(&self, index: &usize) -> bool {
        self.nodes
            .get(*index)
            .map(|node| {
                node.dependencies
                    .iter()
                    .all(|dep| self.completed_tasks.contains(dep))
            })
            .unwrap_or_default()
    }

    fn requeue_waiting_ready(&mut self) {
        // collect first to avoid borrow issues
        let ready_now: Vec<usize> = self
            .waiting_tasks
            .iter()
            .copied()
            .filter(|h| self.are_dependencies_met(h))
            .collect();

        for index in ready_now {
            let prio = self.nodes.get(index).unwrap().priority;
            self.waiting_tasks.remove(&index);
            self.task_queue.push(HeapNode {
                priority: prio,
                index,
            });
        }
    }

    /// Returns Ok(()) if no deadlock.
    /// Returns Err(anyhow) with a clear message if we detect:
    ///  - Unsatisfied/missing deps
    ///  - A dependency cycle among waiting tasks
    fn detect_deadlock(&self) -> anyhow::Result<()> {
        // Not a deadlock if there's still work running or ready
        if !self.running_tasks.is_empty() || !self.task_queue.is_empty() {
            return Ok(());
        }
        // Not a deadlock if nothing is waiting
        if self.waiting_tasks.is_empty() {
            return Ok(());
        }

        // Missing / unsatisfied deps: dep not in completed or waiting
        let mut missing: Vec<(usize, Vec<usize>)> = Vec::new();
        for &id in &self.waiting_tasks {
            let Some(node) = self.nodes.get(id) else {
                missing.push((id, vec![]));
                continue;
            };
            let miss: Vec<usize> = node
                .dependencies
                .iter()
                .copied()
                .filter(|d| !self.completed_tasks.contains(d) && !self.waiting_tasks.contains(d))
                .collect();
            if !miss.is_empty() {
                missing.push((id, miss));
            }
        }
        if !missing.is_empty() {
            let msg = missing
                .into_iter()
                .map(|(t, deps)| format!("{t} -> missing {:?}", deps))
                .collect::<Vec<_>>()
                .join("; ");
            anyhow::bail!("Deadlock: unsatisfied/missing dependencies: {msg}");
        }

        // Cycle detection on the induced subgraph of waiting tasks (Kahn)
        let mut indeg: HashMap<usize, usize> = HashMap::with_capacity(self.waiting_tasks.len());
        for &id in &self.waiting_tasks {
            indeg.insert(id, 0);
        }
        for &index in &self.waiting_tasks {
            let node = self.nodes.get(index).expect("waiting node must exist");
            for dep in &node.dependencies {
                if self.waiting_tasks.contains(dep) {
                    *indeg.get_mut(&index).unwrap() += 1;
                }
            }
        }

        let mut q: VecDeque<usize> = indeg
            .iter()
            .filter_map(|(&id, &deg)| (deg == 0).then_some(id))
            .collect();

        let mut removed = 0usize;
        while let Some(u) = q.pop_front() {
            removed += 1;

            // Decrement indegree of dependents that are also in waiting
            if let Some(deps) = self.reverse_dependencies.get(&u) {
                for &v in deps {
                    if let Some(deg) = indeg.get_mut(&v) {
                        *deg -= 1;
                        if *deg == 0 {
                            q.push_back(v);
                        }
                    }
                }
            }
        }

        if removed < indeg.len() {
            let cycle_nodes: Vec<usize> = indeg
                .into_iter()
                .filter_map(|(id, deg)| (deg > 0).then_some(id))
                .collect();
            anyhow::bail!(
                "Deadlock: dependency cycle detected among tasks: {:?}",
                cycle_nodes
            );
        }

        // All removable, but no ready/running → likely not re-queued after deps satisfied
        anyhow::bail!(
            "Deadlock: waiting exists but nothing ready/running. Re-queue waiting-ready before diagnosing."
        );
    }
    pub async fn run_all(&mut self) -> Result<()> {
        let (tx, mut rx) = mpsc::unbounded_channel::<(usize, Result<()>)>();

        // Pick up tasks that became ready since last run
        self.requeue_waiting_ready();

        loop {
            // Spawn up to concurrency limit
            while self.running_tasks.len() < self.concurrency_limit {
                if let Some(HeapNode { index, .. }) = self.task_queue.pop() {
                    if self.running_tasks.contains(&index) || self.completed_tasks.contains(&index)
                    {
                        continue;
                    }
                    let Some(node) = self.nodes.get(index).cloned() else {
                        continue;
                    };
                    self.running_tasks.insert(index);
                    let txc = tx.clone();
                    tokio::spawn(async move {
                        let res = (node.exec)().await;
                        let _ = txc.send((index, res));
                    });
                } else {
                    break;
                }
            }

            // Exit or diagnose stall
            if self.running_tasks.is_empty() && self.task_queue.is_empty() {
                if self.waiting_tasks.is_empty() {
                    break; // all done
                }
                // Try once more to move ready-from-waiting into the heap
                self.requeue_waiting_ready();
                if self.running_tasks.is_empty()
                    && self.task_queue.is_empty()
                    && !self.waiting_tasks.is_empty()
                {
                    // Still stalled -> this is a true deadlock; explain why.
                    self.detect_deadlock()?;
                }
            }

            // Wait for a task to finish and unlock dependents
            let Some((finished, res)) = rx.recv().await else {
                anyhow::bail!("executor channel closed");
            };

            res.with_context(|| format!("task {finished} failed"))?;
            self.running_tasks.remove(&finished);
            self.completed_tasks.insert(finished);

            if let Some(dependents) = self.reverse_dependencies.get(&finished) {
                let to_check: Vec<usize> = dependents.to_vec();
                for dep_index in to_check {
                    if self.waiting_tasks.contains(&dep_index)
                        && self.are_dependencies_met(&dep_index)
                    {
                        let prio = self.nodes.get(dep_index).unwrap().priority;
                        self.waiting_tasks.remove(&dep_index);
                        self.task_queue.push(HeapNode {
                            priority: prio,
                            index: dep_index,
                        });
                    }
                }
            }
        }

        Ok(())
    }
}

impl Default for TaskRunner {
    fn default() -> Self {
        Self::new()
    }
}
