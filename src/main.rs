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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

use container::{
    ContainerRuntimeContext, LogBuffer, cleanup_container, cleanup_containers,
    create_and_start_container, prepare_container, print_synchronized_logs, stream_logs_buffered,
    stream_logs_immediate, wait_for_container,
};

#[derive(Clone)]
struct PipelineRuntimeContext {
    workspace_dir: PathBuf,
    cache_dir: PathBuf,
    secrets_env: Arc<HashMap<String, String>>,
}

struct Timer {
    start: std::time::Instant,
    operation: String,
    verbose: bool,
}

impl Timer {
    fn new(operation: impl Into<String>, verbose: bool) -> Self {
        Self {
            start: std::time::Instant::now(),
            operation: operation.into(),
            verbose,
        }
    }

    fn elapsed(&self) -> std::time::Duration {
        self.start.elapsed()
    }

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

fn default_version() -> String {
    "1.0".to_string()
}

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

fn collect_secrets_env(
    secrets: &[Secret],
) -> Result<HashMap<String, String>, Box<dyn std::error::Error + Send + Sync>> {
    let mut out = HashMap::new();

    for secret in secrets {
        let value = env::var(&secret.env_var).map_err(|_| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "Secret '{}' is configured to come from env var '{}', but it is not set\n\
                     Hint: export {}=<value> before running forge",
                    secret.name, secret.env_var, secret.env_var
                ),
            ))
        })?;

        out.insert(secret.name.clone(), value);
    }

    Ok(out)
}

fn default_cache_dir() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(xdg_cache_home) = env::var("XDG_CACHE_HOME")
        && !xdg_cache_home.trim().is_empty()
    {
        return Ok(PathBuf::from(xdg_cache_home).join("forge"));
    }

    let home = env::var("HOME").map_err(|_| {
        Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "HOME is not set; cannot determine a persistent cache directory".to_string(),
        ))
    })?;

    Ok(PathBuf::from(home).join(".cache").join("forge"))
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
    forge run

    # Run with a custom config file
    forge run --file ci/pipeline.yaml

    # Run only the 'build' stage
    forge run --stage build

    # Run with verbose output and caching disabled
    forge run --verbose --no-cache

    # Validate pipeline without execution (dry run)
    forge run --dry-run")]
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
    forge init

    # Create a config file with a custom name
    forge init --file my-pipeline.yaml

    # Force overwrite an existing config file
    forge init --force")]
    Init {
        #[arg(short, long, default_value = "forge.yaml")]
        file: String,

        #[arg(short = 'F', long)]
        force: bool,
    },

    #[command(after_help = "EXAMPLES:
    # Validate the default forge.yaml
    forge validate

    # Validate a config file with a custom name
    forge validate --file prod-config.yaml")]
    Validate {
        #[arg(short, long, default_value = "forge.yaml")]
        file: String,
    },
}

fn read_forge_config(path: &Path) -> Result<ForgeConfig, Box<dyn std::error::Error + Send + Sync>> {
    let mut file = File::open(path).map_err(|e| {
        Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "Failed to open configuration file '{}': {}\n\
                 Hint: Run 'forge init' to create an example config, or check if the file path is correct",
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
                 missing colons, or invalid field names. Run 'forge validate' for detailed validation",
                path.display(), e
            ),
        ))
    })?;
    Ok(config)
}

async fn run_command_in_container(
    docker: &Docker,
    step: &Step,
    verbose: bool,
    cache_config: &CacheConfig,
    temp_dir: &std::path::Path,
    runtime: &PipelineRuntimeContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let container_ctx = ContainerRuntimeContext {
        workspace_dir: &runtime.workspace_dir,
        cache_dir: &runtime.cache_dir,
        secrets_env: runtime.secrets_env.as_ref(),
    };
    let setup = prepare_container(
        docker,
        step,
        cache_config,
        temp_dir,
        &container_ctx,
        verbose,
    )
    .await?;

    println!(
        "{}",
        format!("Running step: {}", setup.step_name).yellow().bold()
    );

    let container_id = create_and_start_container(docker, &setup).await?;

    let log_handle = tokio::spawn({
        let docker = docker.clone();
        let container_id = container_id.clone();
        async move { stream_logs_immediate(&docker, &container_id).await }
    });

    let wait_result = wait_for_container(docker, &container_id, &setup.step_name).await;

    let _ = log_handle.await;
    cleanup_container(docker, &container_id).await;

    if wait_result.is_ok() {
        println!("{} Step: {}", "[OK]".green(), setup.step_name);
    } else {
        println!("{} Step: {}", "[FAIL]".red().bold(), setup.step_name);
    }

    wait_result.map(|_| ())
}

