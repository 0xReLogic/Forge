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

/// Representation of a step in a CI/CD pipeline.
///
/// Each step runs in a separate Docker container and can have
/// configurations like image, working directory, and environment variables.
///
/// # Example
///
/// ```yaml
/// steps:
///   - name: Build
///     command: npm run build
///     image: node:16-alpine
///     working_dir: /app
///     env:
///       NODE_ENV: production
/// ```
#[derive(Debug, Serialize, Deserialize, Clone)]
struct Step {
    /// Step name (optional)
    /// If not provided, a numeric index will be used.
    #[serde(default)]
    name: String,

    /// Command to run inside the container
    /// This is the only required field.
    command: String,

    /// Docker image to use
    /// If not provided, `alpine:latest` will be used.
    #[serde(default)]
    image: String,

    /// Working directory inside the container
    /// If not provided, the container's default directory will be used.
    #[serde(default)]
    working_dir: String,

    /// Environment variables to set inside the container
    /// Format: key-value pairs
    #[serde(default)]
    env: std::collections::HashMap<String, String>,

    /// Dependencies on other steps (optional)
    /// This step will only run after all the mentioned steps have completed.
    #[serde(default)]
    depends_on: Vec<String>,
}

/// Representation of a stage in a CI/CD pipeline.
///
/// A stage is a collection of steps that can be executed
/// sequentially or in parallel. Stages can also have dependencies
/// on other stages.
///
/// # Example
///
/// ```yaml
/// stages:
///   - name: build
///     steps:
///       - name: Build
///         command: npm run build
///     parallel: false
///     depends_on: []
/// ```
#[derive(Debug, Serialize, Deserialize, Clone)]
struct Stage {
    /// Stage name
    /// Used for reference in dependencies.
    name: String,

    /// Steps in this stage
    /// There must be at least one step.
    steps: Vec<Step>,

    /// Whether steps in this stage are executed in parallel
    /// If true, all steps will be run simultaneously.
    /// If false, steps will be run sequentially based on dependencies.
    #[serde(default)]
    parallel: bool,

    /// Dependencies on other stages (optional)
    /// This stage will only run after all the mentioned stages have completed.
    #[serde(default)]
    depends_on: Vec<String>,
}

/// Main configuration for FORGE pipeline.
///
/// This is the root structure parsed from the YAML file.
/// It supports two formats: the old format with just `steps` and
/// the new format with `stages`, `cache`, and `secrets`.
///
/// # Old Format Example
///
/// ```yaml
/// steps:
///   - name: Build
///     command: npm run build
/// ```
///
/// # New Format Example
///
/// ```yaml
/// version: "1.0"
/// stages:
///   - name: build
///     steps:
///       - name: Build
///         command: npm run build
/// cache:
///   enabled: true
///   directories:
///     - /app/node_modules
/// secrets:
///   - name: API_TOKEN
///     env_var: FORGE_API_TOKEN
/// ```
#[derive(Debug, Serialize, Deserialize)]
struct ForgeConfig {
    /// Configuration format version
    /// Default: "1.0"
    #[serde(default = "default_version")]
    version: String,

    /// Stages in the pipeline
    /// The new format uses stages for better organization.
    #[serde(default)]
    stages: Vec<Stage>,

    /// Steps in the pipeline (for compatibility with the old format)
    /// If stages is empty, these steps will be run sequentially.
    #[serde(default)]
    steps: Vec<Step>,

    /// Cache configuration
    /// Used to speed up builds by caching certain directories.
    #[serde(default)]
    cache: CacheConfig,

    /// Secrets to use in the pipeline
    /// Used to provide sensitive values to containers.
    #[serde(default)]
    secrets: Vec<Secret>,
}

/// Helper function to provide a default value for the configuration version.
///
/// Always returns "1.0" as the default version.
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
    /// Directories to cache
    /// Paths relative to the working directory inside the container.
    #[serde(default)]
    directories: Vec<String>,

    /// Whether caching is enabled
    /// If false, caching won't be performed even if directories are specified.
    #[serde(default)]
    enabled: bool,
}

