use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::config::{ArtifactDownload, ArtifactUpload};

/// Metadata record for an uploaded artifact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArtifactEntry {
    /// Identifier or alias for this artifact.
    pub name: String,
    /// Original source path relative to workspace.
    pub source_path: String,
    /// Storage directory or file relative to `.forge/runs/<run-id>/artifacts/`.
    pub stored_path: String,
    /// Total size in bytes.
    pub size_bytes: u64,
    /// Total number of files uploaded in this entry.
    pub file_count: usize,
    /// SHA256 checksum (of single file or root directory index).
    pub sha256: Option<String>,
    /// Timestamp when the artifact was uploaded.
    pub created_at: DateTime<Utc>,
}

/// Computes the SHA256 hex digest for a single file.
pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Recursively copies a directory, returning `(total_bytes, total_files)`.
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<(u64, usize)> {
    fs::create_dir_all(dst)?;
    let mut total_bytes = 0;
    let mut total_files = 0;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());

        if ty.is_dir() {
            let (sub_bytes, sub_files) = copy_dir_recursive(&from, &to)?;
            total_bytes += sub_bytes;
            total_files += sub_files;
        } else {
            let bytes = fs::copy(&from, &to)?;
            total_bytes += bytes;
            total_files += 1;
        }
    }

    Ok((total_bytes, total_files))
}

/// Validates that a relative path does not escape the workspace root.
pub fn sanitize_relative_path(path_str: &str) -> Result<PathBuf, String> {
    let path = Path::new(path_str);
    for comp in path.components() {
        if matches!(comp, Component::ParentDir) {
            return Err(format!(
                "Path '{}' contains '..' which is not allowed for security reasons",
                path_str
            ));
        }
    }
    // Strip leading root or prefix if any
    let cleaned: PathBuf = path
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .collect();

    if cleaned.as_os_str().is_empty() {
        return Err(format!("Invalid empty or root path '{}'", path_str));
    }

    Ok(cleaned)
}

/// Manages artifact uploads and downloads for a pipeline run.
#[derive(Clone)]
pub struct ArtifactManager {
    workspace_dir: PathBuf,
    artifacts_dir: PathBuf,
    uploaded_entries: Arc<Mutex<Vec<ArtifactEntry>>>,
}

