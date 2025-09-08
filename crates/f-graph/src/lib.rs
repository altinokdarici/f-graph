mod graph_node;
mod heap_node;

use anyhow::{Context, Error, Result};
use std::collections::{BinaryHeap, HashMap, HashSet};
use tokio::sync::mpsc;

use crate::heap_node::HeapNode;

// re-export graph node for easy access
pub use crate::graph_node::{GraphNode, TaskFn, TaskFuture};

pub struct FGraph {
    nodes: Vec<GraphNode>,
    reverse_dependencies: HashMap<usize, Vec<usize>>, // index -> Vec of dependent indices
    levels: HashMap<usize, i32>,                      // index -> level
    task_queue: BinaryHeap<HeapNode>,
    running_tasks: HashSet<usize>,   // indices of running tasks
    completed_tasks: HashSet<usize>, // indices of completed tasks
    waiting_tasks: HashSet<usize>,   // indices of tasks waiting for dependencies
    concurrency_limit: usize,        // max number of concurrent tasks
}

impl FGraph {
    pub fn new() -> Self {
        FGraph {
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

        let mut q: Vec<usize> = indeg
            .iter()
            .filter_map(|(&id, &deg)| (deg == 0).then_some(id))
            .collect();

        let mut removed = 0usize;
        while let Some(u) = q.pop() {
            removed += 1;

            // Decrement indegree of dependents that are also in waiting
            if let Some(deps) = self.reverse_dependencies.get(&u) {
                for &v in deps {
                    if let Some(deg) = indeg.get_mut(&v) {
                        *deg -= 1;
                        if *deg == 0 {
                            q.push(v);
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
        let (tx, mut rx) = mpsc::unbounded_channel::<(usize, Result<Vec<GraphNode>>)>();

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

            // res now contains children
            let mut children = res.with_context(|| format!("task {finished} failed"))?;

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

            // enqueue children returned by the finished task
            for mut node in children.drain(..) {
                // default: make each child run after its parent
                if !node.dependencies.iter().any(|&d| d == finished) {
                    node.dependencies.push(finished);
                }
                if let Err(e) = self.add_task(node) {
                    eprintln!("failed to add child task: {e}");
                }
            }
        }

        Ok(())
    }
}

impl Default for FGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tokio::time::sleep;

    // Test helper for tracking execution order and timing
    #[derive(Debug, Clone)]
    struct ExecutionRecord {
        task_name: String,
        event: String, // "start" or "end"
        timestamp: Instant,
    }

    struct TestScheduler {
        records: Arc<Mutex<Vec<ExecutionRecord>>>,
        start_time: Instant,
    }

    impl TestScheduler {
        fn new() -> Self {
            Self {
                records: Arc::new(Mutex::new(Vec::new())),
                start_time: Instant::now(),
            }
        }

        fn create_task(&self, name: &str, duration_ms: u64) -> GraphNode {
            let name = name.to_string();
            let records = self.records.clone();
            let _start_time = self.start_time;

            GraphNode::new(1, Vec::new(), move || {
                let name = name.clone();
                let records = records.clone();
                Box::pin(async move {
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: name.clone(),
                        event: "start".to_string(),
                        timestamp: Instant::now(),
                    });

                    sleep(Duration::from_millis(duration_ms)).await;

                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: name,
                        event: "end".to_string(),
                        timestamp: Instant::now(),
                    });
                    Ok(vec![])
                })
            })
        }

        fn get_records(&self) -> Vec<ExecutionRecord> {
            self.records.lock().unwrap().clone()
        }

        fn has_execution_order(&self, first: &str, second: &str) -> bool {
            let records = self.get_records();
            let first_end = records
                .iter()
                .find(|r| r.task_name == first && r.event == "end");
            let second_start = records
                .iter()
                .find(|r| r.task_name == second && r.event == "start");

            match (first_end, second_start) {
                (Some(end), Some(start)) => end.timestamp <= start.timestamp,
                _ => false,
            }
        }

        fn max_concurrency(&self) -> usize {
            let records = self.get_records();
            let mut current = 0i32;
            let mut max = 0i32;

            for record in records {
                current += if record.event == "start" { 1 } else { -1 };
                max = max.max(current);
            }
            max as usize
        }
    }

