//! # FORGE CLI
//!
//! FORGE is a lightweight local CI/CD system built with Rust that allows you
//! to run automation pipelines on your local machine. It's extremely useful for
//! developing and testing pipelines before pushing them to larger CI/CD systems.
//!
//! ## Key Features
//!
//! - Run CI/CD pipelines from simple YAML files
//! - Isolation using Docker containers
//! - Support for various Docker images
//! - Real-time log streaming with colors
//! - Multi-stage pipelines with parallel execution
//! - Caching to speed up builds
//! - Secure secrets management
//!
//! ## Usage
//!
//! ```bash
//! # Initialize a project with an example configuration file
//! forge-cli init
//!
//! # Validate the configuration
//! forge-cli validate
//!
//! # Run the pipeline
//! forge-cli run
//! ```

mod container;

use bollard::Docker;
use clap::{Parser, Subcommand};
use colored::*;
use futures_util::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

use container::{
    LogEntry, cleanup_container, cleanup_containers, create_and_start_container, prepare_container,
    print_synchronized_logs, stream_logs_buffered, stream_logs_immediate, wait_for_container,
};

/// Helper struct for measuring and displaying operation duration
struct Timer {
    start: std::time::Instant,
    operation: String,
    verbose: bool,
}

impl Timer {
    /// Create a new timer for the given operation
    fn new(operation: impl Into<String>, verbose: bool) -> Self {
        Self {
            start: std::time::Instant::now(),
            operation: operation.into(),
            verbose,
        }
    }

    /// Get elapsed time since timer creation
    fn elapsed(&self) -> std::time::Duration {
        self.start.elapsed()
    }

