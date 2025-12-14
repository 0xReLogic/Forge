//! Container management utilities for Forge
//!
//! This module provides helper functions for Docker container lifecycle management,
//! including container setup, execution, log streaming, and cleanup.

use bollard::Docker;
use bollard::container::{Config, CreateContainerOptions, LogOutput, LogsOptions};
use bollard::image::CreateImageOptions;
use bollard::models::{HostConfig, Mount, MountTypeEnum};
use colored::*;
use futures_util::stream::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::{CacheConfig, Step};

/// Represents a configured container ready to be created
#[derive(Clone)]
pub struct ContainerSetup {
    pub name: String,
    pub config: Config<String>,
    pub image: String,
    pub step_name: String,
}

/// Log entry type for buffered logging in parallel execution
#[derive(Clone, Debug)]
pub enum LogEntry {
    StdOut(String),
    StdErr(String),
    Error(String),
}

/// Pull a Docker image if not already available.
///
/// This function checks if the required Docker image is already available
/// on the local machine. If not, it will pull it from the Docker registry.
/// Displays a spinner during the pulling process.
///
/// # Arguments
///
/// * `docker` - Reference to the Docker client
/// * `image` - Name of the Docker image to pull (e.g., "node:16-alpine")
///
/// # Returns
///
/// * `Result<(), Box<dyn std::error::Error + Send + Sync>>` - Success or error
///
/// # Errors
///
/// This function will return an error if:
/// - Connection to the Docker daemon fails
/// - The image is not found in the registry
/// - There's no internet connection (if the image is not already available locally)
pub async fn pull_image(
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

/// Build command with cache setup/teardown scripts
///
/// If caching is enabled, wraps the base command with shell scripts that:
/// 1. Set up cache directories from shared volume before execution
/// 2. Run the main command
/// 3. Copy cache directories back to shared volume after execution
///
/// # Arguments
///
/// * `base_command` - The original command to execute
/// * `cache_config` - Cache configuration
/// * `verbose` - Whether to print cache information
///
/// # Returns
///
/// The command string, potentially wrapped with cache setup/teardown
fn build_command_with_cache(
    base_command: &str,
    cache_config: &CacheConfig,
    verbose: bool,
) -> String {
    if !cache_config.enabled || cache_config.directories.is_empty() {
        return base_command.to_string();
    }

    let mut cache_setup = String::new();
    for dir in &cache_config.directories {
        cache_setup.push_str(&format!("mkdir -p /forge-shared{dir}\n"));
        cache_setup.push_str(&format!("mkdir -p {dir}\n"));
        cache_setup.push_str(&format!("if [ -d /forge-shared{dir} ] && [ \"$(ls -A /forge-shared{dir})\" ]; then cp -r /forge-shared{dir}/* {dir}/ 2>/dev/null || true; fi\n"));
    }

    let mut cache_teardown = String::new();
    for dir in &cache_config.directories {
        cache_teardown.push_str(&format!("mkdir -p /forge-shared{dir}\n"));
        cache_teardown.push_str(&format!("if [ -d {dir} ] && [ \"$(ls -A {dir})\" ]; then cp -r {dir}/* /forge-shared{dir}/ 2>/dev/null || true; fi\n"));
    }

    if verbose {
        println!(
            "  Cache enabled for directories: {:?}",
            cache_config.directories
        );
    }

    format!(
        "#!/bin/sh\n\n# Cache setup\n{cache_setup}\n# Main command\n{base_command}\n\n# Cache teardown\n{cache_teardown}\n\n# Exit with the status of the main command\nexit $?",
    )
}

/// Prepare container configuration from a step
///
/// This function:
/// 1. Pulls the Docker image if needed
/// 2. Generates a unique container name
/// 3. Prepares environment variables
/// 4. Sets up volume mounts
/// 5. Builds the command with cache support
/// 6. Creates the container configuration
///
/// # Arguments
///
/// * `docker` - Reference to the Docker client
/// * `step` - Step configuration
/// * `cache_config` - Cache configuration
/// * `temp_dir` - Temporary directory for inter-container communication
/// * `verbose` - Whether to print verbose information
///
/// # Returns
///
/// A `ContainerSetup` with all configuration ready for container creation
pub async fn prepare_container(
    docker: &Docker,
    step: &Step,
    cache_config: &CacheConfig,
    temp_dir: &Path,
    verbose: bool,
) -> Result<ContainerSetup, Box<dyn std::error::Error + Send + Sync>> {
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

    let step_name = if step.name.is_empty() {
        "unnamed step"
    } else {
        &step.name
    };

    if verbose {
        println!("{}", format!("Running step: {step_name}").yellow().bold());
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

    // Setup volume mounts
    let mut mounts = vec![];
    let shared_mount = Mount {
        target: Some("/forge-shared".to_string()),
        source: Some(temp_dir.to_string_lossy().to_string()),
        typ: Some(MountTypeEnum::BIND),
        ..Default::default()
    };
    mounts.push(shared_mount);

    let host_config = HostConfig {
        auto_remove: Some(false),
        mounts: Some(mounts),
        ..Default::default()
    };

    // Build command with cache support
    let command = build_command_with_cache(&step.command, cache_config, verbose);

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

    Ok(ContainerSetup {
        name: container_name,
        config,
        image: image.to_string(),
        step_name: step_name.to_string(),
    })
}

/// Create and start a Docker container
///
/// # Arguments
///
/// * `docker` - Reference to the Docker client
/// * `setup` - Container setup configuration
///
/// # Returns
///
/// The container ID as a String
pub async fn create_and_start_container(
    docker: &Docker,
    setup: &ContainerSetup,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let options = Some(CreateContainerOptions {
        name: setup.name.clone(),
        ..Default::default()
    });

    let container = docker
        .create_container(options, setup.config.clone())
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
                setup.step_name, e, setup.image
            )))
        })?;

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
                container.id, setup.step_name, e
            )))
        })?;

    Ok(container.id)
}

