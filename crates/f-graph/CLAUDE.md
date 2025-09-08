# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is `f-graph`, a Rust implementation of Microsoft's Promise Graph (p-graph) for managing asynchronous task execution with dependency and concurrency control. The library allows defining task graphs where tasks have priorities and dependencies, and execution is controlled with configurable concurrency limits.

## Commands

### Building and Running
- `cargo build` - Build the project
- `cargo run` - Run the example application in main.rs
- `cargo test` - Run all tests
- `cargo test <testname>` - Run specific tests containing the name
- `cargo check` - Check code without building
- `cargo fmt` - Format code

### Development
- `cargo clippy` - Run linter
- `cargo doc --open` - Generate and open documentation

## Architecture

### Core Components

**FGraph** (`src/lib.rs:13-287`): The main orchestrator that manages task execution
- Maintains dependency graph using reverse dependencies HashMap
- Uses BinaryHeap for priority-based task scheduling
- Tracks task states: waiting, running, completed
- Implements deadlock detection with cycle detection using Kahn's algorithm
- Supports configurable concurrency limits

**GraphNode** (`src/graph_node.rs`): Represents a single task
- Contains priority (i32), dependencies (Vec<usize>), and async execution function
- Uses Arc<dyn Fn() -> TaskFuture> for thread-safe function storage
- TaskFuture type alias for boxed async functions

**HeapNode** (`src/heap_node.rs`): Priority queue element for task scheduling
- Implements Ord for max-heap with priority-first, then index-based ordering
- Ensures deterministic execution order for same-priority tasks

### Key Algorithms

**Dependency Resolution**: Uses reverse dependency mapping to efficiently track when tasks become ready
**Priority Scheduling**: Max-heap ensures highest priority tasks run first
**Deadlock Detection**: Implements Kahn's algorithm for cycle detection in waiting tasks
**Concurrency Control**: Channel-based async task spawning with configurable limits

## Dependencies

- `tokio`: Async runtime with multi-threading, macros, sync primitives, and time utilities
- `async-trait`: For async trait definitions
- `anyhow`: Error handling and context

## Example Usage

See `src/main.rs` for a working example that demonstrates:
- Creating tasks with different priorities
- Setting up dependency chains
- Running tasks with concurrency control
- Multiple execution phases