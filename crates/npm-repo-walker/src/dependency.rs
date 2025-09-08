//! Dependency resolution and task creation logic
//!
//! This module handles the collection of dependencies from package.json files
//! and creates appropriate tasks for dependency resolution within the f-graph system.

use anyhow::Result;
use f_graph::GraphNode;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::package::PackageJson;
use crate::{SharedState, debug_log};

/// Type of dependency for failure handling
#[derive(Debug, Clone, PartialEq)]
pub enum DependencyType {
    /// Runtime dependency - always required
    Runtime,
    /// Development dependency - required only when explicitly included
    Dev,
    /// Optional dependency - never required (future)
    Optional,
    /// Peer dependency - should warn but not fail (future)  
    Peer,
}

/// A dependency with its type information
#[derive(Debug, Clone)]
pub struct TypedDependency {
    pub name: String,
    pub dep_type: DependencyType,
}

/// Configuration for dependency processing
#[derive(Debug, Clone)]
pub struct DependencyConfig {
    /// Whether to include dev dependencies
    pub include_dev_dependencies: bool,
    /// Whether to include optional dependencies  
    pub include_optional_dependencies: bool,
    /// Whether to include peer dependencies
    pub include_peer_dependencies: bool,
}

impl Default for DependencyConfig {
    fn default() -> Self {
        Self {
            include_dev_dependencies: true,
            include_optional_dependencies: true,
            include_peer_dependencies: true,
        }
    }
}

impl DependencyConfig {
    /// Create a config for local packages (includes all dependency types)
    pub fn for_local_package() -> Self {
        Self::default()
    }

    /// Create a config for external packages (only runtime dependencies)
    pub fn for_external_package() -> Self {
        Self {
            include_dev_dependencies: false,
            include_optional_dependencies: true,
            include_peer_dependencies: false,
        }
    }
}

/// Collects all relevant dependencies from a package.json based on configuration
///
/// # Arguments
/// * `package_json` - The parsed package.json content
/// * `config` - Configuration specifying which dependency types to include
/// * `package_path` - Path to the package (used for logging)
///
/// # Returns
/// Vector of typed dependencies to resolve
pub fn collect_dependencies(
    package_json: &PackageJson,
    config: &DependencyConfig,
    package_path: &PathBuf,
) -> Vec<TypedDependency> {
    let mut deps = Vec::new();

    // Always include runtime dependencies
    if let Some(dependencies) = &package_json.dependencies {
        for dep_name in dependencies.keys() {
            deps.push(TypedDependency {
                name: dep_name.clone(),
                dep_type: DependencyType::Runtime,
            });
        }
        debug_log!("📦 Found {} runtime dependencies", dependencies.len());
    }

    // Conditionally include dev dependencies
    if config.include_dev_dependencies {
        if let Some(dev_dependencies) = &package_json.dev_dependencies {
            for dep_name in dev_dependencies.keys() {
                deps.push(TypedDependency {
                    name: dep_name.clone(),
                    dep_type: DependencyType::Dev,
                });
            }
            debug_log!(
                "🔧 Including {} dev dependencies for package at {:?}",
                dev_dependencies.len(),
                package_path
            );
        }
    } else {
        debug_log!(
            "⏭️  Skipping dev dependencies for external package at {:?}",
            package_path
        );
    }

    // Conditionally include optional dependencies
    if config.include_optional_dependencies {
        if let Some(optional_dependencies) = &package_json.optional_dependencies {
            for dep_name in optional_dependencies.keys() {
                deps.push(TypedDependency {
                    name: dep_name.clone(),
                    dep_type: DependencyType::Optional,
                });
            }
            debug_log!(
                "🔄 Including {} optional dependencies for package at {:?}",
                optional_dependencies.len(),
                package_path
            );
        }
    } else {
        debug_log!(
            "⏭️  Skipping optional dependencies for package at {:?}",
            package_path
        );
    }

    // Conditionally include peer dependencies
    if config.include_peer_dependencies {
        if let Some(peer_dependencies) = &package_json.peer_dependencies {
            for dep_name in peer_dependencies.keys() {
                deps.push(TypedDependency {
                    name: dep_name.clone(),
                    dep_type: DependencyType::Peer,
                });
            }
            debug_log!(
                "🤝 Including {} peer dependencies for package at {:?}",
                peer_dependencies.len(),
                package_path
            );
        }
    } else {
        debug_log!(
            "⏭️  Skipping peer dependencies for package at {:?}",
            package_path
        );
    }

    deps
}

