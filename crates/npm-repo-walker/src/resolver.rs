//! Node.js module resolution algorithm with symlink support
//!
//! This module implements dependency resolution logic compatible with npm, yarn, and pnpm
//! package managers, including proper handling of symlinked node_modules structures.
//! Based on Microsoft CloudPack's findPackage implementation.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use lru::LruCache;
use std::num::NonZeroUsize;

use crate::debug_log;

// Global cache for canonicalized paths
static PATH_CACHE: std::sync::LazyLock<Mutex<LruCache<PathBuf, Option<PathBuf>>>> = 
    std::sync::LazyLock::new(|| {
        Mutex::new(LruCache::new(NonZeroUsize::new(10000).unwrap()))
    });

// Global cache for resolved dependency paths
static RESOLUTION_CACHE: std::sync::LazyLock<Mutex<LruCache<(PathBuf, String), Option<PathBuf>>>> = 
    std::sync::LazyLock::new(|| {
        Mutex::new(LruCache::new(NonZeroUsize::new(5000).unwrap()))
    });

/// Enhanced Node.js module resolution algorithm with symlink support and caching
///
/// Searches for a dependency by name starting from a given path and traversing upward
/// through the directory structure, checking node_modules folders along the way.
///
/// # Features
/// - Symlink resolution using fs::canonicalize with LRU cache
/// - pnpm store layout detection and handling
/// - Automatic scope folder skipping (e.g., @scope packages)
/// - Nested node_modules folder handling
/// - Cross-platform path separator support
/// - LRU caching for resolved paths
///
/// # Arguments
/// * `root_path` - Starting directory to search from
/// * `dependency_name` - Name of the package to find
///
/// # Returns
/// * `Some(PathBuf)` - Canonicalized path to the resolved package directory
/// * `None` - Package not found in any accessible node_modules
pub fn resolve_dependency_path(root_path: &Path, dependency_name: &str) -> Option<PathBuf> {
    // Check resolution cache first
    let cache_key = (root_path.to_path_buf(), dependency_name.to_string());
    if let Ok(mut cache) = RESOLUTION_CACHE.lock() {
        if let Some(cached_result) = cache.get(&cache_key) {
            debug_log!(
                "📋 Cache hit for dependency resolution: {} from {:?}", 
                dependency_name, 
                root_path
            );
            return cached_result.clone();
        }
    }
    
    let result = resolve_dependency_path_uncached(root_path, dependency_name);
    
    // Cache the result
    if let Ok(mut cache) = RESOLUTION_CACHE.lock() {
        cache.put(cache_key, result.clone());
    }
    
    result
}

/// Internal uncached version of resolve_dependency_path
fn resolve_dependency_path_uncached(root_path: &Path, dependency_name: &str) -> Option<PathBuf> {
    let mut current_dir = root_path.to_path_buf();

    loop {
        let node_modules_path = current_dir.join("node_modules").join(dependency_name);
        debug_log!(
            "🔎 Checking for {} at {:?}",
            dependency_name,
            node_modules_path
        );

        // Try to resolve with symlink support
        if let Some(resolved_path) = try_resolve_with_symlinks(&node_modules_path) {
            return Some(resolved_path);
        }

        // Move up one directory level
        let parent = match current_dir.parent() {
            Some(parent) => parent.to_path_buf(),
            None => {
                println!(
                    "❌ Reached filesystem root without finding {} from {:?}",
                    dependency_name, root_path
                );
                break;
            }
        };

        // Detect if we're in a pnpm store layout (use single string conversion)
        let current_dir_str = current_dir.to_string_lossy();
        let is_store_layout = current_dir_str.contains("/.pnpm/")
            || current_dir_str.contains("\\.pnpm\\")
            || current_dir_str.contains("/.store/")
            || current_dir_str.contains("\\.store\\");

        current_dir = parent;

        // Skip over scope folders (unless in store layout)
        if !is_store_layout
            && let Some(basename) = current_dir.file_name()
            && basename.to_string_lossy().starts_with('@')
            && let Some(parent) = current_dir.parent()
        {
            current_dir = parent.to_path_buf();
        }

        // Skip over nested node_modules folders
        if current_dir
            .file_name()
            .is_some_and(|name| name == "node_modules")
            && let Some(parent) = current_dir.parent()
        {
            current_dir = parent.to_path_buf();
        }

        // Prevent infinite loops
        if current_dir == current_dir.parent().unwrap_or(&current_dir) {
            break;
        }
    }

    None
}