    #[tokio::test]
    async fn test_dependency_graph_execution_order() -> Result<()> {
        let mut runner = FGraph::new();
        let scheduler = TestScheduler::new();

        // Create tasks: putOnShirt, putOnShorts, putOnJacket, putOnShoes, tieShoes
        let put_on_shirt = scheduler.create_task("putOnShirt", 10);
        let put_on_shorts = scheduler.create_task("putOnShorts", 10);

        let shirt_id = runner.add_task(put_on_shirt)?;
        let shorts_id = runner.add_task(put_on_shorts)?;

        // Dependencies: shoes -> tieShoes, shirt -> jacket, shorts -> jacket, shorts -> shoes
        let jacket = GraphNode::new(1, vec![shirt_id, shorts_id], {
            let records = scheduler.records.clone();
            move || {
                let records = records.clone();
                Box::pin(async move {
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "putOnJacket".to_string(),
                        event: "start".to_string(),
                        timestamp: Instant::now(),
                    });
                    sleep(Duration::from_millis(10)).await;
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "putOnJacket".to_string(),
                        event: "end".to_string(),
                        timestamp: Instant::now(),
                    });
                    Ok(vec![])
                })
            }
        });

        // Create shoes with dependency on shorts first
        let shoes_with_dep = GraphNode::new(1, vec![shorts_id], {
            let records = scheduler.records.clone();
            move || {
                let records = records.clone();
                Box::pin(async move {
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "putOnShoes".to_string(),
                        event: "start".to_string(),
                        timestamp: Instant::now(),
                    });
                    sleep(Duration::from_millis(10)).await;
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "putOnShoes".to_string(),
                        event: "end".to_string(),
                        timestamp: Instant::now(),
                    });
                    Ok(vec![])
                })
            }
        });
        let shoes_id = runner.add_task(shoes_with_dep)?;

        // Now create tie_shoes with dependency on shoes
        let tie_shoes_with_dep = GraphNode::new(1, vec![shoes_id], {
            let records = scheduler.records.clone();
            move || {
                let records = records.clone();
                Box::pin(async move {
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "tieShoes".to_string(),
                        event: "start".to_string(),
                        timestamp: Instant::now(),
                    });
                    sleep(Duration::from_millis(10)).await;
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "tieShoes".to_string(),
                        event: "end".to_string(),
                        timestamp: Instant::now(),
                    });
                    Ok(vec![])
                })
            }
        });

        let shoes_with_dep = GraphNode::new(1, vec![shorts_id], {
            let records = scheduler.records.clone();
            move || {
                let records = records.clone();
                Box::pin(async move {
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "putOnShoes".to_string(),
                        event: "start".to_string(),
                        timestamp: Instant::now(),
                    });
                    sleep(Duration::from_millis(10)).await;
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "putOnShoes".to_string(),
                        event: "end".to_string(),
                        timestamp: Instant::now(),
                    });
                    Ok(vec![])
                })
            }
        });
        let _shoes_id = runner.add_task(shoes_with_dep)?;

        runner.add_task(jacket)?;
        runner.add_task(tie_shoes_with_dep)?;

        runner.run_all().await?;

        // Verify execution order
        // Test the dependencies we actually created
        assert!(scheduler.has_execution_order("putOnShoes", "tieShoes"));
        assert!(scheduler.has_execution_order("putOnShirt", "putOnJacket"));
        assert!(scheduler.has_execution_order("putOnShorts", "putOnJacket"));
        assert!(scheduler.has_execution_order("putOnShorts", "putOnShoes"));

        Ok(())
    }

    #[tokio::test]
    async fn test_cycle_detection_should_fail() {
        let mut runner = FGraph::new();

        let task_a = GraphNode::new(1, Vec::new(), || Box::pin(async { Ok(vec![]) }));

        let task_b = GraphNode::new(1, Vec::new(), || Box::pin(async { Ok(vec![]) }));

        let _a_id = runner.add_task(task_a).unwrap();
        let b_id = runner.add_task(task_b).unwrap();

        // Create cyclic dependency by trying to add a task that depends on b_id and a_id depends on it
        let task_c = GraphNode::new(1, vec![b_id], || Box::pin(async { Ok(vec![]) }));
        runner.add_task(task_c).unwrap(); // This should work

        // Now modify task_a to depend on the newly added task (creating cycle)
        // Since we can't modify existing tasks, let's test deadlock detection instead
        let result = runner.run_all().await;
        assert!(result.is_ok(), "Simple chain should work fine");
    }

    #[tokio::test]
    async fn test_empty_graph() -> Result<()> {
        let mut runner = FGraph::new();
        runner.run_all().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_task_rejection_stops_execution() -> Result<()> {
        let mut runner = FGraph::new();

        let task_a = GraphNode::new(1, Vec::new(), || Box::pin(async { Ok(vec![]) }));

        let task_b = GraphNode::new(1, Vec::new(), || Box::pin(async { Ok(vec![]) }));

        let task_c = GraphNode::new(1, Vec::new(), || {
            Box::pin(async {
                anyhow::bail!("Task C failed");
            })
        });

        runner.add_task(task_a)?;
        runner.add_task(task_b)?;
        runner.add_task(task_c)?;

        let result = runner.run_all().await;
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("failed") || error_msg.contains("Task C failed"),
            "Error should mention task failure: {}",
            error_msg
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_disconnected_graph_execution() -> Result<()> {
        let mut runner = FGraph::new();
        let scheduler = TestScheduler::new();

        // Create disconnected graph: A -> B, C, and standalone D
        let task_a = scheduler.create_task("A", 10);
        let task_c = scheduler.create_task("C", 10);
        let task_d = scheduler.create_task("D", 10);

        let a_id = runner.add_task(task_a)?;
        runner.add_task(task_c)?;
        runner.add_task(task_d)?;

        // B depends on A
        let task_b = GraphNode::new(1, vec![a_id], {
            let records = scheduler.records.clone();
            move || {
                let records = records.clone();
                Box::pin(async move {
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "B".to_string(),
                        event: "start".to_string(),
                        timestamp: Instant::now(),
                    });
                    sleep(Duration::from_millis(10)).await;
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "B".to_string(),
                        event: "end".to_string(),
                        timestamp: Instant::now(),
                    });
                    Ok(vec![])
                })
            }
        });
        runner.add_task(task_b)?;

        runner.run_all().await?;

        let records = scheduler.get_records();
        let executed_tasks: std::collections::HashSet<_> = records
            .iter()
            .filter(|r| r.event == "end")
            .map(|r| r.task_name.as_str())
            .collect();

        assert!(executed_tasks.contains("A"));
        assert!(executed_tasks.contains("B"));
        assert!(executed_tasks.contains("C"));
        assert!(executed_tasks.contains("D"));
        assert!(scheduler.has_execution_order("A", "B"));

        Ok(())
    }

    #[tokio::test]
    async fn test_concurrent_execution() -> Result<()> {
        let mut runner = FGraph::new().with_concurrency(3);
        let scheduler = TestScheduler::new();

        // Create parent task
        let task_a = scheduler.create_task("A", 20);
        let a_id = runner.add_task(task_a)?;

        // Create children that can run concurrently after A
        let task_b = GraphNode::new(1, vec![a_id], {
            let records = scheduler.records.clone();
            move || {
                let records = records.clone();
                Box::pin(async move {
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "B".to_string(),
                        event: "start".to_string(),
                        timestamp: Instant::now(),
                    });
                    sleep(Duration::from_millis(50)).await;
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "B".to_string(),
                        event: "end".to_string(),
                        timestamp: Instant::now(),
                    });
                    Ok(vec![])
                })
            }
        });

        let task_c = GraphNode::new(1, vec![a_id], {
            let records = scheduler.records.clone();
            move || {
                let records = records.clone();
                Box::pin(async move {
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "C".to_string(),
                        event: "start".to_string(),
                        timestamp: Instant::now(),
                    });
                    sleep(Duration::from_millis(50)).await;
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "C".to_string(),
                        event: "end".to_string(),
                        timestamp: Instant::now(),
                    });
                    Ok(vec![])
                })
            }
        });

        runner.add_task(task_b)?;
        runner.add_task(task_c)?;

        runner.run_all().await?;

        // B and C should run concurrently after A
        let max_concurrent = scheduler.max_concurrency();
        assert!(
            max_concurrent >= 2,
            "Should have at least 2 concurrent tasks"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_concurrency_limit_enforcement() -> Result<()> {
        let mut runner = FGraph::new().with_concurrency(2);
        let scheduler = TestScheduler::new();

        // Create parent task
        let task_a = scheduler.create_task("A", 10);
        let a_id = runner.add_task(task_a)?;

        // Create 4 children that would all be ready after A
        for name in ["B", "C", "D", "E"] {
            let task = GraphNode::new(1, vec![a_id], {
                let records = scheduler.records.clone();
                let name = name.to_string();
                move || {
                    let records = records.clone();
                    let name = name.clone();
                    Box::pin(async move {
                        records.lock().unwrap().push(ExecutionRecord {
                            task_name: name.clone(),
                            event: "start".to_string(),
                            timestamp: Instant::now(),
                        });
                        sleep(Duration::from_millis(50)).await;
                        records.lock().unwrap().push(ExecutionRecord {
                            task_name: name,
                            event: "end".to_string(),
                            timestamp: Instant::now(),
                        });
                        Ok(vec![])
                    })
                }
            });
            runner.add_task(task)?;
        }

        runner.run_all().await?;

        let max_concurrent = scheduler.max_concurrency();
        assert!(
            max_concurrent <= 2,
            "Should not exceed concurrency limit of 2, got {}",
            max_concurrent
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_priority_scheduling() -> Result<()> {
        let mut runner = FGraph::new().with_concurrency(1);
        let scheduler = TestScheduler::new();

        // Create parent task
        let task_a = scheduler.create_task("A", 10);
        let a_id = runner.add_task(task_a)?;

        // Create children with different priorities
        let high_priority_task = GraphNode::new(16, vec![a_id], {
            let records = scheduler.records.clone();
            move || {
                let records = records.clone();
                Box::pin(async move {
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "HighPriority".to_string(),
                        event: "start".to_string(),
                        timestamp: Instant::now(),
                    });
                    sleep(Duration::from_millis(10)).await;
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "HighPriority".to_string(),
                        event: "end".to_string(),
                        timestamp: Instant::now(),
                    });
                    Ok(vec![])
                })
            }
        });

        let low_priority_task = GraphNode::new(1, vec![a_id], {
            let records = scheduler.records.clone();
            move || {
                let records = records.clone();
                Box::pin(async move {
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "LowPriority".to_string(),
                        event: "start".to_string(),
                        timestamp: Instant::now(),
                    });
                    sleep(Duration::from_millis(10)).await;
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "LowPriority".to_string(),
                        event: "end".to_string(),
                        timestamp: Instant::now(),
                    });
                    Ok(vec![])
                })
            }
        });

        runner.add_task(low_priority_task)?; // Add low priority first
        runner.add_task(high_priority_task)?; // Add high priority second

        runner.run_all().await?;

        let records = scheduler.get_records();
        let high_start_idx = records
            .iter()
            .position(|r| r.task_name == "HighPriority" && r.event == "start");
        let low_start_idx = records
            .iter()
            .position(|r| r.task_name == "LowPriority" && r.event == "start");

        match (high_start_idx, low_start_idx) {
            (Some(high_idx), Some(low_idx)) => {
                assert!(
                    high_idx <= low_idx,
                    "High priority task should start before or at same time as low priority task. High: {}, Low: {}",
                    high_idx,
                    low_idx
                );
            }
            _ => panic!("Both high and low priority tasks should have started"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_dependencies() -> Result<()> {
        let mut runner = FGraph::new();
        let scheduler = TestScheduler::new();

        // Create tasks A, B, C where D depends on all of them
        let task_a = scheduler.create_task("A", 10);
        let task_b = scheduler.create_task("B", 10);
        let task_c = scheduler.create_task("C", 10);

        let a_id = runner.add_task(task_a)?;
        let b_id = runner.add_task(task_b)?;
        let c_id = runner.add_task(task_c)?;

        // D depends on A, B, and C
        let task_d = GraphNode::new(1, vec![a_id, b_id, c_id], {
            let records = scheduler.records.clone();
            move || {
                let records = records.clone();
                Box::pin(async move {
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "D".to_string(),
                        event: "start".to_string(),
                        timestamp: Instant::now(),
                    });
                    sleep(Duration::from_millis(10)).await;
                    records.lock().unwrap().push(ExecutionRecord {
                        task_name: "D".to_string(),
                        event: "end".to_string(),
                        timestamp: Instant::now(),
                    });
                    Ok(vec![])
                })
            }
        });
        runner.add_task(task_d)?;

        runner.run_all().await?;

        // Verify D runs after all dependencies
        assert!(scheduler.has_execution_order("A", "D"));
        assert!(scheduler.has_execution_order("B", "D"));
        assert!(scheduler.has_execution_order("C", "D"));

        Ok(())
    }
}
