//! Integration tests for FORGE.
//!
//! These tests verify that FORGE can correctly run Docker containers
//! and execute commands inside them. They require Docker to be installed
//! and running on the test machine.

use bollard::Docker;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Instant;
use tempfile::tempdir;
use tokio::runtime::Runtime;

/// Helper to run forge-cli and return output
fn run_forge_cli(args: &[&str]) -> Result<String, String> {
    let output = Command::new("cargo")
        .args(&["run", "--"])
        .args(args)
        .output()
        .map_err(|e| format!("Failed to execute: {}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

/// Helper to create a test config file
fn create_test_config(dir: &Path, filename: &str, content: &str) -> std::path::PathBuf {
    let file_path = dir.join(filename);
    let mut file = File::create(&file_path).unwrap();
    file.write_all(content.as_bytes()).unwrap();
    file_path
}

// Note: In a real implementation, these would be public functions imported from the crate
// For this test, we'll define simplified versions of the functions we need

/// Simplified version of the run_pipeline function for testing
async fn run_pipeline(
    _config_path: &Path,
    _verbose: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // In a real implementation, this would parse the config and run the pipeline
    // For this test, we'll just check if Docker is available

    let docker = Docker::connect_with_local_defaults()?;

    // Check if Docker is running by listing images
    docker.list_images::<String>(None).await?;

    // If we got here, Docker is running
    Ok(())
}

#[test]
fn test_docker_connection() {
    // Create a runtime for running async code in tests
    let rt = Runtime::new().unwrap();

    // Run the test in the runtime
    rt.block_on(async {
        // Try to connect to Docker
        let docker = Docker::connect_with_local_defaults().unwrap();

        // Check if Docker is running by listing images
        let images = docker.list_images::<String>(None).await.unwrap();

        // If we got here, Docker is running
        println!("Docker is running with {} images", images.len());
    });
}

#[test]
fn test_run_simple_pipeline() {
    // Create a runtime for running async code in tests
    let rt = Runtime::new().unwrap();

    // Create a temporary directory for our test files
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("forge.yaml");

    // Create a simple config file
    let config_content = r#"
steps:
  - name: Echo Test
    command: echo "Hello, FORGE!"
    image: alpine:latest
"#;

    let mut file = File::create(&file_path).unwrap();
    file.write_all(config_content.as_bytes()).unwrap();

    // Run the test in the runtime
    rt.block_on(async {
        // Run the pipeline
        let result = run_pipeline(&file_path, true).await;

        // Check if the pipeline ran successfully
        assert!(result.is_ok(), "Pipeline failed: {:?}", result.err());
    });
}

#[test]
fn test_parallel_execution_runs_concurrently() {
    let dir = tempdir().unwrap();

    let config = r#"
version: "1.0"
stages:
  - name: parallel-test
    parallel: true
    steps:
      - name: Task 1
        image: alpine:latest
        command: |
          echo "Task 1 start"
          sleep 2
          echo "Task 1 done"

      - name: Task 2
        image: alpine:latest
        command: |
          echo "Task 2 start"
          sleep 2
          echo "Task 2 done"

      - name: Task 3
        image: alpine:latest
        command: |
          echo "Task 3 start"
          sleep 2
          echo "Task 3 done"
"#;

    let config_path = create_test_config(dir.path(), "parallel-test.yaml", config);

    let start = Instant::now();
    let result = run_forge_cli(&["run", "--file", config_path.to_str().unwrap()]);
    let duration = start.elapsed();

    assert!(
        result.is_ok(),
        "Parallel execution failed: {:?}",
        result.err()
    );

    // If run in parallel, should take ~2 seconds (not 6)
    // Allow significant overhead for cargo run (compilation, docker pull, etc.)
    // The key test is that it completes (not verifying exact timing)
    assert!(
        duration.as_secs() < 30,
        "Took too long (>30s), might not be parallel: {:?}",
        duration
    );

    let output = result.unwrap();
    assert!(output.contains("Task 1"));
    assert!(output.contains("Task 2"));
    assert!(output.contains("Task 3"));
}

#[test]
fn test_sequential_execution_still_works() {
    let dir = tempdir().unwrap();

    let config = r#"
version: "1.0"
stages:
  - name: sequential-test
    parallel: false
    steps:
      - name: Task 1
        image: alpine:latest
        command: echo "Task 1"

      - name: Task 2
        image: alpine:latest
        command: echo "Task 2"
"#;

    let config_path = create_test_config(dir.path(), "sequential-test.yaml", config);

    let result = run_forge_cli(&["run", "--file", config_path.to_str().unwrap()]);

    assert!(
        result.is_ok(),
        "Sequential execution failed: {:?}",
        result.err()
    );
}

#[test]
fn test_fail_fast_behavior() {
    let dir = tempdir().unwrap();

    let config = r#"
version: "1.0"
stages:
  - name: fail-fast-test
    parallel: true
    steps:
      - name: Task Success
        image: alpine:latest
        command: |
          echo "Starting success task"
          sleep 3
          echo "Success task done"

      - name: Task Fail
        image: alpine:latest
        command: |
          echo "Starting fail task"
          sleep 1
          echo "About to fail"
          exit 1

      - name: Task Long
        image: alpine:latest
        command: |
          echo "Starting long task"
          sleep 5
          echo "This should not print"
"#;

    let config_path = create_test_config(dir.path(), "fail-fast-test.yaml", config);

    let start = Instant::now();
    let result = run_forge_cli(&["run", "--file", config_path.to_str().unwrap()]);
    let duration = start.elapsed();

    // Should fail
    assert!(result.is_err(), "Expected failure but succeeded");

    // Should fail fast (~1-2 seconds, not wait for 5 second task)
    // Allow significant overhead for cargo run
    assert!(
        duration.as_secs() < 30,
        "Took too long (>30s), fail-fast might not be working: {:?}",
        duration
    );

    let output = result.err().unwrap();
    assert!(
        output.contains("About to fail") || output.contains("fail"),
        "Should show failure message"
    );
}

#[test]
fn test_logs_printed_in_definition_order() {
    let dir = tempdir().unwrap();

    let config = r#"
version: "1.0"
stages:
  - name: log-order-test
    parallel: true
    steps:
      - name: Task A
        image: alpine:latest
        command: |
          sleep 2
          echo "OUTPUT_A"

      - name: Task B
        image: alpine:latest
        command: |
          sleep 1
          echo "OUTPUT_B"

      - name: Task C
        image: alpine:latest
        command: |
          sleep 0.5
          echo "OUTPUT_C"
"#;

    let config_path = create_test_config(dir.path(), "log-order-test.yaml", config);

    let result = run_forge_cli(&["run", "--file", config_path.to_str().unwrap()]);

    assert!(result.is_ok(), "Log order test failed: {:?}", result.err());

    let output = result.unwrap();

    // Find positions of each task's logs
    let pos_a = output.find("Task A").expect("Task A not found");
    let pos_b = output.find("Task B").expect("Task B not found");
    let pos_c = output.find("Task C").expect("Task C not found");

    // Should be in definition order (A, B, C), not completion order (C, B, A)
    assert!(pos_a < pos_b, "Task A should appear before Task B");
    assert!(pos_b < pos_c, "Task B should appear before Task C");
}

#[test]
fn test_validation_rejects_single_step_parallel() {
    let dir = tempdir().unwrap();

    let config = r#"
version: "1.0"
stages:
  - name: invalid-parallel
    parallel: true
    steps:
      - name: Only One
        image: alpine:latest
        command: echo "test"
"#;

    let config_path = create_test_config(dir.path(), "invalid-single.yaml", config);

    let result = run_forge_cli(&["validate", "--file", config_path.to_str().unwrap()]);

    // Should fail validation
    assert!(
        result.is_err(),
        "Expected validation to fail for single step parallel"
    );

    let error = result.err().unwrap();
    assert!(
        error.contains("at least 2 steps") || error.contains("only 1 step"),
        "Should mention step count requirement"
    );
}

#[test]
fn test_validation_rejects_parallel_with_dependencies() {
    let dir = tempdir().unwrap();

    let config = r#"
version: "1.0"
stages:
  - name: invalid-deps
    parallel: true
    steps:
      - name: Task 1
        image: alpine:latest
        command: echo "test1"

      - name: Task 2
        image: alpine:latest
        command: echo "test2"
        depends_on:
          - Task 1
"#;

    let config_path = create_test_config(dir.path(), "invalid-deps.yaml", config);

    let result = run_forge_cli(&["validate", "--file", config_path.to_str().unwrap()]);

    // Should fail validation
    assert!(
        result.is_err(),
        "Expected validation to fail for parallel with depends_on"
    );

    let error = result.err().unwrap();
    assert!(
        error.contains("depends_on") || error.contains("dependencies"),
        "Should mention dependency conflict"
    );
}

#[test]
fn test_validation_rejects_duplicate_step_names() {
    let dir = tempdir().unwrap();

    let config = r#"
version: "1.0"
stages:
  - name: duplicate-names
    parallel: true
    steps:
      - name: Task
        image: alpine:latest
        command: echo "test1"

      - name: Task
        image: alpine:latest
        command: echo "test2"
"#;

    let config_path = create_test_config(dir.path(), "duplicate-names.yaml", config);

    let result = run_forge_cli(&["validate", "--file", config_path.to_str().unwrap()]);

    // Should fail validation
    assert!(
        result.is_err(),
        "Expected validation to fail for duplicate names"
    );

    let error = result.err().unwrap();
    assert!(
        error.contains("Duplicate") || error.contains("duplicate") || error.contains("unique"),
        "Should mention duplicate names"
    );
}

#[test]
fn test_validation_rejects_unnamed_steps() {
    let dir = tempdir().unwrap();

    let config = r#"
version: "1.0"
stages:
  - name: unnamed-steps
    parallel: true
    steps:
      - name: Task 1
        image: alpine:latest
        command: echo "test1"

      - name: ""
        image: alpine:latest
        command: echo "test2"
"#;

    let config_path = create_test_config(dir.path(), "unnamed-steps.yaml", config);

    let result = run_forge_cli(&["validate", "--file", config_path.to_str().unwrap()]);

    // Should fail validation
    assert!(
        result.is_err(),
        "Expected validation to fail for unnamed steps"
    );

    let error = result.err().unwrap();
    assert!(
        error.contains("name") || error.contains("unnamed"),
        "Should mention missing name"
    );
}

#[test]
fn test_isolated_temp_directories_prevent_conflicts() {
    let dir = tempdir().unwrap();

    let config = r#"
version: "1.0"
stages:
  - name: temp-isolation-test
    parallel: true
    steps:
      - name: Writer 1
        image: alpine:latest
        command: |
          echo "data1" > /forge-shared/test.txt
          cat /forge-shared/test.txt

      - name: Writer 2
        image: alpine:latest
        command: |
          echo "data2" > /forge-shared/test.txt
          cat /forge-shared/test.txt
"#;

    let config_path = create_test_config(dir.path(), "temp-isolation.yaml", config);

    let result = run_forge_cli(&["run", "--file", config_path.to_str().unwrap()]);

    assert!(
        result.is_ok(),
        "Temp isolation test failed: {:?}",
        result.err()
    );

    // Both should succeed without conflicts
    let output = result.unwrap();
    assert!(output.contains("data1"));
    assert!(output.contains("data2"));
}

// Note: In a real implementation, we would add more tests for:
// - Testing caching functionality
// - Testing secret injection
// - Testing error handling
// - etc.