    /// Print elapsed time if verbose mode is enabled
    fn log_if_verbose(&self) {
        if self.verbose {
            println!(
                "  {} completed in {:.2}s",
                self.operation,
                self.elapsed().as_secs_f64()
            );
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.log_if_verbose();
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Step {
    #[serde(default)]
    name: String,

    /// Command to run inside the container
    command: String,

    #[serde(default)]
    image: String,

    #[serde(default)]
    working_dir: String,

    #[serde(default)]
    env: std::collections::HashMap<String, String>,

    #[serde(default)]
    depends_on: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Stage {
    /// Stage name
    name: String,

    /// Steps in this stage
    steps: Vec<Step>,

    #[serde(default)]
    parallel: bool,

    #[serde(default)]
    depends_on: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ForgeConfig {
    #[serde(default = "default_version")]
    version: String,

    #[serde(default)]
    stages: Vec<Stage>,

    #[serde(default)]
    steps: Vec<Step>,

    #[serde(default)]
    cache: CacheConfig,

    #[serde(default)]
    secrets: Vec<Secret>,
}

/// Helper function to provide a default value for the configuration version.
fn default_version() -> String {
    "1.0".to_string()
}

/// Configuration for directory caching.
///
/// Caching can speed up builds by preserving certain directories
/// (like node_modules) between pipeline executions.
///
/// # Example
///
/// ```yaml
/// cache:
///   enabled: true
///   directories:
///     - /app/node_modules
///     - /app/.cache
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CacheConfig {
    #[serde(default)]
    directories: Vec<String>,

    #[serde(default)]
    enabled: bool,
}

// This multi-line string will be inserted into the help messages.
const SAMPLE_YAML: &str = "SAMPLE FORGE.YAML:
    version: \"1.0\"
    stages:
      - name: build
        steps:
          - image: rust:1.70
            command: cargo build --release
      - name: test
        steps:
          - image: rust:1.70
            command: cargo test";

#[derive(Debug, Serialize, Deserialize)]
struct Secret {
    /// Secret name
    name: String,

    /// Name of the environment variable on the host containing the secret value
    env_var: String,
}

#[derive(Parser)]
#[command(
    name = "forge",
    author = "FORGE Team",
    version = "0.1.0",
    about = "Local CI/CD Runner",
    long_about = "FORGE is a CLI tool designed for developers frustrated with the slow feedback cycle of cloud-based CI/CD. By emulating CI/CD pipelines locally using Docker, FORGE aims to drastically improve developer productivity.",
    after_long_help = SAMPLE_YAML,
    disable_version_flag = true,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(short = 'V', long, help = "Print version")]
    version: bool,
}

#[derive(Subcommand)]
enum Commands {
    #[command(after_help = "EXAMPLES:
    # Run default pipeline from forge.yaml
    forge-cli run

    # Run with a custom config file
    forge-cli run --file ci/pipeline.yaml

    # Run only the 'build' stage
    forge-cli run --stage build

    # Run with verbose output and caching disabled
    forge-cli run --verbose --no-cache

    # Validate pipeline without execution (dry run)
    forge-cli run --dry-run")]
    Run {
        #[arg(short, long, default_value = "forge.yaml")]
        file: String,

        #[arg(short, long)]
        verbose: bool,

        #[arg(long)]
        cache: bool,

        #[arg(long)]
        no_cache: bool,

        #[arg(short, long)]
        stage: Option<String>,

        #[arg(
            long,
            help = "Validate pipeline and print what would run, without execution"
        )]
        dry_run: bool,
    },

    #[command(after_help = "EXAMPLES:
    # Create a default forge.yaml in the current directory
    forge-cli init

    # Create a config file with a custom name
    forge-cli init --file my-pipeline.yaml

    # Force overwrite an existing config file
    forge-cli init --force")]
    Init {
        #[arg(short, long, default_value = "forge.yaml")]
        file: String,

        #[arg(short = 'F', long)]
        force: bool,
    },

    #[command(after_help = "EXAMPLES:
    # Validate the default forge.yaml
    forge-cli validate

    # Validate a config file with a custom name
    forge-cli validate --file prod-config.yaml")]
    Validate {
        #[arg(short, long, default_value = "forge.yaml")]
        file: String,
    },
}

/// Read and parse the FORGE configuration file.
fn read_forge_config(path: &Path) -> Result<ForgeConfig, Box<dyn std::error::Error + Send + Sync>> {
    let mut file = File::open(path).map_err(|e| {
        Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "Failed to open configuration file '{}': {}\n\
                 Hint: Run 'forge-cli init' to create an example config, or check if the file path is correct",
                path.display(), e
            ),
        ))
    })?;

    let mut contents = String::new();
    file.read_to_string(&mut contents).map_err(|e| {
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "Failed to read configuration file '{}': {}\n\
                 Hint: Check file permissions and ensure the file is not corrupted",
                path.display(),
                e
            ),
        ))
    })?;

    let config: ForgeConfig = serde_yaml::from_str(&contents).map_err(|e| {
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "Invalid YAML configuration in '{}': {}\n\
                 Hint: Check your YAML syntax - common issues include incorrect indentation, \n\
                 missing colons, or invalid field names. Run 'forge-cli validate' for detailed validation",
                path.display(), e
            ),
        ))
    })?;
    Ok(config)
}

/// Run a command in a Docker container.
///
/// This function creates and runs a Docker container based on the step configuration,
/// runs the specified command, and displays the output in real-time.
/// The container will be removed after the command finishes.
///
/// # Arguments
///
/// * `docker` - Reference to the Docker client
/// * `step` - Step configuration to run
/// * `verbose` - Whether verbose output is enabled
///
/// # Returns
///
/// * `Result<(), Box<dyn std::error::Error + Send + Sync>>` - Success or error
///
/// # Errors
///
/// This function will return an error if:
/// - Connection to the Docker daemon fails
/// - The image is not found
/// - The container fails to be created or run
/// - The command returns a non-zero exit code
///
/// # Example
///
/// ```rust
/// let docker = Docker::connect_with_local_defaults()?;
/// let step = Step {
///     name: "Build".to_string(),
///     command: "npm run build".to_string(),
///     image: "node:16-alpine".to_string(),
///     working_dir: "/app".to_string(),
///     env: HashMap::new(),
///     depends_on: vec![],
/// };
/// run_command_in_container(&docker, &step, true).await?;
/// ```
async fn run_command_in_container(
    docker: &Docker,
    step: &Step,
    verbose: bool,
    cache_config: &CacheConfig,
    temp_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Phase 1: Prepare container configuration
    let setup = prepare_container(docker, step, cache_config, temp_dir, verbose).await?;

    println!(
        "{}",
        format!("Running step: {}", setup.step_name).yellow().bold()
    );

    // Phase 2: Create and start container
    let container_id = create_and_start_container(docker, &setup).await?;

    // Phase 3 & 4: Stream logs and wait (concurrently)
    let log_handle = tokio::spawn({
        let docker = docker.clone();
        let container_id = container_id.clone();
        async move { stream_logs_immediate(&docker, &container_id).await }
    });

    let wait_result = wait_for_container(docker, &container_id, &setup.step_name).await;

    // Ensure logs finish streaming
    let _ = log_handle.await;

    // Phase 5: Cleanup
    cleanup_container(docker, &container_id).await;

    // Report result
    if wait_result.is_ok() {
        println!(
            "{}",
            format!("Step completed successfully: {}", setup.step_name)
                .green()
                .bold()
        );
    } else {
        println!(
            "{}",
            format!("Step failed: {}", setup.step_name).red().bold()
        );
    }

    wait_result.map(|_| ())
}

