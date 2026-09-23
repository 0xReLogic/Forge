pub mod cache;
pub mod config;
pub mod docker;
pub mod logger;
pub mod output;
pub mod persist;
pub mod result;
pub mod runner;
pub mod secrets;
pub mod tui;

use bollard::Docker;
use cache::{compute_cache_key, default_cache_dir, ensure_git_excludes_forge_dir};
use chrono::Utc;
use clap::{Parser, Subcommand};
use colored::*;
use config::{Stage, read_forge_config, validate_parallel_stages};
use logger::Timer;
use output::{write_human_summary, write_json, write_junit};
use persist::{RunMetadata, persist_run, read_git_info};
use result::{
    ExecutionStatus, ExitCode, FailureReason, OutputFormat, PipelineResult, StageResult, StepResult,
};
use runner::{
    PipelineRuntimeContext,
    monitor::{PipelineMonitor, SilentMonitor, StdoutMonitor},
    resolve_stage_dependencies, run_command_in_container, run_stage_parallel,
};
use secrets::collect_secrets_env;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::File;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

const SAMPLE_YAML: &str = "SAMPLE FORGE.YAML:
    version: \"1.0\"
    stages:
      - name: build
        steps:
          - image: rust:1.91-slim
            working_dir: /workspace
            command: cargo build --release
      - name: test
        steps:
          - image: rust:1.91-slim
            working_dir: /workspace
            command: cargo test";

#[derive(Parser)]
#[command(
    name = "forge",
    author = "FORGE Team",
    version,
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
    forge run --dry-run

    # Machine-readable JSON output (stdout only)
    forge run --format json

    # JUnit XML output for CI test reporting
    forge run --format junit")]
    Run {
        #[arg(short, long, default_value = "forge.yaml")]
        file: String,

        #[arg(short, long)]
        verbose: bool,

        #[arg(
            long,
            help = "Force enable caching (overrides config)",
            conflicts_with = "no_cache"
        )]
        cache: bool,

        #[arg(
            long,
            help = "Force disable caching (overrides config)",
            conflicts_with = "cache"
        )]
        no_cache: bool,

        #[arg(short, long)]
        stage: Option<String>,

        #[arg(
            long,
            help = "Validate pipeline and print what would run, without execution"
        )]
        dry_run: bool,

        #[arg(long, help = "Run with interactive TUI dashboard")]
        tui: bool,

        #[arg(
            long,
            default_value = "human",
            help = "Output format: human (default), json, junit"
        )]
        format: String,
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
    - /workspace/node_modules
    - /workspace/.cache

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

