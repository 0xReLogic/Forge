//! Comprehensive tests for FORGE Execution Control and Artifacts.
//!
//! Tests cover:
//! - Timeout and Cancellation (YAML parsing, validation, exit code 5, FailureReason::Timeout)
//! - Retry and Failure Control (retries, retry_delay, continue_on_error, fail_fast)
//! - Artifacts (upload, download, SHA256 checksums, directory recursion, path traversal protection)
//! - Manifest generation (manifest.json, artifacts directory, metadata)

use chrono::Utc;
use forge_runner::artifacts::{ArtifactEntry, ArtifactManager, hash_file, sanitize_relative_path};
use forge_runner::config::{
    ArtifactConfig, ArtifactDownload, ArtifactUpload, CacheConfig, ForgeConfig, Stage,
    parse_duration, validate_execution_control,
};
use forge_runner::persist::{RunMetadata, persist_run};
use forge_runner::result::{
    ExecutionStatus, ExitCode, FailureReason, PipelineResult, StageResult, StepResult,
};
use forge_runner::runner::ContainerTracker;
use std::collections::HashSet;
use std::fs;
use std::time::Duration;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// 1. Timeout Tests
// ---------------------------------------------------------------------------

#[test]
fn test_duration_parser_formats() {
    assert_eq!(parse_duration("10s").unwrap(), Duration::from_secs(10));
    assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
    assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
    assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
    assert_eq!(parse_duration("1m30s").unwrap(), Duration::from_secs(90));
    assert_eq!(parse_duration("45").unwrap(), Duration::from_secs(45));
    assert_eq!(parse_duration("  30s  ").unwrap(), Duration::from_secs(30));

    // Errors
    assert!(parse_duration("").is_err());
    assert!(parse_duration("   ").is_err());
    assert!(parse_duration("invalid").is_err());
    assert!(parse_duration("10x").is_err());
    assert!(parse_duration("-5s").is_err());
}

#[test]
fn test_step_timeout_configuration() {
    let yaml = r#"
version: "1.0"
stages:
  - name: test
    steps:
      - name: unit-test
        command: cargo test
        timeout: 30s
      - name: integration-test
        command: cargo test --test it
        timeout: "2m"
      - name: quick-check
        command: cargo check
        timeout: 15
"#;
    let config: ForgeConfig = serde_yaml::from_str(yaml).unwrap();
    let steps = &config.stages[0].steps;

    assert_eq!(
        steps[0].timeout_duration().unwrap(),
        Some(Duration::from_secs(30))
    );
    assert_eq!(
        steps[1].timeout_duration().unwrap(),
        Some(Duration::from_secs(120))
    );
    assert_eq!(
        steps[2].timeout_duration().unwrap(),
        Some(Duration::from_secs(15))
    );

    assert!(validate_execution_control(&config).is_ok());
}

#[test]
fn test_step_invalid_timeout_fails_validation() {
    let yaml = r#"
version: "1.0"
stages:
  - name: test
    steps:
      - name: bad-step
        command: cargo test
        timeout: "not-a-duration"
"#;
    let config: ForgeConfig = serde_yaml::from_str(yaml).unwrap();
    let err = validate_execution_control(&config).unwrap_err();
    assert!(err.to_string().contains("Invalid timeout 'not-a-duration'"));
}

#[test]
fn test_exit_code_timeout_is_5() {
    assert_eq!(ExitCode::Timeout.as_i32(), 5);

    let reason = FailureReason::Timeout {
        stage: "test".to_string(),
        step: "cargo-test".to_string(),
    };
    assert_eq!(ExitCode::from_failure_reason(&reason), ExitCode::Timeout);
    assert_eq!(reason.short_description(), "test / cargo-test (timeout)");
}

#[test]
fn test_step_result_timeout() {
    let step_res = StepResult::timeout("cargo-test", Duration::from_secs(30), "exceeded 30s");
    assert_eq!(step_res.status, ExecutionStatus::Timeout);
    assert_eq!(step_res.exit_code, Some(5));
    assert_eq!(step_res.error.as_deref(), Some("exceeded 30s"));
    assert_eq!(step_res.duration_secs, Some(30.0));
    assert_eq!(step_res.attempts, 1);
    assert!(!step_res.continued_on_error);
}

// ---------------------------------------------------------------------------
// 2. Retry and Failure Control Tests
// ---------------------------------------------------------------------------

