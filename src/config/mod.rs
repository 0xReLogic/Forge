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

pub fn read_forge_config(
    path: &Path,
) -> Result<ForgeConfig, Box<dyn std::error::Error + Send + Sync>> {
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

/// Target filter specification for selective pipeline execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterTarget {
    /// Filter to run a specific stage (and its transitive dependencies).
    Stage(String),
    /// Filter to run a specific step within a specific stage.
    Step { stage: String, step: String },
}

impl FilterTarget {
    /// Parses a filter string such as "stage" or "stage.step".
    pub fn parse(input: &str) -> Result<Self, String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err("Filter string cannot be empty".to_string());
        }

        if let Some((stage, step)) = trimmed.split_once('.') {
            let stage = stage.trim();
            let step = step.trim();
            if stage.is_empty() {
                return Err("Stage name before '.' cannot be empty".to_string());
            }
            if step.is_empty() {
                return Err(format!(
                    "Step name after '.' cannot be empty in filter '{}'",
                    trimmed
                ));
            }
            Ok(Self::Step {
                stage: stage.to_string(),
                step: step.to_string(),
            })
        } else {
            Ok(Self::Stage(trimmed.to_string()))
        }
    }
}

/// Filters the pipeline to the specified stage or step, preserving dependencies.
pub fn apply_filter(
    config: &mut ForgeConfig,
    filter: &FilterTarget,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let available_stages: Vec<String> = config.stages.iter().map(|s| s.name.clone()).collect();
    let stage_map: HashMap<String, &Stage> =
        config.stages.iter().map(|s| (s.name.clone(), s)).collect();

    let full_filter = match filter {
        FilterTarget::Stage(name) => name.clone(),
        FilterTarget::Step { stage, step } => format!("{stage}.{step}"),
    };
    let resolved_filter = if stage_map.contains_key(&full_filter) {
        FilterTarget::Stage(full_filter)
    } else {
        filter.clone()
    };

    let target_stage_name = match &resolved_filter {
        FilterTarget::Stage(name) => name,
        FilterTarget::Step { stage, .. } => stage,
    };

    if !stage_map.contains_key(target_stage_name) {
        let available_str = if available_stages.is_empty() {
            "none".to_string()
        } else {
            available_stages.join(", ")
        };
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "stage '{}' not found. Available stages: {}",
                target_stage_name, available_str
            ),
        )));
    }

    if let FilterTarget::Step { stage, step } = &resolved_filter {
        let target_stage = stage_map.get(stage).unwrap();
        let available_steps: Vec<String> = target_stage
            .steps
            .iter()
            .map(|st| st.name.clone())
            .filter(|n| !n.is_empty())
            .collect();

        let step_exists = target_stage.steps.iter().any(|st| st.name == *step);
        if !step_exists {
            let available_str = if available_steps.is_empty() {
                "none".to_string()
            } else {
                available_steps.join(", ")
            };
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "step '{}' not found in stage '{}'. Available steps: {}",
                    step, stage, available_str
                ),
            )));
        }
    }

    // Resolve dependencies for target stage
    let mut required = HashSet::new();
    let mut stack = vec![target_stage_name.clone()];

    while let Some(current) = stack.pop() {
        if !required.insert(current.clone()) {
            continue;
        }
        let s = stage_map.get(&current).ok_or_else(|| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Stage '{}' depends on '{}', but '{}' is not defined.\nAvailable stages: {}",
                    target_stage_name,
                    current,
                    current,
                    available_stages.join(", ")
                ),
            ))
        })?;
        for dep in &s.depends_on {
            stack.push(dep.clone());
        }
    }

    // Retain only required stages
    config.stages.retain(|s| required.contains(&s.name));

    // If filtering to a specific step, filter that stage's steps
    if let FilterTarget::Step { stage, step } = &resolved_filter {
        for s in &mut config.stages {
            if s.name == *stage {
                s.steps.retain(|st| st.name == *step);
                // When running a single step, disable parallel flag if it was set
                s.parallel = false;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_target_parse() {
        assert_eq!(
            FilterTarget::parse("test").unwrap(),
            FilterTarget::Stage("test".to_string())
        );
        assert_eq!(
            FilterTarget::parse("test.unit-test").unwrap(),
            FilterTarget::Step {
                stage: "test".to_string(),
                step: "unit-test".to_string()
            }
        );
        assert!(FilterTarget::parse("").is_err());
        assert!(FilterTarget::parse("   ").is_err());
        assert!(FilterTarget::parse(".unit").is_err());
        assert!(FilterTarget::parse("test.").is_err());
    }

    #[test]
    fn test_apply_filter_stage_with_dependencies() {
        let mut config = ForgeConfig {
            version: "1.0".to_string(),
            stages: vec![
                Stage {
                    name: "setup".to_string(),
                    steps: vec![Step {
                        name: "init".to_string(),
                        command: "echo init".to_string(),
                        image: "".to_string(),
                        working_dir: "".to_string(),
                        env: HashMap::new(),
                        depends_on: vec![],
                    }],
                    parallel: false,
                    depends_on: vec![],
                },
                Stage {
                    name: "build".to_string(),
                    steps: vec![Step {
                        name: "compile".to_string(),
                        command: "echo compile".to_string(),
                        image: "".to_string(),
                        working_dir: "".to_string(),
                        env: HashMap::new(),
                        depends_on: vec![],
                    }],
                    parallel: false,
                    depends_on: vec!["setup".to_string()],
                },
                Stage {
                    name: "deploy".to_string(),
                    steps: vec![Step {
                        name: "push".to_string(),
                        command: "echo push".to_string(),
                        image: "".to_string(),
                        working_dir: "".to_string(),
                        env: HashMap::new(),
                        depends_on: vec![],
                    }],
                    parallel: false,
                    depends_on: vec!["build".to_string()],
                },
            ],
            steps: vec![],
            cache: CacheConfig::default(),
            secrets: vec![],
        };

        let filter = FilterTarget::Stage("build".to_string());
        apply_filter(&mut config, &filter).unwrap();

        let stage_names: Vec<String> = config.stages.iter().map(|s| s.name.clone()).collect();
        assert_eq!(stage_names, vec!["setup", "build"]);
    }

    #[test]
    fn test_apply_filter_step() {
        let mut config = ForgeConfig {
            version: "1.0".to_string(),
            stages: vec![Stage {
                name: "test".to_string(),
                steps: vec![
                    Step {
                        name: "unit".to_string(),
                        command: "cargo test --lib".to_string(),
                        image: "".to_string(),
                        working_dir: "".to_string(),
                        env: HashMap::new(),
                        depends_on: vec![],
                    },
                    Step {
                        name: "integration".to_string(),
                        command: "cargo test --test it".to_string(),
                        image: "".to_string(),
                        working_dir: "".to_string(),
                        env: HashMap::new(),
                        depends_on: vec![],
                    },
                ],
                parallel: true,
                depends_on: vec![],
            }],
            steps: vec![],
            cache: CacheConfig::default(),
            secrets: vec![],
        };

        let filter = FilterTarget::Step {
            stage: "test".to_string(),
            step: "unit".to_string(),
        };
        apply_filter(&mut config, &filter).unwrap();

        assert_eq!(config.stages.len(), 1);
        assert_eq!(config.stages[0].steps.len(), 1);
        assert_eq!(config.stages[0].steps[0].name, "unit");
        assert!(!config.stages[0].parallel);
    }

    #[test]
    fn test_apply_filter_prefers_full_dotted_stage_name() {
        let mut config = ForgeConfig {
            version: "1.0".to_string(),
            stages: vec![Stage {
                name: "test.unit".to_string(),
                steps: vec![
                    Step {
                        name: "first".to_string(),
                        command: "echo first".to_string(),
                        image: "".to_string(),
                        working_dir: "".to_string(),
                        env: HashMap::new(),
                        depends_on: vec![],
                    },
                    Step {
                        name: "second".to_string(),
                        command: "echo second".to_string(),
                        image: "".to_string(),
                        working_dir: "".to_string(),
                        env: HashMap::new(),
                        depends_on: vec![],
                    },
                ],
                parallel: false,
                depends_on: vec![],
            }],
            steps: vec![],
            cache: CacheConfig::default(),
            secrets: vec![],
        };

        let filter = FilterTarget::parse("test.unit").unwrap();
        apply_filter(&mut config, &filter).unwrap();

        assert_eq!(config.stages.len(), 1);
        assert_eq!(config.stages[0].name, "test.unit");
        assert_eq!(config.stages[0].steps.len(), 2);
    }

    #[test]
    fn test_apply_filter_nonexistent_stage() {
        let mut config = ForgeConfig {
            version: "1.0".to_string(),
            stages: vec![Stage {
                name: "build".to_string(),
                steps: vec![],
                parallel: false,
                depends_on: vec![],
            }],
            steps: vec![],
            cache: CacheConfig::default(),
            secrets: vec![],
        };

        let filter = FilterTarget::Stage("unknown".to_string());
        let err = apply_filter(&mut config, &filter).unwrap_err();
        assert!(err.to_string().contains("stage 'unknown' not found"));
        assert!(err.to_string().contains("Available stages: build"));
    }

    #[test]
    fn test_apply_filter_nonexistent_step() {
        let mut config = ForgeConfig {
            version: "1.0".to_string(),
            stages: vec![Stage {
                name: "test".to_string(),
                steps: vec![Step {
                    name: "unit".to_string(),
                    command: "echo test".to_string(),
                    image: "".to_string(),
                    working_dir: "".to_string(),
                    env: HashMap::new(),
                    depends_on: vec![],
                }],
                parallel: false,
                depends_on: vec![],
            }],
            steps: vec![],
            cache: CacheConfig::default(),
            secrets: vec![],
        };

        let filter = FilterTarget::Step {
            stage: "test".to_string(),
            step: "integration".to_string(),
        };
        let err = apply_filter(&mut config, &filter).unwrap_err();
        assert!(
            err.to_string()
                .contains("step 'integration' not found in stage 'test'")
        );
        assert!(err.to_string().contains("Available steps: unit"));
    }
}
