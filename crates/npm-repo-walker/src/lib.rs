use anyhow::Result;
use f_graph::{FGraph, GraphNode};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

mod dependency;
mod package;
mod resolver;

use dependency::{
    DependencyConfig, collect_dependencies, create_resolve_tasks, is_external_package,
};
use package::{Package, read_package_json};

macro_rules! debug_log {
    ($($arg:tt)*) => {
        #[cfg(debug_assertions)]
        eprintln!($($arg)*);
    };
}

// Make the macro available to child modules
pub(crate) use debug_log;

#[derive(Debug)]
pub struct SharedState {
    pub discovered: RwLock<HashMap<String, Package>>,
    pub discovered_paths: RwLock<HashSet<PathBuf>>, // Track packages by canonical path to prevent duplicates
    pub processing: RwLock<HashSet<PathBuf>>,       // Track packages currently being processed by path
}

#[derive(Debug)]
pub struct PackageWalker {
    root_path: PathBuf,
    state: Arc<SharedState>,
    pub(crate) concurrency: usize,
}

impl PackageWalker {
    pub fn new(root_path: PathBuf) -> Self {
        // Use adaptive concurrency: max of 8 or number of CPU cores
        let default_concurrency = num_cpus::get().max(8);
        Self::with_concurrency(root_path, default_concurrency)
    }

    pub fn with_concurrency(root_path: PathBuf, concurrency: usize) -> Self {
        // Ensure minimum concurrency of 2 for reasonable performance
        let effective_concurrency = concurrency.max(2);
        Self {
            root_path,
            state: Arc::new(SharedState {
                discovered: RwLock::new(HashMap::new()),
                discovered_paths: RwLock::new(HashSet::new()),
                processing: RwLock::new(HashSet::new()),
            }),
            concurrency: effective_concurrency,
        }
    }

    pub async fn discover_packages(&self) -> Result<HashMap<String, Package>> {
        let mut graph = FGraph::new().with_concurrency(self.concurrency);

        // Start with the root get_info task
        let root_task = Self::create_get_info_task(self.root_path.clone(), self.state.clone())?;
        graph.add_task(root_task)?;

        // Run all tasks - f-graph will handle dynamic task creation
        let start = std::time::Instant::now();
        graph.run_all().await?;
        let duration = start.elapsed();

        let discovered = self.state.discovered.read().unwrap();
        let package_count = discovered.len();
        let packages_per_second = package_count as f64 / duration.as_secs_f64();

        println!(
            "Processed {} packages in {:.3}s ({:.1} packages/sec)",
            package_count,
            duration.as_secs_f64(),
            packages_per_second
        );

        Ok(discovered.clone())
    }

    pub fn create_get_info_task(
        package_path: PathBuf,
        state: Arc<SharedState>,
    ) -> Result<GraphNode> {
        Ok(GraphNode::new(1, Vec::new(), move || {
            let package_path = package_path.clone();
            let state = state.clone();

            Box::pin(async move {
                match read_package_json(&package_path).await {
                    Ok(package_json) => {
                        // Collect dependencies based on package type
                        let is_external = is_external_package(&package_path);
                        let config = if is_external {
                            DependencyConfig::for_external_package()
                        } else {
                            DependencyConfig::for_local_package()
                        };

                        let deps = collect_dependencies(&package_json, &config, &package_path);

                        // Determine the package name to use (alias name for .store packages, actual name for local packages)
                        let package_name =
                            Self::determine_package_name(&package_json.name, &package_path);

                        let package = Package::new(
                            package_name,
                            package_json.version.clone(),
                            package_path.clone(),
                            deps.iter().map(|d| d.name.clone()).collect(),
                        );

                        // Store discovered package in shared state (with path-based deduplication)
                        {
                            // Use canonical path for deduplication
                            let canonical_path = std::fs::canonicalize(&package_path)
                                .unwrap_or(package_path.clone());
                                
                            // Check if already discovered (read lock first)
                            {
                                let discovered_paths = state.discovered_paths.read().unwrap();
                                if discovered_paths.contains(&canonical_path) {
                                    debug_log!(
                                        "⏭️  Skipping duplicate package at canonical path: {:?}",
                                        canonical_path
                                    );
                                    // Remove from processing set
                                    state.processing.write().unwrap().remove(&canonical_path);
                                    return Ok(vec![]);
                                }
                            }
                            
                            // Add to discovered state (write locks)
                            {
                                let mut discovered_paths = state.discovered_paths.write().unwrap();
                                let mut discovered = state.discovered.write().unwrap();
                                let mut processing = state.processing.write().unwrap();
                                
                                // Double-check after acquiring write lock
                                if !discovered_paths.contains(&canonical_path) {
                                    discovered_paths.insert(canonical_path.clone());
                                    discovered.insert(package.name.clone(), package);
                                    debug_log!(
                                        "✅ Added package: {} at canonical path: {:?}",
                                        package_json.name,
                                        canonical_path
                                    );
                                }
                                processing.remove(&canonical_path);
                            }
                        }

                        debug_log!(
                            "📦 Get info completed for: {} at {:?}",
                            package_json.name,
                            package_path
                        );

                        // Create resolve tasks for each dependency as child tasks
                        let child_tasks =
                            create_resolve_tasks(deps, package_path.clone(), state.clone())?;
                        Ok(child_tasks)
                    }
                    Err(e) => {
                        debug_log!(
                            "❌ Failed to get info for package at {:?}: {}",
                            package_path,
                            e
                        );
                        Err(e)
                    }
                }
            })
        }))
    }

