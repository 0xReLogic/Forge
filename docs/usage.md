# Usage Guide

## Quick Start

```bash
# Initialize a project with an example configuration file
forge-cli init

# Validate the configuration
forge-cli validate

# Run the pipeline
forge-cli run
```

## Commands

### Project Initialization

Create a new forge.yaml configuration file:

```bash
forge-cli init
```

Or with a different filename:

```bash
forge-cli init --file custom-forge.yaml
```

Use the `--force` flag to overwrite an existing file:

```bash
forge-cli init --force
```

### Configuration Validation

Validate a configuration file:

```bash
forge-cli validate
```

Or with a different configuration file:

```bash
forge-cli validate --file custom-forge.yaml
```

### Run Pipeline

Run the pipeline:

```bash
forge-cli run
```

Or with a different configuration file:

```bash
forge-cli run --file custom-forge.yaml
```

Run with verbose output (includes performance metrics):

```bash
forge-cli run --verbose
```

Validate pipeline without execution (dry-run mode):

```bash
forge-cli run --dry-run
```

Run a specific stage:

```bash
forge-cli run --stage build
```

Enable or disable caching:

```bash
forge-cli run --cache
forge-cli run --no-cache
```

Combine flags for advanced usage:

```bash
# Dry-run with verbose output
forge-cli run --dry-run --verbose

# Run specific stage with verbose output
forge-cli run --stage test --verbose
```

## Using Secrets

Secrets are defined in the configuration file and their values are taken from environment variables:

```bash
# Set the secret value in the environment
export FORGE_API_TOKEN=your_secret_token

# Run the pipeline with the secret
forge-cli run
```

## Common Workflows

### Development Workflow
```bash
# 1. Initialize project
forge-cli init

# 2. Edit forge.yaml to match your project needs

# 3. Validate configuration
forge-cli validate

# 4. Test run
forge-cli run --verbose

# 5. Iterate and refine
```

### Testing Before CI/CD Push
```bash
# Run the same pipeline locally before pushing to GitHub/GitLab
forge-cli run --file .github/workflows/forge.yaml
```

### Debug Mode
```bash
# Run with maximum verbosity and performance metrics
forge-cli run --verbose --no-cache
```

### Dry-Run Mode
```bash
# Validate pipeline without executing containers
forge-cli run --dry-run

# See what would be executed for a specific stage
forge-cli run --dry-run --stage build
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
