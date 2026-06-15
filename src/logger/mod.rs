use bollard::Docker;
use bollard::container::LogOutput;
use bollard::query_parameters::LogsOptions;
use colored::*;
use futures_util::stream::StreamExt;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::runner::monitor::PipelineMonitor;

pub type LogBuffer = Arc<Mutex<Vec<Option<(String, Vec<LogEntry>)>>>>;

#[derive(Clone, Debug)]
pub enum LogEntry {
    StdOut(String),
    StdErr(String),
    Error(String),
}

pub struct Timer {
    start: std::time::Instant,
    operation: String,
    verbose: bool,
}

impl Timer {
    pub fn new(operation: impl Into<String>, verbose: bool) -> Self {
        Self {
            start: std::time::Instant::now(),
            operation: operation.into(),
            verbose,
        }
    }

    pub fn elapsed(&self) -> std::time::Duration {
        self.start.elapsed()
    }

    pub fn log_if_verbose(&self) {
        if self.verbose {
            println!(
                "  {} completed in {:.2}s",
                self.operation,
                self.elapsed().as_secs_f64()
            );
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.log_if_verbose();
    }
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

    let mut log_stream = docker.logs(container_id, Some(log_options));

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

    let mut log_stream = docker.logs(container_id, Some(log_options));
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

pub async fn stream_logs_to_monitor(
    docker: &Docker,
    container_id: &str,
    step_name: &str,
    monitor: Arc<dyn PipelineMonitor>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let log_options = LogsOptions {
        follow: true,
        stdout: true,
        stderr: true,
        ..Default::default()
    };

    let mut log_stream = docker.logs(container_id, Some(log_options));

    while let Some(result) = log_stream.next().await {
        match result {
            Ok(output) => match output {
                LogOutput::StdOut { message } => {
                    let text = String::from_utf8_lossy(&message).to_string();
                    monitor.on_step_log(step_name, &text, false);
                }
                LogOutput::StdErr { message } => {
                    let text = String::from_utf8_lossy(&message).to_string();
                    monitor.on_step_log(step_name, &text, true);
                }
                _ => {}
            },
            Err(e) => {
                let err_msg = format!("Error streaming logs: {e}");
                monitor.on_step_log(step_name, &err_msg, true);
                break;
            }
        }
    }

    Ok(())
}
