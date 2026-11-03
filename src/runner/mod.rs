pub mod monitor;

use bollard::Docker;
use futures_util::future::select_all;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinHandle;

use crate::artifacts::ArtifactManager;
use crate::config::{CacheConfig, Stage, Step};
use crate::docker::{
    ContainerRuntimeContext, cleanup_container, cleanup_containers, create_and_start_container,
    prepare_container, wait_for_container,
};
use crate::logger::stream_logs_to_monitor;
use crate::result::FailureReason;
use monitor::PipelineMonitor;

/// Error emitted when a step exceeds its configured timeout duration.
#[derive(Debug, Clone)]
pub struct TimeoutError {
    pub step_name: String,
    pub duration: Duration,
}

impl std::fmt::Display for TimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Step '{}' timed out after {:.1}s",
            self.step_name,
            self.duration.as_secs_f64()
        )
    }
}

impl std::error::Error for TimeoutError {}

/// Global tracker for all active Docker container IDs in the pipeline.
/// Enables reliable graceful cleanup upon Ctrl+C interrupt or timeouts.
#[derive(Clone, Default)]
pub struct ContainerTracker {
    pub active_containers: Arc<Mutex<HashSet<String>>>,
}

impl ContainerTracker {
    pub fn new() -> Self {
        Self {
            active_containers: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub async fn track(&self, container_id: String) {
        let mut set = self.active_containers.lock().await;
        set.insert(container_id);
    }

    pub async fn untrack(&self, container_id: &str) {
        let mut set = self.active_containers.lock().await;
        set.remove(container_id);
    }

    pub async fn cleanup_all(&self, docker: &Docker, verbose: bool) {
        let ids: Vec<String> = {
            let mut set = self.active_containers.lock().await;
            set.drain().collect()
        };

        for id in ids {
            if verbose {
                eprintln!("Stopping container on interrupt: {id}");
            }
            let _ = docker.stop_container(&id, None).await;
            let _ = docker.remove_container(&id, None).await;
        }
    }
}

#[derive(Clone)]
pub struct PipelineRuntimeContext {
    pub workspace_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub secrets_env: Arc<HashMap<String, String>>,
    pub container_tracker: ContainerTracker,
    pub artifact_manager: Option<Arc<ArtifactManager>>,
}

/// Executes a single container attempt for a step, enforcing timeouts and container tracking.
async fn run_single_step_container(
    docker: &Docker,
    step: &Step,
    verbose: bool,
    cache_config: &CacheConfig,
    temp_dir: &Path,
    runtime: &PipelineRuntimeContext,
    monitor: Arc<dyn PipelineMonitor>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let container_ctx = ContainerRuntimeContext {
        workspace_dir: &runtime.workspace_dir,
        cache_dir: &runtime.cache_dir,
        secrets_env: runtime.secrets_env.as_ref(),
    };
    let setup = prepare_container(
        docker,
        step,
        cache_config,
        temp_dir,
        &container_ctx,
        verbose,
    )
    .await?;

    monitor.on_step_start(&setup.step_name, &setup.image);

    let container_id = create_and_start_container(docker, &setup).await?;
    runtime.container_tracker.track(container_id.clone()).await;
    monitor.on_container_created(&container_id);

    let timeout_opt = step.timeout_duration().map_err(|e| {
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
            as Box<dyn std::error::Error + Send + Sync>
    })?;

    let log_handle = tokio::spawn({
        let docker = docker.clone();
        let container_id = container_id.clone();
        let step_name = setup.step_name.clone();
        let monitor = Arc::clone(&monitor);
        async move { stream_logs_to_monitor(&docker, &container_id, &step_name, monitor).await }
    });

    let wait_future = wait_for_container(docker, &container_id, &setup.step_name);

    let wait_result = if let Some(dur) = timeout_opt {
        match tokio::time::timeout(dur, wait_future).await {
            Ok(res) => res,
            Err(_) => {
                let _ = log_handle.await;
                cleanup_container(docker, &container_id, verbose).await;
                runtime.container_tracker.untrack(&container_id).await;
                monitor.on_container_destroyed(&container_id);
                monitor.on_step_complete(&setup.step_name, false);
                return Err(Box::new(TimeoutError {
                    step_name: setup.step_name.clone(),
                    duration: dur,
                }));
            }
        }
    } else {
        wait_future.await
    };

    let _ = log_handle.await;
    cleanup_container(docker, &container_id, verbose).await;
    runtime.container_tracker.untrack(&container_id).await;
    monitor.on_container_destroyed(&container_id);

    let success = wait_result.is_ok();
    monitor.on_step_complete(&setup.step_name, success);

    wait_result.map(|_| ())
}

/// Runs a command in a Docker container with timeout and retry policies.
/// Returns `(result, attempts_count)`.
pub async fn run_command_in_container(
    docker: &Docker,
    step: &Step,
    verbose: bool,
    cache_config: &CacheConfig,
    temp_dir: &Path,
    runtime: &PipelineRuntimeContext,
    monitor: Arc<dyn PipelineMonitor>,
) -> (Result<(), Box<dyn std::error::Error + Send + Sync>>, u32) {
    let retries = step.retries_count();
    let retry_delay = match step.retry_delay_duration() {
        Ok(d) => d,
        Err(e) => {
            return (
                Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    e,
                ))),
                0,
            );
        }
    };

    // Pre-step artifact downloads if configured
    if let Some(ref art_config) = step.artifacts
        && let Some(ref mgr) = runtime.artifact_manager
    {
        for dl in &art_config.download {
            if let Err(e) = mgr.download(dl) {
                return (
                    Err(Box::new(std::io::Error::other(format!(
                        "Failed to download step artifact '{}': {e}",
                        dl.name()
                    )))),
                    0,
                );
            }
        }
    }

    let mut attempt = 0;
    loop {
        attempt += 1;
        let res = run_single_step_container(
            docker,
            step,
            verbose,
            cache_config,
            temp_dir,
            runtime,
            Arc::clone(&monitor),
        )
        .await;

        match res {
            Ok(()) => {
                // Post-step artifact uploads if configured
                if let Some(ref art_config) = step.artifacts
                    && let Some(ref mgr) = runtime.artifact_manager
                {
                    for up in &art_config.upload {
                        if let Err(e) = mgr.upload(up) {
                            return (
                                Err(Box::new(std::io::Error::other(format!(
                                    "Failed to upload step artifact '{}': {e}",
                                    up.path()
                                )))),
                                attempt,
                            );
                        }
                    }
                }
                return (Ok(()), attempt);
            }
            Err(_e) if attempt <= retries => {
                if verbose {
                    eprintln!(
                        "Step '{}' failed (attempt {}/{}). Retrying in {:?}...",
                        if step.name.is_empty() {
                            "unnamed"
                        } else {
                            &step.name
                        },
                        attempt,
                        retries + 1,
                        retry_delay
                    );
                }
                if !retry_delay.is_zero() {
                    tokio::time::sleep(retry_delay).await;
                }
            }
            Err(e) => return (Err(e), attempt),
        }
    }
}

