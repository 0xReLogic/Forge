//! Output formatters for pipeline results (human summary, JSON, JUnit XML).
//! Each formatter writes to a `Write` impl so it can be tested without touching
//! stdout/stderr. Stdout/stderr routing is the caller's responsibility.

use crate::result::{ExecutionStatus, PipelineResult};
use std::io::{self, Write};

pub fn write_human_summary(result: &PipelineResult, out: &mut dyn Write) -> io::Result<()> {
    let separator = "─".repeat(44);
    writeln!(out, "\n{separator}")?;
    writeln!(out, "FORGE PIPELINE SUMMARY")?;
    writeln!(out, "{separator}")?;

    let name_width = result
        .stages
        .iter()
        .map(|s| s.name.len())
        .max()
        .unwrap_or(0)
        .max(5);

    for stage in &result.stages {
        let symbol = stage.status.symbol();
        let duration_str = match stage.duration_secs {
            Some(d) => format!("{d:.1}s"),
            None => "skipped".to_string(),
        };
        writeln!(
            out,
            "{:<width$}  {}  {}",
            stage.name,
            symbol,
            duration_str,
            width = name_width
        )?;
    }

    writeln!(out)?;

    let status_str = if result.is_success() {
        "SUCCESS"
    } else {
        "FAILED"
    };
    writeln!(
        out,
        "Total: {:.1}s   Status: {status_str}",
        result.duration_secs
    )?;

    if let Some(ref failure) = result.failure {
        writeln!(out, "Failed: {}", failure.short_description())?;
    }

    writeln!(out, "{separator}")?;

    Ok(())
}

pub fn write_json(result: &PipelineResult, out: &mut dyn Write) -> io::Result<()> {
    let json = serde_json::to_string_pretty(result)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    writeln!(out, "{json}")
}

