# Examples

## Node.js Project

```yaml
version: "1.0"
stages:
  - name: setup
    steps:
      - name: Install Dependencies
        command: npm install
        image: node:16-alpine
        working_dir: /workspace
    parallel: false
  - name: test
    steps:
      - name: Lint
        command: npm run lint
        image: node:16-alpine
        working_dir: /workspace
      - name: Unit Tests
        command: npm test
        image: node:16-alpine
        working_dir: /workspace
    parallel: true
    depends_on:
      - setup
  - name: build
    steps:
      - name: Build
        command: npm run build
        image: node:16-alpine
        working_dir: /workspace
        env:
          NODE_ENV: production
    depends_on:
      - test
cache:
  enabled: true
  directories:
    - /workspace/node_modules
```

## Rust Project

```yaml
version: "1.0"
stages:
  - name: check
    steps:
      - name: Cargo Check
        command: cargo check
        image: rust:1.85-slim
        working_dir: /workspace
  - name: test
    steps:
      - name: Cargo Test
        command: cargo test
        image: rust:1.85-slim
        working_dir: /workspace
    depends_on:
      - check
  - name: build
    steps:
      - name: Cargo Build
        command: cargo build --release
        image: rust:1.85-slim
        working_dir: /workspace
    depends_on:
      - test
cache:
  enabled: true
  directories:
    - /workspace/target
    - /usr/local/cargo/registry
```

## Python Project

```yaml
version: "1.0"
stages:
  - name: setup
    steps:
      - name: Install Dependencies
        command: pip install -r requirements.txt
        image: python:3.11-slim
        working_dir: /workspace
    parallel: false
  - name: test
    steps:
      - name: Run Tests
        command: pytest
        image: python:3.11-slim
        working_dir: /workspace
      - name: Lint
        command: pylint src/
        image: python:3.11-slim
        working_dir: /workspace
    parallel: true
    depends_on:
      - setup
cache:
  enabled: true
  directories:
    - /workspace/.venv
    - /root/.cache/pip
```

## Go Project

```yaml
version: "1.0"
stages:
  - name: build
    steps:
      - name: Go Build
        command: go build -v ./...
        image: golang:1.21-alpine
        working_dir: /workspace
  - name: test
    steps:
      - name: Go Test
        command: go test -v ./...
        image: golang:1.21-alpine
        working_dir: /workspace
    depends_on:
      - build
cache:
  enabled: true
  directories:
    - /go/pkg/mod
```

## Multi-Language Monorepo

```yaml
version: "1.0"
stages:
  - name: frontend
    steps:
      - name: Install Frontend Deps
        command: npm install
        image: node:16-alpine
        working_dir: /workspace/frontend
      - name: Build Frontend
        command: npm run build
        image: node:16-alpine
        working_dir: /workspace/frontend
    parallel: false
  
  - name: backend
    steps:
      - name: Install Backend Deps
        command: pip install -r requirements.txt
        image: python:3.11-slim
        working_dir: /workspace/backend
      - name: Run Backend Tests
        command: pytest
        image: python:3.11-slim
        working_dir: /workspace/backend
    parallel: false
  
  - name: integration
    steps:
      - name: Integration Tests
        command: npm run test:e2e
        image: node:16-alpine
        working_dir: /workspace
    depends_on:
      - frontend
      - backend
cache:
  enabled: true
  directories:
    - /workspace/frontend/node_modules
    - /workspace/backend/.venv
```

## Using Secrets

```yaml
version: "1.0"
stages:
  - name: deploy
    steps:
      - name: Deploy to Server
        command: ./deploy.sh
        image: alpine:latest
        working_dir: /workspace
        env:
          DEPLOY_ENV: production
secrets:
  - name: API_TOKEN
    env_var: FORGE_API_TOKEN
  - name: SSH_KEY
    env_var: FORGE_SSH_KEY
```

Set secrets before running:
```bash
export FORGE_API_TOKEN=your_token_here
export FORGE_SSH_KEY=your_ssh_key_here
forge run
```

## More Examples

Check the [examples/](../examples/) directory for more complete examples with actual project files:

- **[examples/go/](../examples/go/)** - Go project with tests
- **[examples/java/](../examples/java/)** - Java project with javac
- **[examples/nodejs/](../examples/nodejs/)** - Node.js project example
- **[examples/python/](../examples/python/)** - Python project with pytest
- **[examples/dotnet/](../examples/dotnet/)** - C#/.NET project
- **[examples/rust/](../examples/rust/)** - Rust project example
- **[examples/dependency-example.yaml](../examples/dependency-example.yaml)** - Stage dependencies example