/// Representation of a secret in the pipeline.
///
/// Secrets are used to provide sensitive values (like API tokens)
/// to containers without storing them in the configuration file.
///
/// # Example
///
/// ```yaml
/// secrets:
///   - name: API_TOKEN
///     env_var: FORGE_API_TOKEN
/// ```
///
/// The secret value is taken from the `FORGE_API_TOKEN` environment variable on the host
/// and provided as the `API_TOKEN` environment variable inside the container.
#[derive(Debug, Serialize, Deserialize)]
struct Secret {
    /// Secret name
    /// Will be the name of the environment variable inside the container.
    name: String,

    /// Name of the environment variable on the host containing the secret value
    /// The value of this environment variable will be used as the secret value.
    env_var: String,
}

/// Main CLI structure for FORGE.
///
/// Defines all subcommands and options available in the CLI.
/// Uses the `clap` library for command-line argument parsing.
#[derive(Parser)]
#[command(
    name = "forge",
    author = "FORGE Team",
    version = "0.1.0",
    about = "Local CI/CD Runner",
    long_about = "FORGE is a CLI tool designed for developers frustrated with the slow feedback cycle of cloud-based CI/CD. By emulating CI/CD pipelines locally using Docker, FORGE aims to drastically improve developer productivity.",
    disable_version_flag = true,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// Subcommand to run (run, init, or validate)
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(short = 'V', long, help = "Print version")]
    version: bool,
}

/// Enum defining all subcommands available in the FORGE CLI.
///
/// Each variant represents one subcommand with its own options.
#[derive(Subcommand)]
enum Commands {
    /// Run the FORGE pipeline
    ///
    /// This subcommand reads the configuration file, validates it, and runs
    /// the pipeline according to that configuration. The pipeline can be in the old format
    /// with just steps, or the new format with stages, cache, and secrets.
    ///
    /// # Examples
    ///
    /// ```bash
    /// forge-cli run
    /// forge-cli run --file custom-forge.yaml --verbose
    /// forge-cli run --stage build
    /// forge-cli run --cache
    /// ```
    Run {
        /// Path to the forge.yaml file
        /// Default: "forge.yaml" in the current directory
        #[arg(short, long, default_value = "forge.yaml")]
        file: String,

        /// Show more detailed output
        /// Includes information about containers, images, and full logs
        #[arg(short, long)]
        verbose: bool,

        /// Enable caching (overrides configuration)
        /// If set, will enable caching even if disabled in the configuration
        #[arg(long)]
        cache: bool,

        /// Disable caching (overrides configuration)
        /// If set, will disable caching even if enabled in the configuration
        #[arg(long)]
        no_cache: bool,

        /// Run only a specific stage
        /// If set, only the mentioned stage will be run
        #[arg(short, long)]
        stage: Option<String>,
    },

    /// Initialize a new forge.yaml file
    ///
    /// This subcommand creates an example configuration file that can be modified
    /// as needed. The file contains a simple pipeline with some steps, stages, cache, and secrets.
    ///
    /// # Examples
    ///
    /// ```bash
    /// forge-cli init
    /// forge-cli init --file custom-forge.yaml
    /// forge-cli init --force
    /// ```
    Init {
        /// Path to create the forge.yaml file
        /// Default: "forge.yaml" in the current directory
        #[arg(short, long, default_value = "forge.yaml")]
        file: String,

        /// Force overwrite if file exists
        /// If not set and the file already exists, an error will be shown
        #[arg(short = 'F', long)]
        force: bool,
    },

    /// Validate a forge.yaml file
    ///
    /// This subcommand reads the configuration file and validates its format
    /// without running the pipeline. Useful for checking if the configuration
    /// is valid before running it.
    ///
    /// # Examples
    ///
    /// ```bash
    /// forge-cli validate
    /// forge-cli validate --file custom-forge.yaml
    /// ```
    Validate {
        /// Path to the forge.yaml file to validate
        /// Default: "forge.yaml" in the current directory
        #[arg(short, long, default_value = "forge.yaml")]
        file: String,
    },
}

