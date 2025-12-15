//! Performance benchmarks for parallel vs sequential execution.
//!
//! These benchmarks measure the performance improvement of parallel execution
//! over sequential execution for various scenarios.
//!
//! Run with: cargo bench
//!
//! Note: Make sure to build in release mode first: cargo build --release

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

/// Helper to get the path to the release binary
fn get_forge_binary() -> PathBuf {
    let mut path = std::env::current_dir().unwrap();
    path.push("target");
    path.push("release");
    path.push("forge");
    path
}

/// Helper to run forge with timing
fn run_forge(binary_path: &PathBuf, config_path: &str) -> std::time::Duration {
    let start = std::time::Instant::now();

    let output = Command::new(binary_path)
        .args(&["run", "--file", config_path])
        .output()
        .expect("Failed to execute forge");

    let duration = start.elapsed();

    assert!(
        output.status.success(),
        "Forge execution failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    duration
}

/// Benchmark: 3 tasks, 2 seconds each
fn benchmark_3_tasks_2s(c: &mut Criterion) {
    // Ensure binary is built
    let binary_path = get_forge_binary();
    if !binary_path.exists() {
        panic!("Binary not found. Please run 'cargo build --release' first");
    }

    let dir = tempdir().unwrap();

    // Sequential version
    let seq_config = r#"
version: "1.0"
stages:
  - name: sequential
    parallel: false
    steps:
      - name: Task 1
        image: alpine:latest
        command: sleep 2
      - name: Task 2
        image: alpine:latest
        command: sleep 2
      - name: Task 3
        image: alpine:latest
        command: sleep 2
"#;

    let seq_path = dir.path().join("sequential.yaml");
    File::create(&seq_path)
        .unwrap()
        .write_all(seq_config.as_bytes())
        .unwrap();

    // Parallel version
    let par_config = r#"
version: "1.0"
stages:
  - name: parallel
    parallel: true
    steps:
      - name: Task 1
        image: alpine:latest
        command: sleep 2
      - name: Task 2
        image: alpine:latest
        command: sleep 2
      - name: Task 3
        image: alpine:latest
        command: sleep 2
"#;

    let par_path = dir.path().join("parallel.yaml");
    File::create(&par_path)
        .unwrap()
        .write_all(par_config.as_bytes())
        .unwrap();

    let mut group = c.benchmark_group("3_tasks_2s_each");
    // Reduce sample size for faster benchmarks (Docker operations are slow)
    group.sample_size(10);
    // Increase measurement time to account for Docker overhead
    group.measurement_time(std::time::Duration::from_secs(180));

    group.bench_with_input(
        BenchmarkId::new("sequential", "3x2s"),
        &seq_path,
        |b, path| {
            b.iter(|| run_forge(black_box(&binary_path), black_box(path.to_str().unwrap())));
        },
    );

    group.bench_with_input(
        BenchmarkId::new("parallel", "3x2s"),
        &par_path,
        |b, path| {
            b.iter(|| run_forge(black_box(&binary_path), black_box(path.to_str().unwrap())));
        },
    );

    group.finish();
}

/// Benchmark: 5 tasks, 1 second each
fn benchmark_5_tasks_1s(c: &mut Criterion) {
    let binary_path = get_forge_binary();
    if !binary_path.exists() {
        panic!("Binary not found. Please run 'cargo build --release' first");
    }

    let dir = tempdir().unwrap();

    // Sequential version
    let seq_config = r#"
version: "1.0"
stages:
  - name: sequential
    parallel: false
    steps:
      - name: Task 1
        image: alpine:latest
        command: sleep 1
      - name: Task 2
        image: alpine:latest
        command: sleep 1
      - name: Task 3
        image: alpine:latest
        command: sleep 1
      - name: Task 4
        image: alpine:latest
        command: sleep 1
      - name: Task 5
        image: alpine:latest
        command: sleep 1
"#;

    let seq_path = dir.path().join("sequential_5.yaml");
    File::create(&seq_path)
        .unwrap()
        .write_all(seq_config.as_bytes())
        .unwrap();

    // Parallel version
    let par_config = r#"
version: "1.0"
stages:
  - name: parallel
    parallel: true
    steps:
      - name: Task 1
        image: alpine:latest
        command: sleep 1
      - name: Task 2
        image: alpine:latest
        command: sleep 1
      - name: Task 3
        image: alpine:latest
        command: sleep 1
      - name: Task 4
        image: alpine:latest
        command: sleep 1
      - name: Task 5
        image: alpine:latest
        command: sleep 1
"#;

    let par_path = dir.path().join("parallel_5.yaml");
    File::create(&par_path)
        .unwrap()
        .write_all(par_config.as_bytes())
        .unwrap();

    let mut group = c.benchmark_group("5_tasks_1s_each");
    // Reduce sample size for faster benchmarks
    group.sample_size(10);
    // Increase measurement time to account for Docker overhead
    group.measurement_time(std::time::Duration::from_secs(120));

    group.bench_with_input(
        BenchmarkId::new("sequential", "5x1s"),
        &seq_path,
        |b, path| {
            b.iter(|| run_forge(black_box(&binary_path), black_box(path.to_str().unwrap())));
        },
    );

    group.bench_with_input(
        BenchmarkId::new("parallel", "5x1s"),
        &par_path,
        |b, path| {
            b.iter(|| run_forge(black_box(&binary_path), black_box(path.to_str().unwrap())));
        },
    );

    group.finish();
}

/// Simple performance comparison test
#[test]
fn test_parallel_is_faster_than_sequential() {
    let binary_path = get_forge_binary();
    if !binary_path.exists() {
        println!(
            "Binary not found at {:?}. Building in release mode...",
            binary_path
        );
        let status = Command::new("cargo")
            .args(&["build", "--release"])
            .status()
            .expect("Failed to build");
        assert!(status.success(), "Build failed");
    }

    let dir = tempdir().unwrap();

    // Sequential: 3 tasks × 2 seconds = 6 seconds
    let seq_config = r#"
version: "1.0"
stages:
  - name: sequential
    parallel: false
    steps:
      - name: Task 1
        image: alpine:latest
        command: sleep 2
      - name: Task 2
        image: alpine:latest
        command: sleep 2
      - name: Task 3
        image: alpine:latest
        command: sleep 2
"#;

    let seq_path = dir.path().join("seq_perf_test.yaml");
    File::create(&seq_path)
        .unwrap()
        .write_all(seq_config.as_bytes())
        .unwrap();

    // Parallel: 3 tasks running simultaneously = ~2 seconds
    let par_config = r#"
version: "1.0"
stages:
  - name: parallel
    parallel: true
    steps:
      - name: Task 1
        image: alpine:latest
        command: sleep 2
      - name: Task 2
        image: alpine:latest
        command: sleep 2
      - name: Task 3
        image: alpine:latest
        command: sleep 2
"#;

    let par_path = dir.path().join("par_perf_test.yaml");
    File::create(&par_path)
        .unwrap()
        .write_all(par_config.as_bytes())
        .unwrap();

    println!("Running sequential execution...");
    let seq_time = run_forge(&binary_path, seq_path.to_str().unwrap());
    println!("Sequential took: {:?}", seq_time);

    println!("Running parallel execution...");
    let par_time = run_forge(&binary_path, par_path.to_str().unwrap());
    println!("Parallel took: {:?}", par_time);

    let speedup = seq_time.as_secs_f64() / par_time.as_secs_f64();
    println!("Speedup: {:.2}x", speedup);

    // Parallel should be faster (allow 1.5x minimum due to Docker overhead)
    // Ideally it should be ~3x for 3 tasks, but Docker operations add overhead
    assert!(
        speedup >= 1.5,
        "Parallel execution should be at least 1.5x faster. Got: {:.2}x",
        speedup
    );

    // Generate report
    println!("\n=== Performance Report ===");
    println!("Sequential: {:?}", seq_time);
    println!("Parallel:   {:?}", par_time);
    println!("Speedup:    {:.2}x", speedup);
    println!("Improvement: {:.1}%", (speedup - 1.0) * 100.0);
}

criterion_group!(benches, benchmark_3_tasks_2s, benchmark_5_tasks_1s);
criterion_main!(benches);
