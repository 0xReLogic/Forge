//! Run persistence: writes result.json, metadata.json, and creates logs/ under
//! `.forge/runs/<run-id>/` after every pipeline execution.
//! Secrets must never reach this module. Callers are responsible for ensuring
//! no secret values appear in PipelineResult or RunMetadata.

use crate::result::PipelineResult;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Allowlist-based metadata stored alongside result.json.
/// Only fields explicitly listed here are persisted — no blind env dump.
#[derive(Debug, Serialize, Deserialize)]
pub struct RunMetadata {
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub forge_version: String,
    /// Absolute path to the forge.yaml that was executed.
    pub config_path: String,
    /// Git commit hash if available (from `git rev-parse HEAD`).
    pub git_commit: Option<String>,
    /// Git branch if available.
    pub git_branch: Option<String>,
    /// Operating system name (e.g. "linux", "macos", "windows").
    pub os: String,
}

/// Returns the run directory: `<workspace>/.forge/runs/<run-id>`.
pub fn run_dir(workspace_dir: &Path, run_id: &str) -> PathBuf {
    workspace_dir.join(".forge").join("runs").join(run_id)
}

/// Persists a completed pipeline run to disk.
///
/// Creates:
/// ```text
/// .forge/runs/<run-id>/
/// ├── result.json
/// ├── metadata.json
/// └── logs/          (empty directory — reserved for per-step log files)
/// ```
///
/// Failures are non-fatal: a persistence error is returned but the caller
/// decides whether to surface it. The pipeline result itself is unaffected.
pub fn persist_run(
    workspace_dir: &Path,
    result: &PipelineResult,
    metadata: &RunMetadata,
) -> io::Result<PathBuf> {
    let dir = run_dir(workspace_dir, &result.run_id);
    fs::create_dir_all(&dir)?;

    // result.json
    let result_json = serde_json::to_string_pretty(result)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(dir.join("result.json"), result_json.as_bytes())?;

    // metadata.json
    let meta_json = serde_json::to_string_pretty(metadata)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(dir.join("metadata.json"), meta_json.as_bytes())?;

    // logs/ directory — steps will write here in a future version
    fs::create_dir_all(dir.join("logs"))?;

    Ok(dir)
}

/// Reads git information from the workspace directory.
/// Returns `(commit, branch)`, both `None` if git is unavailable.
pub fn read_git_info(workspace_dir: &Path) -> (Option<String>, Option<String>) {
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string());

    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(workspace_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string());

    (commit, branch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::{ExecutionStatus, PipelineResult, StageResult, StepResult};
    use chrono::Utc;
    use std::time::Duration;
    use tempfile::tempdir;

    fn make_result(run_id: &str) -> PipelineResult {
        let now = Utc::now();
        PipelineResult {
            run_id: run_id.into(),
            started_at: now,
            completed_at: now,
            duration_secs: 5.0,
            status: ExecutionStatus::Success,
            failure: None,
            stages: vec![StageResult::from_steps(
                "build",
                false,
                Duration::from_secs(5),
                vec![StepResult::success("compile", Duration::from_secs(5))],
            )],
            forge_version: "v1.2.0".into(),
        }
    }

    fn make_metadata(run_id: &str) -> RunMetadata {
        RunMetadata {
            run_id: run_id.into(),
            started_at: Utc::now(),
            forge_version: "v1.2.0".into(),
            config_path: "/tmp/forge.yaml".into(),
            git_commit: None,
            git_branch: None,
            os: std::env::consts::OS.to_string(),
        }
    }

    #[test]
    fn persist_run_creates_expected_files() {
        let dir = tempdir().unwrap();
        let result = make_result("abc-123");
        let meta = make_metadata("abc-123");

        let run_path = persist_run(dir.path(), &result, &meta).unwrap();

        assert!(run_path.join("result.json").exists());
        assert!(run_path.join("metadata.json").exists());
        assert!(run_path.join("logs").is_dir());
    }

    #[test]
    fn persist_run_result_json_is_valid() {
        let dir = tempdir().unwrap();
        let result = make_result("def-456");
        let meta = make_metadata("def-456");
        let run_path = persist_run(dir.path(), &result, &meta).unwrap();

        let content = fs::read_to_string(run_path.join("result.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("valid JSON");
        assert_eq!(parsed["run_id"], "def-456");
        assert_eq!(parsed["status"], "success");
    }

    #[test]
    fn persist_run_metadata_json_is_valid() {
        let dir = tempdir().unwrap();
        let result = make_result("ghi-789");
        let meta = make_metadata("ghi-789");
        let run_path = persist_run(dir.path(), &result, &meta).unwrap();

        let content = fs::read_to_string(run_path.join("metadata.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("valid JSON");
        assert_eq!(parsed["run_id"], "ghi-789");
        assert_eq!(parsed["forge_version"], "v1.2.0");
    }

    #[test]
    fn run_dir_path_is_correct() {
        let base = Path::new("/workspace");
        let p = run_dir(base, "my-run-id");
        assert_eq!(p, Path::new("/workspace/.forge/runs/my-run-id"));
    }

    #[test]
    fn persist_run_is_idempotent() {
        let dir = tempdir().unwrap();
        let result = make_result("idem-001");
        let meta = make_metadata("idem-001");
        persist_run(dir.path(), &result, &meta).unwrap();
        persist_run(dir.path(), &result, &meta).unwrap();
        let content =
            fs::read_to_string(run_dir(dir.path(), "idem-001").join("result.json")).unwrap();
        assert!(content.contains("idem-001"));
    }
}
