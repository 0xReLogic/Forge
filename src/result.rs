//! Structured execution result types for FORGE v1.2.
//!
//! These types form the stable contract between the pipeline executor and all
//! consumers: human output, JSON/JUnit formatters, run persistence, and future
//! features such as `--filter`, retry, and `forge replay`.
//!
//! Design principles:
//! - All status values are typed enums — no free-form strings for status fields.
//! - `serde` derives are included so the same types can be serialized to JSON
//!   and deserialized for replay without a separate DTO layer.
//! - Duration is stored as `f64` seconds for human readability in JSON.
//! - Secret values must never appear in these types; callers are responsible.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Exit codes
// ---------------------------------------------------------------------------

/// Canonical exit codes emitted by the FORGE CLI.
///
/// These are the only exit codes the process will produce. Adding a new code
/// requires updating `ExitCode::from_failure_reason` and the docs below.
///
/// ```text
/// 0 = success
/// 1 = pipeline execution failure (a step or stage returned non-zero)
/// 2 = config / validation error (bad YAML, missing field, circular dep)
/// 3 = Docker / runtime error (daemon unreachable, image pull failed)
/// 4 = cancelled (user interrupted with Ctrl-C or TUI quit)
/// 5 = timeout (step exceeded timeout — reserved for v1.4)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitCode {
    Success = 0,
    PipelineFailure = 1,
    ConfigError = 2,
    RuntimeError = 3,
    Cancelled = 4,
    Timeout = 5,
}

impl ExitCode {
    /// Returns the numeric code suitable for `process::exit`.
    pub fn as_i32(self) -> i32 {
        self as i32
    }

    /// Derive the exit code from a `FailureReason`.
    pub fn from_failure_reason(reason: &FailureReason) -> Self {
        match reason {
            FailureReason::StepFailed { .. } | FailureReason::StepError { .. } => {
                Self::PipelineFailure
            }
            FailureReason::ConfigError { .. } => Self::ConfigError,
            FailureReason::DockerError { .. } | FailureReason::RuntimeError { .. } => {
                Self::RuntimeError
            }
            FailureReason::Cancelled => Self::Cancelled,
            FailureReason::Timeout { .. } => Self::Timeout,
        }
    }
}

// ---------------------------------------------------------------------------
// Execution status
// ---------------------------------------------------------------------------

/// Execution status of a pipeline, stage, or step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    /// Completed without error.
    Success,
    /// Completed with an error or non-zero exit code.
    Failed,
    /// Was not run because an earlier dependency failed.
    Skipped,
    /// Was aborted by user or by a sibling-failure abort signal.
    Cancelled,
    /// Exceeded the configured timeout (reserved for v1.4).
    Timeout,
}

impl ExecutionStatus {
    /// Returns `true` if the status represents a successful outcome.
    pub fn is_success(&self) -> bool {
        *self == Self::Success
    }

    /// Returns a short display symbol for the human summary.
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Success => "✓",
            Self::Failed => "✗",
            Self::Skipped => "-",
            Self::Cancelled => "⊘",
            Self::Timeout => "⏱",
        }
    }
}

// ---------------------------------------------------------------------------
// Failure reason
// ---------------------------------------------------------------------------

/// Structured reason for a pipeline or step failure.
///
/// This enum drives both exit-code classification and the failure description
/// in the pipeline summary and JSON output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FailureReason {
    /// A step's command returned a non-zero exit code.
    StepFailed {
        stage: String,
        step: String,
        exit_code: i32,
    },
    /// A step encountered an unexpected execution error (Docker API, I/O, etc.)
    /// that is not simply a non-zero command exit code.
    StepError {
        stage: String,
        step: String,
        message: String,
    },
    /// The pipeline configuration is invalid (YAML parse error, missing field,
    /// circular dependency, etc.).
    ConfigError { message: String },
    /// Docker is unavailable or a Docker API call failed.
    DockerError { message: String },
    /// A generic runtime error that doesn't fit the categories above.
    RuntimeError { message: String },
    /// The pipeline was cancelled by the user.
    Cancelled,
    /// A step exceeded its timeout (reserved for v1.4).
    Timeout { stage: String, step: String },
}