/// Classifies a pipeline-level error into a `FailureReason`.
///
/// The error string from `wait_for_container` embeds the exit code;
/// we parse it out so the classification is as precise as possible.
fn classify_error(err: &str, stage_name: &str, step_name: &str) -> FailureReason {
    if let Some(code) = FailureReason::parse_exit_code_from_message(err) {
        return FailureReason::StepFailed {
            stage: stage_name.to_string(),
            step: step_name.to_string(),
            exit_code: code,
        };
    }

    let lower = err.to_ascii_lowercase();
    if lower.contains("docker")
        || lower.contains("daemon")
        || lower.contains("container")
        || lower.contains("image")
    {
        return FailureReason::DockerError {
            message: err.lines().next().unwrap_or(err).to_string(),
        };
    }

    FailureReason::StepError {
        stage: stage_name.to_string(),
        step: step_name.to_string(),
        message: err.lines().next().unwrap_or(err).to_string(),
    }
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
            tui,
            format,
        }) => {
            // Parse output format early so we can fail fast on a bad value
            // before touching Docker or config.
            let output_format: OutputFormat = format.parse().map_err(|e: String| {
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
            })?;

            let pipeline_start = Instant::now();
            let pipeline_started_at = Utc::now();
            let run_id = uuid::Uuid::new_v4().to_string();

            // In JSON/JUnit mode suppress progress output to keep stdout clean.
            let silent = matches!(output_format, OutputFormat::Json | OutputFormat::Junit);

            if !tui && !silent {
                println!(
                    "{}",
                    if dry_run {
                        "FORGE Pipeline Runner (DRY RUN MODE)".cyan().bold()
                    } else {
                        "FORGE Pipeline Runner".cyan().bold()
                    }
                );
            }

            let config_path = Path::new(&file);
            if !config_path.exists() {
                let reason = FailureReason::ConfigError {
                    message: format!(
                        "Configuration file not found: '{}'\n\
                         Hint: Run 'forge init' to create an example config",
                        file
                    ),
                };
                let code = ExitCode::from_failure_reason(&reason);
                eprintln!(
                    "{}",
                    format!("Error: {}", reason.short_description())
                        .red()
                        .bold()
                );
                std::process::exit(code.as_i32());
            }

            let _ = dotenvy::dotenv();
            if let Some(config_parent) = config_path.parent() {
                let _ = dotenvy::from_path(config_parent.join(".env"));
            }

            let _config_timer = Timer::new("Configuration parsing", verbose && !silent);
            let mut config = match read_forge_config(config_path) {
                Ok(c) => c,
                Err(e) => {
                    let reason = FailureReason::ConfigError {
                        message: e.to_string(),
                    };
                    let code = ExitCode::from_failure_reason(&reason);
                    eprintln!("{}", format!("Error: {e}").red().bold());
                    std::process::exit(code.as_i32());
                }
            };
            drop(_config_timer);

            let workspace_dir = env::current_dir()?;
            let runtime = PipelineRuntimeContext {
                cache_dir: {
                    let cache_root = default_cache_dir(&workspace_dir);
                    let cache_key = compute_cache_key(&workspace_dir);
                    cache_root.join(cache_key)
                },
                workspace_dir: workspace_dir.clone(),
                secrets_env: Arc::new(collect_secrets_env(&config.secrets)?),
            };

            if cache {
                config.cache.enabled = true;
            }
            if no_cache {
                config.cache.enabled = false;
            }

            if let Err(e) = validate_parallel_stages(&config) {
                let reason = FailureReason::ConfigError {
                    message: e.to_string(),
                };
                let code = ExitCode::from_failure_reason(&reason);
                eprintln!("{}", format!("Error: {e}").red().bold());
                std::process::exit(code.as_i32());
            }

            let _docker_timer = Timer::new("Docker connection", verbose && !silent);
            let docker = match Docker::connect_with_local_defaults() {
                Ok(d) => d,
                Err(e) => {
                    let reason = FailureReason::DockerError {
                        message: format!("Failed to connect to Docker: {e}"),
                    };
                    let code = ExitCode::from_failure_reason(&reason);
                    eprintln!("{}", format!("Error: {e}").red().bold());
                    std::process::exit(code.as_i32());
                }
            };

            if let Err(e) = docker.ping().await {
                let reason = FailureReason::DockerError {
                    message: format!("Docker daemon is not responding: {e}"),
                };
                let code = ExitCode::from_failure_reason(&reason);
                eprintln!("{}", format!("Error: {e}").red().bold());
                std::process::exit(code.as_i32());
            }
            drop(_docker_timer);

            if config.stages.is_empty() && !config.steps.is_empty() {
                config.stages.push(Stage {
                    name: "default".to_string(),
                    steps: config.steps.clone(),
                    parallel: false,
                    depends_on: vec![],
                });
            }

            // Stage filtering
            if let Some(stage_name) = stage {
                let available_stages: Vec<String> =
                    config.stages.iter().map(|s| s.name.clone()).collect();

                let stage_map: HashMap<String, &Stage> =
                    config.stages.iter().map(|s| (s.name.clone(), s)).collect();

                if stage_map.contains_key(&stage_name) {
                    let mut required = HashSet::new();
                    let mut stack = vec![stage_name.clone()];

                    while let Some(current) = stack.pop() {
                        if !required.insert(current.clone()) {
                            continue;
                        }
                        let s = stage_map.get(&current).ok_or_else(|| {
                            Box::new(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!(
                                    "Stage '{}' depends on '{}', but '{}' is not defined.\nAvailable stages: {}",
                                    stage_name, current, current, available_stages.join(", ")
                                ),
                            ))
                        })?;
                        for dep in &s.depends_on {
                            stack.push(dep.clone());
                        }
                    }
                    config.stages.retain(|s| required.contains(&s.name));
                }

                if config.stages.is_empty() {
                    let reason = FailureReason::ConfigError {
                        message: format!(
                            "Stage '{}' not found. Available: {}",
                            stage_name,
                            if available_stages.is_empty() {
                                "none".to_string()
                            } else {
                                available_stages.join(", ")
                            }
                        ),
                    };
                    let code = ExitCode::from_failure_reason(&reason);
                    eprintln!(
                        "{}",
                        format!("Error: {}", reason.short_description())
                            .red()
                            .bold()
                    );
                    std::process::exit(code.as_i32());
                }
            }

            if config.stages.is_empty() {
                let reason = FailureReason::ConfigError {
                    message: "No stages or steps found in configuration".into(),
                };
                let code = ExitCode::from_failure_reason(&reason);
                eprintln!(
                    "{}",
                    format!("Error: {}", reason.short_description())
                        .red()
                        .bold()
                );
                std::process::exit(code.as_i32());
            }

            if dry_run {
                if !silent {
                    println!("{} Configuration validated: {}", "[OK]".green(), file);
                    println!("{} Docker connection verified", "[OK]".green());
                }

                let execution_order = resolve_stage_dependencies(&config.stages)?;
                let stage_map: HashMap<String, &Stage> =
                    config.stages.iter().map(|s| (s.name.clone(), s)).collect();

                if !silent {
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
                }
                return Ok(());
            }

            let temp_dir = env::temp_dir().join(format!("forge-{}", uuid::Uuid::new_v4()));
            if let Err(e) = std::fs::create_dir_all(&temp_dir) {
                let reason = FailureReason::RuntimeError {
                    message: format!("Failed to create temp directory: {e}"),
                };
                let code = ExitCode::from_failure_reason(&reason);
                eprintln!("{}", format!("Error: {e}").red().bold());
                std::process::exit(code.as_i32());
            }

            let execution_order = resolve_stage_dependencies(&config.stages)?;
            if config.cache.enabled {
                let _ = ensure_git_excludes_forge_dir(&runtime.workspace_dir);
                std::fs::create_dir_all(&runtime.cache_dir)?;
            }

            if verbose && !tui && !silent {
                println!(
                    "{} Execution order: {}",
                    "[INFO]".blue(),
                    execution_order.join(" -> ")
                );
            }

            if tui {
                // TUI mode: pipeline runs in a background task; TUI renders in main thread.
                let tui_state = Arc::new(Mutex::new(tui::TuiState::new()));
                let monitor: Arc<dyn PipelineMonitor> =
                    Arc::new(tui::TuiMonitor::new(Arc::clone(&tui_state)));

                let stage_map: HashMap<String, Stage> = config
                    .stages
                    .iter()
                    .map(|s| (s.name.clone(), s.clone()))
                    .collect();
                let scheduled_stages: Vec<Stage> = execution_order
                    .iter()
                    .filter_map(|n| stage_map.get(n))
                    .cloned()
                    .collect();
                monitor.on_pipeline_start(&scheduled_stages);

                let run_pipeline = {
                    let docker = docker.clone();
                    let config = config.clone();
                    let temp_dir = temp_dir.clone();
                    let runtime = runtime.clone();
                    let monitor = Arc::clone(&monitor);
                    let execution_order = execution_order.clone();

                    async move {
                        let stage_map: HashMap<String, Stage> = config
                            .stages
                            .iter()
                            .map(|s| (s.name.clone(), s.clone()))
                            .collect();

                        for stage_name in &execution_order {
                            let stage = stage_map.get(stage_name).unwrap();
                            monitor.on_stage_start(&stage.name, stage.parallel);

                            let stage_res = if stage.parallel {
                                run_stage_parallel(
                                    &docker,
                                    &stage.steps,
                                    false,
                                    &config.cache,
                                    &temp_dir,
                                    &runtime,
                                    Arc::clone(&monitor),
                                )
                                .await
                            } else {
                                let mut res = Ok(());
                                for step in &stage.steps {
                                    if let Err(e) = run_command_in_container(
                                        &docker,
                                        step,
                                        false,
                                        &config.cache,
                                        &temp_dir,
                                        &runtime,
                                        Arc::clone(&monitor),
                                    )
                                    .await
                                    {
                                        res = Err(e);
                                        break;
                                    }
                                }
                                res
                            };

                            let success = stage_res.is_ok();
                            monitor.on_stage_complete(&stage.name, success);

                            if !success {
                                let _ = tokio::fs::remove_dir_all(&temp_dir).await;
                                monitor.on_pipeline_complete(false, pipeline_start.elapsed());
                                return stage_res;
                            }
                        }

                        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
                        monitor.on_pipeline_complete(true, pipeline_start.elapsed());
                        Ok(())
                    }
                };

                let handle = tokio::spawn(run_pipeline);
                let tui_res = tui::run_tui(Arc::clone(&tui_state)).await;
                handle.abort();

                let containers = {
                    let s = tui_state.lock().unwrap();
                    s.active_containers.clone()
                };
                if !containers.is_empty() {
                    println!("{}", "Cleaning up running containers...".yellow().bold());
                    let docker = Docker::connect_with_local_defaults().unwrap();
                    for cid in containers {
                        let _ = docker.stop_container(&cid, None).await;
                        let _ = docker.remove_container(&cid, None).await;
                    }
                }

                tui_res?;

                let (success, is_running) = {
                    let s = tui_state.lock().unwrap();
                    (s.pipeline_success, s.pipeline_success.is_none())
                };

                if let Some(false) = success {
                    std::process::exit(ExitCode::PipelineFailure.as_i32());
                } else if is_running {
                    std::process::exit(ExitCode::Cancelled.as_i32());
                }

                return Ok(());
            }

            // Non-TUI mode: structured execution with result collection.
            let monitor: Arc<dyn PipelineMonitor> = if silent {
                Arc::new(SilentMonitor)
            } else {
                Arc::new(StdoutMonitor::new())
            };

            let stage_map: HashMap<String, Stage> = config
                .stages
                .iter()
                .map(|s| (s.name.clone(), s.clone()))
                .collect();

            let scheduled_stages: Vec<Stage> = execution_order
                .iter()
                .filter_map(|n| stage_map.get(n))
                .cloned()
                .collect();

            monitor.on_pipeline_start(&scheduled_stages);

            let mut stage_results: Vec<StageResult> = Vec::new();
            let mut pipeline_failure: Option<FailureReason> = None;

            for stage_name in &execution_order {
                let stage = stage_map.get(stage_name).unwrap();

                if pipeline_failure.is_some() {
                    stage_results.push(StageResult::skipped(&stage.name, stage.parallel));
                    continue;
                }

                let stage_start = Instant::now();
                let _stage_timer =
                    Timer::new(format!("Stage '{}'", stage.name), verbose && !silent);

                monitor.on_stage_start(&stage.name, stage.parallel);

                let (stage_res, step_results) = if stage.parallel {
                    run_parallel_stage_with_results(
                        &docker,
                        stage,
                        verbose && !silent,
                        &config.cache,
                        &temp_dir,
                        &runtime,
                        Arc::clone(&monitor),
                    )
                    .await
                } else {
                    run_sequential_stage_with_results(
                        &docker,
                        stage,
                        verbose && !silent,
                        &config.cache,
                        &temp_dir,
                        &runtime,
                        Arc::clone(&monitor),
                    )
                    .await
                };

                let stage_duration = stage_start.elapsed();
                let success = stage_res.is_ok();
                monitor.on_stage_complete(&stage.name, success);

                let stage_result = StageResult::from_steps(
                    &stage.name,
                    stage.parallel,
                    stage_duration,
                    step_results,
                );
                stage_results.push(stage_result);

                if let Err(e) = stage_res {
                    let failed_step = stage
                        .steps
                        .first()
                        .map(|s| s.name.as_str())
                        .unwrap_or("unknown");
                    pipeline_failure =
                        Some(classify_error(&e.to_string(), &stage.name, failed_step));

                    if verbose && !silent {
                        println!("Removing temporary directory: {}", temp_dir.display());
                    }
                    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
                }
            }

            let pipeline_completed_at = Utc::now();
            let pipeline_duration = pipeline_start.elapsed();
            let pipeline_success = pipeline_failure.is_none();

            monitor.on_pipeline_complete(pipeline_success, pipeline_duration);

            if temp_dir.exists()
                && let Err(e) = tokio::fs::remove_dir_all(&temp_dir).await
                && verbose
                && !silent
            {
                eprintln!("Failed to remove temporary directory: {e}");
            }

            let pipeline_result = PipelineResult {
                run_id: run_id.clone(),
                started_at: pipeline_started_at,
                completed_at: pipeline_completed_at,
                duration_secs: pipeline_duration.as_secs_f64(),
                status: if pipeline_success {
                    ExecutionStatus::Success
                } else {
                    ExecutionStatus::Failed
                },
                failure: pipeline_failure,
                stages: stage_results,
                forge_version: env!("BUILD_VERSION").to_string(),
            };

            // Persist run — non-fatal
            let (git_commit, git_branch) = read_git_info(&workspace_dir);
            let metadata = RunMetadata {
                run_id: run_id.clone(),
                started_at: pipeline_started_at,
                forge_version: env!("BUILD_VERSION").to_string(),
                config_path: config_path
                    .canonicalize()
                    .unwrap_or_else(|_| config_path.to_path_buf())
                    .to_string_lossy()
                    .to_string(),
                git_commit,
                git_branch,
                os: std::env::consts::OS.to_string(),
            };
            if let Err(e) = persist_run(&workspace_dir, &pipeline_result, &metadata) {
                eprintln!(
                    "{}",
                    format!("Warning: could not persist run: {e}").yellow()
                );
            }

            match output_format {
                OutputFormat::Human => {
                    if let Err(e) = write_human_summary(&pipeline_result, &mut std::io::stdout()) {
                        eprintln!("Warning: failed to write summary: {e}");
                    }
                }
                OutputFormat::Json => {
                    if let Err(e) = write_json(&pipeline_result, &mut std::io::stdout()) {
                        eprintln!("Error: failed to write JSON output: {e}");
                    }
                }
                OutputFormat::Junit => {
                    if let Err(e) = write_junit(&pipeline_result, &mut std::io::stdout()) {
                        eprintln!("Error: failed to write JUnit output: {e}");
                    }
                }
            }

            let exit_code = pipeline_result.exit_code();
            if exit_code != ExitCode::Success {
                std::process::exit(exit_code.as_i32());
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
                         Hint: Run 'forge init' to create an example config",
                        file
                    ),
                )));
            }

            let _ = dotenvy::dotenv();
            if let Some(config_parent) = config_path.parent() {
                let _ = dotenvy::from_path(config_parent.join(".env"));
            }

            let config = read_forge_config(config_path)?;

            if config.stages.is_empty() && config.steps.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Configuration validation failed: No stages or steps defined".to_string(),
                )));
            }

            for stage in &config.stages {
                if stage.steps.is_empty() {
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "Configuration validation failed: Stage '{}' has no steps",
                            stage.name
                        ),
                    )));
                }
            }

            for stage in &config.stages {
                for (i, step) in stage.steps.iter().enumerate() {
                    if step.command.trim().is_empty() {
                        return Err(Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!(
                                "Configuration validation failed: Step {} in stage '{}' has empty command",
                                i + 1,
                                stage.name
                            ),
                        )));
                    }
                }
            }

            validate_parallel_stages(&config)?;

            if !config.stages.is_empty() {
                resolve_stage_dependencies(&config.stages)?;
            }

            println!("{}", "Configuration is valid!".green().bold());

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

