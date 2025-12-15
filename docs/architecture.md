# Architecture

FORGE consists of several logical components currently implemented in a small set of modules:

## Components

### 1. YAML Parser
Reads and validates configuration files. Supports both basic and advanced formats with schema validation.

**Responsibilities:**
- Parse YAML configuration
- Validate structure and required fields
- Convert to internal data structures

### 2. Orchestrator
Manages pipeline execution, including dependencies and parallelism.

**Responsibilities:**
- Build execution graph from stages/steps
- Resolve dependencies
- Schedule parallel execution
- Handle stage ordering

### 3. Docker Client
Interacts with Docker API to run containers.

**Responsibilities:**
- Create and manage containers
- Pull Docker images
- Mount volumes and set environment variables
- Stream logs from containers

### 4. Logger
Handles log streaming from containers to the terminal.

**Responsibilities:**
- Stream container output in real-time
- Apply color coding for readability
- Show progress indicators
- Format error messages

### 5. Cache Manager
Manages directory caching to speed up builds.

**Responsibilities:**
- Identify cacheable directories
- Copy files to/from cache location
- Manage cached directory restore/save during step execution

### 6. Secret Manager
Securely manages secrets.

**Responsibilities:**
- Read secrets from environment variables
- Inject secrets into containers
- Ensure secrets are not logged
- Validate secret availability

## Current Structure

```
src/
├── main.rs
│   ├── CLI definitions (Clap)
│   ├── Config structs (Serde)
│   ├── Orchestration logic
│   └── Stage/step execution
└── container.rs
    ├── Docker integration (Bollard)
    ├── Mounts (/forge-shared, /workspace, /forge-cache)
    ├── Log streaming
    └── Container cleanup
```

Cache details:

- Containers see the cache at `/forge-cache`.
- On the host, cache is stored repo-locally under `./.forge/cache/<cache_key>/`.
- The cache key is derived from common lockfiles so different dependency states use different cache folders.

## Future Structure (Planned Refactoring)

```
src/
├── main.rs (CLI entry point)
├── lib.rs (Public API)
├── config/
│   ├── mod.rs
│   ├── parser.rs
│   └── validator.rs
├── docker/
│   ├── mod.rs
│   ├── client.rs
│   └── image.rs
├── runner/
│   ├── mod.rs
│   ├── orchestrator.rs
│   ├── executor.rs
│   └── graph.rs
├── cache/
│   ├── mod.rs
│   └── manager.rs
├── secrets/
│   ├── mod.rs
│   └── manager.rs
└── logger/
    ├── mod.rs
    └── formatter.rs
```

## Design Principles

1. **Modularity**: Each component should be independently testable
2. **Separation of Concerns**: Clear boundaries between components
3. **Error Handling**: Comprehensive error types with context
4. **Async by Default**: Use Tokio for async operations
5. **Type Safety**: Leverage Rust's type system for correctness

## Data Flow

```
YAML Config
    ↓
Parser & Validator
    ↓
Orchestrator (builds execution graph)
    ↓
Cache Manager (restore cached dirs)
    ↓
Docker Client (create containers)
    ↓
Executor (run steps/stages)
    ↓
Logger (stream output)
    ↓
Cache Manager (save to cache)
    ↓
Results & Cleanup
```

## Technology Stack

- **Language**: Rust (Edition 2024)
- **CLI**: Clap 4.x
- **Async Runtime**: Tokio
- **Docker API**: Bollard
- **Serialization**: Serde + serde_yaml
- **Terminal UI**: colored, indicatif

## Performance Considerations

- **Parallel Execution**: Steps within a stage can run concurrently
- **Streaming**: Logs are streamed in real-time for sequential steps; for parallel stages, logs are buffered and printed in definition order
- **Caching**: Reduces redundant work across runs
- **Lazy Pulling**: Docker images only pulled when needed

## Security Considerations

- **Secrets**: Secret values are not printed by FORGE in verbose environment listings (they are masked), but they can still be exposed if user commands echo them
- **Container Isolation**: Each step runs in isolated container
- **Resource Limits**: Future support for CPU/memory limits
- **Volume Mounting**: The host project directory is mounted into containers at `/workspace`