/// Run a single step in parallel mode (buffers logs for synchronized output)
///
/// Creates an isolated subdirectory within temp_dir for this step to prevent
/// race conditions when multiple steps access the shared filesystem simultaneously.
/// Each step gets its own `/forge-shared` mount pointing to a unique directory.
///
/// Uses image_pull_locks to prevent concurrent pulls of the same image, saving
/// Docker Hub rate limit quota and network bandwidth.
///
/// # Arguments
///
/// * `docker` - Reference to the Docker client
/// * `step` - Step configuration to run
/// * `verbose` - Whether verbose output is enabled
/// * `cache_config` - Cache configuration
/// * `temp_dir` - Base temporary directory (will create step-N subdirectory inside)
/// * `step_index` - Index of this step in the original steps array (for ordering logs and temp dir isolation)
/// * `log_buffer` - Shared buffer for collecting logs
/// * `container_ids` - Shared list of container IDs for cleanup
/// * `image_pull_locks` - Shared map of semaphores to deduplicate concurrent image pulls
async fn run_step_parallel(
    docker: &Docker,
    step: &Step,
    verbose: bool,
    cache_config: &CacheConfig,
    temp_dir: &Path,
    step_index: usize,
    log_buffer: Arc<Mutex<Vec<Option<(String, Vec<LogEntry>)>>>>,
    container_ids: Arc<Mutex<Vec<String>>>,
    image_pull_locks: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Create isolated temp directory for this step to prevent race conditions
    let step_temp_dir = temp_dir.join(format!("step-{}", step_index));
    tokio::fs::create_dir_all(&step_temp_dir).await?;

    if verbose {
        println!(
            "  Created isolated temp directory: {}",
            step_temp_dir.display()
        );
    }

    // Acquire lock for this image to prevent concurrent pulls (saves Docker Hub rate limit)
    let image_name = &step.image;

    // Get or create a semaphore for this image
    let image_lock = {
        let mut locks = image_pull_locks.lock().await;
        locks
            .entry(image_name.clone())
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone()
    };

    // Acquire the permit - only one task can pull this image at a time
    let _permit = image_lock.acquire().await.unwrap();

    // Phase 1: Prepare container configuration with isolated temp dir
    // (Image will be pulled here, but only one task per image will actually pull)
    let setup = prepare_container(docker, step, cache_config, &step_temp_dir, verbose).await?;

    // Release the permit (automatically happens when _permit is dropped)
    drop(_permit);

    // Phase 2: Create and start container
    let container_id = create_and_start_container(docker, &setup).await?;

    // Track container ID for cleanup
    {
        let mut ids = container_ids.lock().await;
        ids.push(container_id.clone());
    }

    // Phase 3 & 4: Buffer logs and wait (concurrently)
    let log_handle = tokio::spawn({
        let docker = docker.clone();
        let container_id = container_id.clone();
        let step_name = setup.step_name.clone();
        let log_buffer = Arc::clone(&log_buffer);
        async move {
            stream_logs_buffered(&docker, &container_id, &step_name, step_index, log_buffer).await
        }
    });

    let wait_result = wait_for_container(docker, &container_id, &setup.step_name).await;

    // Ensure logs finish streaming
    let _ = log_handle.await;

    // Note: Cleanup will be done in batch by run_stage_parallel

    wait_result.map(|_| ())
}

