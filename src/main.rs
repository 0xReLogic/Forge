use bollard::Docker;
use bollard::container::{Config, CreateContainerOptions};
use bollard::image::CreateImageOptions;
use bollard::models::{HostConfig, Mount, MountTypeEnum};
use clap::{Parser, Subcommand};
use colored::*;
use futures_util::stream::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::File;
use std::io::Read;
use std::path::Path;

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

#[derive(Debug, Serialize, Deserialize, Default)]
struct CacheConfig {
    #[serde(default)]
    directories: Vec<String>,

    #[serde(default)]
    enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct Secret {
    /// Secret name
    name: String,

    /// Name of the environment variable on the host containing the secret value
    env_var: String,
}

// --- ADDED ---
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

#[derive(Parser)]
#[command(
    name = "forge",
    author = "FORGE Team",
    version = "0.1.0",
    about = "Local CI/CD Runner",
    long_about = "FORGE is a CLI tool designed for developers frustrated with the slow feedback cycle of cloud-based CI/CD. By emulating CI/CD pipelines locally using Docker, FORGE aims to drastically improve developer productivity.",
    // --- ADDED ---
    // This adds the sample YAML to the main --help output, as requested.
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
    // --- ADDED ---
    // This adds the detailed EXAMPLES block to the `run` subcommand's help.
    #[command(after_help = "EXAMPLES:
    # Run default pipeline from forge.yaml
    forge-cli run

    # Run with a custom config file
    forge-cli run --file ci/pipeline.yaml

    # Run only the 'build' stage
    forge-cli run --stage build

    # Run with verbose output and caching disabled
    forge-cli run --verbose --no-cache")]
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

    // --- ADDED ---
    // Added examples for the `init` subcommand.
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

    // --- ADDED ---
    // Added examples for the `validate` subcommand.
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
async fn pull_image(
    docker: &Docker,
    image: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("{}", format!("Pulling image: {image}").cyan().bold());

    let options = Some(CreateImageOptions {
        from_image: image.to_string(),
        ..Default::default()
    });

    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ")
            .template("{spinner:.blue} {msg}")
            .unwrap(),
    );
    spinner.set_message(format!("Pulling {image}"));

    let mut stream = docker.create_image(options, None, None);

    while let Some(result) = stream.next().await {
        match result {
            Ok(info) => {
                if let Some(status) = info.status {
                    spinner.set_message(format!("{image}: {status}"));
                }
            }
            Err(e) => {
                spinner.finish_with_message(format!("Failed to pull image: {image}"));
                return Err(Box::new(std::io::Error::other(format!(
                    "Failed to pull Docker image '{}': {}\n\
                         Possible causes:\n\
                         • Image name is incorrect or doesn't exist\n\
                         • No internet connection\n\
                         • Docker registry is unreachable\n\
                         • Authentication required for private images\n\
                         Hint: Try 'docker pull {}' manually to test connectivity",
                    image, e, image
                ))));
            }
        }
    }

