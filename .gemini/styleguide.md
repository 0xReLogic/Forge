  # FORGE Code Review Style Guide

  FORGE is a local-first CI/CD pipeline runner built in Rust. It uses Docker for
  container execution, Tokio for async, Clap for CLI, and Ratatui for TUI.
  The project is intentionally open to contributors who are learning Rust.

  Reviews should find real problems. Not enforce personal preferences.

  ---

  ## Project Principles

  - Correctness and reliability come before style.
  - Simple, readable code is preferred over clever abstractions.
  - Public behavior (CLI flags, exit codes, JSON output, YAML config) is a
    contract. Breaking changes need strong justification.
  - This project is beginner-friendly. Reviews should be encouraging, not
    exhausting.

  ---

  ## Rust Code Quality

  Flag these when they appear in non-test production code:

  - `unwrap()` or `expect()` on user input, config parsing, or Docker API calls.
    These should use `?` or explicit error handling instead.
  - Unnecessary `.clone()` that could be avoided with a borrow.
  - Unused imports, dead code, or shadowed variables with confusing intent.
  - Magic numbers or string literals that should be named constants.

  Do not flag:

  - `unwrap()` inside `#[cfg(test)]` blocks — this is acceptable in tests.
  - Iterators vs. for loops when both are equally clear.
  - Naming that is already consistent with the surrounding code.
  - Formatting issues — `rustfmt` handles this automatically.

  ---

  ## Correctness and Logic

  Prioritize these above all other review categories:

  - Incorrect stage dependency resolution or execution order.
  - Incorrect parallel vs. sequential execution behavior.
  - Wrong exit code emitted for a given failure type.
  - Incorrect JSON or JUnit output structure.
  - Pipeline result that does not reflect actual execution outcome.
  - Filter logic that includes or excludes wrong stages/steps.
  - Logic that works for the happy path but breaks on failure or empty input.

  ---

  ## Error Handling

  FORGE uses `Box<dyn std::error::Error + Send + Sync>` as its primary error type
  throughout. The `FailureReason` enum in `src/result.rs` classifies errors for
  exit code mapping and structured output.

  Flag:

  - Errors swallowed with `let _ = ...` without explanation where the result
    matters.
  - Error messages that don't tell the user what failed, where, and why.
  - Incorrect `FailureReason` classification (e.g., treating a config error as a
    Docker error).
  - `process::exit` called with a hardcoded number instead of `ExitCode::as_i32()`.

  Do not flag:

  - Non-fatal errors that are intentionally logged as warnings (e.g., run
    persistence failure).
  - The `Box<dyn Error>` pattern — this is the established convention in FORGE.

  ---

  ## Memory and Resource Safety

  This is critical for a tool that manages Docker containers and spawns async tasks.

  Flag when containers, processes, or tasks may not be cleaned up on:

  - Step failure
  - Stage failure
  - Early return
  - Cancellation (Ctrl-C or TUI quit)
  - Panic or unexpected error

  Specifically look for:

  - Docker containers started but not removed on failure paths.
  - Tokio tasks spawned but never awaited or aborted.
  - Temporary directories created but not removed on error.
  - Log buffers or `Vec` that grow without bound during pipeline execution.
  - File descriptors or other OS resources that can remain alive unintentionally
    due to leaked ownership, abandoned tasks, or resources escaping their
    intended lifetime.

  Do not flag speculative memory concerns without a realistic scenario.

  ---

  ## Concurrency and Async Safety

  FORGE uses Tokio for async and runs pipeline steps concurrently when
  `parallel: true`. The `collect_parallel_results` function in
  `src/runner/mod.rs` uses `select_all` for fail-fast behavior.

  Flag:

  - Shared mutable state accessed without proper synchronization (`Arc<Mutex<_>>`
    is the existing pattern).
  - Blocking filesystem, subprocess, or CPU-heavy operations on Tokio worker
    threads that can materially block async task progress — unless the code
    already uses an async API or the blocking behavior is intentional.
  - Tasks spawned inside parallel execution that are not tracked or cleaned up.
  - Race conditions in container tracking or log buffering.

  Do not flag `Arc<Mutex<_>>` usage — this is the established pattern in FORGE's
  `TuiState` and `ParallelContext`.

  ---

  ## Docker and Process Management

  Flag:

  - Container created but cleanup not guaranteed on all code paths.
  - Exit code from `wait_for_container` not checked or incorrectly handled.
  - Environment variables passed to containers that may include secrets
    unintentionally.
  - Volume mounts that expose unintended host filesystem paths.
  - Stdout/stderr of containers not routed through the `PipelineMonitor` interface.

  Do not flag the bollard API usage patterns that are already established in
  `src/docker/mod.rs` unless there is a concrete correctness concern.

  ---

  ## CLI and TUI

  Flag:

  - New `forge run` flags that don't preserve existing behavior when omitted.
  - Exit codes that don't match the `ExitCode` enum in `src/result.rs`.
  - Progress output or log lines written to stdout in `--format json` or
    `--format junit` mode — these modes must keep stdout clean for piping.
  - User-facing errors that are unclear or fail to provide actionable context.
  Where practical, include a useful hint or remediation guidance.
  - TUI changes that don't account for container cleanup on quit.

  Do not flag the existing `println!`/`eprintln!` approach — FORGE does not use
  a logging framework and this is intentional.

  ---

  ## Security

  Flag:

  - Values from `secrets_env` printed to stdout, logs, or persisted files.
  - Environment variables blindly dumped to `metadata.json` or `result.json`.
  - Shell command construction from user input without sanitization.
  - Docker volume mounts that could expose sensitive host paths.
  - Container configurations that grant unnecessary capabilities.

  The `metadata.json` written by `src/persist.rs` uses an allowlist of safe
  fields. Any PR that extends this should be reviewed carefully.

  ---

  ## Testing

  Flag when a PR:

  - Adds new CLI behavior with no corresponding test.
  - Fixes a bug with no regression test (when one is practical to add).
  - Changes serialized output (JSON, JUnit) without updating or adding
    serialization tests.

  Do not require tests for:

  - Pure documentation changes.
  - Trivial formatting or comment updates.
  - Changes that are already covered by existing tests.

  The project has unit tests in `src/` (`#[cfg(test)]` modules) and integration
  tests in `tests/`. Both patterns are acceptable.

  ---

  ## Performance

  Flag:

  - Repeated cloning of large structs inside hot loops.
  - Unnecessary serialization/deserialization in tight paths.
  - Unbounded log or output buffer growth during long pipeline runs.

  Do not flag micro-optimizations. FORGE prioritizes correctness and readability.

  ---

  ## Maintainability

  Flag:

  - Functions longer than ~80 lines that contain multiple distinct responsibilities
  or materially obscure control flow. Length alone is not a bug.
  - Deeply nested match/if blocks that obscure control flow.
  - Duplicate logic that already exists elsewhere in the codebase.
  - Hard-coded values that should reference existing constants or enums.

  Do not request large refactors unrelated to the PR's stated goal.

  ---

  ## API and Data Contract Stability

  The following are stable contracts that must not change without strong justification:

  - `ExitCode` numeric values (`0`–`5`).
  - `ExecutionStatus` enum variants and their JSON serialization.
  - `FailureReason` enum variant names and their `"type"` tag in JSON.
  - `PipelineResult`, `StageResult`, `StepResult` field names in JSON.
  - `forge run` flag names and their behavior when omitted.
  - YAML configuration structure (`stages`, `steps`, `parallel`, `depends_on`).

  Flag any PR that changes these without a corresponding migration plan.

  ---

  ## Logging and Observability

  FORGE routes output through `PipelineMonitor` implementations. In
  `--format json` and `--format junit` modes, `SilentMonitor` is used to keep
  stdout clean.

  Flag:

  - Direct `println!` calls in code paths that run during `--format json` or
    `--format junit` execution.
  - Step logs written to stdout instead of going through the monitor interface.
  - Secrets or sensitive values appearing in any output stream.

  ---

  ## Documentation

  Flag when:

  - A public function or type has complex behavior with no doc comment.
  - An architectural decision is made that future contributors would not be able
    to infer from the code alone.

  Do not require doc comments on every private helper function.

  ---

  ## Contributor-Friendly Reviews

  This project welcomes contributors who are learning Rust. Reviews should reflect that.

  **Do flag:**
  - Real bugs and correctness issues.
  - Security problems.
  - Resource leaks.
  - Missing tests for behavioral changes.
  - Unclear error messages.

  **Do not flag:**
  - Personal preference over naming when it follows existing conventions.
  - Formatting — `rustfmt` handles this.
  - "I would have structured this differently" without a concrete reason.
  - Requests for abstractions that aren't needed yet.
  - Refactors unrelated to the PR.
  - Minor nitpicks as blocking issues.

  When suggesting a change, explain what the problem is and why it matters.
  A good review comment has: problem, impact, and direction — not just "change this."

  ---

  ## Review Severity

  Use these consistently:

  **CRITICAL** — exploitable security vulnerability, data loss, catastrophic
  correctness failure, severe resource leak.

  **HIGH** — significant correctness bug, serious security issue, major race
  condition, breaking change to a stable contract.

  **MEDIUM** — meaningful bug, missing error handling for a realistic case,
  important missing test, maintainability issue likely to cause future bugs.

  **LOW** — minor readability concern, small improvement, non-blocking suggestion.

  Nitpicks should not block merging.

  ---

  ## Dependency Discipline

Flag new dependencies when they duplicate functionality already available in
the standard library or existing project dependencies, or when the added
dependency introduces significant maintenance or security cost without clear
benefit.

Do not reject a dependency merely because a manual implementation is possible.
If the crate is well-maintained and solves the problem cleanly, it is a
reasonable choice.

---

## Unsafe Rust

Flag new `unsafe` code when:

- Safety invariants are not clearly documented in a comment adjacent to the
  unsafe block.
- The unsafe operation is unnecessary and a safe alternative exists without
  meaningful trade-offs.
- The safety boundary is too broad or difficult to audit.

Do not flag existing `unsafe` code solely because it is unsafe. Review the
actual safety invariants and whether they are upheld.

FORGE does not currently use `unsafe` — if it appears in a PR it warrants
careful review.

---

## What Reviewers Should NOT Do

  - Comment on formatting that `rustfmt` handles automatically.
  - Request changes based on personal style preference.
  - Ask for abstractions or generics that aren't needed by the current PR.
  - Expand the scope of a PR by requesting unrelated refactors.
  - Make assumptions about Rust, Docker, or async behavior without verifying
    against official documentation.
  - Discourage beginner contributors with condescending tone.
  - Treat every LOW severity observation as a required fix.