pub struct ParallelContext {
    pub container_ids: Arc<Mutex<Vec<String>>>,
    pub image_pull_locks: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
}

#[derive(Clone)]
pub struct ParallelTaskContext {
    pub step_index: usize,
    pub ctx: Arc<ParallelContext>,
}

/// Output information for an executed parallel step.
#[derive(Debug)]
pub struct ParallelStepOutput {
    pub step_index: usize,
    pub step_name: String,
    pub continue_on_error: bool,
    pub result: Result<(), String>,
    pub exit_code: Option<i32>,
    pub is_timeout: bool,
    pub attempts: u32,
    pub duration: Duration,
}

#[allow(clippy::too_many_arguments)]
async fn run_single_parallel_attempt(
    docker: &Docker,
    step: &Step,
    verbose: bool,
    cache_config: &CacheConfig,
    step_temp_dir: &Path,
    task: &ParallelTaskContext,
    runtime: &PipelineRuntimeContext,
    monitor: Arc<dyn PipelineMonitor>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let image_name = &step.image;
    let image_lock = {
        let mut locks = task.ctx.image_pull_locks.lock().await;
        locks
            .entry(image_name.clone())
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone()
    };

    let _permit = image_lock.acquire().await.map_err(|_| {
        std::io::Error::other("Image pull lock closed unexpectedly during parallel execution")
    })?;
    let container_ctx = ContainerRuntimeContext {
        workspace_dir: &runtime.workspace_dir,
        cache_dir: &runtime.cache_dir,
        secrets_env: runtime.secrets_env.as_ref(),
    };
    let setup = prepare_container(
        docker,
        step,
        cache_config,
        step_temp_dir,
        &container_ctx,
        verbose,
    )
    .await?;
    drop(_permit);

    monitor.on_step_start(&setup.step_name, &setup.image);

    let container_id = create_and_start_container(docker, &setup).await?;
    monitor.on_container_created(&container_id);
    runtime.container_tracker.track(container_id.clone()).await;
    {
        let mut ids = task.ctx.container_ids.lock().await;
        ids.push(container_id.clone());
    }

    let timeout_opt = step.timeout_duration().map_err(|e| {
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
            as Box<dyn std::error::Error + Send + Sync>
    })?;

    let log_handle = tokio::spawn({
        let docker = docker.clone();
        let container_id = container_id.clone();
        let step_name = setup.step_name.clone();
        let monitor = Arc::clone(&monitor);
        async move { stream_logs_to_monitor(&docker, &container_id, &step_name, monitor).await }
    });

    let wait_future = wait_for_container(docker, &container_id, &setup.step_name);

    let wait_result = if let Some(dur) = timeout_opt {
        match tokio::time::timeout(dur, wait_future).await {
            Ok(res) => res,
            Err(_) => {
                let _ = log_handle.await;
                cleanup_container(docker, &container_id, verbose).await;
                runtime.container_tracker.untrack(&container_id).await;
                monitor.on_container_destroyed(&container_id);
                monitor.on_step_complete(&setup.step_name, false);
                return Err(Box::new(TimeoutError {
                    step_name: setup.step_name.clone(),
                    duration: dur,
                }));
            }
        }
    } else {
        wait_future.await
    };

    let _ = log_handle.await;
    let success = wait_result.is_ok();
    monitor.on_step_complete(&setup.step_name, success);

    wait_result.map(|_| ())
}