/// Attempts to resolve a path with symlink support and caching
///
/// This function checks if a path exists and attempts to canonicalize it to resolve
/// any symbolic links. It then verifies that a package.json exists at the resolved location.
/// Results are cached using an LRU cache to improve performance.
///
/// # Arguments
/// * `path` - The path to resolve and validate
///
/// # Returns  
/// * `Some(PathBuf)` - Canonicalized path if valid package found
/// * `None` - Path doesn't exist, can't be resolved, or doesn't contain package.json
pub fn try_resolve_with_symlinks(path: &Path) -> Option<PathBuf> {
    // Check cache first
    let path_buf = path.to_path_buf();
    if let Ok(mut cache) = PATH_CACHE.lock() {
        if let Some(cached_result) = cache.get(&path_buf) {
            debug_log!(
                "📋 Cache hit for path resolution: {:?}", 
                path
            );
            return cached_result.clone();
        }
    }
    
    let result = try_resolve_with_symlinks_uncached(path);
    
    // Cache the result
    if let Ok(mut cache) = PATH_CACHE.lock() {
        cache.put(path_buf, result.clone());
    }
    
    result
}

/// Internal uncached version of try_resolve_with_symlinks
fn try_resolve_with_symlinks_uncached(path: &Path) -> Option<PathBuf> {
    // First check if the path exists
    if !path.exists() {
        return None;
    }

    // Try to canonicalize (resolve symlinks) like fs.realpathSync.native
    let real_path = match std::fs::canonicalize(path) {
        Ok(resolved) => resolved,
        Err(_) => {
            // If canonicalization fails, fall back to the original path
            // This handles cases where symlinks might be broken but path exists
            path.to_path_buf()
        }
    };

    // Verify package.json exists at the resolved location
    if real_path.join("package.json").exists() {
        Some(real_path)
    } else {
        None
    }
}

/// Async version for future use - currently not used to avoid breaking changes
#[allow(dead_code)]
async fn try_resolve_with_symlinks_async(path: &Path) -> Option<PathBuf> {
    // Check if path exists asynchronously
    if tokio::fs::try_exists(path).await.unwrap_or(false) {
        return None;
    }

    // Try to canonicalize (resolve symlinks)
    let real_path = match tokio::fs::canonicalize(path).await {
        Ok(resolved) => resolved,
        Err(_) => path.to_path_buf(),
    };

    // Verify package.json exists at the resolved location
    let package_json_path = real_path.join("package.json");
    if tokio::fs::try_exists(&package_json_path).await.unwrap_or(false) {
        Some(real_path)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_try_resolve_with_symlinks_nonexistent_path() {
        let temp_dir = TempDir::new().unwrap();
        let nonexistent = temp_dir.path().join("nonexistent");

        assert_eq!(try_resolve_with_symlinks(&nonexistent), None);
    }

    #[test]
    fn test_try_resolve_with_symlinks_no_package_json() {
        let temp_dir = TempDir::new().unwrap();
        let empty_dir = temp_dir.path().join("empty");
        fs::create_dir(&empty_dir).unwrap();

        assert_eq!(try_resolve_with_symlinks(&empty_dir), None);
    }

    #[test]
    fn test_try_resolve_with_symlinks_valid_package() {
        let temp_dir = TempDir::new().unwrap();
        let package_dir = temp_dir.path().join("valid-package");
        fs::create_dir(&package_dir).unwrap();
        fs::write(package_dir.join("package.json"), r#"{"name":"test"}"#).unwrap();

        let resolved = try_resolve_with_symlinks(&package_dir).unwrap();
        assert!(resolved.join("package.json").exists());
        assert!(resolved.is_absolute());
    }

    #[test]
    fn test_resolve_dependency_path_not_found() {
        let temp_dir = TempDir::new().unwrap();

        assert_eq!(
            resolve_dependency_path(temp_dir.path(), "nonexistent-package"),
            None
        );
    }
}
