# Architecture

FORGE consists of several logical components currently implemented in a monolithic file:

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
- Validate cache integrity
- Clean up old cache entries

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
└── main.rs (monolithic - ~1000 lines)
    ├── CLI definitions (Clap)
    ├── Config structs (Serde)
    ├── Docker integration (Bollard)
    ├── Orchestration logic
    ├── Cache management
    └── Secret handling
```

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
- **Streaming**: Logs are streamed in real-time, not buffered
- **Caching**: Reduces redundant work across runs
- **Lazy Pulling**: Docker images only pulled when needed

## Security Considerations

- **Secrets**: Never logged or exposed in output
- **Container Isolation**: Each step runs in isolated container
- **Resource Limits**: Future support for CPU/memory limits
- **Volume Mounting**: Read-only by default where possible