/// Creates resolve tasks for a list of typed dependencies
///
/// # Arguments
/// * `dependencies` - List of typed dependencies to resolve
/// * `parent_path` - Path of the parent package
/// * `state` - Shared state for the walker
///
/// # Returns
/// Vector of GraphNode tasks for dependency resolution
pub fn create_resolve_tasks(
    dependencies: Vec<TypedDependency>,
    parent_path: PathBuf,
    state: Arc<Mutex<SharedState>>,
) -> Result<Vec<GraphNode>> {
    let mut child_tasks = Vec::new();

    for typed_dep in dependencies {
        debug_log!(
            "🔍 Creating resolve task for {} dependency: {}",
            match typed_dep.dep_type {
                DependencyType::Runtime => "runtime",
                DependencyType::Dev => "dev",
                DependencyType::Optional => "optional",
                DependencyType::Peer => "peer",
            },
            typed_dep.name
        );
        let resolve_task = create_resolve_task(typed_dep, parent_path.clone(), state.clone())?;
        child_tasks.push(resolve_task);
    }

    Ok(child_tasks)
}

/// Creates a single resolve task for a typed dependency
///
/// This function creates a task that will attempt to resolve a dependency's path
/// and create a get_info task if the dependency is found. Failure handling depends
/// on the dependency type:
/// - Runtime/Dev: Fail the task if dependency cannot be resolved
/// - Optional: Never fail, just log and continue
/// - Peer: Warn but don't fail (future implementation)
fn create_resolve_task(
    typed_dependency: TypedDependency,
    parent_path: PathBuf,
    state: Arc<Mutex<SharedState>>,
) -> Result<GraphNode> {
    use crate::resolver::resolve_dependency_path;

    Ok(GraphNode::new(1, Vec::new(), move || {
        let typed_dep = typed_dependency.clone();
        let parent_path = parent_path.clone();
        let state = state.clone();

        Box::pin(async move {
            debug_log!(
                "🔍 Resolving {} dependency: {} from {:?}",
                match typed_dep.dep_type {
                    DependencyType::Runtime => "runtime",
                    DependencyType::Dev => "dev",
                    DependencyType::Optional => "optional",
                    DependencyType::Peer => "peer",
                },
                typed_dep.name,
                parent_path
            );

            match resolve_dependency_path(&parent_path, &typed_dep.name) {
                Some(dep_path) => {
                    debug_log!("✅ Resolved {} to {:?}", typed_dep.name, dep_path);

                    // Use canonical path for checking
                    let canonical_path =
                        std::fs::canonicalize(&dep_path).unwrap_or(dep_path.clone());

                    // Atomically check and mark as processing using path
                    {
                        let mut state_guard = state.lock().unwrap();
                        if state_guard.discovered_paths.contains(&canonical_path)
                            || state_guard.processing.contains(&canonical_path)
                        {
                            debug_log!("⏭️  Skipping already processed path: {:?}", canonical_path);
                            return Ok(vec![]);
                        }
                        // Mark as processing in the same critical section
                        state_guard.processing.insert(canonical_path.clone());
                    }

                    // Import the create_get_info_task function
                    let info_task =
                        crate::PackageWalker::create_get_info_task(dep_path, state.clone())?;
                    Ok(vec![info_task])
                }
                None => {
                    // Handle failure based on dependency type
                    match typed_dep.dep_type {
                        DependencyType::Runtime | DependencyType::Dev => {
                            // These dependencies are required - fail the task
                            let dep_type_str = match typed_dep.dep_type {
                                DependencyType::Runtime => "runtime",
                                DependencyType::Dev => "dev",
                                _ => unreachable!(),
                            };
                            anyhow::bail!(
                                "Failed to resolve required {} dependency '{}' from package at {}",
                                dep_type_str,
                                typed_dep.name,
                                parent_path.display()
                            );
                        }
                        DependencyType::Optional => {
                            // Optional dependencies are allowed to fail
                            debug_log!(
                                "⚠️  Optional dependency '{}' could not be resolved from {} (continuing)",
                                typed_dep.name,
                                parent_path.display()
                            );
                            Ok(vec![])
                        }
                        DependencyType::Peer => {
                            // Peer dependencies should warn but not fail
                            debug_log!(
                                "⚠️  Peer dependency '{}' could not be resolved from {} (peer deps should be provided by consumer)",
                                typed_dep.name,
                                parent_path.display()
                            );
                            Ok(vec![])
                        }
                    }
                }
            }
        })
    }))
}