struct ParallelContext {
    log_buffer: LogBuffer,
    container_ids: Arc<Mutex<Vec<String>>>,
    image_pull_locks: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
}

#[derive(Clone)]
struct ParallelTaskContext {
    step_index: usize,
    ctx: Arc<ParallelContext>,
}

async fn run_step_parallel(
    docker: &Docker,
    step: &Step,
    verbose: bool,
    cache_config: &CacheConfig,
    temp_dir: &Path,
    task: ParallelTaskContext,
    runtime: &PipelineRuntimeContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let step_temp_dir = temp_dir.join(format!("step-{}", task.step_index));
    tokio::fs::create_dir_all(&step_temp_dir).await?;

    if verbose {
        println!(
            "  Created isolated temp directory: {}",
            step_temp_dir.display()
        );
    }

    let image_name = &step.image;
    let image_lock = {
        let mut locks = task.ctx.image_pull_locks.lock().await;
        locks
            .entry(image_name.clone())
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone()
    };

    let _permit = image_lock.acquire().await.unwrap();
    let container_ctx = ContainerRuntimeContext {
        workspace_dir: &runtime.workspace_dir,
        cache_dir: &runtime.cache_dir,
        secrets_env: runtime.secrets_env.as_ref(),
    };
    let setup = prepare_container(
        docker,
        step,
        cache_config,
        &step_temp_dir,
        &container_ctx,
        verbose,
    )
    .await?;
    drop(_permit);

    let container_id = create_and_start_container(docker, &setup).await?;
    {
        let mut ids = task.ctx.container_ids.lock().await;
        ids.push(container_id.clone());
    }

    let log_handle = tokio::spawn({
        let docker = docker.clone();
        let container_id = container_id.clone();
        let step_name = setup.step_name.clone();
        let log_buffer = Arc::clone(&task.ctx.log_buffer);
        async move {
            stream_logs_buffered(
                &docker,
                &container_id,
                &step_name,
                task.step_index,
                log_buffer,
            )
            .await
        }
    });

    let wait_result = wait_for_container(docker, &container_id, &setup.step_name).await;
    let _ = log_handle.await;

    wait_result.map(|_| ())
}