/// Runs steps sequentially and returns (overall result, per-step results).
async fn run_sequential_stage_with_results(
    docker: &Docker,
    stage: &Stage,
    verbose: bool,
    cache: &config::CacheConfig,
    temp_dir: &std::path::Path,
    runtime: &PipelineRuntimeContext,
    monitor: Arc<dyn PipelineMonitor>,
) -> (
    Result<(), Box<dyn std::error::Error + Send + Sync>>,
    Vec<StepResult>,
) {
    let mut step_results = Vec::new();

    for step in &stage.steps {
        let step_start = Instant::now();
        let step_name = if step.name.is_empty() {
            "unnamed"
        } else {
            &step.name
        };

        let res = run_command_in_container(
            docker,
            step,
            verbose,
            cache,
            temp_dir,
            runtime,
            Arc::clone(&monitor),
        )
        .await;

        let duration = step_start.elapsed();

        match res {
            Ok(()) => {
                step_results.push(StepResult::success(step_name, duration));
            }
            Err(ref e) => {
                let exit_code = FailureReason::parse_exit_code_from_message(&e.to_string());
                step_results.push(StepResult::failed(
                    step_name,
                    duration,
                    exit_code,
                    e.to_string(),
                ));
                // Mark remaining steps as skipped
                for remaining in stage.steps.iter().skip(step_results.len()) {
                    let name = if remaining.name.is_empty() {
                        "unnamed"
                    } else {
                        &remaining.name
                    };
                    step_results.push(StepResult::skipped(name));
                }
                return (res, step_results);
            }
        }
    }

    (Ok(()), step_results)
}

