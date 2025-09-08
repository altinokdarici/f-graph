//! Node.js module resolution algorithm with symlink support
//!
//! This module implements dependency resolution logic compatible with npm, yarn, and pnpm
//! package managers, including proper handling of symlinked node_modules structures.
//! Based on Microsoft CloudPack's findPackage implementation.

use std::path::{Path, PathBuf};

use crate::debug_log;

/// Enhanced Node.js module resolution algorithm with symlink support
///
/// Searches for a dependency by name starting from a given path and traversing upward
/// through the directory structure, checking node_modules folders along the way.
///
/// # Features
/// - Symlink resolution using fs::canonicalize
/// - pnpm store layout detection and handling
/// - Automatic scope folder skipping (e.g., @scope packages)
/// - Nested node_modules folder handling
/// - Cross-platform path separator support
///
/// # Arguments
/// * `root_path` - Starting directory to search from
/// * `dependency_name` - Name of the package to find
///
/// # Returns
/// * `Some(PathBuf)` - Canonicalized path to the resolved package directory
/// * `None` - Package not found in any accessible node_modules
pub fn resolve_dependency_path(root_path: &Path, dependency_name: &str) -> Option<PathBuf> {
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

        // Detect if we're in a pnpm store layout
        let is_store_layout = current_dir.to_string_lossy().contains("/.pnpm/")
            || current_dir.to_string_lossy().contains("\\.pnpm\\")
            || current_dir.to_string_lossy().contains("/.store/")
            || current_dir.to_string_lossy().contains("\\.store\\");

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

/// Attempts to resolve a path with symlink support
///
/// This function checks if a path exists and attempts to canonicalize it to resolve
/// any symbolic links. It then verifies that a package.json exists at the resolved location.
///
/// # Arguments
/// * `path` - The path to resolve and validate
///
/// # Returns  
/// * `Some(PathBuf)` - Canonicalized path if valid package found
/// * `None` - Path doesn't exist, can't be resolved, or doesn't contain package.json
pub fn try_resolve_with_symlinks(path: &Path) -> Option<PathBuf> {
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
