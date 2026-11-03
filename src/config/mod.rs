use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

/// Configuration for artifacts upload and download in a step or stage.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct ArtifactConfig {
    #[serde(default)]
    pub upload: Vec<ArtifactUpload>,

    #[serde(default)]
    pub download: Vec<ArtifactDownload>,
}

/// Artifact upload specification.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum ArtifactUpload {
    /// Direct path string (artifact name will default to file/dir basename)
    Simple(String),
    /// Explicit path and custom artifact name
    Detailed {
        path: String,
        #[serde(default)]
        name: Option<String>,
    },
}

impl ArtifactUpload {
    pub fn path(&self) -> &str {
        match self {
            Self::Simple(p) => p.as_str(),
            Self::Detailed { path, .. } => path.as_str(),
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Simple(_) => None,
            Self::Detailed { name, .. } => name.as_deref(),
        }
    }
}

/// Artifact download specification.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum ArtifactDownload {
    /// Download by name (restored to original relative path or workspace root)
    Simple(String),
    /// Download by name to an explicit destination path
    Detailed {
        name: String,
        #[serde(default)]
        path: Option<String>,
    },
}

impl ArtifactDownload {
    pub fn name(&self) -> &str {
        match self {
            Self::Simple(n) => n.as_str(),
            Self::Detailed { name, .. } => name.as_str(),
        }
    }

    pub fn destination(&self) -> Option<&str> {
        match self {
            Self::Simple(_) => None,
            Self::Detailed { path, .. } => path.as_deref(),
        }
    }
}

/// Parses a duration string (e.g., "30s", "2m", "1h", "500ms", "45").
/// Plain numbers without units default to seconds.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("Duration string cannot be empty".to_string());
    }

    // Direct numeric string -> seconds
    if let Ok(secs) = s.parse::<u64>() {
        return Ok(Duration::from_secs(secs));
    }

    let mut total_millis: u64 = 0;
    let mut num_buf = String::new();
    let mut unit_buf = String::new();

    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            if !unit_buf.is_empty() {
                let delta = parse_unit_val(&num_buf, &unit_buf)?;
                total_millis = total_millis
                    .checked_add(delta)
                    .ok_or_else(|| "Duration overflow".to_string())?;
                num_buf.clear();
                unit_buf.clear();
            }
            num_buf.push(c);
            chars.next();
        } else if c.is_ascii_alphabetic() {
            if num_buf.is_empty() {
                return Err(format!(
                    "Invalid duration format '{s}': missing number before unit"
                ));
            }
            unit_buf.push(c.to_ascii_lowercase());
            chars.next();
        } else if c.is_whitespace() {
            chars.next();
        } else {
            return Err(format!("Invalid character '{c}' in duration '{s}'"));
        }
    }

    if !num_buf.is_empty() && !unit_buf.is_empty() {
        let delta = parse_unit_val(&num_buf, &unit_buf)?;
        total_millis = total_millis
            .checked_add(delta)
            .ok_or_else(|| "Duration overflow".to_string())?;
    } else if !num_buf.is_empty() && unit_buf.is_empty() {
        let secs = num_buf
            .parse::<u64>()
            .map_err(|e| format!("Invalid number in duration '{s}': {e}"))?;
        let delta = secs
            .checked_mul(1000)
            .ok_or_else(|| "Duration overflow".to_string())?;
        total_millis = total_millis
            .checked_add(delta)
            .ok_or_else(|| "Duration overflow".to_string())?;
    } else {
        return Err(format!("Incomplete duration string '{s}'"));
    }

    Ok(Duration::from_millis(total_millis))
}

