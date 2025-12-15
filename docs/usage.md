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

Combine flags for advanced usage:

```bash
# Dry-run with verbose output
forge run --dry-run --verbose

# Run specific stage with verbose output
forge run --stage test --verbose
```

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