impl FailureReason {
    /// Returns a short human-readable description used in the summary line.
    pub fn short_description(&self) -> String {
        match self {
            Self::StepFailed {
                stage,
                step,
                exit_code,
            } => format!("{stage} / {step} (exit {exit_code})"),
            Self::StepError { stage, step, .. } => format!("{stage} / {step}"),
            Self::ConfigError { message } => format!("config error: {message}"),
            Self::DockerError { message } => format!("docker error: {message}"),
            Self::RuntimeError { message } => format!("runtime error: {message}"),
            Self::Cancelled => "cancelled by user".to_string(),
            Self::Timeout { stage, step } => format!("{stage} / {step} (timeout)"),
        }
    }

    /// Parse a Docker exit code from the error message produced by
    /// `wait_for_container`, which embeds the exit code in its message string.
    /// Returns `None` if the message does not contain a recognizable code.
    pub fn parse_exit_code_from_message(msg: &str) -> Option<i32> {
        // The message format is: "Step '...' failed with exit code N\n..."
        let prefix = "exit code ";
        let idx = msg.find(prefix)?;
        let rest = &msg[idx + prefix.len()..];
        let code_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        code_str.parse().ok()
    }
}

// ---------------------------------------------------------------------------
// Step result
// ---------------------------------------------------------------------------

/// Execution result for a single pipeline step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// Step name as defined in forge.yaml.
    pub name: String,
    /// Final status of this step.
    pub status: ExecutionStatus,
    /// Wall-clock duration in seconds (`None` if the step never started).
    pub duration_secs: Option<f64>,
    /// Docker container exit code (`None` if not applicable or not available).
    pub exit_code: Option<i32>,
    /// Error or failure message (`None` on success).
    pub error: Option<String>,
}

impl StepResult {
    pub fn success(name: impl Into<String>, duration: Duration) -> Self {
        Self {
            name: name.into(),
            status: ExecutionStatus::Success,
            duration_secs: Some(duration.as_secs_f64()),
            exit_code: Some(0),
            error: None,
        }
    }

    pub fn failed(
        name: impl Into<String>,
        duration: Duration,
        exit_code: Option<i32>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: ExecutionStatus::Failed,
            duration_secs: Some(duration.as_secs_f64()),
            exit_code,
            error: Some(error.into()),
        }
    }

    pub fn skipped(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: ExecutionStatus::Skipped,
            duration_secs: None,
            exit_code: None,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Stage result
// ---------------------------------------------------------------------------

/// Execution result for a pipeline stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageResult {
    /// Stage name as defined in forge.yaml.
    pub name: String,
    /// Final status of this stage.
    pub status: ExecutionStatus,
    /// Wall-clock duration in seconds (`None` if the stage never started).
    pub duration_secs: Option<f64>,
    /// Whether this stage was configured as `parallel: true`.
    pub parallel: bool,
    /// Results for each step within this stage.
    pub steps: Vec<StepResult>,
}

impl StageResult {
    /// Creates a new `StageResult` whose status is derived from its steps.
    /// The stage is `Failed` if any step is failed; `Skipped` if never started.
    pub fn from_steps(
        name: impl Into<String>,
        parallel: bool,
        duration: Duration,
        steps: Vec<StepResult>,
    ) -> Self {
        let status = if steps.iter().any(|s| s.status == ExecutionStatus::Failed) {
            ExecutionStatus::Failed
        } else if steps.iter().all(|s| s.status == ExecutionStatus::Skipped) {
            ExecutionStatus::Skipped
        } else {
            ExecutionStatus::Success
        };

        Self {
            name: name.into(),
            status,
            duration_secs: Some(duration.as_secs_f64()),
            parallel,
            steps,
        }
    }

    pub fn skipped(name: impl Into<String>, parallel: bool) -> Self {
        Self {
            name: name.into(),
            status: ExecutionStatus::Skipped,
            duration_secs: None,
            parallel,
            steps: vec![],
        }
    }
}

// ---------------------------------------------------------------------------
// Pipeline result
// ---------------------------------------------------------------------------

/// Complete execution result for a pipeline run.
///
/// This is the root type persisted to `.forge/runs/<run-id>/result.json`
/// and emitted on stdout when `--format json` is used.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineResult {
    /// Unique run identifier (UUIDv4).
    pub run_id: String,
    /// RFC 3339 timestamp of when the pipeline started.
    pub started_at: DateTime<Utc>,
    /// RFC 3339 timestamp of when the pipeline completed.
    pub completed_at: DateTime<Utc>,
    /// Total wall-clock duration in seconds.
    pub duration_secs: f64,
    /// Overall pipeline status.
    pub status: ExecutionStatus,
    /// Structured failure reason (`None` on success).
    pub failure: Option<FailureReason>,
    /// Results for each stage that was scheduled to run.
    pub stages: Vec<StageResult>,
    /// Forge version that produced this result.
    pub forge_version: String,
}