async fn run_stage_parallel(
    docker: &Docker,
    steps: &[Step],
    verbose: bool,
    cache_config: &CacheConfig,
    temp_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Use Vec with capacity to store logs at specific indices
    let log_buffer = Arc::new(Mutex::new(vec![None; steps.len()]));

    let container_ids = Arc::new(Mutex::new(Vec::new()));

    // Image pull cache to prevent concurrent pulls of the same image
    // Key: image name, Value: Semaphore (only 1 permit = only 1 pull at a time per image)
    let image_pull_locks: Arc<Mutex<HashMap<String, Arc<Semaphore>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let mut tasks = FuturesUnordered::new();

    for (index, step) in steps.iter().enumerate() {
        let docker = docker.clone();
        let step = step.clone();
        let cache = cache_config.clone();
        let temp_dir = temp_dir.to_path_buf();
        let log_buffer = Arc::clone(&log_buffer);
        let container_ids = Arc::clone(&container_ids);
        let image_pull_locks = Arc::clone(&image_pull_locks);

        tasks.push(tokio::spawn(async move {
            run_step_parallel(
                &docker,
                &step,
                verbose,
                &cache,
                &temp_dir,
                index,
                log_buffer,
                container_ids,
                image_pull_locks,
            )
            .await
        }));
    }

    // Collect results - fail fast on first error
    let mut error_result = None;
    while let Some(result) = tasks.next().await {
        match result {
            Ok(Ok(_)) => continue,
            Ok(Err(e)) => {
                // Store error and break to show logs before failing
                error_result = Some(e);
                break;
            }
            Err(e) => {
                error_result = Some(e.into());
                break;
            }
        }
    }

    // Print logs in order (even if there was an error, for debugging)
    print_synchronized_logs(&log_buffer).await;

    // Cleanup containers
    cleanup_containers(docker, &container_ids).await;

    // Return error if any task failed
    if let Some(err) = error_result {
        return Err(err);
    }

    Ok(())
}

/// Create an example forge.yaml file.
fn create_example_config(
    path: &str,
    force: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if Path::new(path).exists() && !force {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "Configuration file '{}' already exists\n\
                 Hint: Use --force to overwrite, or choose a different filename with --file",
                path
            ),
        )));
    }

    let example_config = r#"# FORGE Configuration File
version: "1.0"

# Define stages in your pipeline
stages:
  - name: setup
    steps:
      - name: Install Dependencies
        command: echo "Installing dependencies..."
        image: alpine:latest
    parallel: false

  - name: test
    steps:
      - name: Run Tests
        command: echo "Running tests..."
        image: alpine:latest
    depends_on:
      - setup

  - name: build
    steps:
      - name: Build Application
        command: echo "Building application..."
        image: alpine:latest
    depends_on:
      - test

# Cache configuration
cache:
  enabled: true
  directories:
    - /app/node_modules
    - /app/.cache

# Secrets configuration
secrets:
  - name: API_TOKEN
    env_var: FORGE_API_TOKEN
"#;

    let mut file = File::create(path).map_err(|e| {
        Box::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "Failed to create configuration file '{}': {}\n\
                 Possible causes:\n\
                 • Insufficient write permissions in directory\n\
                 • Directory doesn't exist\n\
                 • Disk space full\n\
                 Hint: Check directory permissions and available disk space",
                path, e
            ),
        ))
    })?;

    std::io::Write::write_all(&mut file, example_config.as_bytes()).map_err(|e| {
        Box::new(std::io::Error::other(format!(
            "Failed to write to configuration file '{}': {}\n\
                 Possible causes:\n\
                 • Disk space full\n\
                 • File system error\n\
                 • Process interrupted\n\
                 Hint: Check available disk space with 'df -h'",
            path, e
        )))
    })?;

    println!(
        "{}",
        format!("Created example configuration file: {path}")
            .green()
            .bold()
    );
    println!("Edit this file to configure your pipeline.");

    Ok(())
}