async fn run_stage_parallel(
    docker: &Docker,
    steps: &[Step],
    verbose: bool,
    cache_config: &CacheConfig,
    temp_dir: &std::path::Path,
    runtime: &PipelineRuntimeContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ctx = Arc::new(ParallelContext {
        log_buffer: Arc::new(Mutex::new(vec![None; steps.len()])),
        container_ids: Arc::new(Mutex::new(Vec::new())),
        image_pull_locks: Arc::new(Mutex::new(HashMap::new())),
    });

    let mut tasks = FuturesUnordered::new();

    for (index, step) in steps.iter().enumerate() {
        let docker = docker.clone();
        let step = step.clone();
        let cache = cache_config.clone();
        let temp_dir = temp_dir.to_path_buf();
        let task = ParallelTaskContext {
            step_index: index,
            ctx: Arc::clone(&ctx),
        };
        let runtime = runtime.clone();

        tasks.push(tokio::spawn(async move {
            run_step_parallel(&docker, &step, verbose, &cache, &temp_dir, task, &runtime).await
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
    print_synchronized_logs(&ctx.log_buffer).await;

    // Cleanup containers
    cleanup_containers(docker, &ctx.container_ids).await;

    // Return error if any task failed
    if let Some(err) = error_result {
        return Err(err);
    }

    Ok(())
}

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

fn resolve_stage_dependencies(
    stages: &[Stage],
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    let stage_names: HashSet<String> = stages.iter().map(|s| s.name.clone()).collect();
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();

    for stage in stages {
        graph.entry(stage.name.clone()).or_default();
        in_degree.entry(stage.name.clone()).or_insert(0);

        for dep in &stage.depends_on {
            if dep == &stage.name {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "Stage '{}' cannot depend on itself.\nRemove '{}' from its own depends_on list.",
                        stage.name, stage.name
                    ),
                )));
            }

            if !stage_names.contains(dep) {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "Stage '{}' depends on '{}', but '{}' is not defined.\nAvailable stages: {}",
                        stage.name,
                        dep,
                        dep,
                        stage_names.iter().cloned().collect::<Vec<_>>().join(", ")
                    ),
                )));
            }

            graph
                .entry(dep.clone())
                .or_default()
                .push(stage.name.clone());
            *in_degree.entry(stage.name.clone()).or_insert(0) += 1;
        }
    }

    // Kahn's algorithm for topological sort
    let mut queue: Vec<String> = in_degree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(name, _)| name.clone())
        .collect();
    queue.sort(); // Deterministic order

    let mut result = Vec::new();

    while let Some(current) = queue.pop() {
        result.push(current.clone());

        if let Some(dependents) = graph.get(&current) {
            for dependent in dependents {
                if let Some(deg) = in_degree.get_mut(dependent) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(dependent.clone());
                        queue.sort();
                    }
                }
            }
        }
    }

    if result.len() != stages.len() {
        let remaining: Vec<String> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg > 0)
            .map(|(name, _)| name.clone())
            .collect();

        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "Circular dependency detected in stages: {}\n\
                 These stages form a dependency cycle and cannot be resolved.\n\
                 Hint: Remove one of the dependencies to break the cycle.",
                remaining.join(" -> ")
            ),
        )));
    }

    Ok(result)
}