impl PipelineResult {
    /// Returns the appropriate `ExitCode` for `process::exit`.
    pub fn exit_code(&self) -> ExitCode {
        match &self.failure {
            None => ExitCode::Success,
            Some(reason) => ExitCode::from_failure_reason(reason),
        }
    }

    /// Returns `true` if the overall pipeline succeeded.
    pub fn is_success(&self) -> bool {
        self.status.is_success()
    }

    /// Returns the first failed step across all stages, if any.
    pub fn first_failure(&self) -> Option<(&StageResult, &StepResult)> {
        for stage in &self.stages {
            for step in &stage.steps {
                if step.status == ExecutionStatus::Failed {
                    return Some((stage, step));
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Output format
// ---------------------------------------------------------------------------

/// Output format requested via `--format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Default colored human-readable output with pipeline summary (default).
    Human,
    /// Machine-readable JSON on stdout; diagnostics on stderr.
    Json,
    /// JUnit XML suitable for CI test reporting.
    Junit,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "human" => Ok(Self::Human),
            "json" => Ok(Self::Json),
            "junit" => Ok(Self::Junit),
            other => Err(format!(
                "unknown format '{other}'; valid values: human, json, junit"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // -- ExitCode --

    #[test]
    fn exit_code_values_are_stable() {
        assert_eq!(ExitCode::Success.as_i32(), 0);
        assert_eq!(ExitCode::PipelineFailure.as_i32(), 1);
        assert_eq!(ExitCode::ConfigError.as_i32(), 2);
        assert_eq!(ExitCode::RuntimeError.as_i32(), 3);
        assert_eq!(ExitCode::Cancelled.as_i32(), 4);
        assert_eq!(ExitCode::Timeout.as_i32(), 5);
    }

    #[test]
    fn exit_code_from_failure_reason_maps_correctly() {
        assert_eq!(
            ExitCode::from_failure_reason(&FailureReason::StepFailed {
                stage: "test".into(),
                step: "cargo-test".into(),
                exit_code: 101,
            }),
            ExitCode::PipelineFailure
        );
        assert_eq!(
            ExitCode::from_failure_reason(&FailureReason::ConfigError {
                message: "bad yaml".into()
            }),
            ExitCode::ConfigError
        );
        assert_eq!(
            ExitCode::from_failure_reason(&FailureReason::DockerError {
                message: "daemon down".into()
            }),
            ExitCode::RuntimeError
        );
        assert_eq!(
            ExitCode::from_failure_reason(&FailureReason::Cancelled),
            ExitCode::Cancelled
        );
        assert_eq!(
            ExitCode::from_failure_reason(&FailureReason::Timeout {
                stage: "test".into(),
                step: "cargo-test".into()
            }),
            ExitCode::Timeout
        );
    }

    // -- ExecutionStatus --

    #[test]
    fn execution_status_symbols() {
        assert_eq!(ExecutionStatus::Success.symbol(), "✓");
        assert_eq!(ExecutionStatus::Failed.symbol(), "✗");
        assert_eq!(ExecutionStatus::Skipped.symbol(), "-");
        assert_eq!(ExecutionStatus::Cancelled.symbol(), "⊘");
        assert_eq!(ExecutionStatus::Timeout.symbol(), "⏱");
    }

    #[test]
    fn execution_status_is_success() {
        assert!(ExecutionStatus::Success.is_success());
        assert!(!ExecutionStatus::Failed.is_success());
        assert!(!ExecutionStatus::Skipped.is_success());
    }

    // -- FailureReason --

    #[test]
    fn parse_exit_code_from_message() {
        let msg = "Step 'cargo-test' failed with exit code 101\nHint: Check command output";
        assert_eq!(FailureReason::parse_exit_code_from_message(msg), Some(101));

        let msg2 = "Step 'build' failed with exit code 2";
        assert_eq!(FailureReason::parse_exit_code_from_message(msg2), Some(2));

        let msg3 = "Docker daemon unreachable";
        assert_eq!(FailureReason::parse_exit_code_from_message(msg3), None);
    }

    #[test]
    fn failure_reason_short_description() {
        let r = FailureReason::StepFailed {
            stage: "test".into(),
            step: "cargo-test".into(),
            exit_code: 101,
        };
        assert_eq!(r.short_description(), "test / cargo-test (exit 101)");

        let r2 = FailureReason::Cancelled;
        assert_eq!(r2.short_description(), "cancelled by user");
    }

    // -- StepResult --

    #[test]
    fn step_result_success() {
        let r = StepResult::success("build", Duration::from_millis(1234));
        assert_eq!(r.status, ExecutionStatus::Success);
        assert_eq!(r.exit_code, Some(0));
        assert!(r.error.is_none());
        let d = r.duration_secs.unwrap();
        assert!((d - 1.234).abs() < 0.001);
    }

    #[test]
    fn step_result_failed() {
        let r = StepResult::failed("test", Duration::from_secs(5), Some(101), "tests failed");
        assert_eq!(r.status, ExecutionStatus::Failed);
        assert_eq!(r.exit_code, Some(101));
        assert_eq!(r.error.as_deref(), Some("tests failed"));
    }

    #[test]
    fn step_result_skipped() {
        let r = StepResult::skipped("lint");
        assert_eq!(r.status, ExecutionStatus::Skipped);
        assert!(r.duration_secs.is_none());
        assert!(r.exit_code.is_none());
    }

    // -- StageResult --

    #[test]
    fn stage_result_derives_failed_from_steps() {
        let steps = vec![
            StepResult::success("build", Duration::from_secs(1)),
            StepResult::failed("test", Duration::from_secs(2), Some(1), "oops"),
        ];
        let stage = StageResult::from_steps("ci", false, Duration::from_secs(3), steps);
        assert_eq!(stage.status, ExecutionStatus::Failed);
    }

    #[test]
    fn stage_result_derives_success_from_steps() {
        let steps = vec![
            StepResult::success("build", Duration::from_secs(1)),
            StepResult::success("test", Duration::from_secs(2)),
        ];
        let stage = StageResult::from_steps("ci", false, Duration::from_secs(3), steps);
        assert_eq!(stage.status, ExecutionStatus::Success);
    }

    #[test]
    fn stage_result_skipped() {
        let s = StageResult::skipped("deploy", false);
        assert_eq!(s.status, ExecutionStatus::Skipped);
        assert!(s.steps.is_empty());
    }

    // -- OutputFormat --

    #[test]
    fn output_format_parse() {
        use std::str::FromStr;
        assert_eq!(
            OutputFormat::from_str("human").unwrap(),
            OutputFormat::Human
        );
        assert_eq!(OutputFormat::from_str("json").unwrap(), OutputFormat::Json);
        assert_eq!(
            OutputFormat::from_str("junit").unwrap(),
            OutputFormat::Junit
        );
        assert_eq!(
            OutputFormat::from_str("HUMAN").unwrap(),
            OutputFormat::Human
        );
        assert!(OutputFormat::from_str("xml").is_err());
    }

    // -- Serialization round-trip --

    #[test]
    fn pipeline_result_json_round_trip() {
        let now = Utc::now();
        let pr = PipelineResult {
            run_id: "test-run-id".into(),
            started_at: now,
            completed_at: now,
            duration_secs: 5.0,
            status: ExecutionStatus::Success,
            failure: None,
            stages: vec![StageResult::from_steps(
                "build",
                false,
                Duration::from_secs(5),
                vec![StepResult::success("compile", Duration::from_secs(5))],
            )],
            forge_version: "v1.2.0".into(),
        };

        let json = serde_json::to_string(&pr).expect("serialization failed");
        let restored: PipelineResult = serde_json::from_str(&json).expect("deserialization failed");
        assert_eq!(restored.run_id, "test-run-id");
        assert_eq!(restored.status, ExecutionStatus::Success);
        assert_eq!(restored.stages.len(), 1);
        assert_eq!(restored.stages[0].steps.len(), 1);
    }

    #[test]
    fn failure_reason_json_has_type_tag() {
        let r = FailureReason::StepFailed {
            stage: "test".into(),
            step: "cargo-test".into(),
            exit_code: 101,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"type\":\"step_failed\""));
        assert!(json.contains("\"exit_code\":101"));
    }
}
