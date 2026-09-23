# Usage Guide

## Quick Start

```bash
# Initialize a project with an example configuration file
forge init

# Validate the configuration
forge validate

# Run the pipeline
forge run
```

## Workspace Mount

FORGE mounts the current directory into containers at `/workspace`.
If a step does not specify `working_dir`, the container working directory defaults to `/workspace`.

## Commands

### Project Initialization

Create a new forge.yaml configuration file:

```bash
forge init
```

Or with a different filename:

```bash
forge init --file custom-forge.yaml
```

Use the `--force` flag to overwrite an existing file:

```bash
forge init --force
```

### Configuration Validation

Validate a configuration file:

```bash
forge validate
```

Or with a different configuration file:

```bash
forge validate --file custom-forge.yaml
```

### Run Pipeline

Run the pipeline:

```bash
forge run
```

Or with a different configuration file:

```bash
forge run --file custom-forge.yaml
```

Run with verbose output (includes performance metrics):

```bash
forge run --verbose
```

Validate pipeline without execution (dry-run mode):

```bash
forge run --dry-run
```

Run a specific stage:

```bash
forge run --stage build
```

When `--stage` is used, FORGE runs the selected stage and any stages it depends on.

Enable or disable caching:

```bash
forge run --cache
forge run --no-cache
```

Cache storage:

- FORGE stores cache data inside your project at `./.forge/cache/`.
- To reset cache for a project, delete the `.forge/` directory.

Select output format:

```bash
forge run --format human   # default, colored terminal output with pipeline summary
forge run --format json    # machine-readable JSON on stdout, diagnostics on stderr
forge run --format junit   # JUnit XML for CI test reporting
```

When using `--format json`, stdout contains only the JSON object and is safe to pipe:

```bash
forge run --format json > result.json
cat result.json | jq '.failure'
```

Exit codes are consistent across all formats:

```
0 = success
1 = pipeline execution failure
2 = config / validation error
3 = Docker / runtime error
4 = cancelled
5 = timeout
```

Run history is stored at `.forge/runs/<run-id>/` after every execution:

```
.forge/runs/<run-id>/
├── result.json    # structured execution result
├── metadata.json  # run metadata (commit, config path, os, forge version)
└── logs/          # reserved for per-step log files
```

Combine flags for advanced usage:

```bash
# Dry-run with verbose output
forge run --dry-run --verbose

# Run specific stage with verbose output
forge run --stage test --verbose
```
## Interactive TUI Dashboard

FORGE provides an interactive terminal user interface (TUI) to monitor pipeline execution in real-time.

### Running in TUI Mode
To start the pipeline with the TUI dashboard:
```bash
forge run --tui
```

### Controls
- `q` / `Q` - Quit the TUI dashboard (stops and cleans up active containers).
- `Tab` - Switch focus between the **Stages & Steps** panel and the **Logs** panel.
- `Up/Down` arrows - Navigate/Scroll the focused panel.
- `PageUp/PageDown` - Scroll logs page-by-page.
- `a` / `A` - Toggle **Auto-Scroll** for the logs panel.

## Using Secrets

Secrets are defined in the configuration file and their values are taken from environment variables:

```bash
# Option A: put env vars in a .env file (recommended)
cp .env.example .env
# edit .env and set FORGE_API_TOKEN=...

# FORGE automatically loads .env from:
# - current working directory
# - the config file directory (when using --file path/to/forge.yaml)

# Option B: export in your shell
export FORGE_API_TOKEN=your_secret_token

# Run the pipeline with the secret
forge run
```

## Common Workflows

### Development Workflow
```bash
# 1. Initialize project
forge init

# 2. Edit forge.yaml to match your project needs

# 3. Validate configuration
forge validate

# 4. Test run
forge run --verbose

# 5. Iterate and refine
```

### Testing Before CI/CD Push
```bash
# Run the same pipeline locally before pushing to GitHub/GitLab
forge run --file .github/workflows/forge.yaml
```

### Debug Mode
```bash
# Run with maximum verbosity and performance metrics
forge run --verbose --no-cache
```

### Dry-Run Mode
```bash
# Validate pipeline without executing containers
forge run --dry-run

# See what would be executed for a specific stage
forge run --dry-run --stage build
```

## Performance Monitoring

When running with `--verbose` flag, FORGE displays performance metrics:

- Configuration parsing time
- Docker connection time
- Per-stage execution time
- Total pipeline duration

Example output:
```
FORGE Pipeline Runner
  Configuration parsing completed in 0.00s
  Docker connection completed in 0.03s
Stage: build
  ...
  Stage 'build' completed in 15.42s
Pipeline completed successfully!

Total pipeline duration: 15.45s
```