#[allow(clippy::too_many_arguments)]
pub async fn run_step_parallel(
    docker: &Docker,
    step: &Step,
    verbose: bool,
    cache_config: &CacheConfig,
    temp_dir: &Path,
    task: ParallelTaskContext,
    runtime: &PipelineRuntimeContext,
    monitor: Arc<dyn PipelineMonitor>,
) -> ParallelStepOutput {
    let step_start = Instant::now();
    let step_name = if step.name.is_empty() {
        format!("step-{}", task.step_index)
    } else {
        step.name.clone()
    };

    let step_temp_dir = temp_dir.join(format!("step-{}", task.step_index));
    if let Err(e) = tokio::fs::create_dir_all(&step_temp_dir).await {
        return ParallelStepOutput {
            step_index: task.step_index,
            step_name,
            continue_on_error: step.continue_on_error,
            result: Err(format!("Failed to create step temp dir: {e}")),
            exit_code: None,
            is_timeout: false,
            attempts: 0,
            duration: step_start.elapsed(),
        };
    }

    let retries = step.retries_count();
    let retry_delay = match step.retry_delay_duration() {
        Ok(d) => d,
        Err(e) => {
            return ParallelStepOutput {
                step_index: task.step_index,
                step_name,
                continue_on_error: step.continue_on_error,
                result: Err(format!("Invalid retry delay: {e}")),
                exit_code: None,
                is_timeout: false,
                attempts: 0,
                duration: step_start.elapsed(),
            };
        }
    };

    let mut attempt = 0;
    loop {
        attempt += 1;
        let res = run_single_parallel_attempt(
            docker,
            step,
            verbose,
            cache_config,
            &step_temp_dir,
            &task,
            runtime,
            Arc::clone(&monitor),
        )
        .await;

        match res {
            Ok(()) => {
                return ParallelStepOutput {
                    step_index: task.step_index,
                    step_name,
                    continue_on_error: step.continue_on_error,
                    result: Ok(()),
                    exit_code: Some(0),
                    is_timeout: false,
                    attempts: attempt,
                    duration: step_start.elapsed(),
                };
            }
            Err(_e) if attempt <= retries => {
                if verbose {
                    eprintln!(
                        "Parallel step '{}' failed (attempt {}/{}). Retrying in {:?}...",
                        step_name,
                        attempt,
                        retries + 1,
                        retry_delay
                    );
                }
                if !retry_delay.is_zero() {
                    tokio::time::sleep(retry_delay).await;
                }
            }
            Err(e) => {
                let err_str = e.to_string();
                let exit_code = FailureReason::parse_exit_code_from_message(&err_str);
                let is_timeout = e.is::<TimeoutError>() || err_str.contains("timed out");
                return ParallelStepOutput {
                    step_index: task.step_index,
                    step_name,
                    continue_on_error: step.continue_on_error,
                    result: Err(err_str),
                    exit_code,
                    is_timeout,
                    attempts: attempt,
                    duration: step_start.elapsed(),
                };
            }
        }
    }
}

