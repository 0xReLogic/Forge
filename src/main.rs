pub mod config;
pub mod secrets;
pub mod cache;
pub mod logger;
pub mod docker;
pub mod runner;
pub mod tui;

use bollard::Docker;
use cache::{compute_cache_key, default_cache_dir, ensure_git_excludes_forge_dir};
use clap::{Parser, Subcommand};
use colored::*;
use config::{Stage, read_forge_config, validate_parallel_stages};
use logger::Timer;
use runner::{
    PipelineRuntimeContext, resolve_stage_dependencies, run_command_in_container,
    run_stage_parallel, monitor::{PipelineMonitor, StdoutMonitor},
};
use secrets::collect_secrets_env;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::File;
use std::path::Path;
use std::sync::{Arc, Mutex};







// This multi-line string will be inserted into the help messages.
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
    forge run --dry-run")]
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
        }) => {
            // Start overall pipeline timer
            let pipeline_start = std::time::Instant::now();

            if !tui {
                println!(
                    "{}",
                    if dry_run {
                        "FORGE Pipeline Runner (DRY RUN MODE)".cyan().bold()
                    } else {
                        "FORGE Pipeline Runner".cyan().bold()
                    }
                );
            }

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

            let workspace_dir = env::current_dir()?;
            let runtime = PipelineRuntimeContext {
                cache_dir: {
                    let cache_root = default_cache_dir(&workspace_dir);
                    let cache_key = compute_cache_key(&workspace_dir);
                    cache_root.join(cache_key)
                },
                workspace_dir,
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
            if config.cache.enabled {
                let _ = ensure_git_excludes_forge_dir(&runtime.workspace_dir);
                std::fs::create_dir_all(&runtime.cache_dir)?;
            }

            if verbose && !tui {
                println!(
                    "{} Execution order: {}",
                    "[INFO]".blue(),
                    execution_order.join(" -> ")
                );
            }

            let (monitor, tui_state) = if tui {
                let state = Arc::new(Mutex::new(tui::TuiState::new()));
                let m = Arc::new(tui::TuiMonitor::new(Arc::clone(&state)));
                (m as Arc<dyn PipelineMonitor>, Some(state))
            } else {
                let m = Arc::new(StdoutMonitor::new());
                (m as Arc<dyn PipelineMonitor>, None)
            };

            let run_pipeline = {
                let docker = docker.clone();
                let config = config.clone();
                let temp_dir = temp_dir.clone();
                let runtime = runtime.clone();
                let monitor = Arc::clone(&monitor);
                let execution_order = execution_order.clone();
                
                async move {
                    let stage_map: HashMap<String, Stage> = config.stages.iter().map(|s| (s.name.clone(), s.clone())).collect();
                    
                    monitor.on_pipeline_start(&config.stages);

                    for stage_name in &execution_order {
                        let stage = stage_map.get(stage_name).unwrap();
                        let _stage_timer = Timer::new(format!("Stage '{}'", stage.name), verbose && !tui);
                        
                        monitor.on_stage_start(&stage.name, stage.parallel);

                        // Run steps in parallel or sequentially
                        let stage_res = if stage.parallel {
                            run_stage_parallel(
                                &docker,
                                &stage.steps,
                                verbose && !tui,
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
                                    verbose && !tui,
                                    &config.cache,
                                    &temp_dir,
                                    &runtime,
                                    Arc::clone(&monitor),
                                )
                                .await {
                                    res = Err(e);
                                    break;
                                }
                            }
                            res
                        };

                        let success = stage_res.is_ok();
                        monitor.on_stage_complete(&stage.name, success);

                        if !success {
                            if verbose && !tui {
                                println!("Removing temporary directory: {}", temp_dir.display());
                            }
                            let _ = tokio::fs::remove_dir_all(&temp_dir).await;

                            monitor.on_pipeline_complete(false, pipeline_start.elapsed());
                            return stage_res;
                        }
                    }

                    // Clean up the temporary directory after the pipeline is done
                    if verbose && !tui {
                        println!("Removing temporary directory: {}", temp_dir.display());
                    }

                    if let Err(e) = tokio::fs::remove_dir_all(&temp_dir).await {
                        if verbose && !tui {
                            eprintln!("Failed to remove temporary directory: {e}");
                        }
                    } else if verbose && !tui {
                        println!("Temporary directory removed successfully");
                    }

                    monitor.on_pipeline_complete(true, pipeline_start.elapsed());
                    Ok(())
                }
            };

            if let Some(state) = tui_state {
                // TUI mode: spawn the runner in a background task
                let handle = tokio::spawn(run_pipeline);
                
                // Run TUI in the main thread (blocks until 'q' or exit)
                let tui_res = tui::run_tui(Arc::clone(&state)).await;
                
                // Cancel the runner task if it's still running
                handle.abort();
                
                // Stop and remove any remaining active containers
                let containers = {
                    let s = state.lock().unwrap();
                    s.active_containers.clone()
                };
                if !containers.is_empty() {
                    println!("{}", "Cleaning up running containers...".yellow().bold());
                    let docker = Docker::connect_with_local_defaults().unwrap();
                    for cid in containers {
                        println!("Stopping container {}...", cid);
                        let _ = docker.stop_container(&cid, None).await;
                        let _ = docker.remove_container(&cid, None).await;
                    }
                }

                if let Err(e) = tui_res {
                    return Err(e);
                }

                let (success, is_running) = {
                    let s = state.lock().unwrap();
                    (s.pipeline_success, s.pipeline_success.is_none())
                };

                if let Some(false) = success {
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "Pipeline execution failed. Review the TUI logs for details.",
                    )));
                } else if is_running {
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "Pipeline execution was aborted by the user.",
                    )));
                }
            } else {
                // Non-TUI mode: run in foreground
                run_pipeline.await?;
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