pub fn write_junit(result: &PipelineResult, out: &mut dyn Write) -> io::Result<()> {
    let total_tests: usize = result.stages.iter().map(|s| s.steps.len()).sum();
    let total_failures: usize = result
        .stages
        .iter()
        .flat_map(|s| &s.steps)
        .filter(|s| s.status == ExecutionStatus::Failed)
        .count();
    let total_skipped: usize = result
        .stages
        .iter()
        .flat_map(|s| &s.steps)
        .filter(|s| s.status == ExecutionStatus::Skipped)
        .count();

    writeln!(out, r#"<?xml version="1.0" encoding="UTF-8"?>"#)?;
    writeln!(
        out,
        r#"<testsuites name="forge" tests="{total_tests}" failures="{total_failures}" skipped="{total_skipped}" time="{:.3}">"#,
        result.duration_secs
    )?;

    for stage in &result.stages {
        let stage_tests = stage.steps.len();
        let stage_failures = stage
            .steps
            .iter()
            .filter(|s| s.status == ExecutionStatus::Failed)
            .count();
        let stage_skipped = stage
            .steps
            .iter()
            .filter(|s| s.status == ExecutionStatus::Skipped)
            .count();
        let stage_time = stage.duration_secs.unwrap_or(0.0);

        writeln!(
            out,
            r#"  <testsuite name="{}" tests="{stage_tests}" failures="{stage_failures}" skipped="{stage_skipped}" time="{stage_time:.3}">"#,
            xml_escape(&stage.name)
        )?;

        for step in &stage.steps {
            let step_time = step.duration_secs.unwrap_or(0.0);
            match step.status {
                ExecutionStatus::Success => {
                    writeln!(
                        out,
                        r#"    <testcase name="{}" classname="{}" time="{step_time:.3}"/>"#,
                        xml_escape(&step.name),
                        xml_escape(&stage.name),
                    )?;
                }
                ExecutionStatus::Failed => {
                    let message = step
                        .error
                        .as_deref()
                        .unwrap_or("Step failed")
                        .lines()
                        .next()
                        .unwrap_or("Step failed");
                    writeln!(
                        out,
                        r#"    <testcase name="{}" classname="{}" time="{step_time:.3}">"#,
                        xml_escape(&step.name),
                        xml_escape(&stage.name),
                    )?;
                    writeln!(out, r#"      <failure message="{}"/>"#, xml_escape(message))?;
                    writeln!(out, r#"    </testcase>"#)?;
                }
                ExecutionStatus::Skipped
                | ExecutionStatus::Cancelled
                | ExecutionStatus::Timeout => {
                    writeln!(
                        out,
                        r#"    <testcase name="{}" classname="{}" time="{step_time:.3}">"#,
                        xml_escape(&step.name),
                        xml_escape(&stage.name),
                    )?;
                    writeln!(out, r#"      <skipped/>"#)?;
                    writeln!(out, r#"    </testcase>"#)?;
                }
            }
        }

        writeln!(out, r#"  </testsuite>"#)?;
    }

    writeln!(out, r#"</testsuites>"#)?;
    Ok(())
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::{ExecutionStatus, FailureReason, PipelineResult, StageResult, StepResult};
    use chrono::Utc;
    use std::time::Duration;

    fn success_result() -> PipelineResult {
        let now = Utc::now();
        PipelineResult {
            run_id: "test-run".into(),
            started_at: now,
            completed_at: now,
            duration_secs: 21.1,
            status: ExecutionStatus::Success,
            failure: None,
            stages: vec![
                StageResult::from_steps(
                    "build",
                    false,
                    Duration::from_millis(12400),
                    vec![StepResult::success("compile", Duration::from_millis(12400))],
                ),
                StageResult::from_steps(
                    "lint",
                    false,
                    Duration::from_millis(8700),
                    vec![StepResult::success("clippy", Duration::from_millis(8700))],
                ),
            ],
            forge_version: "v1.2.0".into(),
        }
    }

    fn failed_result() -> PipelineResult {
        let now = Utc::now();
        PipelineResult {
            run_id: "test-fail".into(),
            started_at: now,
            completed_at: now,
            duration_secs: 21.1,
            status: ExecutionStatus::Failed,
            failure: Some(FailureReason::StepFailed {
                stage: "test".into(),
                step: "cargo-test".into(),
                exit_code: 101,
            }),
            stages: vec![
                StageResult::from_steps(
                    "build",
                    false,
                    Duration::from_millis(12400),
                    vec![StepResult::success("compile", Duration::from_millis(12400))],
                ),
                StageResult::from_steps(
                    "test",
                    false,
                    Duration::from_millis(8700),
                    vec![StepResult::failed(
                        "cargo-test",
                        Duration::from_millis(8700),
                        Some(101),
                        "test suite failed",
                    )],
                ),
                StageResult::skipped("deploy", false),
            ],
            forge_version: "v1.2.0".into(),
        }
    }

    #[test]
    fn human_summary_success() {
        let mut buf = Vec::new();
        write_human_summary(&success_result(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("FORGE PIPELINE SUMMARY"));
        assert!(s.contains("build"));
        assert!(s.contains("✓"));
        assert!(s.contains("SUCCESS"));
        assert!(!s.contains("Failed:"));
    }

    #[test]
    fn human_summary_failure_shows_step() {
        let mut buf = Vec::new();
        write_human_summary(&failed_result(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("FAILED"));
        assert!(s.contains("test / cargo-test"));
        assert!(s.contains("101"));
    }

    #[test]
    fn human_summary_skipped_stage_shows_dash() {
        let mut buf = Vec::new();
        write_human_summary(&failed_result(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("deploy"));
        assert!(s.contains("-"));
    }

    #[test]
    fn json_is_valid_and_has_stable_fields() {
        let mut buf = Vec::new();
        write_json(&success_result(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
        assert_eq!(v["status"], "success");
        assert!(v.get("run_id").is_some());
        assert!(v.get("started_at").is_some());
        assert!(v.get("duration_secs").is_some());
        assert!(v.get("stages").is_some());
        assert!(v.get("forge_version").is_some());
    }

    #[test]
    fn json_failure_has_typed_failure_field() {
        let mut buf = Vec::new();
        write_json(&failed_result(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["status"], "failed");
        assert_eq!(v["failure"]["type"], "step_failed");
        assert_eq!(v["failure"]["exit_code"], 101);
    }

    #[test]
    fn junit_is_well_formed_xml() {
        let mut buf = Vec::new();
        write_junit(&success_result(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
        assert!(s.contains("<testsuites"));
        assert!(s.contains("</testsuites>"));
        assert!(s.contains("<testsuite"));
        assert!(s.contains("</testsuite>"));
    }

    #[test]
    fn junit_failed_step_has_failure_element() {
        let mut buf = Vec::new();
        write_junit(&failed_result(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("<failure"));
        assert!(s.contains("test suite failed"));
    }

    #[test]
    fn junit_skipped_step_has_skipped_element() {
        let now = Utc::now();
        let result = PipelineResult {
            run_id: "s".into(),
            started_at: now,
            completed_at: now,
            duration_secs: 1.0,
            status: ExecutionStatus::Success,
            failure: None,
            stages: vec![StageResult::from_steps(
                "test",
                false,
                Duration::from_secs(1),
                vec![
                    StepResult::success("unit", Duration::from_millis(500)),
                    StepResult::skipped("integration"),
                ],
            )],
            forge_version: "v1.2.0".into(),
        };
        let mut buf = Vec::new();
        write_junit(&result, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("<skipped/>"));
    }

    #[test]
    fn junit_xml_entities_are_escaped() {
        let now = Utc::now();
        let result = PipelineResult {
            run_id: "e".into(),
            started_at: now,
            completed_at: now,
            duration_secs: 1.0,
            status: ExecutionStatus::Failed,
            failure: None,
            stages: vec![StageResult::from_steps(
                "test & validate",
                false,
                Duration::from_secs(1),
                vec![StepResult::failed(
                    "check <output>",
                    Duration::from_millis(500),
                    Some(1),
                    r#"error: "quotes" & <tags>"#,
                )],
            )],
            forge_version: "v1.2.0".into(),
        };
        let mut buf = Vec::new();
        write_junit(&result, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("test &amp; validate"));
        assert!(s.contains("check &lt;output&gt;"));
    }

    #[test]
    fn xml_escape_all_entities() {
        assert_eq!(xml_escape("&"), "&amp;");
        assert_eq!(xml_escape("<foo>"), "&lt;foo&gt;");
        assert_eq!(xml_escape(r#"""#), "&quot;");
        assert_eq!(xml_escape("'"), "&apos;");
        assert_eq!(xml_escape("plain"), "plain");
    }
}