pub type StepResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// Legacy compatibility helper for awaiting raw step handles.
pub async fn collect_parallel_results(
    handles: Vec<JoinHandle<StepResult>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if handles.is_empty() {
        return Ok(());
    }

    let mut remaining = handles;

    loop {
        let (result, _index, rest) = select_all(remaining).await;
        remaining = rest;

        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                for handle in remaining {
                    handle.abort();
                    let _ = handle.await;
                }
                return Err(e);
            }
            Err(join_err) if join_err.is_cancelled() => {}
            Err(join_err) => {
                for handle in remaining {
                    handle.abort();
                    let _ = handle.await;
                }
                return Err(Box::new(std::io::Error::other(format!(
                    "Parallel step task panicked: {join_err}"
                ))));
            }
        }

        if remaining.is_empty() {
            break;
        }
    }

    Ok(())
}

/// Await parallel step tasks; on first unhandled failure abort siblings so containers do not leak.
/// Steps with `continue_on_error: true` will NOT abort sibling tasks.
pub async fn collect_parallel_outputs(
    handles: Vec<JoinHandle<ParallelStepOutput>>,
) -> (
    Result<(), Box<dyn std::error::Error + Send + Sync>>,
    Vec<ParallelStepOutput>,
) {
    if handles.is_empty() {
        return (Ok(()), Vec::new());
    }

    let mut remaining = handles;
    let mut outputs = Vec::new();
    let mut first_error: Option<Box<dyn std::error::Error + Send + Sync>> = None;

    loop {
        let (output_res, _index, rest) = select_all(remaining).await;
        remaining = rest;

        match output_res {
            Ok(output) => {
                let is_failure = output.result.is_err();
                let continue_on_err = output.continue_on_error;
                let step_name = output.step_name.clone();
                outputs.push(output);

                if is_failure && !continue_on_err && first_error.is_none() {
                    let err_msg = outputs.last().unwrap().result.as_ref().unwrap_err().clone();
                    first_error = Some(Box::new(std::io::Error::other(format!(
                        "Step '{step_name}' failed: {err_msg}"
                    ))));

                    // Abort all remaining sibling tasks immediately
                    for handle in remaining {
                        handle.abort();
                        let _ = handle.await;
                    }
                    break;
                }
            }
            Err(join_err) if join_err.is_cancelled() => {}
            Err(join_err) => {
                let err: Box<dyn std::error::Error + Send + Sync> = Box::new(
                    std::io::Error::other(format!("Parallel step task panicked: {join_err}")),
                );
                if first_error.is_none() {
                    first_error = Some(err);
                }
                for handle in remaining {
                    handle.abort();
                    let _ = handle.await;
                }
                break;
            }
        }

        if remaining.is_empty() {
            break;
        }
    }

    outputs.sort_by_key(|o| o.step_index);

    let stage_result = match first_error {
        Some(e) => Err(e),
        None => Ok(()),
    };

    (stage_result, outputs)
}

