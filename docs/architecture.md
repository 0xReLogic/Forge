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

### 7. Interactive Terminal UI Dashboard (TUI)
Provides a rich, interactive real-time dashboard displaying execution duration, stage/step status trees, and step log buffers.

**Responsibilities:**
- Manage TUI rendering loop (via `ratatui` and `crossterm`)
- Handle keyboard input (step selection, log scrolling, auto-scroll toggling, abort execution)
- Update visual status indicators for running, successful, failed, and pending items
- Suppress stdout logging during active TUI dashboard execution


## Current Structure

```
src/
├── main.rs          # CLI entry point (parses arguments and runs the runner)
├── lib.rs           # Public library module declarations
├── config/          # Configurations parsing and validation
│   └── mod.rs
├── docker/          # Docker client integration (Bollard wrapper)
│   └── mod.rs
├── runner/          # Graph-based stage execution & orchestrator
│   ├── mod.rs
│   └── monitor.rs   # Pipeline Monitor event interface & StdoutMonitor
├── cache/           # Caching manager
│   └── mod.rs
├── secrets/         # Secrets environment collector
│   └── mod.rs
├── logger/          # Timer, LogBuffer & log streaming
│   └── mod.rs
└── tui.rs           # Interactive Terminal UI dashboard & drawing logic
```

Cache details:

- Containers see the cache at `/forge-cache`.
- On the host, cache is stored repo-locally under `./.forge/cache/<cache_key>/`.
- The cache key is derived from common lockfiles so different dependency states use different cache folders.

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
- **Terminal UI**: colored, indicatif, ratatui, crossterm

## Performance Considerations

- **Parallel Execution**: Steps within a stage can run concurrently
- **Streaming**: Logs are streamed in real-time to a `PipelineMonitor` event handler. The standard `StdoutMonitor` handles sequential real-time printing and parallel definitions buffering, while the `TuiMonitor` updates a thread-safe dashboard state for real-time visualization.
- **Caching**: Reduces redundant work across runs
- **Lazy Pulling**: Docker images only pulled when needed

## Security Considerations

- **Secrets**: Secret values are not printed by FORGE in verbose environment listings (they are masked), but they can still be exposed if user commands echo them
- **Container Isolation**: Each step runs in isolated container
- **Resource Limits**: Future support for CPU/memory limits
- **Volume Mounting**: The host project directory is mounted into containers at `/workspace`
