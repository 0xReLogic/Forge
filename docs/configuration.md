# Configuration Reference

FORGE uses YAML format for pipeline configuration. Two formats are supported:

## Basic Format

The basic format supports a list of steps that are executed sequentially.

```yaml
steps:
  - name: Install Dependencies
    command: npm install
    image: node:16-alpine
    working_dir: /workspace
    env:
      NODE_ENV: development
  - name: Run Tests
    command: npm test
    image: node:16-alpine
    working_dir: /workspace
```

## Advanced Format with Multi-Stage

The advanced format supports stages, dependencies, parallel execution, caching, and secrets.

```yaml
version: "1.0"
stages:
  - name: build
    steps:
      - name: Install Dependencies
        command: npm install
        image: node:16-alpine
        working_dir: /workspace
    parallel: false
  - name: test
    steps:
      - name: Run Tests
        command: npm test
        image: node:16-alpine
        working_dir: /workspace
    depends_on:
      - build
cache:
  enabled: true
  directories:
    - /workspace/node_modules
secrets:
  - name: API_TOKEN
    env_var: FORGE_API_TOKEN
```

## Configuration Properties

### Pipeline Properties

| Property | Description | Required | Default |
|----------|-------------|---------|---------|
| `version` | Configuration format version | No | `"1.0"` |
| `fail_fast` | Stop pipeline on first unhandled stage failure. If `false`, non-dependent stages continue executing | No | `true` |
| `stages` | List of stages to execute | No | `[]` |
| `steps` | List of steps (shorthand for single-stage pipelines) | No | `[]` |
| `cache` | Directory caching configuration | No | `{}` |
| `secrets` | Secret environment variable bindings | No | `[]` |

### Step Properties

| Property | Description | Required | Default |
|----------|-------------|---------|---------|
| `name` | Step name | No | `""` |
| `command` | Command to run | Yes | - |
| `image` | Docker image to use | No | `alpine:latest` |
| `working_dir` | Working directory inside the container | No | `/workspace` |
| `env` | Environment variables | No | `{}` |
| `timeout` | Step execution timeout (e.g. `30s`, `2m`, `1h`, `45`) | No | `None` |
| `retries` | Number of retry attempts on step failure | No | `0` |
| `retry_delay` | Delay between retries (e.g. `5s`, `10s`, `5`) | No | `0s` |
| `continue_on_error` | Continue stage execution even if this step fails | No | `false` |
| `artifacts` | Artifacts to upload or download in this step | No | `None` |

### Stage Properties

| Property | Description | Required | Default |
|----------|-------------|---------|---------|
| `name` | Stage name | Yes | - |
| `steps` | Steps in the stage | Yes | - |
| `parallel` | Whether steps are executed in parallel | No | `false` |
| `depends_on` | Dependencies on other stages | No | `[]` |
| `continue_on_error` | Continue subsequent stages even if this stage fails | No | `false` |
| `artifacts` | Artifacts to upload or download in this stage | No | `None` |

### Artifact Properties

Artifacts can be uploaded and downloaded between stages or steps. Uploaded artifacts are persisted under `.forge/runs/<run-id>/artifacts/` and tracked in `manifest.json`.

```yaml
artifacts:
  upload:
    - path: target/release/forge
      name: binary # optional
    - "dist/"      # simple path shorthand
  download:
    - name: binary
      path: bin/forge # optional target destination
```

### Exit Codes

| Exit Code | Name | Description |
|-----------|------|-------------|
| `0` | `success` | Pipeline completed successfully |
| `1` | `pipeline_failure` | Step or stage returned non-zero |
| `2` | `config_error` | Invalid YAML, missing field, or validation failure |
| `3` | `runtime_error` | Docker daemon error or container runtime issue |
| `4` | `cancelled` | User interrupted pipeline (Ctrl+C / SIGINT) |
| `5` | `timeout` | Step exceeded configured timeout |

### Cache Properties