pub async fn run_stage_parallel(
    docker: &Docker,
    steps: &[Step],
    verbose: bool,
    cache_config: &CacheConfig,
    temp_dir: &Path,
    runtime: &PipelineRuntimeContext,
    monitor: Arc<dyn PipelineMonitor>,
) -> (
    Result<(), Box<dyn std::error::Error + Send + Sync>>,
    Vec<ParallelStepOutput>,
) {
    let ctx = Arc::new(ParallelContext {
        container_ids: Arc::new(Mutex::new(Vec::new())),
        image_pull_locks: Arc::new(Mutex::new(HashMap::new())),
    });

    let mut handles = Vec::with_capacity(steps.len());

    for (index, step) in steps.iter().enumerate() {
        let docker = docker.clone();
        let step = step.clone();
        let cache = cache_config.clone();
        let temp_dir = temp_dir.to_path_buf();
        let task = ParallelTaskContext {
            step_index: index,
            ctx: Arc::clone(&ctx),
        };
        let runtime = runtime.clone();
        let monitor = Arc::clone(&monitor);

        handles.push(tokio::spawn(async move {
            run_step_parallel(
                &docker, &step, verbose, &cache, &temp_dir, task, &runtime, monitor,
            )
            .await
        }));
    }

    let (run_result, outputs) = collect_parallel_outputs(handles).await;

    // Always cleanup tracked containers (including siblings aborted after a failure).
    let ids = { ctx.container_ids.lock().await.clone() };
    cleanup_containers(docker, &ctx.container_ids, verbose).await;
    for id in ids {
        runtime.container_tracker.untrack(&id).await;
        monitor.on_container_destroyed(&id);
    }

    (run_result, outputs)
}

pub fn resolve_stage_dependencies(
    stages: &[Stage],
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    let stage_names: HashSet<String> = stages.iter().map(|s| s.name.clone()).collect();
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();

    for stage in stages {
        graph.entry(stage.name.clone()).or_default();
        in_degree.entry(stage.name.clone()).or_insert(0);

        for dep in &stage.depends_on {
            if dep == &stage.name {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "Stage '{}' cannot depend on itself.\nRemove '{}' from its own depends_on list.",
                        stage.name, stage.name
                    ),
                )));
            }

            if !stage_names.contains(dep) {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "Stage '{}' depends on '{}', but '{}' is not defined.\nAvailable stages: {}",
                        stage.name,
                        dep,
                        dep,
                        stage_names.iter().cloned().collect::<Vec<_>>().join(", ")
                    ),
                )));
            }

            graph
                .entry(dep.clone())
                .or_default()
                .push(stage.name.clone());
            *in_degree.entry(stage.name.clone()).or_insert(0) += 1;
        }
    }

    // Kahn's algorithm for topological sort
    let mut queue: Vec<String> = in_degree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(name, _)| name.clone())
        .collect();
    queue.sort(); // Deterministic order

    let mut result = Vec::new();

    while let Some(current) = queue.pop() {
        result.push(current.clone());

        if let Some(dependents) = graph.get(&current) {
            for dependent in dependents {
                if let Some(deg) = in_degree.get_mut(dependent) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(dependent.clone());
                        queue.sort();
                    }
                }
            }
        }
    }

    if result.len() != stages.len() {
        let remaining: Vec<String> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg > 0)
            .map(|(name, _)| name.clone())
            .collect();

        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "Circular dependency detected in stages: {}\n\
                 These stages form a dependency cycle and cannot be resolved.\n\
                 Hint: Remove one of the dependencies to break the cycle.",
                remaining.join(" -> ")
            ),
        )));
    }

    Ok(result)
}