    spinner.finish_with_message(format!(
        "{}",
        format!("Image pulled successfully: {image}").green()
    ));
    Ok(())
}
async fn run_command_in_container(
    docker: &Docker,
    step: &Step,
    verbose: bool,
    cache_config: &CacheConfig,
    temp_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let image = if step.image.is_empty() {
        "alpine:latest"
    } else {
        &step.image
    };

    // Pull the image if needed
    pull_image(docker, image).await?;

    // Create a unique container name
    let container_name = format!("forge-{}", uuid::Uuid::new_v4());

    // Prepare environment variables
    let env: Vec<String> = step.env.iter().map(|(k, v)| format!("{k}={v}")).collect();

    // Create container
    let step_name = if step.name.is_empty() {
        "unnamed step"
    } else {
        &step.name
    };
    println!("{}", format!("Running step: {step_name}").yellow().bold());
    if verbose {
        println!("  Command: {command}", command = step.command);
        println!("  Image: {image}");
        if !step.working_dir.is_empty() {
            println!("  Working directory: {dir}", dir = step.working_dir);
        }
        if !step.env.is_empty() {
            println!("  Environment variables:");
            for (k, v) in &step.env {
                println!("    {k}={v}");
            }
        }
    }

    let options = Some(CreateContainerOptions {
        name: container_name.clone(),
        ..Default::default()
    });

    // Setup volume mounts for caching
    let mut mounts = vec![];

    // Add bind mount for shared data between steps
    let shared_mount = Mount {
        target: Some("/forge-shared".to_string()),
        source: Some(temp_dir.to_string_lossy().to_string()),
        typ: Some(MountTypeEnum::BIND),
        ..Default::default()
    };
    mounts.push(shared_mount);

    // Setup host config with mounts
    let host_config = HostConfig {
        auto_remove: Some(false), // Change to false to prevent automatic removal
        mounts: Some(mounts),
        ..Default::default()
    };

    // If caching is enabled, add cache directories to the command
    let mut command = step.command.clone();
    if cache_config.enabled && !cache_config.directories.is_empty() {
        // Create a script for cache setup
        let mut cache_setup = String::new();
        for dir in &cache_config.directories {
            // Create the directory in the shared volume if it doesn't exist
            cache_setup.push_str(&format!("mkdir -p /forge-shared{dir}\n"));
            // Create the target directory if it doesn't exist
            cache_setup.push_str(&format!("mkdir -p {dir}\n"));
            // Copy from shared volume to the target directory if it exists
            cache_setup.push_str(&format!("if [ -d /forge-shared{dir} ] && [ \"$(ls -A /forge-shared{dir})\" ]; then cp -r /forge-shared{dir}/* {dir}/ 2>/dev/null || true; fi\n"));
        }

        // Create a script for cache teardown
        let mut cache_teardown = String::new();
        for dir in &cache_config.directories {
            // Create the directory in the shared volume if it doesn't exist
            cache_teardown.push_str(&format!("mkdir -p /forge-shared{dir}\n"));
            // Copy from the target directory to the shared volume if it exists
            cache_teardown.push_str(&format!("if [ -d {dir} ] && [ \"$(ls -A {dir})\" ]; then cp -r {dir}/* /forge-shared{dir}/ 2>/dev/null || true; fi\n"));
        }

        // Create a combined script
        let script = format!(
            "#!/bin/sh\n\n# Cache setup\n{cache_setup}\n# Main command\n{command}\n\n# Cache teardown\n{cache_teardown}\n\n# Exit with the status of the main command\nexit $?",
        );

        // Use the script as the command
        command = script;

        if verbose {
            println!(
                "  Cache enabled for directories: {:?}",
                cache_config.directories
            );
        }
    }

    let config = Config {
        image: Some(image.to_string()),
        cmd: Some(vec!["/bin/sh".to_string(), "-c".to_string(), command]),
        env: Some(env),
        working_dir: if step.working_dir.is_empty() {
            None
        } else {
            Some(step.working_dir.clone())
        },
        host_config: Some(host_config),
        ..Default::default()
    };

    let container = docker
        .create_container(options, config)
        .await
        .map_err(|e| {
            Box::new(std::io::Error::other(format!(
                "Failed to create Docker container for step '{}': {}\n\
                 Possible causes:\n\
                 • Docker daemon is not running\n\
                 • Insufficient disk space\n\
                 • Invalid container configuration\n\
                 • Docker image '{}' is corrupted\n\
                 Hint: Try 'docker ps' to check if Docker is running",
                step_name, e, image
            )))
        })?;

    // Start container
    docker
        .start_container::<String>(&container.id, None)
        .await
        .map_err(|e| {
            Box::new(std::io::Error::other(format!(
                "Failed to start Docker container '{}' for step '{}': {}\n\
                     Possible causes:\n\
                     • Docker daemon stopped responding\n\
                     • Container configuration is invalid\n\
                     • Insufficient system resources\n\
                     Hint: Check Docker daemon status with 'docker info'",
                container.id, step_name, e
            )))
        })?;

    // Wait for container to finish first
    let mut wait_stream = docker.wait_container::<String>(&container.id, None);
    let wait_future = wait_stream.next();

    // Stream logs while waiting
    let log_options = bollard::container::LogsOptions {
        follow: true,
        stdout: true,
        stderr: true,
        ..Default::default()
    };

    let mut log_stream = docker.logs::<String>(&container.id, Some(log_options));

    while let Some(result) = log_stream.next().await {
        match result {
            Ok(output) => match output {
                bollard::container::LogOutput::StdOut { message } => {
                    println!("{}", String::from_utf8_lossy(&message));
                }
                bollard::container::LogOutput::StdErr { message } => {
                    eprintln!("{}", String::from_utf8_lossy(&message).red());
                }
                _ => {}
            },
            Err(e) => {
                eprintln!("Error streaming logs: {e}");
                break;
            }
        }
    }

    // Get the wait result
    let wait_result = wait_future.await;

    // Process the wait result
    let exit_status = match wait_result {
        Some(Ok(exit)) => {
            if exit.status_code == 0 {
                println!(
                    "{}",
                    format!("Step completed successfully: {step_name}")
                        .green()
                        .bold()
                );
                true
            } else {
                let error_msg = format!(
                    "Step failed with exit code {}: {}",
                    exit.status_code, step_name
                );
                println!("{}", error_msg.red().bold());
                false
            }
        }
        Some(Err(e)) => {
            let error_msg = format!("Error waiting for container: {e}");
            println!("{}", error_msg.red().bold());
            false
        }
        None => {
            let error_msg = "Container exited without providing a status code";
            println!("{}", error_msg.red().bold());
            false
        }
    };

    // Clean up the container manually
    match docker.remove_container(&container.id, None).await {
        Ok(_) => println!("Container removed: {}", container.id),
        Err(e) => eprintln!("Failed to remove container: {e}"),
    }
    if exit_status {
        Ok(())
    } else {
        Err(Box::new(std::io::Error::other(format!(
            "Step '{}' failed with exit code {}\n\
                 Command: {}\n\
                 Image: {}\n\
                 Hint: Check the command output above for error details. \n\
                 You can run with --verbose for more detailed logging",
            step_name,
            "non-zero", // We'll need to capture the actual exit code
            step.command,
            image
        ))))
    }
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

            let mut config = read_forge_config(config_path)?;

            // Override cache settings if specified
            if cache {
                config.cache.enabled = true;
            }
            if no_cache {
                config.cache.enabled = false;
            }

            // Connect to Docker
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
                println!("{}", format!("Stage: {}", stage.name).cyan().bold());

                // Run steps in parallel or sequentially
                if stage.parallel {
                    // TODO: Implement parallel execution
                    // For now, just run sequentially
                    for step in &stage.steps {
                        run_command_in_container(&docker, step, verbose, &config.cache, &temp_dir)
                            .await?;
                    }
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
