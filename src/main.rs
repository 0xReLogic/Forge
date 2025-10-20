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

const LONG_ABOUT: &str = r#"FORGE is a CLI tool designed for developers frustrated with the slow feedback cycle of cloud-based CI/CD. By emulating CI/CD pipelines locally using Docker, FORGE aims to drastically improve developer productivity.

It runs steps and stages defined in a `forge.yaml` file.

EXAMPLE forge.yaml:
version: "1.0"
stages:
  - name: build
    steps:
      - name: build-app
        image: rust:latest
        command: cargo build --release
        working_dir: /app
  - name: test
    depends_on: [build]
    steps:
      - name: run-unit-tests
        image: rust:latest
        command: cargo test
        working_dir: /app"#;

#[derive(Parser)]
#[command(
    name = "forge",
    author = "FORGE Team",
    version = "0.1.0",
    about = "Local CI/CD Runner",
    long_about = LONG_ABOUT,
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
    #[command(long_about = r#"
Runs a FORGE pipeline from a specified configuration file.

By default, it runs all stages in the order they are defined, respecting `depends_on` relationships. You can also run a specific stage and its dependencies.

EXAMPLES:
1. Run the entire pipeline defined in `forge.yaml`:
   forge run

2. Run only the 'test' stage and its dependencies:
   forge run --stage test

3. Run with verbose output and a different config file:
   forge run --file .forge/ci.yaml --verbose
"#)]
    Run {
        #[arg(short, long, default_value = "forge.yaml", help = "Path to the forge configuration file")]
        file: String,

        #[arg(short, long, help = "Enable verbose output", long_help = "Enable verbose output, streaming logs from each step's container to stdout in real-time.")]
        verbose: bool,

        #[arg(long, help = "Force enable caching", long_help = "Force enable caching of specified directories. This overrides the `enabled` setting in the `forge.yaml` file.")]
        cache: bool,

        #[arg(long, help = "Force disable caching", long_help = "Force disable caching of specified directories. This overrides the `enabled` setting in the `forge.yaml` file.")]
        no_cache: bool,

        #[arg(short, long, help = "Run a specific stage and its dependencies")]
        stage: Option<String>,
    },

    #[command(long_about = r#"
Creates a default `forge.yaml` file in the current directory.

This is a great starting point for new projects. The generated file includes comments explaining the different fields and a simple two-stage pipeline (build and test).

EXAMPLES:
1. Create a `forge.yaml` if one doesn't exist:
   forge init

2. Create a config file with a different name:
   forge init --file custom-forge.yaml

3. Overwrite an existing `forge.yaml` file:
   forge init --force
"#)]
    Init {
        #[arg(short, long, default_value = "forge.yaml", help = "Path for the new forge configuration file")]
        file: String,

        #[arg(short = 'F', long, help = "Overwrite the file if it already exists")]
        force: bool,
    },

    #[command(long_about = r#"
Parses and validates the `forge.yaml` configuration file without executing the pipeline.

This command is useful for catching syntax errors, typos, and structural problems in your configuration before committing it or running the pipeline.

EXAMPLES:
1. Validate the default `forge.yaml`:
   forge validate

2. Validate a different configuration file:
   forge validate --file .forge/ci.yaml
"#)]
    Validate {
        #[arg(short, long, default_value = "forge.yaml", help = "Path to the forge configuration file to validate")]
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
                         • Authentication required f",
                    image, e
                ))));
            }
        }
    }
    Ok(())
}