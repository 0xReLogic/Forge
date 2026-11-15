# FORGE Roadmap

This document outlines the planned direction for FORGE. It is not a binding schedule — priorities may shift based on contributor interest and real-world usage feedback.

---

## v1.2 — Structured Execution Contract ✓ completed

Every run now produces typed results, consistent exit codes, machine-readable output,
a human-readable summary, and a persisted run directory for future replay.

Core types: `PipelineResult`, `StageResult`, `StepResult`, `ExecutionStatus`, `FailureReason`, `ExitCode`

Exit codes:

```
0 = success
1 = pipeline execution failure
2 = config / validation error
3 = Docker / runtime error
4 = cancelled
5 = timeout
```

Output formats:

```bash
forge run                  # human summary (default)
forge run --format json    # machine-readable JSON on stdout
forge run --format junit   # JUnit XML for CI test reporting
```

Run persistence:

```
.forge/runs/<run-id>/
├── result.json
├── metadata.json
└── logs/
```

---

## v1.3 — Stage and Step Filtering ✓ completed

After the result contract was in place, filtering became straightforward and is
now available in `forge run`.

```bash
forge run --filter test
forge run --filter test.cargo-test
```

This is critical for agent iteration: when only one step fails, the agent does
not have to rerun the entire pipeline. Filtering also makes Forge significantly
faster to use interactively during development.

Delivered in `v1.3.0`:

- [#69](https://github.com/0xReLogic/Forge/issues/69) — Add `--filter` flag to `forge run`
- [#70](https://github.com/0xReLogic/Forge/issues/70) — Validate `--filter` input and produce user-friendly errors
- [#71](https://github.com/0xReLogic/Forge/issues/71) — Add unit tests for filter resolution logic
- [#72](https://github.com/0xReLogic/Forge/issues/72) — Document `--filter` in README and usage guide (no Rust required)
- [#73](https://github.com/0xReLogic/Forge/issues/73) — `--filter` should automatically include stage dependencies

---

## v1.4 — Execution Control and Artifacts ✓ completed

Combines step-level runtime control, failure recovery policies, and artifact management between pipeline stages.

### Timeout and Cancellation
- Per-step timeout configuration in YAML
- Graceful shutdown on Ctrl+C with container cleanup
- Timeout counted as a distinct exit code (not a generic failure)

### Retry and Failure Control
- Per-step retry policy (`retries: 3`, `retry_delay: 5s`)
- Per-step and per-stage `continue_on_error` flag
- `fail_fast` at the pipeline level

### Artifacts
- Artifact upload and download between stages
- Artifacts stored under `.forge/runs/<run-id>/artifacts/`
- Referenced in YAML by path or name
- Foundation for replay to restore prior run state

---

## v1.5 — Resource Limits and Security Isolation ✓ completed

Running arbitrary commands in containers means security should be a first-class concern, not an afterthought.

- CPU and memory hard limits per step
- Network isolation modes (none, host, allowlist)
- Read-only filesystem options
- Secret isolation improvements
- Volume restrictions and container capability restrictions
- Pinned image digests for hermetic, reproducible builds

Delivered in `v1.5.0`:

- Per-step CPU and memory cgroup limits
- `bridge`, `none`, `host`, and fail-closed `allowlist` network policies
- Read-only root filesystem and workspace/cache mounts
- Explicit per-step secret allowlists
- Capability add/drop controls and restricted host volume surface
- Optional mandatory sha256 image digest validation

---

## v1.6 — GitHub Actions Compatibility

The goal is to let developers run their existing GitHub Actions workflows locally before pushing. Not full compatibility from day one — a useful subset first, expanded over time.

Starting with:

```yaml
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo test
```

Then progressively supporting `env`, `with`, `needs`, `if`, `matrix`, `artifacts`, and `services`.

This turns "similar to GitHub Actions" into "runs GitHub Actions workflows locally."

---

## v1.7 — Matrix Builds and Service Containers

- Matrix builds across variables (language versions, OS targets, build flags)
- Service containers (postgres, redis, mysql, etc.) alongside pipeline steps
- Local test matrix without pushing to remote CI

---

## v1.8 — Developer Experience

- `forge watch` — rerun pipeline on file change
- `forge doctor` — diagnose environment and config issues
- `forge logs` — browse logs from previous runs
- `forge retry` — rerun a specific stage or step
- `forge rerun-failed` — rerun only failed steps from the last run
- TUI improvements building on existing Ratatui foundation

---

## v1.9 — WASM Plugin System

WebAssembly Component Model matured in 2025–2026 and is now production-ready as a plugin runtime. Better foundation for Forge's action system than shell scripts or raw binaries — sandboxed, cross-language, near-native speed.

- Actions as WASM modules (Rust, Go, Python, or any WASM target)
- Sandboxed execution with controlled API surface
- Simple local registry or directory-based resolution

```yaml
steps:
  - uses: forge/rust-check
  - uses: forge/docker-build
  - uses: forge/security-scan
```

---

## v1.10 — Observability (OpenTelemetry)

OpenTelemetry is the de-facto observability standard in 2026. Forge can emit traces per pipeline run, stage, and step to any local collector.

- Structured span attributes: duration, exit code, image, command
- Export to Jaeger, Tempo, or any OTel-compatible collector
- Enables debugging pipeline failures with traces, not just log text

---

## v1.11 — Remote Runner

Once the local runner is mature, extend it to support remote execution as lightweight self-hosted CI infrastructure.

- `forge server` — host a runner
- `forge agent` — connect a worker
- CLI-first, no web dashboard required

---

## forge replay

`forge replay` is not a single feature added at the end — it is the result of building the right foundations across multiple versions.

Each run from v1.2 onward produces:

```
.forge/runs/<run-id>/
├── result.json       # structured execution result (v1.2)
├── metadata.json     # commit, config, environment snapshot (v1.2)
├── logs/             # per-step log files (v1.2)
├── artifacts/        # uploaded artifacts (v1.4)
└── manifest.json     # full run manifest for replay (v1.4)
```

When those pieces are in place:

```bash
forge replay <run-id>
```

FORGE restores the commit, environment, pipeline config, cache state, and artifacts, then reruns the pipeline locally. This closes the feedback loop that FORGE was originally built to shorten.

---

## Ecosystem Positioning

Forge fits naturally alongside the tools developers already use:

- **cargo-nextest** — Forge orchestrates, nextest executes Rust tests
- **mise** — mise manages environment and tool versions, Forge runs the pipeline
- **Ratatui** — Forge's TUI is built on Ratatui; both are part of the Rust CLI ecosystem
- **Docker** — Forge's isolation layer, already in place

---

## What FORGE is not trying to become

FORGE has a clear identity: a local-first, offline-capable, CLI and TUI-driven pipeline runner built in Rust. The goal is to deepen that identity, not broaden it into a SaaS product, web dashboard, or cloud platform.

The order of priorities reflects this:

```
Structured Execution Contract
      ->
Stage and Step Filtering
      ->
Execution Control and Artifacts (Timeout, Retry, Artifacts)
      ->
Resource Limits and Security
      ->
GitHub Actions Compatibility
      ->
Matrix / Services
      ->
Developer Experience
      ->
WASM Plugins
      ->
Observability
      ->
Remote Runner
      ->
Replay
```
