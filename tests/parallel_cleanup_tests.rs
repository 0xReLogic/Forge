use forge::runner::collect_parallel_results;
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

    collect_parallel_results(handles).await.expect("all tasks should succeed");
}

#[tokio::test]
async fn collect_parallel_results_aborts_remaining_tasks_on_failure() {
    let started = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicUsize::new(0));

    let started_fail = Arc::clone(&started);
    let handles: Vec<JoinHandle<StepResult>> = vec![
        tokio::spawn(async move {
            started_fail.fetch_add(1, Ordering::SeqCst);
            Err(
                Box::new(std::io::Error::other("step failed"))
                    as Box<dyn std::error::Error + Send + Sync>,
            )
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