    fn determine_package_name(actual_name: &str, package_path: &Path) -> String {
        let path_str = package_path.to_string_lossy();

        // Check if this is a package in the .store directory
        if let Some(store_pos) = path_str.find("/.store/") {
            // For .store packages, we need to extract the package name after /node_modules/
            if let Some(node_modules_pos) = path_str[store_pos..].find("/node_modules/") {
                let full_pos = store_pos + node_modules_pos + "/node_modules/".len();
                return path_str[full_pos..].to_string();
            }
        }

        // For local packages, use the actual package.json name
        actual_name.to_string()
    }

    pub fn get_discovered_packages(&self) -> HashMap<String, Package> {
        let discovered = self.state.discovered.read().unwrap();
        discovered.clone()
    }

    pub fn concurrency(&self) -> usize {
        self.concurrency
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_read_package_json() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let package_json_content = r#"
        {
            "name": "test-package",
            "version": "1.0.0",
            "dependencies": {
                "lodash": "^4.17.21",
                "express": "^4.18.0"
            }
        }
        "#;

        fs::write(temp_dir.path().join("package.json"), package_json_content)?;

        let package_json = read_package_json(temp_dir.path()).await?;
        assert_eq!(package_json.name, "test-package");
        assert_eq!(package_json.version, Some("1.0.0".to_string()));
        assert!(package_json.dependencies.is_some());

        Ok(())
    }

    #[tokio::test]
    async fn test_package_walker_creation() {
        let temp_dir = TempDir::new().unwrap();
        let walker = PackageWalker::new(temp_dir.path().to_path_buf());
        assert!(walker.get_discovered_packages().is_empty());
        // Test that default concurrency is at least 8 (adaptive based on CPU cores)
        assert!(walker.concurrency() >= 8, "Default concurrency should be at least 8, got: {}", walker.concurrency());
    }

    #[tokio::test]
    async fn test_package_walker_with_custom_concurrency() {
        let temp_dir = TempDir::new().unwrap();
        let walker = PackageWalker::with_concurrency(temp_dir.path().to_path_buf(), 8);
        assert!(walker.get_discovered_packages().is_empty());
        assert_eq!(walker.concurrency(), 8);
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

    #[tokio::test]
    async fn test_walker_fails_on_missing_dev_dependency() {
        use std::fs;
        use tempfile::TempDir;

        // Create a temporary directory with a package.json that has an unresolvable dev dependency
        let temp_dir = TempDir::new().unwrap();
        let package_json_content = r#"
        {
            "name": "test-package",
            "version": "1.0.0",
            "dependencies": {},
            "devDependencies": {
                "nonexistent-dev-package": "^1.0.0"
            }
        }
        "#;

        fs::write(temp_dir.path().join("package.json"), package_json_content).unwrap();

        // Create a PackageWalker for this directory
        let walker = PackageWalker::new(temp_dir.path().to_path_buf());

        // This should fail because the dev dependency can't be resolved
        let result = walker.discover_packages().await;

        println!("Walker result: {:?}", result);

        // The discovery should fail with an error about the missing dev dependency
        assert!(
            result.is_err(),
            "Walker should fail when dev dependency cannot be resolved"
        );

        let error_msg = result.unwrap_err().to_string();
        println!("Error message: {}", error_msg);
        assert!(
            error_msg.contains("Failed to resolve required dev dependency")
                || error_msg.contains("nonexistent-dev-package")
                || error_msg.contains("task") && error_msg.contains("failed"),
            "Error should mention the missing dev dependency or task failure: {}",
            error_msg
        );
    }
}
