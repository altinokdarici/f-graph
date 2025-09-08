# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Rust monorepo workspace containing two crates:
- **f-graph**: A Rust implementation of Microsoft's Promise Graph (p-graph) for managing asynchronous task execution with dependency and concurrency control
- **npm-repo-walker**: A package that uses f-graph (currently minimal implementation)

## Commands

### Building and Testing
- `cargo build` - Build all workspace members
- `cargo test` - Run tests for all workspace members
- `cargo test <pattern>` - Run specific tests matching pattern
- `cargo run -p f-graph` - Run the f-graph example
- `cargo run -p npm-repo-walker` - Run npm-repo-walker

### Development
- `cargo check` - Check all workspace members without building
- `cargo clippy` - Run linter on all workspace members
- `cargo fmt` - Format code for all workspace members
- `cargo doc --open` - Generate and open documentation

### Individual Crate Operations
- `cargo build -p <crate-name>` - Build specific crate
- `cargo test -p <crate-name>` - Test specific crate

## Workspace Architecture

### Structure
- **Root Cargo.toml**: Defines workspace with resolver = "3" and shared dependencies
- **crates/f-graph**: Core library implementing the promise graph
- **crates/npm-repo-walker**: Consumer crate using f-graph as workspace dependency

### Key Design Patterns

**f-graph Core Components:**
- **FGraph**: Main orchestrator managing task execution with dependency resolution, priority scheduling, and concurrency control
- **GraphNode**: Represents individual tasks with priorities, dependencies, and async execution functions
- **HeapNode**: Priority queue element ensuring deterministic task ordering

**Dependency Management**: Uses reverse dependency mapping for efficient ready-task tracking and Kahn's algorithm for deadlock detection

**Concurrency**: Channel-based async task spawning with configurable limits using tokio runtime

### Workspace Dependencies
- f-graph is exposed as a workspace dependency for internal crates
- External dependencies: tokio (async runtime), async-trait, anyhow (error handling)

## Development Notes

- Uses Rust 2024 edition
- Requires tokio runtime with multi-threading, macros, sync, and time features
- The f-graph crate contains detailed examples in src/main.rs
- npm-repo-walker is currently a placeholder implementation