fn parse_unit_val(num_str: &str, unit: &str) -> Result<u64, String> {
    let num: u64 = num_str
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;
    match unit {
        "ms" | "millis" | "milliseconds" => Ok(num),
        "s" | "sec" | "secs" | "second" | "seconds" => num
            .checked_mul(1000)
            .ok_or_else(|| "Duration overflow".to_string()),
        "m" | "min" | "mins" | "minute" | "minutes" => num
            .checked_mul(60 * 1000)
            .ok_or_else(|| "Duration overflow".to_string()),
        "h" | "hr" | "hrs" | "hour" | "hours" => num
            .checked_mul(3600 * 1000)
            .ok_or_else(|| "Duration overflow".to_string()),
        _ => Err(format!("Unknown duration unit '{unit}'")),
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
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

    /// Optional timeout for this step (e.g. "30s", "5m", 30)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,

    /// Number of retries on step failure
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,

    /// Delay between retry attempts (e.g. "5s", "10s", 5)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_delay: Option<String>,

    /// Whether pipeline execution should proceed even if this step fails
    #[serde(default)]
    pub continue_on_error: bool,

    /// Artifacts uploaded or downloaded by this step
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<ArtifactConfig>,
}

impl Step {
    /// Returns the parsed timeout Duration, if configured.
    pub fn timeout_duration(&self) -> Result<Option<Duration>, String> {
        self.timeout.as_deref().map(parse_duration).transpose()
    }

    /// Returns the configured retry count, defaulting to 0.
    pub fn retries_count(&self) -> u32 {
        self.retries.unwrap_or(0)
    }

    /// Returns the parsed retry delay Duration, defaulting to 0s.
    pub fn retry_delay_duration(&self) -> Result<Duration, String> {
        match &self.retry_delay {
            Some(d) => parse_duration(d),
            None => Ok(Duration::from_secs(0)),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct Stage {
    /// Stage name
    pub name: String,

    /// Steps in this stage
    pub steps: Vec<Step>,

    #[serde(default)]
    pub parallel: bool,

    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Whether pipeline execution should proceed even if this stage fails
    #[serde(default)]
    pub continue_on_error: bool,

    /// Artifacts uploaded or downloaded by this stage
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<ArtifactConfig>,
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

    #[serde(default = "default_fail_fast")]
    pub fail_fast: bool,

    #[serde(default)]
    pub stages: Vec<Stage>,

    #[serde(default)]
    pub steps: Vec<Step>,

    #[serde(default)]
    pub cache: CacheConfig,

    #[serde(default)]
    pub secrets: Vec<Secret>,
}

pub fn default_fail_fast() -> bool {
    true
}

pub fn default_version() -> String {
    "1.0".to_string()
}

impl Default for ForgeConfig {
    fn default() -> Self {
        Self {
            version: default_version(),
            fail_fast: default_fail_fast(),
            stages: Vec::new(),
            steps: Vec::new(),
            cache: CacheConfig::default(),
            secrets: Vec::new(),
        }
    }
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

pub fn validate_execution_control(
    config: &ForgeConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let check_artifacts = |art: &ArtifactConfig,
                           context: &str|
     -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for upload in &art.upload {
            let path = upload.path().trim();
            if path.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Artifact upload in {context} has an empty path"),
                )));
            }
            if path.contains("..") {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Artifact upload path '{path}' in {context} cannot contain '..'"),
                )));
            }
        }
        for download in &art.download {
            let name = download.name().trim();
            if name.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Artifact download in {context} has an empty name"),
                )));
            }
            if let Some(dest) = download.destination()
                && dest.contains("..")
            {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "Artifact download destination '{dest}' in {context} cannot contain '..'"
                    ),
                )));
            }
        }
        Ok(())
    };

    let check_step =
        |step: &Step, stage_name: &str| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            if let Some(ref t) = step.timeout {
                step.timeout_duration().map_err(|e| {
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "Invalid timeout '{t}' in step '{}' (stage '{}'): {e}",
                            step.name, stage_name
                        ),
                    ))
                })?;
            }
            if let Some(ref d) = step.retry_delay {
                step.retry_delay_duration().map_err(|e| {
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "Invalid retry_delay '{d}' in step '{}' (stage '{}'): {e}",
                            step.name, stage_name
                        ),
                    ))
                })?;
            }
            if let Some(ref art) = step.artifacts {
                check_artifacts(
                    art,
                    &format!("step '{}' in stage '{}'", step.name, stage_name),
                )?;
            }
            Ok(())
        };

    for stage in &config.stages {
        if let Some(ref art) = stage.artifacts {
            check_artifacts(art, &format!("stage '{}'", stage.name))?;
        }
        for step in &stage.steps {
            check_step(step, &stage.name)?;
        }
    }

    for step in &config.steps {
        check_step(step, "root")?;
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
            stages: vec![
                Stage {
                    name: "setup".to_string(),
                    steps: vec![Step {
                        name: "init".to_string(),
                        command: "echo init".to_string(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                Stage {
                    name: "build".to_string(),
                    steps: vec![Step {
                        name: "compile".to_string(),
                        command: "echo compile".to_string(),
                        ..Default::default()
                    }],
                    depends_on: vec!["setup".to_string()],
                    ..Default::default()
                },
                Stage {
                    name: "deploy".to_string(),
                    steps: vec![Step {
                        name: "push".to_string(),
                        command: "echo push".to_string(),
                        ..Default::default()
                    }],
                    depends_on: vec!["build".to_string()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let filter = FilterTarget::Stage("build".to_string());
        apply_filter(&mut config, &filter).unwrap();

        let stage_names: Vec<String> = config.stages.iter().map(|s| s.name.clone()).collect();
        assert_eq!(stage_names, vec!["setup", "build"]);
    }

    #[test]
    fn test_apply_filter_step() {
        let mut config = ForgeConfig {
            stages: vec![Stage {
                name: "test".to_string(),
                steps: vec![
                    Step {
                        name: "unit".to_string(),
                        command: "cargo test --lib".to_string(),
                        ..Default::default()
                    },
                    Step {
                        name: "integration".to_string(),
                        command: "cargo test --test it".to_string(),
                        ..Default::default()
                    },
                ],
                parallel: true,
                ..Default::default()
            }],
            ..Default::default()
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
            stages: vec![Stage {
                name: "test.unit".to_string(),
                steps: vec![
                    Step {
                        name: "first".to_string(),
                        command: "echo first".to_string(),
                        ..Default::default()
                    },
                    Step {
                        name: "second".to_string(),
                        command: "echo second".to_string(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
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
            stages: vec![Stage {
                name: "build".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let filter = FilterTarget::Stage("unknown".to_string());
        let err = apply_filter(&mut config, &filter).unwrap_err();
        assert!(err.to_string().contains("stage 'unknown' not found"));
        assert!(err.to_string().contains("Available stages: build"));
    }

    #[test]
    fn test_apply_filter_nonexistent_step() {
        let mut config = ForgeConfig {
            stages: vec![Stage {
                name: "test".to_string(),
                steps: vec![Step {
                    name: "unit".to_string(),
                    command: "echo test".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
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

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("1m30s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("45").unwrap(), Duration::from_secs(45));
        assert!(parse_duration("").is_err());
        assert!(parse_duration("invalid").is_err());
        assert!(parse_duration("30x").is_err());
    }

    #[test]
    fn test_execution_control_yaml_deserialization() {
        let yaml = r#"
version: "1.0"
fail_fast: false
stages:
  - name: build
    continue_on_error: true
    artifacts:
      upload:
        - name: dist
          path: target/release/app
        - "docs/"
      download:
        - name: vendor
          path: vendor/
    steps:
      - name: compile
        command: cargo build
        timeout: 45s
        retries: 2
        retry_delay: 3s
        continue_on_error: true
"#;
        let config: ForgeConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!config.fail_fast);
        assert_eq!(config.stages.len(), 1);
        let stage = &config.stages[0];
        assert!(stage.continue_on_error);
        assert!(stage.artifacts.is_some());
        let art = stage.artifacts.as_ref().unwrap();
        assert_eq!(art.upload.len(), 2);
        assert_eq!(art.download.len(), 1);

        let step = &stage.steps[0];
        assert_eq!(step.timeout.as_deref(), Some("45s"));
        assert_eq!(
            step.timeout_duration().unwrap(),
            Some(Duration::from_secs(45))
        );
        assert_eq!(step.retries_count(), 2);
        assert_eq!(step.retry_delay_duration().unwrap(), Duration::from_secs(3));
        assert!(step.continue_on_error);

        assert!(validate_execution_control(&config).is_ok());
    }

    #[test]
    fn test_validate_execution_control_catches_invalid_timeout() {
        let config = ForgeConfig {
            version: "1.0".to_string(),
            fail_fast: true,
            stages: vec![Stage {
                name: "test".to_string(),
                steps: vec![Step {
                    name: "unit".to_string(),
                    command: "cargo test".to_string(),
                    image: "".to_string(),
                    working_dir: "".to_string(),
                    env: HashMap::new(),
                    depends_on: vec![],
                    timeout: Some("bad-timeout".to_string()),
                    retries: None,
                    retry_delay: None,
                    continue_on_error: false,
                    artifacts: None,
                }],
                parallel: false,
                depends_on: vec![],
                continue_on_error: false,
                artifacts: None,
            }],
            steps: vec![],
            cache: CacheConfig::default(),
            secrets: vec![],
        };
        assert!(validate_execution_control(&config).is_err());
    }
}
