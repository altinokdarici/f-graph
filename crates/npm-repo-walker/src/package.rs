//! Package.json parsing and package definition structures
//!
//! This module provides functionality for reading and parsing package.json files,
//! as well as defining the core data structures for representing npm packages.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;

/// Raw package.json structure as it appears in the filesystem
///
/// This struct represents the actual contents of a package.json file,
/// including optional fields that may or may not be present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageJson {
    pub name: String,
    pub version: Option<String>,
    pub dependencies: Option<HashMap<String, String>>,
    #[serde(rename = "devDependencies")]
    pub dev_dependencies: Option<HashMap<String, String>>,
    #[serde(rename = "optionalDependencies")]
    pub optional_dependencies: Option<HashMap<String, String>>,
    #[serde(rename = "peerDependencies")]
    pub peer_dependencies: Option<HashMap<String, String>>,
}

/// Internal representation of a discovered package
///
/// This struct represents a processed package with all the information
/// needed for dependency resolution and tracking within the walker.
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: Option<String>,
    pub path: PathBuf,
    pub dependencies: Vec<String>,
}

impl Package {
    /// Creates a new Package instance
    ///
    /// # Arguments
    /// * `name` - The package name
    /// * `version` - Optional version string
    /// * `path` - Filesystem path to the package directory
    /// * `dependencies` - List of dependency names
    pub fn new(
        name: String,
        version: Option<String>,
        path: PathBuf,
        dependencies: Vec<String>,
    ) -> Self {
        Self {
            name,
            version,
            path,
            dependencies,
        }
    }
}

/// Reads and parses a package.json file from the given directory
///
/// This function looks for a package.json file in the specified directory,
/// reads its contents, and parses it into a PackageJson struct.
///
/// # Arguments
/// * `package_path` - Path to the directory containing package.json
///
/// # Returns
/// * `Ok(PackageJson)` - Successfully parsed package.json
/// * `Err(anyhow::Error)` - File not found, read error, or parse error
///
pub async fn read_package_json(package_path: &Path) -> Result<PackageJson> {
    let package_json_path = package_path.join("package.json");
    let content = fs::read_to_string(&package_json_path)
        .await
        .with_context(|| format!("Failed to read package.json at {:?}", package_json_path))?;

    let package_json: PackageJson = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse package.json at {:?}", package_json_path))?;

    Ok(package_json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_read_package_json_success() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let package_json_content = r#"
        {
            "name": "test-package",
            "version": "1.0.0",
            "dependencies": {
                "lodash": "^4.17.21",
                "express": "^4.18.0"
            },
            "optionalDependencies": {
                "fsevents": "^2.3.2"
            }
        }
        "#;

        fs::write(temp_dir.path().join("package.json"), package_json_content)?;

        let package_json = read_package_json(temp_dir.path()).await?;
        assert_eq!(package_json.name, "test-package");
        assert_eq!(package_json.version, Some("1.0.0".to_string()));
        assert!(package_json.dependencies.is_some());
        assert!(package_json.optional_dependencies.is_some());

        let deps = package_json.dependencies.unwrap();
        assert_eq!(deps.get("lodash"), Some(&"^4.17.21".to_string()));
        assert_eq!(deps.get("express"), Some(&"^4.18.0".to_string()));

        let optional_deps = package_json.optional_dependencies.unwrap();
        assert_eq!(optional_deps.get("fsevents"), Some(&"^2.3.2".to_string()));

        Ok(())
    }

    #[tokio::test]
    async fn test_read_package_json_minimal() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let package_json_content = r#"{"name": "minimal-package"}"#;

        fs::write(temp_dir.path().join("package.json"), package_json_content)?;

        let package_json = read_package_json(temp_dir.path()).await?;
        assert_eq!(package_json.name, "minimal-package");
        assert_eq!(package_json.version, None);
        assert_eq!(package_json.dependencies, None);

        Ok(())
    }

    #[tokio::test]
    async fn test_read_package_json_not_found() {
        let temp_dir = TempDir::new().unwrap();

        let result = read_package_json(temp_dir.path()).await;
        assert!(result.is_err());

        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("Failed to read package.json"));
    }

    #[tokio::test]
    async fn test_read_package_json_invalid_json() {
        let temp_dir = TempDir::new().unwrap();
        let invalid_content = "{ invalid json content";

        fs::write(temp_dir.path().join("package.json"), invalid_content).unwrap();

        let result = read_package_json(temp_dir.path()).await;
        assert!(result.is_err());

        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("Failed to parse package.json"));
    }

    #[tokio::test]
    async fn test_read_package_json_with_optional_and_peer_deps() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let package_json_content = r#"
        {
            "name": "full-package",
            "version": "2.0.0",
            "dependencies": {
                "lodash": "^4.17.21"
            },
            "devDependencies": {
                "jest": "^29.0.0"
            },
            "optionalDependencies": {
                "fsevents": "^2.3.2",
                "chokidar": "^3.5.3"
            },
            "peerDependencies": {
                "react": ">=16.0.0"
            }
        }
        "#;

        fs::write(temp_dir.path().join("package.json"), package_json_content)?;

        let package_json = read_package_json(temp_dir.path()).await?;
        assert_eq!(package_json.name, "full-package");
        assert_eq!(package_json.version, Some("2.0.0".to_string()));

        // Test all dependency types
        let deps = package_json.dependencies.unwrap();
        assert_eq!(deps.len(), 1);
        assert!(deps.contains_key("lodash"));

        let dev_deps = package_json.dev_dependencies.unwrap();
        assert_eq!(dev_deps.len(), 1);
        assert!(dev_deps.contains_key("jest"));

        let opt_deps = package_json.optional_dependencies.unwrap();
        assert_eq!(opt_deps.len(), 2);
        assert!(opt_deps.contains_key("fsevents"));
        assert!(opt_deps.contains_key("chokidar"));

        let peer_deps = package_json.peer_dependencies.unwrap();
        assert_eq!(peer_deps.len(), 1);
        assert!(peer_deps.contains_key("react"));

        Ok(())
    }

    #[test]
    fn test_package_creation() {
        let package = Package::new(
            "test-pkg".to_string(),
            Some("1.0.0".to_string()),
            PathBuf::from("/path/to/pkg"),
            vec!["dep1".to_string(), "dep2".to_string()],
        );

        assert_eq!(package.name, "test-pkg");
        assert_eq!(package.version, Some("1.0.0".to_string()));
        assert_eq!(package.path, PathBuf::from("/path/to/pkg"));
        assert_eq!(package.dependencies, vec!["dep1", "dep2"]);
    }
}