#[test]
fn test_step_retry_configuration() {
    let yaml = r#"
version: "1.0"
stages:
  - name: test
    steps:
      - name: flaky-step
        command: ./run_flaky.sh
        retries: 3
        retry_delay: 5s
      - name: default-step
        command: ./run_stable.sh
"#;
    let config: ForgeConfig = serde_yaml::from_str(yaml).unwrap();
    let steps = &config.stages[0].steps;

    assert_eq!(steps[0].retries_count(), 3);
    assert_eq!(
        steps[0].retry_delay_duration().unwrap(),
        Duration::from_secs(5)
    );

    // Default when omitted
    assert_eq!(steps[1].retries_count(), 0);
    assert_eq!(
        steps[1].retry_delay_duration().unwrap(),
        Duration::from_secs(0)
    );

    assert!(validate_execution_control(&config).is_ok());
}

#[test]
fn test_step_invalid_retry_delay_fails_validation() {
    let yaml = r#"
version: "1.0"
stages:
  - name: test
    steps:
      - name: bad-step
        command: echo 1
        retries: 2
        retry_delay: "invalid_delay"
"#;
    let config: ForgeConfig = serde_yaml::from_str(yaml).unwrap();
    let err = validate_execution_control(&config).unwrap_err();
    assert!(
        err.to_string()
            .contains("Invalid retry_delay 'invalid_delay'")
    );
}

#[test]
fn test_continue_on_error_step_and_stage() {
    let yaml = r#"
version: "1.0"
stages:
  - name: lint-stage
    continue_on_error: true
    steps:
      - name: optional-linter
        command: cargo clippy
        continue_on_error: true
      - name: strict-check
        command: cargo check
"#;
    let config: ForgeConfig = serde_yaml::from_str(yaml).unwrap();
    let stage = &config.stages[0];
    assert!(stage.continue_on_error);
    assert!(stage.steps[0].continue_on_error);
    assert!(!stage.steps[1].continue_on_error);
}

#[test]
fn test_stage_result_with_continue_on_error() {
    let mut step1 = StepResult::failed("linter", Duration::from_secs(2), Some(1), "clippy warning");
    step1.continued_on_error = true;
    let step2 = StepResult::success("build", Duration::from_secs(5));

    let stage_res = StageResult::from_steps(
        "build-stage",
        false,
        Duration::from_secs(7),
        vec![step1, step2],
    );

    // Because the only failed step had continued_on_error == true, stage status is Success!
    assert_eq!(stage_res.status, ExecutionStatus::Success);
}

#[test]
fn test_pipeline_first_failure_ignores_continued_on_error() {
    let mut step1 = StepResult::failed("lint", Duration::from_secs(1), Some(1), "err");
    step1.continued_on_error = true;
    let step2 = StepResult::success("test", Duration::from_secs(2));

    let stage =
        StageResult::from_steps("stage-1", false, Duration::from_secs(3), vec![step1, step2]);

    let pipeline = PipelineResult {
        run_id: "test-run".into(),
        started_at: Utc::now(),
        completed_at: Utc::now(),
        duration_secs: 3.0,
        status: ExecutionStatus::Success,
        failure: None,
        stages: vec![stage],
        forge_version: "1.4.0".into(),
    };

    assert!(pipeline.first_failure().is_none());
    assert_eq!(pipeline.exit_code(), ExitCode::Success);
}

#[test]
fn test_fail_fast_configuration() {
    let yaml_default = r#"
version: "1.0"
stages:
  - name: build
    steps:
      - command: echo build
"#;
    let config_default: ForgeConfig = serde_yaml::from_str(yaml_default).unwrap();
    assert!(config_default.fail_fast);

    let yaml_false = r#"
version: "1.0"
fail_fast: false
stages:
  - name: build
    steps:
      - command: echo build
"#;
    let config_false: ForgeConfig = serde_yaml::from_str(yaml_false).unwrap();
    assert!(!config_false.fail_fast);
}

#[test]
fn test_fail_fast_false_independent_stage_dependency_logic() {
    // Stage A fails.
    // Stage B depends on A -> should be skipped.
    // Stage C is independent -> should NOT be skipped when fail_fast is false.
    let _stage_a = Stage {
        name: "A".into(),
        steps: vec![],
        parallel: false,
        depends_on: vec![],
        continue_on_error: false,
        artifacts: None,
    };
    let stage_b = Stage {
        name: "B".into(),
        steps: vec![],
        parallel: false,
        depends_on: vec!["A".into()],
        continue_on_error: false,
        artifacts: None,
    };
    let stage_c = Stage {
        name: "C".into(),
        steps: vec![],
        parallel: false,
        depends_on: vec![],
        continue_on_error: false,
        artifacts: None,
    };

    let mut failed_stages = HashSet::new();
    failed_stages.insert("A".to_string());

    let should_skip_b = stage_b
        .depends_on
        .iter()
        .any(|dep| failed_stages.contains(dep));
    let should_skip_c = stage_c
        .depends_on
        .iter()
        .any(|dep| failed_stages.contains(dep));

    assert!(
        should_skip_b,
        "Stage B must be skipped because dependency A failed"
    );
    assert!(!should_skip_c, "Stage C is independent and should execute");
}

