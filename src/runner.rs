use crate::f_graph::{Graph, HashableGraphNode};
use anyhow::{Context, Result};
use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap, HashSet, VecDeque},
};
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
struct HeapNode {
    priority: i32,
    hash: u64,
}

// Max-heap: higher priority first; for ties, smaller hash first (deterministic)
impl Ord for HeapNode {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority
            .cmp(&other.priority)
            .reverse()
            .then_with(|| self.hash.cmp(&other.hash))
    }
}

impl PartialOrd for HeapNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub struct TaskRunner {
    graph: Graph,
    task_queue: BinaryHeap<HeapNode>,
    running_tasks: HashSet<u64>,   // hashes of running tasks
    completed_tasks: HashSet<u64>, // hashes of completed tasks
    waiting_tasks: HashSet<u64>,   // hashes of tasks waiting for dependencies
    concurrency_limit: usize,      // max number of concurrent tasks
}

impl TaskRunner {
    pub fn new() -> Self {
        TaskRunner {
            graph: Graph::new(),
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

    pub fn add_task(&mut self, node: HashableGraphNode) -> Result<()> {
        let node_hash = node.hash;
        let priority = node.priority;

        // add to graph first
        self.graph.add_node(node)?;

        // now it's safe to check deps (or compute from the node we just had)
        let ready = self.are_dependencies_met(&node_hash);

        if ready {
            self.task_queue.push(HeapNode {
                priority,
                hash: node_hash,
            });
        } else {
            self.waiting_tasks.insert(node_hash);
        }

        return Ok(());
    }

    fn are_dependencies_met(&self, node_hash: &u64) -> bool {
        self.graph
            .get_node(*node_hash)
            .map(|node| {
                node.dependencies
                    .iter()
                    .all(|dep| self.completed_tasks.contains(dep))
            })
            .unwrap_or_default()
    }

    fn requeue_waiting_ready(&mut self) {
        // collect first to avoid borrow issues
        let ready_now: Vec<u64> = self
            .waiting_tasks
            .iter()
            .copied()
            .filter(|h| self.are_dependencies_met(h))
            .collect();

        for h in ready_now {
            let prio = self.graph.get_node(h).unwrap().priority;
            self.waiting_tasks.remove(&h);
            self.task_queue.push(HeapNode {
                priority: prio,
                hash: h,
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

        // 1) Missing / unsatisfied deps: dep not in completed or waiting
        let mut missing: Vec<(u64, Vec<u64>)> = Vec::new();
        for &id in &self.waiting_tasks {
            let Some(node) = self.graph.get_node(id) else {
                missing.push((id, vec![]));
                continue;
            };
            let miss: Vec<u64> = node
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

        // 2) Cycle detection on the induced subgraph of waiting tasks (Kahn)
        let mut indeg: HashMap<u64, usize> = HashMap::with_capacity(self.waiting_tasks.len());
        for &id in &self.waiting_tasks {
            indeg.insert(id, 0);
        }
        for &id in &self.waiting_tasks {
            let node = self.graph.get_node(id).expect("waiting node must exist");
            for dep in &node.dependencies {
                if self.waiting_tasks.contains(dep) {
                    *indeg.get_mut(&id).unwrap() += 1;
                }
            }
        }

        let mut q: VecDeque<u64> = indeg
            .iter()
            .filter_map(|(&id, &deg)| (deg == 0).then_some(id))
            .collect();

        let mut removed = 0usize;
        while let Some(u) = q.pop_front() {
            removed += 1;

            // Decrement indegree of dependents that are also in waiting
            if let Some(deps) = self.graph.get_dependents(u) {
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
            let cycle_nodes: Vec<u64> = indeg
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
        let (tx, mut rx) = mpsc::unbounded_channel::<(u64, Result<()>)>();

        // Pick up tasks that became ready since last run
        self.requeue_waiting_ready();

        loop {
            // Spawn up to concurrency limit
            while self.running_tasks.len() < self.concurrency_limit {
                if let Some(HeapNode { hash, .. }) = self.task_queue.pop() {
                    if self.running_tasks.contains(&hash) || self.completed_tasks.contains(&hash) {
                        continue;
                    }
                    let Some(node) = self.graph.get_node(hash).cloned() else {
                        continue;
                    };
                    self.running_tasks.insert(hash);
                    let txc = tx.clone();
                    tokio::spawn(async move {
                        let res = (node.exec)().await;
                        let _ = txc.send((hash, res));
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

            if let Some(dependents) = self.graph.get_dependents(finished) {
                let to_check: Vec<u64> = dependents.to_vec();
                for dep_hash in to_check {
                    if self.waiting_tasks.contains(&dep_hash)
                        && self.are_dependencies_met(&dep_hash)
                    {
                        let prio = self.graph.get_node(dep_hash).unwrap().priority;
                        self.waiting_tasks.remove(&dep_hash);
                        self.task_queue.push(HeapNode {
                            priority: prio,
                            hash: dep_hash,
                        });
                    }
                }
            }
        }

        Ok(())
    }
}