/// Read and parse the FORGE configuration file.
///
/// This function reads a YAML file from the given path and parses it
/// into a `ForgeConfig` structure. If the file doesn't exist or its format is invalid,
/// an error will be returned.
///
/// # Arguments
///
/// * `path` - Path to the FORGE configuration file (usually forge.yaml)
///
/// # Returns
///
/// * `Result<ForgeConfig, Box<dyn std::error::Error + Send + Sync>>` - Parsing result or error
///
/// # Errors
///
/// This function will return an error if:
/// - The file is not found
/// - The file cannot be read
/// - The YAML format is invalid
/// - The YAML format doesn't match the `ForgeConfig` structure
///
/// # Example
///
/// ```rust
/// let config = read_forge_config(Path::new("forge.yaml"))?;
/// println!("Loaded config with {} stages", config.stages.len());
/// ```
fn read_forge_config(path: &Path) -> Result<ForgeConfig, Box<dyn std::error::Error + Send + Sync>> {
    let mut file = File::open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let config: ForgeConfig = serde_yaml::from_str(&contents)?;
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
///
/// This function creates a new forge.yaml file with an example configuration.
/// If the file already exists and `force` is false, an error will be returned.
///
/// # Arguments
///
/// * `path` - Path where the file should be created
/// * `force` - Whether to overwrite an existing file
///
/// # Returns
///
/// * `Result<(), Box<dyn std::error::Error + Send + Sync>>` - Success or error
///
/// # Errors
///
/// This function will return an error if:
/// - The file already exists and `force` is false
/// - The file cannot be created or written to
///
/// # Example
///
/// ```rust
/// create_example_config("forge.yaml", false)?;
/// ```
fn create_example_config(
    path: &str,
    force: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if Path::new(path).exists() && !force {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("File {path} already exists. Use --force to overwrite."),
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

    let mut file = File::create(path)?;
    std::io::Write::write_all(&mut file, example_config.as_bytes())?;

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
        }) => {
            println!("{}", "FORGE Pipeline Runner".cyan().bold());

            // Read and parse the configuration file
            let config_path = Path::new(&file);
            if !config_path.exists() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Configuration file not found: {file}"),
                )));
            }

            let mut config = read_forge_config(config_path)?;

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
            let docker = Docker::connect_with_local_defaults()?;

            // Check if Docker is running
            docker.ping().await?;

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
                config.stages.retain(|s| s.name == stage_name);
                if config.stages.is_empty() {
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("Stage not found: {stage_name}"),
                    )));
                }
            }

            // Create a temporary directory for sharing data between containers
            let temp_dir = env::temp_dir().join(format!("forge-{}", uuid::Uuid::new_v4()));

            // Create the directory if it doesn't exist
            if !temp_dir.exists() {
                if let Err(e) = std::fs::create_dir_all(&temp_dir) {
                    eprintln!("Failed to create temporary directory: {e}");
                    // Continue anyway, as this is not critical
                } else if verbose {
                    println!("Created temporary directory: {}", temp_dir.display());
                }
            }

            // Run the pipeline
            for stage in &config.stages {
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
            Ok(())
        }
        Some(Commands::Init { file, force }) => create_example_config(&file, force),
        Some(Commands::Validate { file }) => {
            println!("{}", "Validating configuration file...".cyan().bold());

            let config_path = Path::new(&file);
            if !config_path.exists() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Configuration file not found: {file}"),
                )));
            }

            let config = read_forge_config(config_path)?;

            // Validate the configuration
            if config.stages.is_empty() && config.steps.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Configuration must have at least one stage or step",
                )));
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
        None => {
            eprintln!("Error: No command provided. Use --help for usage information.");
            std::process::exit(1);
        }
    }
}

/// Main function that sets up the async runtime and runs the FORGE CLI.
///
/// This function creates a new Tokio runtime and runs the `forge_main` function
/// inside it. It handles any errors that occur and prints them to stderr.
///
/// # Returns
///
/// * `Result<(), Box<dyn std::error::Error + Send + Sync>>` - Success or error
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