impl ArtifactManager {
    /// Creates a new `ArtifactManager`.
    ///
    /// * `workspace_dir` - Absolute path to host workspace.
    /// * `artifacts_dir` - Destination folder: `.forge/runs/<run-id>/artifacts`.
    pub fn new(workspace_dir: PathBuf, artifacts_dir: PathBuf) -> Self {
        Self {
            workspace_dir,
            artifacts_dir,
            uploaded_entries: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Uploads an artifact specified by `ArtifactUpload`.
    pub fn upload(
        &self,
        upload: &ArtifactUpload,
    ) -> Result<ArtifactEntry, Box<dyn std::error::Error + Send + Sync>> {
        let (raw_path, custom_name) = match upload {
            ArtifactUpload::Simple(path) => (path.as_str(), None),
            ArtifactUpload::Detailed { path, name } => (path.as_str(), name.as_deref()),
        };

        let safe_rel = sanitize_relative_path(raw_path).map_err(|e| {
            Box::new(io::Error::new(io::ErrorKind::InvalidInput, e))
                as Box<dyn std::error::Error + Send + Sync>
        })?;

        let source = self.workspace_dir.join(&safe_rel);
        if !source.exists() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "Artifact source path does not exist: '{}' (resolved to '{}')",
                    raw_path,
                    source.display()
                ),
            )));
        }

        let name = match custom_name {
            Some(n) if !n.trim().is_empty() => n.trim().to_string(),
            _ => safe_rel
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "artifact".to_string()),
        };

        let dest = self.artifacts_dir.join(&name);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }

        let is_dir = source.is_dir();
        let (size_bytes, file_count, sha256) = if is_dir {
            let (bytes, count) = copy_dir_recursive(&source, &dest)?;
            (bytes, count, None)
        } else {
            let bytes = fs::copy(&source, &dest)?;
            let hash = hash_file(&source).ok();
            (bytes, 1, hash)
        };

        let entry = ArtifactEntry {
            name: name.clone(),
            source_path: raw_path.to_string(),
            stored_path: name,
            size_bytes,
            file_count,
            sha256,
            created_at: Utc::now(),
        };

        {
            let mut list = self.uploaded_entries.lock().unwrap();
            // Remove previous version with same name if any
            list.retain(|e| e.name != entry.name);
            list.push(entry.clone());
        }

        Ok(entry)
    }

    /// Downloads an artifact specified by `ArtifactDownload`.
    pub fn download(
        &self,
        download: &ArtifactDownload,
    ) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
        let (artifact_name, dest_override) = match download {
            ArtifactDownload::Simple(name) => (name.as_str(), None),
            ArtifactDownload::Detailed { name, path } => (name.as_str(), path.as_deref()),
        };

        let entry = {
            let list = self.uploaded_entries.lock().unwrap();
            list.iter()
                .find(|e| e.name == artifact_name || e.source_path == artifact_name)
                .cloned()
        };

        let entry = match entry {
            Some(e) => e,
            None => {
                let available = self.list_uploaded();
                let available_names: Vec<String> =
                    available.iter().map(|e| e.name.clone()).collect();
                let available_str = if available_names.is_empty() {
                    "none".to_string()
                } else {
                    available_names.join(", ")
                };
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "Artifact '{}' not found. Available artifacts: {}",
                        artifact_name, available_str
                    ),
                )));
            }
        };

        let stored_source = self.artifacts_dir.join(&entry.stored_path);
        if !stored_source.exists() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "Artifact '{}' was recorded but storage file '{}' is missing",
                    entry.name,
                    stored_source.display()
                ),
            )));
        }

        let dest_rel_str = dest_override.unwrap_or(entry.source_path.as_str());
        let safe_dest_rel = sanitize_relative_path(dest_rel_str).map_err(|e| {
            Box::new(io::Error::new(io::ErrorKind::InvalidInput, e))
                as Box<dyn std::error::Error + Send + Sync>
        })?;

        let destination = self.workspace_dir.join(&safe_dest_rel);

        if stored_source.is_dir() {
            copy_dir_recursive(&stored_source, &destination)?;
        } else {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&stored_source, &destination)?;
        }

        Ok(destination)
    }

    /// Returns a list of all artifacts uploaded so far.
    pub fn list_uploaded(&self) -> Vec<ArtifactEntry> {
        let list = self.uploaded_entries.lock().unwrap();
        list.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_sanitize_relative_path() {
        assert!(sanitize_relative_path("target/release/forge").is_ok());
        assert!(sanitize_relative_path("../escape").is_err());
        assert!(sanitize_relative_path("foo/../../bar").is_err());
        assert_eq!(
            sanitize_relative_path("build/dist/output.tar.gz").unwrap(),
            PathBuf::from("build/dist/output.tar.gz")
        );
    }

    #[test]
    fn test_artifact_upload_and_download_single_file() {
        let ws = tempdir().unwrap();
        let art_dir = tempdir().unwrap();

        // Create a dummy file in workspace
        let src_file = ws.path().join("dist").join("app.txt");
        fs::create_dir_all(src_file.parent().unwrap()).unwrap();
        fs::write(&src_file, "hello artifact").unwrap();

        let mgr = ArtifactManager::new(ws.path().to_path_buf(), art_dir.path().to_path_buf());

        // Upload
        let upload = ArtifactUpload::Detailed {
            path: "dist/app.txt".to_string(),
            name: Some("my-app".to_string()),
        };
        let entry = mgr.upload(&upload).unwrap();
        assert_eq!(entry.name, "my-app");
        assert_eq!(entry.file_count, 1);
        assert_eq!(entry.size_bytes, 14);
        assert!(entry.sha256.is_some());

        // Verify stored in artifacts dir
        assert!(art_dir.path().join("my-app").exists());

        // Remove from workspace
        fs::remove_file(&src_file).unwrap();
        assert!(!src_file.exists());

        // Download to a new location
        let download = ArtifactDownload::Detailed {
            name: "my-app".to_string(),
            path: Some("restored/app.txt".to_string()),
        };
        let dest = mgr.download(&download).unwrap();
        assert!(dest.exists());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "hello artifact");
    }

    #[test]
    fn test_artifact_upload_and_download_directory() {
        let ws = tempdir().unwrap();
        let art_dir = tempdir().unwrap();

        // Create directory with multiple files
        let src_dir = ws.path().join("output");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("file1.txt"), "content 1").unwrap();
        fs::write(src_dir.join("file2.txt"), "content 2").unwrap();

        let mgr = ArtifactManager::new(ws.path().to_path_buf(), art_dir.path().to_path_buf());

        // Upload directory
        let upload = ArtifactUpload::Simple("output".to_string());
        let entry = mgr.upload(&upload).unwrap();
        assert_eq!(entry.name, "output");
        assert_eq!(entry.file_count, 2);

        // Download directory
        let download = ArtifactDownload::Detailed {
            name: "output".to_string(),
            path: Some("downloaded_output".to_string()),
        };
        let dest = mgr.download(&download).unwrap();
        assert!(dest.join("file1.txt").exists());
        assert!(dest.join("file2.txt").exists());
        assert_eq!(
            fs::read_to_string(dest.join("file1.txt")).unwrap(),
            "content 1"
        );
    }

    #[test]
    fn test_artifact_not_found_error() {
        let ws = tempdir().unwrap();
        let art_dir = tempdir().unwrap();
        let mgr = ArtifactManager::new(ws.path().to_path_buf(), art_dir.path().to_path_buf());

        let download = ArtifactDownload::Simple("nonexistent".to_string());
        let res = mgr.download(&download);
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .to_string()
                .contains("Artifact 'nonexistent' not found")
        );
    }
}