/// Stream logs immediately to stdout/stderr (for sequential execution)
///
/// # Arguments
///
/// * `docker` - Reference to the Docker client
/// * `container_id` - ID of the running container
pub async fn stream_logs_immediate(
    docker: &Docker,
    container_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let log_options = LogsOptions {
        follow: true,
        stdout: true,
        stderr: true,
        ..Default::default()
    };

    let mut log_stream = docker.logs::<String>(container_id, Some(log_options));

    while let Some(result) = log_stream.next().await {
        match result {
            Ok(output) => match output {
                LogOutput::StdOut { message } => {
                    print!("{}", String::from_utf8_lossy(&message));
                }
                LogOutput::StdErr { message } => {
                    eprint!("{}", String::from_utf8_lossy(&message).red());
                }
                _ => {}
            },
            Err(e) => {
                eprintln!("Error streaming logs: {e}");
                break;
            }
        }
    }

    Ok(())
}

/// Buffer logs for later synchronized output (for parallel execution)
///
/// # Arguments
///
/// * `docker` - Reference to the Docker client
/// * `container_id` - ID of the running container
/// * `step_name` - Name of the step (for log identification)
/// * `step_index` - Index of this step in the original steps array (for ordering)
/// * `log_buffer` - Shared buffer to store logs at specific indices
pub async fn stream_logs_buffered(
    docker: &Docker,
    container_id: &str,
    step_name: &str,
    step_index: usize,
    log_buffer: Arc<Mutex<Vec<Option<(String, Vec<LogEntry>)>>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let log_options = LogsOptions {
        follow: true,
        stdout: true,
        stderr: true,
        ..Default::default()
    };

    let mut log_stream = docker.logs::<String>(container_id, Some(log_options));
    let mut entries = Vec::new();

    while let Some(result) = log_stream.next().await {
        match result {
            Ok(output) => {
                let entry = match output {
                    LogOutput::StdOut { message } => {
                        LogEntry::StdOut(String::from_utf8_lossy(&message).to_string())
                    }
                    LogOutput::StdErr { message } => {
                        LogEntry::StdErr(String::from_utf8_lossy(&message).to_string())
                    }
                    _ => continue,
                };
                entries.push(entry);
            }
            Err(e) => {
                entries.push(LogEntry::Error(format!("Error streaming logs: {e}")));
                break;
            }
        }
    }

    let mut buffer = log_buffer.lock().await;
    buffer[step_index] = Some((step_name.to_string(), entries));

    Ok(())
}