| Property | Description | Required | Default |
|----------|-------------|---------|---------|
| `enabled` | Whether caching is enabled | No | `false` |
| `directories` | Directories to cache | No | `[]` |

### Secret Properties

| Property | Description | Required | Default |
|----------|-------------|---------|---------|
| `name` | Secret name | Yes | - |
| `env_var` | Environment variable name on the host | Yes | - |

## Multi-Stage Execution

FORGE supports multi-stage pipelines where stages can depend on each other. This allows you to create complex workflows with dependencies:

1. **Sequential Execution**: Stages run in order based on their dependencies
2. **Parallel Steps**: Steps within a stage can run in parallel if `parallel: true` is set
3. **Dependency Management**: Use `depends_on` to specify which stages must complete before a stage can start
4. **Automatic Ordering**: FORGE automatically resolves dependencies using topological sorting

### Dependency Validation

FORGE validates all stage dependencies before execution:

- **Missing dependencies**: Error if a stage depends on a non-existent stage
- **Self-dependencies**: Error if a stage depends on itself
- **Circular dependencies**: Error if stages form a dependency cycle (A → B → A)

Use `--dry-run` to validate dependencies without executing:

```bash
forge run --file forge.yaml --dry-run
```

Example output:
```
Execution order:
  1. setup (no dependencies)
  2. build (depends on: setup)
  3. test (depends on: build)
  4. deploy (depends on: test)
```

Example of a multi-stage pipeline:

```yaml
stages:
  - name: setup
    steps:
      - name: Install Dependencies
        command: npm install
    parallel: false
  
  - name: test
    steps:
      - name: Unit Tests
        command: npm test
      - name: Integration Tests
        command: npm run test:integration
    parallel: true
    depends_on:
      - setup
  
  - name: build
    steps:
      - name: Build Application
        command: npm run build
    depends_on:
      - test
```

## Parallel Execution

Steps within a stage can run simultaneously by setting `parallel: true`. This dramatically reduces pipeline execution time when you have independent tasks.

### Basic Example

```yaml
version: "1.0"
stages:
  - name: tests
    parallel: true
    steps:
      - name: Unit Tests
        image: node:18-alpine
        command: npm run test:unit
      
      - name: Integration Tests
        image: node:18-alpine
        command: npm run test:integration
      
      - name: Lint Code
        image: node:18-alpine
        command: npm run lint
```

**Result**: All three tasks run simultaneously instead of sequentially (3× faster).

### Multi-Language Build

```yaml
version: "1.0"
stages:
  - name: build
    parallel: true
    steps:
      - name: Build Frontend
        image: node:18-alpine
        command: cd frontend && npm install && npm run build
      
      - name: Build Backend
        image: golang:1.21-alpine
        command: cd backend && go build -o app
      
      - name: Build Python Service
        image: python:3.11-alpine
        command: cd service && pip install -r requirements.txt
```

### Requirements

- Parallel stages must have **at least 2 steps**
- All steps must have **unique names**
- Steps do not support `depends_on` (use stage-level `depends_on` instead)

### Validation

Check your configuration before running:

```bash
forge validate --file forge.yaml
```

## Caching

FORGE provides a caching mechanism to speed up your pipelines by preserving files between runs:

1. **Directory Caching**: Specify directories to cache between steps and stages
2. **Automatic Management**: FORGE automatically handles copying files to and from the cache
3. **Cache Control**: Enable or disable caching via configuration or command line flags

Cache storage:

- The cache is stored inside your project at `./.forge/cache/`.
- FORGE automatically derives a cache key from common lockfiles (e.g. `Cargo.lock`, `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`, `go.sum`), so different dependency states use different cache folders.
- To reset cache for a project, delete the `.forge/` directory.

Example of cache configuration:

```yaml
cache:
  enabled: true
  directories:
    - /workspace/node_modules
    - /workspace/.cache
    - /workspace/dist
```

You can also control caching from the command line:

```bash
# Force enable caching
forge run --cache

# Force disable caching
forge run --no-cache
```
