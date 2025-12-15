use bollard::Docker;
use bollard::container::{Config, CreateContainerOptions, LogOutput, LogsOptions};
use bollard::image::CreateImageOptions;
use bollard::models::{HostConfig, Mount, MountTypeEnum};
use colored::*;
use futures_util::stream::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::{CacheConfig, Step};

pub type LogBuffer = Arc<Mutex<Vec<Option<(String, Vec<LogEntry>)>>>>;

#[derive(Clone)]
pub struct ContainerSetup {
    pub name: String,
    pub config: Config<String>,
    pub image: String,
    pub step_name: String,
}

pub struct ContainerRuntimeContext<'a> {
    pub workspace_dir: &'a Path,
    pub cache_dir: &'a Path,
    pub secrets_env: &'a HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub enum LogEntry {
    StdOut(String),
    StdErr(String),
    Error(String),
}

pub async fn pull_image(
    docker: &Docker,
    image: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if docker.inspect_image(image).await.is_ok() {
        println!("  {} Image ready: {}", "[OK]".green(), image);
        return Ok(());
    }

    println!("  {} Pulling image: {}", "[..]".blue(), image);

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

    spinner.finish_with_message(format!("[OK] Image ready: {image}"));
    Ok(())
}

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
        cache_setup.push_str(&format!("mkdir -p /forge-cache{dir}\n"));
        cache_setup.push_str(&format!("mkdir -p {dir}\n"));
        cache_setup.push_str(&format!(
            "if [ -d /forge-cache{dir} ] && [ \"$(ls -A /forge-cache{dir})\" ]; then cp -a /forge-cache{dir}/. {dir}/ 2>/dev/null || true; fi\n"
        ));
    }

    let mut cache_teardown = String::new();
    for dir in &cache_config.directories {
        cache_teardown.push_str(&format!("mkdir -p /forge-cache{dir}\n"));
        cache_teardown.push_str(&format!(
            "if [ -d {dir} ] && [ \"$(ls -A {dir})\" ]; then cp -a {dir}/. /forge-cache{dir}/ 2>/dev/null || true; fi\n"
        ));
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

pub async fn prepare_container(
    docker: &Docker,
    step: &Step,
    cache_config: &CacheConfig,
    temp_dir: &Path,
    ctx: &ContainerRuntimeContext<'_>,
    verbose: bool,
) -> Result<ContainerSetup, Box<dyn std::error::Error + Send + Sync>> {
    let image = if step.image.is_empty() {
        "alpine:latest"
    } else {
        &step.image
    };

    pull_image(docker, image).await?;

    let container_name = format!("forge-{}", uuid::Uuid::new_v4());
    let mut env_map: HashMap<String, String> = step.env.clone();
    for (k, v) in ctx.secrets_env {
        env_map.insert(k.clone(), v.clone());
    }
    let env: Vec<String> = env_map.iter().map(|(k, v)| format!("{k}={v}")).collect();

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
        if !env_map.is_empty() {
            println!("  Environment variables:");
            let mut keys: Vec<&String> = env_map.keys().collect();
            keys.sort();
            for k in keys {
                if ctx.secrets_env.contains_key(k) {
                    println!("    {k}=********");
                } else {
                    let v = env_map.get(k).map(String::as_str).unwrap_or("");
                    println!("    {k}={v}");
                }
            }
        }
    }

    let mut mounts = vec![];
    let shared_mount = Mount {
        target: Some("/forge-shared".to_string()),
        source: Some(temp_dir.to_string_lossy().to_string()),
        typ: Some(MountTypeEnum::BIND),
        ..Default::default()
    };
    mounts.push(shared_mount);

    let workspace_mount = Mount {
        target: Some("/workspace".to_string()),
        source: Some(ctx.workspace_dir.to_string_lossy().to_string()),
        typ: Some(MountTypeEnum::BIND),
        ..Default::default()
    };
    mounts.push(workspace_mount);

    if cache_config.enabled && !cache_config.directories.is_empty() {
        let cache_mount = Mount {
            target: Some("/forge-cache".to_string()),
            source: Some(ctx.cache_dir.to_string_lossy().to_string()),
            typ: Some(MountTypeEnum::BIND),
            ..Default::default()
        };
        mounts.push(cache_mount);
    }

    let host_config = HostConfig {
        auto_remove: Some(false),
        mounts: Some(mounts),
        ..Default::default()
    };

    let command = build_command_with_cache(&step.command, cache_config, verbose);

    let config = Config {
        image: Some(image.to_string()),
        cmd: Some(vec!["/bin/sh".to_string(), "-c".to_string(), command]),
        env: Some(env),
        working_dir: if step.working_dir.is_empty() {
            Some("/workspace".to_string())
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

pub async fn stream_logs_buffered(
    docker: &Docker,
    container_id: &str,
    step_name: &str,
    step_index: usize,
    log_buffer: LogBuffer,
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

pub async fn cleanup_container(docker: &Docker, container_id: &str) {
    if let Err(e) = docker.stop_container(container_id, None).await
        && !e.to_string().contains("304")
    {
        eprintln!("Warning: Failed to stop container {}: {}", container_id, e);
    }

    match docker.remove_container(container_id, None).await {
        Ok(_) => println!("Container removed: {}", container_id),
        Err(e) => eprintln!("Failed to remove container: {e}"),
    }
}

pub async fn print_synchronized_logs(log_buffer: &LogBuffer) {
    let logs = log_buffer.lock().await;

    for (step_name, entries) in logs.iter().flatten() {
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

pub async fn cleanup_containers(docker: &Docker, container_ids: &Arc<Mutex<Vec<String>>>) {
    let ids = { container_ids.lock().await.clone() };

    for id in ids.iter() {
        if let Err(e) = docker.stop_container(id, None).await
            && !e.to_string().contains("304")
        {
            eprintln!("Warning: Failed to stop container {}: {}", id, e);
        }
    }

    for id in ids.iter() {
        if let Err(e) = docker.remove_container(id, None).await {
            eprintln!("Warning: Failed to remove container {}: {}", id, e);
        }
    }
}