/// Determines if a package is external (from node_modules) or local
///
/// # Arguments
/// * `package_path` - Path to the package directory
///
/// # Returns
/// true if the package is external, false if it's local
pub fn is_external_package(package_path: &Path) -> bool {
    let path_str = package_path.to_string_lossy();
    path_str.contains("/node_modules/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn create_test_package_json() -> PackageJson {
        let mut dependencies = HashMap::new();
        dependencies.insert("lodash".to_string(), "^4.17.21".to_string());
        dependencies.insert("express".to_string(), "^4.18.0".to_string());

        let mut dev_dependencies = HashMap::new();
        dev_dependencies.insert("jest".to_string(), "^29.0.0".to_string());
        dev_dependencies.insert("typescript".to_string(), "^4.8.0".to_string());

        let mut optional_dependencies = HashMap::new();
        optional_dependencies.insert("fsevents".to_string(), "^2.3.2".to_string());
        optional_dependencies.insert("chokidar".to_string(), "^3.5.3".to_string());

        let mut peer_dependencies = HashMap::new();
        peer_dependencies.insert("react".to_string(), ">=16.0.0".to_string());

        PackageJson {
            name: "test-package".to_string(),
            version: Some("1.0.0".to_string()),
            dependencies: Some(dependencies),
            dev_dependencies: Some(dev_dependencies),
            optional_dependencies: Some(optional_dependencies),
            peer_dependencies: Some(peer_dependencies),
        }
    }

    #[test]
    fn test_collect_dependencies_for_local_package() {
        let package_json = create_test_package_json();
        let config = DependencyConfig::for_local_package();
        let package_path = PathBuf::from("/home/user/my-project");

        let deps = collect_dependencies(&package_json, &config, &package_path);

        assert_eq!(deps.len(), 7); // 2 runtime + 2 dev + 2 optional + 1 peer

        // Check runtime dependencies
        let runtime_deps: Vec<&str> = deps
            .iter()
            .filter(|d| d.dep_type == DependencyType::Runtime)
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(runtime_deps.len(), 2);
        assert!(runtime_deps.contains(&"lodash"));
        assert!(runtime_deps.contains(&"express"));

        // Check dev dependencies
        let dev_deps: Vec<&str> = deps
            .iter()
            .filter(|d| d.dep_type == DependencyType::Dev)
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(dev_deps.len(), 2);
        assert!(dev_deps.contains(&"jest"));
        assert!(dev_deps.contains(&"typescript"));

        // Check optional dependencies
        let optional_deps: Vec<&str> = deps
            .iter()
            .filter(|d| d.dep_type == DependencyType::Optional)
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(optional_deps.len(), 2);
        assert!(optional_deps.contains(&"fsevents"));
        assert!(optional_deps.contains(&"chokidar"));

        // Check peer dependencies
        let peer_deps: Vec<&str> = deps
            .iter()
            .filter(|d| d.dep_type == DependencyType::Peer)
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(peer_deps.len(), 1);
        assert!(peer_deps.contains(&"react"));
    }

    #[test]
    fn test_collect_dependencies_for_external_package() {
        let package_json = create_test_package_json();
        let config = DependencyConfig::for_external_package();
        let package_path = PathBuf::from("/project/node_modules/lodash");

        let deps = collect_dependencies(&package_json, &config, &package_path);

        assert_eq!(deps.len(), 4); // 2 runtime + 2 optional (no dev or peer)

        // Check runtime dependencies
        let runtime_deps: Vec<&str> = deps
            .iter()
            .filter(|d| d.dep_type == DependencyType::Runtime)
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(runtime_deps.len(), 2);
        assert!(runtime_deps.contains(&"lodash"));
        assert!(runtime_deps.contains(&"express"));

        // Check that optional dependencies are included
        let optional_deps: Vec<&str> = deps
            .iter()
            .filter(|d| d.dep_type == DependencyType::Optional)
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(optional_deps.len(), 2);
        assert!(optional_deps.contains(&"fsevents"));
        assert!(optional_deps.contains(&"chokidar"));

        // Check that no dev dependencies are included
        let dev_deps: Vec<&str> = deps
            .iter()
            .filter(|d| d.dep_type == DependencyType::Dev)
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(dev_deps.len(), 0);

        // Check that no peer dependencies are included
        let peer_deps: Vec<&str> = deps
            .iter()
            .filter(|d| d.dep_type == DependencyType::Peer)
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(peer_deps.len(), 0);
    }

    #[test]
    fn test_is_external_package() {
        // Local packages (should return false)
        assert!(!is_external_package(&PathBuf::from(
            "/home/user/my-project"
        )));
        assert!(!is_external_package(&PathBuf::from(
            "/workspace/local-package"
        )));

        // External packages (should return true)
        assert!(is_external_package(&PathBuf::from(
            "/project/node_modules/lodash"
        )));
        assert!(is_external_package(&PathBuf::from(
            "/workspace/node_modules/@types/node"
        )));
        assert!(is_external_package(&PathBuf::from(
            "/home/.pnpm/store/node_modules/react"
        )));
    }

    #[test]
    fn test_dependency_config_defaults() {
        let default_config = DependencyConfig::default();
        assert!(default_config.include_dev_dependencies);
        assert!(default_config.include_optional_dependencies);
        assert!(default_config.include_peer_dependencies);

        let local_config = DependencyConfig::for_local_package();
        assert!(local_config.include_dev_dependencies);
        assert!(local_config.include_optional_dependencies);
        assert!(local_config.include_peer_dependencies);

        let external_config = DependencyConfig::for_external_package();
        assert!(!external_config.include_dev_dependencies);
        assert!(external_config.include_optional_dependencies);
        assert!(!external_config.include_peer_dependencies);
    }

    #[tokio::test]
    async fn test_dev_dependency_failure() {
        use std::sync::{Arc, Mutex};
        use tempfile::TempDir;

        // Create a temp directory structure without the missing dependency
        let temp_dir = TempDir::new().unwrap();
        let state = Arc::new(Mutex::new(crate::SharedState {
            discovered: std::collections::HashMap::new(),
            discovered_paths: std::collections::HashSet::new(),
            processing: std::collections::HashSet::new(),
        }));

        // Create a typed dependency for a non-existent dev dependency
        let missing_dev_dep = TypedDependency {
            name: "nonexistent-dev-package".to_string(),
            dep_type: DependencyType::Dev,
        };

        // Create a resolve task for this dependency
        let resolve_task =
            create_resolve_task(missing_dev_dep, temp_dir.path().to_path_buf(), state).unwrap();

        // Execute the task - it should fail for dev dependencies
        let task_fn = resolve_task.exec;
        let result = task_fn().await;

        // Dev dependency failure should cause the task to fail
        assert!(
            result.is_err(),
            "Dev dependency resolution should fail when dependency is missing"
        );

        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("Failed to resolve required dev dependency"));
        assert!(error_msg.contains("nonexistent-dev-package"));
    }

    #[tokio::test]
    async fn test_runtime_dependency_failure() {
        use std::sync::{Arc, Mutex};
        use tempfile::TempDir;

        // Create a temp directory structure without the missing dependency
        let temp_dir = TempDir::new().unwrap();
        let state = Arc::new(Mutex::new(crate::SharedState {
            discovered: std::collections::HashMap::new(),
            discovered_paths: std::collections::HashSet::new(),
            processing: std::collections::HashSet::new(),
        }));

        // Create a typed dependency for a non-existent runtime dependency
        let missing_runtime_dep = TypedDependency {
            name: "nonexistent-runtime-package".to_string(),
            dep_type: DependencyType::Runtime,
        };

        // Create a resolve task for this dependency
        let resolve_task =
            create_resolve_task(missing_runtime_dep, temp_dir.path().to_path_buf(), state).unwrap();

        // Execute the task - it should fail for runtime dependencies
        let task_fn = resolve_task.exec;
        let result = task_fn().await;

        // Runtime dependency failure should cause the task to fail
        assert!(
            result.is_err(),
            "Runtime dependency resolution should fail when dependency is missing"
        );

        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("Failed to resolve required runtime dependency"));
        assert!(error_msg.contains("nonexistent-runtime-package"));
    }

    #[tokio::test]
    async fn test_optional_dependency_success() {
        use std::sync::{Arc, Mutex};
        use tempfile::TempDir;

        // Create a temp directory structure without the missing optional dependency
        let temp_dir = TempDir::new().unwrap();
        let state = Arc::new(Mutex::new(crate::SharedState {
            discovered: std::collections::HashMap::new(),
            discovered_paths: std::collections::HashSet::new(),
            processing: std::collections::HashSet::new(),
        }));

        // Create a typed dependency for a non-existent optional dependency
        let missing_optional_dep = TypedDependency {
            name: "nonexistent-optional-package".to_string(),
            dep_type: DependencyType::Optional,
        };

        // Create a resolve task for this dependency
        let resolve_task =
            create_resolve_task(missing_optional_dep, temp_dir.path().to_path_buf(), state)
                .unwrap();

        // Execute the task - it should succeed for optional dependencies
        let task_fn = resolve_task.exec;
        let result = task_fn().await;

        // Optional dependency failure should NOT cause the task to fail
        assert!(
            result.is_ok(),
            "Optional dependency resolution should succeed even when dependency is missing"
        );

        // Should return an empty vec (no child tasks)
        let child_tasks = result.unwrap();
        assert!(
            child_tasks.is_empty(),
            "Optional dependency should return no child tasks when missing"
        );
    }

    #[tokio::test]
    async fn test_peer_dependency_success() {
        use std::sync::{Arc, Mutex};
        use tempfile::TempDir;

        // Create a temp directory structure without the missing peer dependency
        let temp_dir = TempDir::new().unwrap();
        let state = Arc::new(Mutex::new(crate::SharedState {
            discovered: std::collections::HashMap::new(),
            discovered_paths: std::collections::HashSet::new(),
            processing: std::collections::HashSet::new(),
        }));

        // Create a typed dependency for a non-existent peer dependency
        let missing_peer_dep = TypedDependency {
            name: "nonexistent-peer-package".to_string(),
            dep_type: DependencyType::Peer,
        };

        // Create a resolve task for this dependency
        let resolve_task =
            create_resolve_task(missing_peer_dep, temp_dir.path().to_path_buf(), state).unwrap();

        // Execute the task - it should succeed for peer dependencies
        let task_fn = resolve_task.exec;
        let result = task_fn().await;

        // Peer dependency failure should NOT cause the task to fail
        assert!(
            result.is_ok(),
            "Peer dependency resolution should succeed even when dependency is missing"
        );

        // Should return an empty vec (no child tasks)
        let child_tasks = result.unwrap();
        assert!(
            child_tasks.is_empty(),
            "Peer dependency should return no child tasks when missing"
        );
    }
}