// ---------------------------------------------------------------------------
// 3. Artifacts Tests
// ---------------------------------------------------------------------------

#[test]
fn test_artifact_yaml_deserialization() {
    let yaml = r#"
upload:
  - "dist/bin/forge"
  - name: logs-bundle
    path: "var/logs"
download:
  - "dist/bin/forge"
  - name: vendor-cache
    path: "vendor/"
"#;
    let art: ArtifactConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(art.upload.len(), 2);
    assert_eq!(art.download.len(), 2);

    assert_eq!(art.upload[0].path(), "dist/bin/forge");
    assert_eq!(art.upload[0].name(), None);
    assert_eq!(art.upload[1].path(), "var/logs");
    assert_eq!(art.upload[1].name(), Some("logs-bundle"));

    assert_eq!(art.download[0].name(), "dist/bin/forge");
    assert_eq!(art.download[0].destination(), None);
    assert_eq!(art.download[1].name(), "vendor-cache");
    assert_eq!(art.download[1].destination(), Some("vendor/"));
}

#[test]
fn test_artifact_manager_upload_and_download_flow() {
    let ws = tempdir().unwrap();
    let runs_dir = tempdir().unwrap();
    let artifacts_dir = runs_dir.path().join("artifacts");

    let mgr = ArtifactManager::new(ws.path().to_path_buf(), artifacts_dir.clone());

    // 1. Create a build artifact file in workspace
    let src_file = ws.path().join("target").join("release").join("app");
    fs::create_dir_all(src_file.parent().unwrap()).unwrap();
    fs::write(&src_file, "binary executable content 12345").unwrap();

    // 2. Upload artifact
    let upload = ArtifactUpload::Detailed {
        path: "target/release/app".into(),
        name: Some("app-binary".into()),
    };
    let entry = mgr.upload(&upload).unwrap();

    assert_eq!(entry.name, "app-binary");
    assert_eq!(entry.file_count, 1);
    assert_eq!(entry.size_bytes, 31);
    assert!(entry.sha256.is_some());
    assert!(artifacts_dir.join("app-binary").exists());

    // 3. Delete file in workspace
    fs::remove_file(&src_file).unwrap();
    assert!(!src_file.exists());

    // 4. Download artifact to different location
    let download = ArtifactDownload::Detailed {
        name: "app-binary".into(),
        path: Some("bin/app-restored".into()),
    };
    let restored_path = mgr.download(&download).unwrap();

    assert!(restored_path.exists());
    assert_eq!(
        fs::read_to_string(&restored_path).unwrap(),
        "binary executable content 12345"
    );

    // 5. Verify listed uploaded artifacts
    let uploaded = mgr.list_uploaded();
    assert_eq!(uploaded.len(), 1);
    assert_eq!(uploaded[0].name, "app-binary");
}

#[test]
fn test_artifact_manager_upload_directory() {
    let ws = tempdir().unwrap();
    let runs_dir = tempdir().unwrap();
    let artifacts_dir = runs_dir.path().join("artifacts");

    let mgr = ArtifactManager::new(ws.path().to_path_buf(), artifacts_dir.clone());

    // Create a directory with nested structure
    let dir_path = ws.path().join("dist");
    fs::create_dir_all(dir_path.join("sub")).unwrap();
    fs::write(dir_path.join("file1.txt"), "hello").unwrap();
    fs::write(dir_path.join("sub").join("file2.txt"), "world").unwrap();

    let upload = ArtifactUpload::Simple("dist".into());
    let entry = mgr.upload(&upload).unwrap();
    assert_eq!(entry.name, "dist");
    assert_eq!(entry.file_count, 2);

    // Download to new folder
    let download = ArtifactDownload::Detailed {
        name: "dist".into(),
        path: Some("downloaded_dist".into()),
    };
    let dest = mgr.download(&download).unwrap();
    assert!(dest.join("file1.txt").exists());
    assert!(dest.join("sub").join("file2.txt").exists());
    assert_eq!(fs::read_to_string(dest.join("file1.txt")).unwrap(), "hello");
    assert_eq!(
        fs::read_to_string(dest.join("sub").join("file2.txt")).unwrap(),
        "world"
    );
}

