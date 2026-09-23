use forge_runner::runner::collect_parallel_results;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::task::JoinHandle;

type StepResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn collect_parallel_results_succeeds_when_all_tasks_ok() {
    let handles: Vec<JoinHandle<StepResult>> = vec![
        tokio::spawn(async { Ok(()) }),
        tokio::spawn(async { Ok(()) }),
    ];

    collect_parallel_results(handles)
        .await
        .expect("all tasks should succeed");
}

#[tokio::test]
async fn collect_parallel_results_aborts_remaining_tasks_on_failure() {
    let started = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicUsize::new(0));

    let started_fail = Arc::clone(&started);
    let handles: Vec<JoinHandle<StepResult>> = vec![
        tokio::spawn(async move {
            started_fail.fetch_add(1, Ordering::SeqCst);
            Err(Box::new(std::io::Error::other("step failed"))
                as Box<dyn std::error::Error + Send + Sync>)
        }),
        tokio::spawn({
            let started = Arc::clone(&started);
            let finished = Arc::clone(&finished);
            async move {
                started.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(500)).await;
                finished.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }),
    ];

    let err = collect_parallel_results(handles)
        .await
        .expect_err("first failing task should fail the stage");
    assert!(err.to_string().contains("step failed"));
    assert_eq!(started.load(Ordering::SeqCst), 2);
    assert_eq!(
        finished.load(Ordering::SeqCst),
        0,
        "slow sibling must be aborted before completion"
    );
}

/// Validates true concurrent fail-fast: slow task is first in the list,
/// fast failing task is last. With `select_all`, the fast failure should
/// abort the slow task without waiting for it to complete.
#[tokio::test]
async fn collect_parallel_results_aborts_slow_first_task_when_later_task_fails() {
    let finished = Arc::new(AtomicUsize::new(0));

    let finished_clone = Arc::clone(&finished);
    let handles: Vec<JoinHandle<StepResult>> = vec![
        // Slow task first — would block sequential await for 500ms
        tokio::spawn({
            let finished = Arc::clone(&finished);
            async move {
                tokio::time::sleep(Duration::from_millis(500)).await;
                finished.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }),
        // Fast failing task last
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            finished_clone.fetch_add(1, Ordering::SeqCst);
            Err(Box::new(std::io::Error::other("fast task failed"))
                as Box<dyn std::error::Error + Send + Sync>)
        }),
    ];

    let err = collect_parallel_results(handles)
        .await
        .expect_err("fast failing task should abort the stage");
    assert!(err.to_string().contains("fast task failed"));
    // Only the fast failing task incremented finished (value=1);
    // the slow task must have been aborted before its sleep completed.
    assert_eq!(
        finished.load(Ordering::SeqCst),
        1,
        "slow first task must be aborted before it completes"
    );
}

#[tokio::test]
async fn collect_parallel_results_handles_empty_input() {
    collect_parallel_results(vec![])
        .await
        .expect("empty handles should succeed immediately");
}
