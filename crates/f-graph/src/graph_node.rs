use anyhow::Result;
use std::{
    fmt::{self},
    future::Future,
    pin::Pin,
    sync::Arc,
};

pub type TaskFuture = Pin<Box<dyn Future<Output = Result<Vec<GraphNode>>> + Send>>;
pub type TaskFn = Arc<dyn Fn() -> TaskFuture + Send + Sync>;

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
