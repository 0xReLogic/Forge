use colored::*;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use crate::config::Stage;

pub trait PipelineMonitor: Send + Sync {
    fn on_pipeline_start(&self, stages: &[Stage]);
    fn on_stage_start(&self, stage_name: &str, parallel: bool);
    fn on_stage_complete(&self, stage_name: &str, success: bool);
    fn on_step_start(&self, step_name: &str, image: &str);
    fn on_step_log(&self, step_name: &str, log_line: &str, is_stderr: bool);
    fn on_step_complete(&self, step_name: &str, success: bool);
    fn on_pipeline_complete(&self, success: bool, duration: Duration);
    fn on_container_created(&self, _container_id: &str) {}
    fn on_container_destroyed(&self, _container_id: &str) {}
}

pub struct StdoutMonitor {
    stages_order: Mutex<Vec<Stage>>,
    current_stage_parallel: Mutex<bool>,
    // Store parallel logs as (step_name -> list of (log_text, is_stderr))
    parallel_logs: Mutex<HashMap<String, Vec<(String, bool)>>>,
}

impl StdoutMonitor {
    pub fn new() -> Self {
        Self {
            stages_order: Mutex::new(Vec::new()),
            current_stage_parallel: Mutex::new(false),
            parallel_logs: Mutex::new(HashMap::new()),
        }
    }
}

impl PipelineMonitor for StdoutMonitor {
    fn on_pipeline_start(&self, stages: &[Stage]) {
        let mut order = self.stages_order.lock().unwrap();
        *order = stages.to_vec();
    }

    fn on_stage_start(&self, stage_name: &str, parallel: bool) {
        let mut curr_parallel = self.current_stage_parallel.lock().unwrap();
        *curr_parallel = parallel;

        let stages = self.stages_order.lock().unwrap();
        let idx = stages
            .iter()
            .position(|s| s.name == stage_name)
            .unwrap_or(0);
        let total = stages.len();

        let parallel_tag = if parallel { " [parallel]" } else { "" };
        println!(
            "\n{} Stage {}/{}: {}{}",
            ">>>".cyan().bold(),
            idx + 1,
            total,
            stage_name.cyan().bold(),
            parallel_tag.yellow()
        );
    }

    fn on_stage_complete(&self, stage_name: &str, _success: bool) {
        let is_parallel = *self.current_stage_parallel.lock().unwrap();
        if is_parallel {
            // Print parallel logs in definition order
            let stages = self.stages_order.lock().unwrap();
            let stage = stages.iter().find(|s| s.name == stage_name);
            let mut logs_map = self.parallel_logs.lock().unwrap();

            if let Some(s) = stage {
                for step in &s.steps {
                    if let Some(entries) = logs_map.remove(&step.name) {
                        println!(
                            "\n{}",
                            format!("=== Logs for step: {} ===", step.name)
                                .cyan()
                                .bold()
                        );
                        for (msg, is_stderr) in entries {
                            if is_stderr {
                                eprint!("{}", msg.red());
                            } else {
                                print!("{}", msg);
                            }
                        }
                    }
                }
            }
        }
    }

    fn on_step_start(&self, step_name: &str, _image: &str) {
        let is_parallel = *self.current_stage_parallel.lock().unwrap();
        if !is_parallel {
            println!("{}", format!("Running step: {}", step_name).yellow().bold());
        }
    }

    fn on_step_log(&self, step_name: &str, log_line: &str, is_stderr: bool) {
        let is_parallel = *self.current_stage_parallel.lock().unwrap();
        if is_parallel {
            let mut logs_map = self.parallel_logs.lock().unwrap();
            logs_map
                .entry(step_name.to_string())
                .or_default()
                .push((log_line.to_string(), is_stderr));
        } else {
            if is_stderr {
                eprint!("{}", log_line.red());
            } else {
                print!("{}", log_line);
            }
        }
    }

    fn on_step_complete(&self, step_name: &str, success: bool) {
        let is_parallel = *self.current_stage_parallel.lock().unwrap();
        if !is_parallel {
            if success {
                println!("{} Step: {}", "[OK]".green(), step_name);
            } else {
                println!("{} Step: {}", "[FAIL]".red().bold(), step_name);
            }
        }
    }

    fn on_pipeline_complete(&self, success: bool, duration: Duration) {
        if success {
            println!(
                "\n{} {}",
                "[OK]".green().bold(),
                "Pipeline completed successfully!".green().bold()
            );
        } else {
            println!(
                "\n{} {}",
                "[FAIL]".red().bold(),
                "Pipeline execution failed.".red().bold()
            );
        }
        println!("    Total duration: {:.2}s", duration.as_secs_f64());
    }
}
