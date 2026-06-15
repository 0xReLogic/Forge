use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Step {
    #[serde(default)]
    pub name: String,

    /// Command to run inside the container
    pub command: String,

    #[serde(default)]
    pub image: String,

    #[serde(default)]
    pub working_dir: String,

    #[serde(default)]
    pub env: HashMap<String, String>,

    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Stage {
    /// Stage name
    pub name: String,

    /// Steps in this stage
    pub steps: Vec<Step>,

    #[serde(default)]
    pub parallel: bool,

    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct CacheConfig {
    #[serde(default)]
    pub directories: Vec<String>,

    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Secret {
    /// Secret name
    pub name: String,

    /// Name of the environment variable on the host containing the secret value
    pub env_var: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ForgeConfig {
    #[serde(default = "default_version")]
    pub version: String,

    #[serde(default)]
    pub stages: Vec<Stage>,

    #[serde(default)]
    pub steps: Vec<Step>,

    #[serde(default)]
    pub cache: CacheConfig,

    #[serde(default)]
    pub secrets: Vec<Secret>,
}

pub fn default_version() -> String {
    "1.0".to_string()
}

pub fn read_forge_config(path: &Path) -> Result<ForgeConfig, Box<dyn std::error::Error + Send + Sync>> {
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

pub fn validate_parallel_stages(
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