async fn forge_main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    if cli.version {
        println!(
            "{} {}",
            "FORGE".cyan().bold(),
            env!("BUILD_VERSION").green()
        );
        println!("  Commit: {}", env!("GIT_VERSION"));
        println!("  Built:  {}", env!("BUILD_TIMESTAMP"));
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
                         Hint: Run 'forge init' to create an example config, or specify a different file with --file",
                        file
                    ),
                )));
            }

            let _ = dotenvy::dotenv();
            if let Some(config_parent) = config_path.parent() {
                let _ = dotenvy::from_path(config_parent.join(".env"));
            }

            let _config_timer = Timer::new("Configuration parsing", verbose);
            let mut config = read_forge_config(config_path)?;
            drop(_config_timer);

            let runtime = PipelineRuntimeContext {
                workspace_dir: env::current_dir()?,
                cache_dir: default_cache_dir()?,
                secrets_env: Arc::new(collect_secrets_env(&config.secrets)?),
            };

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

                let stage_map: HashMap<String, &Stage> =
                    config.stages.iter().map(|s| (s.name.clone(), s)).collect();

                if !stage_map.contains_key(&stage_name) {
                    config.stages.retain(|_| false);
                } else {
                    let mut required = HashSet::new();
                    let mut stack = vec![stage_name.clone()];

                    while let Some(current) = stack.pop() {
                        if !required.insert(current.clone()) {
                            continue;
                        }

                        let stage = stage_map.get(&current).ok_or_else(|| {
                            Box::new(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!(
                                    "Stage '{}' depends on '{}', but '{}' is not defined.\nAvailable stages: {}",
                                    stage_name,
                                    current,
                                    current,
                                    available_stages.join(", ")
                                ),
                            ))
                        })?;

                        for dep in &stage.depends_on {
                            stack.push(dep.clone());
                        }
                    }

                    config.stages.retain(|s| required.contains(&s.name));
                }

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
                     Run 'forge init' to see an example configuration"
                        .to_string(),
                )));
            }

            if dry_run {
                println!("{} Configuration validated: {}", "[OK]".green(), file);
                println!("{} Docker connection verified", "[OK]".green());

                let execution_order = resolve_stage_dependencies(&config.stages)?;
                let stage_map: HashMap<String, &Stage> =
                    config.stages.iter().map(|s| (s.name.clone(), s)).collect();

                let stages_count = config.stages.len();
                let steps_count: usize = config.stages.iter().map(|s| s.steps.len()).sum();
                println!(
                    "{} {} stages, {} steps",
                    "[OK]".green(),
                    stages_count,
                    steps_count
                );

                println!("\n{}", "Execution order:".cyan().bold());
                for (i, stage_name) in execution_order.iter().enumerate() {
                    let stage = stage_map.get(stage_name).unwrap();
                    let deps = if stage.depends_on.is_empty() {
                        "".to_string()
                    } else {
                        format!(" (after: {})", stage.depends_on.join(", "))
                            .dimmed()
                            .to_string()
                    };
                    let parallel_tag = if stage.parallel {
                        " [parallel]".yellow().to_string()
                    } else {
                        "".to_string()
                    };
                    println!("  {}. {}{}{}", i + 1, stage_name.cyan(), parallel_tag, deps);
                    for (j, step) in stage.steps.iter().enumerate() {
                        let cmd = if step.command.trim().is_empty() {
                            "<no command>"
                        } else {
                            step.command.as_str()
                        };
                        println!("     {} {}", format!("[{}]", j + 1).dimmed(), cmd);
                    }
                }
                println!(
                    "\n{} {}",
                    "[OK]".green().bold(),
                    "Pipeline validation completed!".green()
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
                     Run 'forge init' to see an example configuration"
                        .to_string(),
                )));
            }

            // Resolve stage dependencies and get execution order
            let execution_order = resolve_stage_dependencies(&config.stages)?;
            let stage_map: HashMap<String, &Stage> =
                config.stages.iter().map(|s| (s.name.clone(), s)).collect();

            if config.cache.enabled {
                std::fs::create_dir_all(&runtime.cache_dir)?;
            }

            if verbose {
                println!(
                    "{} Execution order: {}",
                    "[INFO]".blue(),
                    execution_order.join(" -> ")
                );
            }

            // Run the pipeline in dependency order
            for (stage_idx, stage_name) in execution_order.iter().enumerate() {
                let stage = stage_map.get(stage_name).unwrap();
                let _stage_timer = Timer::new(format!("Stage '{}'", stage.name), verbose);
                let parallel_tag = if stage.parallel { " [parallel]" } else { "" };
                println!(
                    "\n{} Stage {}/{}: {}{}",
                    ">>>".cyan().bold(),
                    stage_idx + 1,
                    execution_order.len(),
                    stage.name.cyan().bold(),
                    parallel_tag.yellow()
                );

                // Run steps in parallel or sequentially
                if stage.parallel {
                    run_stage_parallel(
                        &docker,
                        &stage.steps,
                        verbose,
                        &config.cache,
                        &temp_dir,
                        &runtime,
                    )
                    .await?;
                } else {
                    for step in &stage.steps {
                        run_command_in_container(
                            &docker,
                            step,
                            verbose,
                            &config.cache,
                            &temp_dir,
                            &runtime,
                        )
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

            println!(
                "\n{} {}",
                "[OK]".green().bold(),
                "Pipeline completed successfully!".green().bold()
            );

            // Print total pipeline duration
            println!(
                "    Total duration: {:.2}s",
                pipeline_start.elapsed().as_secs_f64()
            );

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
                         Hint: Run 'forge init' to create an example config, or check the file path",
                        file
                    ),
                )));
            }

            let _ = dotenvy::dotenv();
            if let Some(config_parent) = config_path.parent() {
                let _ = dotenvy::from_path(config_parent.join(".env"));
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
                     Hint: See examples in the documentation or run 'forge init' for a template"
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

            if !config.stages.is_empty() {
                let _ = resolve_stage_dependencies(&config.stages)?;
            }

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
                 • forge run      - Execute the pipeline\n\
                 • forge init     - Create example config\n\
                 • forge validate - Check config syntax\n\
                 • forge --help   - Show detailed help\n\
                 \n\
                 Hint: Start with 'forge init' to create your first pipeline"
                .to_string(),
        ))),
    }
}

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