/// Runs steps in parallel and returns (overall result, per-step results).
async fn run_parallel_stage_with_results(
    docker: &Docker,
    stage: &Stage,
    verbose: bool,
    cache: &config::CacheConfig,
    temp_dir: &std::path::Path,
    runtime: &PipelineRuntimeContext,
    monitor: Arc<dyn PipelineMonitor>,
) -> (
    Result<(), Box<dyn std::error::Error + Send + Sync>>,
    Vec<StepResult>,
) {
    let res = run_stage_parallel(
        docker,
        &stage.steps,
        verbose,
        cache,
        temp_dir,
        runtime,
        Arc::clone(&monitor),
    )
    .await;

    // Parallel execution: individual step timings are not tracked at this level yet.
    // Each step either succeeded or we get a single aggregated error back.
    // Build best-effort step results.
    let step_results: Vec<StepResult> = stage
        .steps
        .iter()
        .map(|s| {
            let name = if s.name.is_empty() {
                "unnamed"
            } else {
                &s.name
            };
            if res.is_ok() {
                StepResult::success(name, std::time::Duration::ZERO)
            } else {
                // We can't know which step failed from the aggregated error alone;
                // mark all as failed — a future improvement can track per-task results.
                StepResult::failed(
                    name,
                    std::time::Duration::ZERO,
                    None,
                    "parallel stage failed",
                )
            }
        })
        .collect();

    (res, step_results)
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        if let Err(e) = forge_main().await {
            eprintln!("{}", format!("Error: {e}").red().bold());
            std::process::exit(ExitCode::PipelineFailure.as_i32());
        }
        Ok(())
    })
}
