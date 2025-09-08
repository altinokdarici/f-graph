use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeapNode {
    pub priority: i32,
    pub index: usize,
}

// Max-heap: higher priority first; for ties, smaller index first (deterministic)
impl Ord for HeapNode {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.index.cmp(&self.index)) // Reverse index order for deterministic behavior
    }
}

impl PartialOrd for HeapNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BinaryHeap;

    impl HeapNode {
        pub fn new(priority: i32, index: usize) -> Self {
            HeapNode { priority, index }
        }
    }

    #[test]
    fn test_ordering() {
        // Higher priority wins
        assert!(HeapNode::new(20, 5) > HeapNode::new(10, 3));

        // For same priority, lower index wins (deterministic)
        assert!(HeapNode::new(10, 3) > HeapNode::new(10, 7));

        // Priority takes precedence over index
        assert!(HeapNode::new(20, 100) > HeapNode::new(10, 1));
    }

    #[test]
    fn test_deterministic_same_priority() {
        let mut heap = BinaryHeap::new();

        // Add nodes with same priority in random order
        for i in [5, 1, 8, 3, 2] {
            heap.push(HeapNode::new(42, i));
        }

        // Should pop in ascending index order
        let mut indices = Vec::new();
        while let Some(node) = heap.pop() {
            indices.push(node.index);
        }
        assert_eq!(indices, vec![1, 2, 3, 5, 8]);
    }

    #[test]
    fn test_edge_cases() {
        // Test extreme values
        assert!(HeapNode::new(i32::MAX, 0) > HeapNode::new(i32::MIN, 0));
        assert!(HeapNode::new(0, 0) > HeapNode::new(0, usize::MAX));
    }
}
