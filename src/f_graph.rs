use anyhow::{Error, Result};
use std::{
    collections::HashMap,
    fmt::{self},
    future::Future,
    hash::{DefaultHasher, Hash, Hasher},
    pin::Pin,
    sync::Arc,
};

type TaskFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
type TaskFn = Arc<dyn Fn() -> TaskFuture + Send + Sync>;

#[derive(Clone)]
pub struct HashableGraphNode {
    pub priority: i32,
    pub dependencies: Vec<u64>,
    pub hash: u64,
    pub exec: TaskFn,
}

// Pretty-print without touching `exec`
impl fmt::Debug for HashableGraphNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HashableGraphNode")
            .field("priority", &self.priority)
            .field("dependencies", &self.dependencies)
            .field("hash", &self.hash)
            .finish_non_exhaustive()
    }
}

// Equality by identity (hash). `exec` intentionally ignored.
impl PartialEq for HashableGraphNode {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl Eq for HashableGraphNode {} // OK because equality is total on `hash`

impl HashableGraphNode {
    pub fn new<F>(priority: i32, dependencies: Vec<u64>, exec: F) -> Self
    where
        F: Fn() -> TaskFuture + Send + Sync + 'static,
    {
        let mut hasher = DefaultHasher::new();
        priority.hash(&mut hasher);
        dependencies.hash(&mut hasher);
        let hash = hasher.finish();

        HashableGraphNode {
            priority,
            dependencies,
            hash,
            exec: Arc::new(exec),
        }
    }
}

pub struct Graph {
    nodes: HashMap<u64, HashableGraphNode>,
    reverse_dependencies: HashMap<u64, Vec<u64>>, // index -> Vec of dependent indices
    levels: HashMap<u64, i32>,                    // index -> level
}

impl Graph {
    pub fn new() -> Self {
        Graph {
            nodes: HashMap::new(),
            reverse_dependencies: HashMap::new(),
            levels: HashMap::new(),
        }
    }

    pub fn add_node(&mut self, node: HashableGraphNode) -> Result<()> {
        let node_hash = node.hash;

        // check if the node is already in the nodes
        if self.nodes.contains_key(&node_hash) {
            return Err(Error::msg(format!(
                "Node already exists in the graph: {}",
                node_hash
            )));
        }

        let mut max_dep_level = -1;
        for &dep_hash in &node.dependencies {
            if !self.nodes.contains_key(&dep_hash) {
                return Err(Error::msg("Dependency node not found in the graph"));
            }

            if let Some(&dep_level) = self.levels.get(&dep_hash) {
                max_dep_level = max_dep_level.max(dep_level);
            }
        }

        // All validations passed - now update the graph atomically
        self.nodes.insert(node_hash, node);
        self.levels.insert(node_hash, max_dep_level + 1);
        self.reverse_dependencies.insert(node_hash, Vec::new());

        for &dep_hash in &self.nodes.get(&node_hash).unwrap().dependencies {
            self.reverse_dependencies
                .entry(dep_hash)
                .or_default()
                .push(node_hash);
        }

        Ok(())
    }

    pub fn get_dependents(&self, node_hash: u64) -> Option<&Vec<u64>> {
        self.reverse_dependencies.get(&node_hash)
    }

    pub fn get_node(&self, node_hash: u64) -> Option<&HashableGraphNode> {
        self.nodes.get(&node_hash)
    }
}