/// Validate parallel stages configuration
///
/// Checks that parallel-enabled stages have valid configuration:
/// - At least 2 steps (parallel execution with 1 step doesn't make sense)
/// - No step-level dependencies (conflicts with parallel execution)
/// - All steps have valid commands
/// - No duplicate step names within a stage
/// - All steps have names (required for log identification)
///
/// # Arguments
///
/// * `config` - The ForgeConfig to validate
///
/// # Returns
///
/// * `Result<(), Box<dyn std::error::Error + Send + Sync>>` - Success or validation error
fn validate_parallel_stages(
    config: &ForgeConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for stage in &config.stages {
        if !stage.parallel {
            continue; // Only validate parallel stages
        }

        // Validation 1: Parallel stages should have at least 2 steps
        if stage.steps.len() < 2 {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Parallel stage '{}' has only {} step. Parallel execution requires at least 2 steps.",
                    stage.name,
                    stage.steps.len()
                ),
            )));
        }

        // Validation 2 & 3: Check each step
        let mut step_names = HashSet::new();
        for (idx, step) in stage.steps.iter().enumerate() {
            // Check for empty command
            if step.command.trim().is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "Step #{} in parallel stage '{}' has an empty command",
                        idx + 1,
                        stage.name
                    ),
                )));
            }

            // Check for step name (required for logging)
            if step.name.trim().is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "Step #{} in parallel stage '{}' must have a name for log identification",
                        idx + 1,
                        stage.name
                    ),
                )));
            }

            // Check for duplicate step names
            if !step_names.insert(step.name.clone()) {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "Duplicate step name '{}' in parallel stage '{}'. Each step must have a unique name.",
                        step.name, stage.name
                    ),
                )));
            }

            // Check for step-level dependencies (conflicts with parallel execution)
            if !step.depends_on.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "Step '{}' in parallel stage '{}' has dependencies. Steps in parallel stages cannot have 'depends_on' - they all run simultaneously.",
                        step.name, stage.name
                    ),
                )));
            }
        }
    }

    Ok(())
}

