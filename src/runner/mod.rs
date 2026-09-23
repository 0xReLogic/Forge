pub mod monitor;

use bollard::Docker;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinHandle;

use crate::config::{CacheConfig, Stage, Step};
use crate::docker::{
    ContainerRuntimeContext, cleanup_container, cleanup_containers, create_and_start_container,
    prepare_container, wait_for_container,
};
use crate::logger::stream_logs_to_monitor;
use monitor::PipelineMonitor;

#[derive(Clone)]
pub struct PipelineRuntimeContext {
    pub workspace_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub secrets_env: Arc<HashMap<String, String>>,
}

pub async fn run_command_in_container(
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
    monitor.on_container_created(&container_id);

    let log_handle = tokio::spawn({
        let docker = docker.clone();
        let container_id = container_id.clone();
        let step_name = setup.step_name.clone();
        let monitor = Arc::clone(&monitor);
        async move { stream_logs_to_monitor(&docker, &container_id, &step_name, monitor).await }
    });

    let wait_result = wait_for_container(docker, &container_id, &setup.step_name).await;

    let _ = log_handle.await;
    cleanup_container(docker, &container_id, verbose).await;
    monitor.on_container_destroyed(&container_id);

    let success = wait_result.is_ok();
    monitor.on_step_complete(&setup.step_name, success);

    wait_result.map(|_| ())
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
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let step_temp_dir = temp_dir.join(format!("step-{}", task.step_index));
    tokio::fs::create_dir_all(&step_temp_dir).await?;

    if verbose {
        println!(
            "  Created isolated temp directory: {}",
            step_temp_dir.display()
        );
    }

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
        &step_temp_dir,
        &container_ctx,
        verbose,
    )
    .await?;
    drop(_permit);

    monitor.on_step_start(&setup.step_name, &setup.image);

    let container_id = create_and_start_container(docker, &setup).await?;
    monitor.on_container_created(&container_id);
    {
        let mut ids = task.ctx.container_ids.lock().await;
        ids.push(container_id.clone());
    }

    let log_handle = tokio::spawn({
        let docker = docker.clone();
        let container_id = container_id.clone();
        let step_name = setup.step_name.clone();
        let monitor = Arc::clone(&monitor);
        async move { stream_logs_to_monitor(&docker, &container_id, &step_name, monitor).await }
    });

    let wait_result = wait_for_container(docker, &container_id, &setup.step_name).await;
    let _ = log_handle.await;

    let success = wait_result.is_ok();
    monitor.on_step_complete(&setup.step_name, success);

    wait_result.map(|_| ())
}

type StepResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// Await parallel step tasks; on first failure abort siblings so containers do not leak.
pub async fn collect_parallel_results(
    handles: Vec<JoinHandle<StepResult>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut error_result: Option<Box<dyn std::error::Error + Send + Sync>> = None;

    for handle in handles {
        if error_result.is_some() {
            handle.abort();
            if let Err(join_err) = handle.await && !join_err.is_cancelled() {
                error_result.get_or_insert(Box::new(std::io::Error::other(format!(
                    "Parallel step task join error after cancellation: {join_err}"
                ))));
            }
            continue;
        }

        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => error_result = Some(e),
            Err(join_err) if join_err.is_cancelled() => {}
            Err(join_err) => {
                error_result = Some(Box::new(std::io::Error::other(format!(
                    "Parallel step task panicked or was cancelled: {join_err}"
                ))))
            }
        }
    }

    if let Some(err) = error_result {
        return Err(err);
    }
    Ok(())
}

pub async fn run_stage_parallel(
    docker: &Docker,
    steps: &[Step],
    verbose: bool,
    cache_config: &CacheConfig,
    temp_dir: &Path,
    runtime: &PipelineRuntimeContext,
    monitor: Arc<dyn PipelineMonitor>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

    let run_result = collect_parallel_results(handles).await;

    // Always cleanup tracked containers (including siblings aborted after a failure).
    let ids = { ctx.container_ids.lock().await.clone() };
    cleanup_containers(docker, &ctx.container_ids, verbose).await;
    for id in ids {
        monitor.on_container_destroyed(&id);
    }

    run_result
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