/// Wait for container completion and return exit status
///
/// # Arguments
///
/// * `docker` - Reference to the Docker client
/// * `container_id` - ID of the running container
/// * `step_name` - Name of the step (for error messages)
///
/// # Returns
///
/// `Ok(true)` if container exited with code 0, `Err` otherwise
pub async fn wait_for_container(
    docker: &Docker,
    container_id: &str,
    step_name: &str,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let mut wait_stream = docker.wait_container::<String>(container_id, None);
    let wait_result = wait_stream.next().await;

    match wait_result {
        Some(Ok(exit)) => {
            if exit.status_code == 0 {
                Ok(true)
            } else {
                Err(Box::new(std::io::Error::other(format!(
                    "Step '{}' failed with exit code {}\n\
                     Hint: Check the command output above for error details. \n\
                     You can run with --verbose for more detailed logging",
                    step_name, exit.status_code
                ))))
            }
        }
        Some(Err(e)) => Err(Box::new(std::io::Error::other(format!(
            "Error waiting for container: {}\n\
             Possible causes:\n\
             • Docker daemon crashed or stopped\n\
             • Container was forcibly terminated\n\
             • Network connection to Docker was lost\n\
             Hint: Check Docker daemon logs for more information",
            e
        )))),
        None => Err(Box::new(std::io::Error::other(
            "Container exited without providing a status code\n\
             Possible causes:\n\
             • Container was killed externally\n\
             • Docker daemon restarted during execution\n\
             Hint: Check 'docker ps -a' to see container status",
        ))),
    }
}

/// Remove a Docker container
///
/// # Arguments
///
/// * `docker` - Reference to the Docker client
/// * `container_id` - ID of the container to remove
pub async fn cleanup_container(docker: &Docker, container_id: &str) {
    match docker.remove_container(container_id, None).await {
        Ok(_) => println!("Container removed: {}", container_id),
        Err(e) => eprintln!("Failed to remove container: {e}"),
    }
}

/// Print buffered logs in order (for parallel execution)
///
/// Prints logs in the order they were defined in the steps array,
/// not in the order tasks completed.
///
/// # Arguments
///
/// * `log_buffer` - Buffer containing all logs from parallel steps (indexed by step order)
pub async fn print_synchronized_logs(
    log_buffer: &Arc<Mutex<Vec<Option<(String, Vec<LogEntry>)>>>>,
) {
    let logs = log_buffer.lock().await;

    for log_entry in logs.iter() {
        if let Some((step_name, entries)) = log_entry {
            println!(
                "\n{}",
                format!("=== Logs for step: {} ===", step_name)
                    .cyan()
                    .bold()
            );
            for entry in entries {
                match entry {
                    LogEntry::StdOut(msg) => print!("{}", msg),
                    LogEntry::StdErr(msg) => eprint!("{}", msg.red()),
                    LogEntry::Error(msg) => eprintln!("{}", msg.red().bold()),
                }
            }
        }
    }
}

/// Cleanup multiple containers (used in parallel execution cleanup)
///
/// Stops all containers first (to handle fail-fast scenarios where containers
/// are still running), then removes them.
///
/// # Arguments
///
/// * `docker` - Reference to the Docker client
/// * `container_ids` - Shared list of container IDs to remove
pub async fn cleanup_containers(docker: &Docker, container_ids: &Arc<Mutex<Vec<String>>>) {
    let ids = container_ids.lock().await;

    // First, stop all containers (may still be running in fail-fast scenarios)
    for id in ids.iter() {
        // Stop with a 2-second timeout
        if let Err(e) = docker.stop_container(id, None).await {
            // Container might already be stopped, that's okay
            if !e.to_string().contains("304") {
                // 304 = Not Modified (already stopped)
                eprintln!("Warning: Failed to stop container {}: {}", id, e);
            }
        }
    }

    // Then remove all containers
    for id in ids.iter() {
        if let Err(e) = docker.remove_container(id, None).await {
            eprintln!("Warning: Failed to remove container {}: {}", id, e);
        }
    }
}