/// Main function for the FORGE CLI.
///
/// This function parses command-line arguments and executes the appropriate
/// subcommand (run, init, or validate).
///
/// # Returns
///
/// * `Result<(), Box<dyn std::error::Error + Send + Sync>>` - Success or error
///
/// # Errors
///
/// This function will return an error if:
/// - Command-line arguments are invalid
/// - The subcommand fails to execute
///
/// # Example
///
/// ```rust
/// fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
///     tokio::runtime::Runtime::new()?.block_on(async {
///         forge_main().await
///     })
/// }
/// ```
async fn forge_main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    if cli.version {
        println!("Forge {}", env!("BUILD_VERSION"));
        println!("Commit: {}", env!("GIT_VERSION"));
        println!("Built: {}", env!("BUILD_TIMESTAMP"));
        return Ok(());
    }

    match cli.command {
        Some(Commands::Run {
            file,
            verbose,
            cache,
            no_cache,
            stage,
            dry_run,
        }) => {
            // Start overall pipeline timer
            let pipeline_start = std::time::Instant::now();

            println!(
                "{}",
                if dry_run {
                    "FORGE Pipeline Runner (DRY RUN MODE)".cyan().bold()
                } else {
                    "FORGE Pipeline Runner".cyan().bold()
                }
            );

            // Read and parse the configuration file
            let config_path = Path::new(&file);
            if !config_path.exists() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "Configuration file not found: '{}'\n\
                         Hint: Run 'forge-cli init' to create an example config, or specify a different file with --file",
                        file
                    ),
                )));
            }

            let _config_timer = Timer::new("Configuration parsing", verbose);
            let mut config = read_forge_config(config_path)?;
            drop(_config_timer);

            // Override cache settings if specified
            if cache {
                config.cache.enabled = true;
            }
            if no_cache {
                config.cache.enabled = false;
            }

            // Validate parallel stages before running
            validate_parallel_stages(&config)?;

            // Connect to Docker
            let _docker_timer = Timer::new("Docker connection", verbose);
            let docker = Docker::connect_with_local_defaults().map_err(|e| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    format!(
                        "Failed to connect to Docker: {}\n\
                         Possible causes:\n\
                         • Docker daemon is not running\n\
                         • Docker is not installed\n\
                         • Insufficient permissions to access Docker socket\n\
                         Solutions:\n\
                         • Start Docker Desktop (Windows/macOS) or 'sudo systemctl start docker' (Linux)\n\
                         • Add your user to the docker group: 'sudo usermod -aG docker $USER'\n\
                         • Verify Docker is working: 'docker --version'",
                        e
                    ),
                ))
            })?;

            // Check if Docker is running
            docker.ping().await.map_err(|e| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    format!(
                        "Docker daemon is not responding: {}\n\
                         The Docker service appears to be stopped or unresponsive.\n\
                         Solutions:\n\
                         • Restart Docker Desktop (Windows/macOS)\n\
                         • Restart Docker service: 'sudo systemctl restart docker' (Linux)\n\
                         • Check Docker status: 'docker info'",
                        e
                    ),
                ))
            })?;
            drop(_docker_timer);

            // If using the old format (just steps), convert to the new format
            if config.stages.is_empty() && !config.steps.is_empty() {
                config.stages.push(Stage {
                    name: "default".to_string(),
                    steps: config.steps.clone(),
                    parallel: false,
                    depends_on: vec![],
                });
            }

            // Filter stages if a specific stage is requested
            if let Some(stage_name) = stage {
                let available_stages: Vec<String> =
                    config.stages.iter().map(|s| s.name.clone()).collect();
                config.stages.retain(|s| s.name == stage_name);
                if config.stages.is_empty() {
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!(
                            "Stage '{}' not found in configuration\n\
                             Available stages: {}\n\
                             Hint: Check your forge.yaml file for correct stage names",
                            stage_name,
                            if available_stages.is_empty() {
                                "none".to_string()
                            } else {
                                available_stages.join(", ")
                            }
                        ),
                    )));
                }
            }

            // Basic validation: ensure there are stages after any stage filtering
            if config.stages.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "No stages or steps found in configuration\n\
                     Hint: Your forge.yaml file must contain either 'stages' or 'steps'. \n\
                     Run 'forge-cli init' to see an example configuration"
                        .to_string(),
                )));
            }

            if dry_run {
                println!(
                    "{}",
                    format!("Configuration validated: {}", file).green().bold()
                );
                println!("{}", "Docker connection verified".green().bold());

                let stages_count = config.stages.len();
                let steps_count: usize = config.stages.iter().map(|s| s.steps.len()).sum();
                println!(
                    "{}",
                    format!("{} stages found, {} total steps", stages_count, steps_count)
                        .cyan()
                        .bold()
                );
                println!("Would execute:");
                for stage in &config.stages {
                    println!("Stage: {} ({} steps)", stage.name, stage.steps.len());
                    for (i, step) in stage.steps.iter().enumerate() {
                        let cmd = if step.command.trim().is_empty() {
                            "<no command>"
                        } else {
                            step.command.as_str()
                        };
                        println!("- Step {}: {}", i + 1, cmd);
                    }
                }
                println!(
                    "{}",
                    "Pipeline validation completed successfully!".green().bold()
                );
                return Ok(());
            }

            // Create a temporary directory for sharing data between containers
            let temp_dir = env::temp_dir().join(format!("forge-{}", uuid::Uuid::new_v4()));

            // Create the directory if it doesn't exist
            if !temp_dir.exists() {
                if let Err(e) = std::fs::create_dir_all(&temp_dir) {
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!(
                            "Failed to create temporary directory '{}': {}\n\
                             Possible causes:\n\
                             • Insufficient permissions in temp directory\n\
                             • Disk space full\n\
                             • File system error\n\
                             Hint: Check permissions and disk space in your temp directory",
                            temp_dir.display(),
                            e
                        ),
                    )));
                } else if verbose {
                    println!("Created temporary directory: {}", temp_dir.display());
                }
            }

            // Validate pipeline before execution
            if config.stages.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "No stages or steps found in configuration\n\
                     Hint: Your forge.yaml file must contain either 'stages' or 'steps'. \n\
                     Run 'forge-cli init' to see an example configuration"
                        .to_string(),
                )));
            }

            // Run the pipeline
            for stage in &config.stages {
                let _stage_timer = Timer::new(format!("Stage '{}'", stage.name), verbose);
                println!("{}", format!("Stage: {}", stage.name).cyan().bold());

                // Run steps in parallel or sequentially
                if stage.parallel {
                    run_stage_parallel(&docker, &stage.steps, verbose, &config.cache, &temp_dir)
                        .await?;
                } else {
                    for step in &stage.steps {
                        run_command_in_container(&docker, step, verbose, &config.cache, &temp_dir)
                            .await?;
                    }
                }

                // No cleanup here, we'll do it after all stages are done
            }

            // Clean up the temporary directory after the pipeline is done
            if verbose {
                println!("Removing temporary directory: {}", temp_dir.display());
            }

            if let Err(e) = std::fs::remove_dir_all(&temp_dir) {
                eprintln!("Failed to remove temporary directory: {e}");
                // Continue anyway, as this is not critical
            } else if verbose {
                println!("Temporary directory removed successfully");
            }

            println!("{}", "Pipeline completed successfully!".green().bold());

            // Print total pipeline duration
            if verbose {
                println!(
                    "\n{}",
                    format!(
                        "Total pipeline duration: {:.2}s",
                        pipeline_start.elapsed().as_secs_f64()
                    )
                    .cyan()
                    .bold()
                );
            }

            Ok(())
        }
        Some(Commands::Init { file, force }) => create_example_config(&file, force),
        Some(Commands::Validate { file }) => {
            println!("{}", "Validating configuration file...".cyan().bold());

            let config_path = Path::new(&file);
            if !config_path.exists() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "Configuration file not found: '{}'\n\
                         Hint: Run 'forge-cli init' to create an example config, or check the file path",
                        file
                    ),
                )));
            }

            let config = read_forge_config(config_path)?;

            // Validate the configuration
            if config.stages.is_empty() && config.steps.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Configuration validation failed: No stages or steps defined\n\
                     Your configuration must contain either:\n\
                     • A 'stages' section with at least one stage\n\
                     • A 'steps' section with at least one step\n\
                     Hint: See examples in the documentation or run 'forge-cli init' for a template"
                        .to_string(),
                )));
            }

            // Validate that all stages have at least one step
            for stage in &config.stages {
                if stage.steps.is_empty() {
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "Configuration validation failed: Stage '{}' has no steps\n\
                             Each stage must contain at least one step with a 'command' field\n\
                             Hint: Add steps to the stage or remove the empty stage",
                            stage.name
                        ),
                    )));
                }
            }

            // Validate that all steps have commands
            for stage in &config.stages {
                for (i, step) in stage.steps.iter().enumerate() {
                    if step.command.trim().is_empty() {
                        return Err(Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!(
                                "Configuration validation failed: Step {} in stage '{}' has empty command\n\
                                 Each step must have a non-empty 'command' field\n\
                                 Hint: Add a command like 'echo \"Hello World\"' or remove the step",
                                i + 1,
                                stage.name
                            ),
                        )));
                    }
                }
            }

            // Validate parallel stages
            validate_parallel_stages(&config)?;

            // Check for circular dependencies in stages
            // TODO: Implement circular dependency check

            println!("{}", "Configuration is valid!".green().bold());

            // Print summary
            if !config.stages.is_empty() {
                println!("Stages:");
                for stage in &config.stages {
                    println!("  - {} ({} steps)", stage.name, stage.steps.len());
                }
            } else {
                println!("Steps: {}", config.steps.len());
            }

            if config.cache.enabled {
                println!("Cache: Enabled");
                println!("Cached directories:");
                for dir in &config.cache.directories {
                    println!("  - {dir}");
                }
            } else {
                println!("Cache: Disabled");
            }

            if !config.secrets.is_empty() {
                println!("Secrets:");
                for secret in &config.secrets {
                    println!("  - {} (from {})", secret.name, secret.env_var);
                }
            }

            Ok(())
        }
        None => Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "No command provided\n\
                 Available commands:\n\
                 • forge-cli run      - Execute the pipeline\n\
                 • forge-cli init     - Create example config\n\
                 • forge-cli validate - Check config syntax\n\
                 • forge-cli --help   - Show detailed help\n\
                 \n\
                 Hint: Start with 'forge-cli init' to create your first pipeline"
                .to_string(),
        ))),
    }
}

/// Main function that sets up the async runtime and runs the FORGE CLI.
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        if let Err(e) = forge_main().await {
            eprintln!("{}", format!("Error: {e}").red().bold());
            std::process::exit(1);
        }
        Ok(())
    })
}