#[test]
fn test_artifact_path_traversal_protection() {
    assert!(sanitize_relative_path("../../../etc/passwd").is_err());
    assert!(sanitize_relative_path("safe/path").is_ok());

    let yaml = r#"
version: "1.0"
stages:
  - name: test
    artifacts:
      upload:
        - "../secret_file"
    steps:
      - command: echo 1
"#;
    let config: ForgeConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(validate_execution_control(&config).is_err());
}

#[test]
fn test_artifact_sha256_checksum() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("sample.txt");
    fs::write(&file, "hello world\n").unwrap();

    let hash = hash_file(&file).unwrap();
    // echo -n "hello world\n" | sha256sum
    assert_eq!(
        hash,
        "a948904f2f0f479b8f8197694b30184b0d2ed1c1cd2a1ec0fb85d299a192a447"
    );
}

// ---------------------------------------------------------------------------
// 4. Persistence and Manifest Tests
// ---------------------------------------------------------------------------

#[test]
fn test_persist_run_generates_manifest_json() {
    let dir = tempdir().unwrap();
    let now = Utc::now();

    let result = PipelineResult {
        run_id: "v14-run-001".into(),
        started_at: now,
        completed_at: now,
        duration_secs: 1.25,
        status: ExecutionStatus::Success,
        failure: None,
        stages: vec![StageResult::from_steps(
            "build",
            false,
            Duration::from_millis(1250),
            vec![StepResult::success("compile", Duration::from_millis(1250))],
        )],
        forge_version: "1.4.0".into(),
    };

    let meta = RunMetadata {
        run_id: "v14-run-001".into(),
        started_at: now,
        forge_version: "1.4.0".into(),
        config_path: "/workspace/forge.yaml".into(),
        git_commit: Some("c52faf2".into()),
        git_branch: Some("main".into()),
        os: "linux".into(),
    };

    let config = ForgeConfig {
        version: "1.0".into(),
        fail_fast: true,
        stages: vec![],
        steps: vec![],
        cache: CacheConfig::default(),
        secrets: vec![],
    };

    let artifacts = vec![ArtifactEntry {
        name: "bin".into(),
        source_path: "target/release/forge".into(),
        stored_path: "bin".into(),
        size_bytes: 1024,
        file_count: 1,
        sha256: Some("abcdef123456".into()),
        created_at: now,
    }];

    let run_path = persist_run(dir.path(), &result, &meta, &config, &artifacts).unwrap();

    assert!(run_path.join("result.json").exists());
    assert!(run_path.join("metadata.json").exists());
    assert!(run_path.join("manifest.json").exists());
    assert!(run_path.join("logs").is_dir());
    assert!(run_path.join("artifacts").is_dir());

    let manifest_content = fs::read_to_string(run_path.join("manifest.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&manifest_content).unwrap();

    assert_eq!(parsed["run_id"], "v14-run-001");
    assert_eq!(parsed["forge_version"], "1.4.0");
    assert_eq!(parsed["artifacts"][0]["name"], "bin");
    assert_eq!(parsed["artifacts"][0]["size_bytes"], 1024);
    assert_eq!(parsed["artifacts"][0]["sha256"], "abcdef123456");
    assert_eq!(parsed["metadata"]["git_commit"], "c52faf2");
}

// ---------------------------------------------------------------------------
// 5. Exit Code and Cancellation Tests
// ---------------------------------------------------------------------------

#[test]
fn test_all_exit_codes() {
    assert_eq!(ExitCode::Success.as_i32(), 0);
    assert_eq!(ExitCode::PipelineFailure.as_i32(), 1);
    assert_eq!(ExitCode::ConfigError.as_i32(), 2);
    assert_eq!(ExitCode::RuntimeError.as_i32(), 3);
    assert_eq!(ExitCode::Cancelled.as_i32(), 4);
    assert_eq!(ExitCode::Timeout.as_i32(), 5);

    assert_eq!(
        ExitCode::from_failure_reason(&FailureReason::Cancelled),
        ExitCode::Cancelled
    );
    assert_eq!(
        ExitCode::from_failure_reason(&FailureReason::Timeout {
            stage: "s".into(),
            step: "p".into()
        }),
        ExitCode::Timeout
    );
}

#[tokio::test]
async fn test_container_tracker_operations() {
    let tracker = ContainerTracker::new();
    tracker.track("container-1".into()).await;
    tracker.track("container-2".into()).await;

    {
        let set = tracker.active_containers.lock().await;
        assert_eq!(set.len(), 2);
        assert!(set.contains("container-1"));
        assert!(set.contains("container-2"));
    }

    tracker.untrack("container-1").await;
    {
        let set = tracker.active_containers.lock().await;
        assert_eq!(set.len(), 1);
        assert!(!set.contains("container-1"));
        assert!(set.contains("container-2"));
    }
}
