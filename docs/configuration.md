# Configuration Reference

FORGE uses YAML format for pipeline configuration. Two formats are supported:

## Basic Format

The basic format supports a list of steps that are executed sequentially.

```yaml
steps:
  - name: Install Dependencies
    command: npm install
    image: node:16-alpine
    working_dir: /app
    env:
      NODE_ENV: development
  - name: Run Tests
    command: npm test
    image: node:16-alpine
    working_dir: /app
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
        working_dir: /app
    parallel: false
  - name: test
    steps:
      - name: Run Tests
        command: npm test
        image: node:16-alpine
        working_dir: /app
    depends_on:
      - build
cache:
  enabled: true
  directories:
    - /app/node_modules
secrets:
  - name: API_TOKEN
    env_var: FORGE_API_TOKEN
```

## Configuration Properties

### Step Properties

| Property | Description | Required | Default |
|----------|-------------|---------|---------|
| `name` | Step name | No | `""` |
| `command` | Command to run | Yes | - |
| `image` | Docker image to use | No | `alpine:latest` |
| `working_dir` | Working directory inside the container | No | `""` |
| `env` | Environment variables | No | `{}` |
| `depends_on` | Dependencies on other steps | No | `[]` |

### Stage Properties

| Property | Description | Required | Default |
|----------|-------------|---------|---------|
| `name` | Stage name | Yes | - |
| `steps` | Steps in the stage | Yes | - |
| `parallel` | Whether steps are executed in parallel | No | `false` |
| `depends_on` | Dependencies on other stages | No | `[]` |

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
- Steps cannot have `depends_on` (they run simultaneously)

### Validation

Check your configuration before running:

```bash
forge-cli validate --file forge.yaml
```

## Caching

FORGE provides a caching mechanism to speed up your pipelines by preserving files between runs:

1. **Directory Caching**: Specify directories to cache between steps and stages
2. **Automatic Management**: FORGE automatically handles copying files to and from the cache
3. **Cache Control**: Enable or disable caching via configuration or command line flags

Example of cache configuration:

```yaml
cache:
  enabled: true
  directories:
    - /app/node_modules
    - /app/.cache
    - /app/dist
```

You can also control caching from the command line:

```bash
# Force enable caching
forge-cli run --cache

# Force disable caching
forge-cli run --no-cache
```
