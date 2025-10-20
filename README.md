# FORGE - Local CI/CD Runner

<div align="center">
  <!-- Placeholder for logo, will be added later -->
  <p><strong>Fast, Offline, Reliable, Go-anywhere Execution</strong></p>
  <p>
    <a href="https://github.com/0xReLogic/Forge/actions/workflows/ci.yml">
      <img src="https://github.com/0xReLogic/Forge/actions/workflows/ci.yml/badge.svg" alt="FORGE CI">
    </a>
    <a href="https://opensource.org/licenses/MIT">
      <img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT">
    </a>
    <a href="https://github.com/0xReLogic/Forge/issues?q=is%3Aissue+is%3Aopen+label%3Ahacktoberfest">
      <img src="https://img.shields.io/badge/Hacktoberfest-Friendly-orange" alt="Hacktoberfest">
    </a>
  </p>
</div>

## About FORGE

FORGE is a lightweight local CI/CD tool built with Rust that allows you to run automation pipelines on your local machine. It's extremely useful for developing and testing pipelines before pushing them to larger CI/CD systems.

### Why FORGE?

- **Local & Fast**: Run pipelines on your local machine without waiting for CI/CD servers
- **Offline-First**: Works without an internet connection (as long as Docker images are available)
- **Compatible**: Syntax similar to GitHub Actions and GitLab CI
- **Lightweight**: Minimal resource consumption
- **Portable**: Runs on Windows, macOS, and Linux

## Features

- Run CI/CD pipelines from simple YAML files
- Isolation using Docker containers
- Support for various Docker images
- Real-time log streaming with colors
- Environment variables management
- Intuitive command-line interface
- Multi-stage pipelines with parallel execution
- Caching to speed up builds
- Secure secrets management
- Dependencies between steps and stages

## Quick Start

### Installation

```bash
# With Cargo
cargo install forge-cli

# Or from source
git clone https://github.com/0xReLogic/Forge.git
cd forge
cargo build --release
```

**Prerequisites**: [Rust](https://www.rust-lang.org/tools/install) (1.70+) and [Docker](https://docs.docker.com/get-docker/) (20.10+)

For detailed installation instructions, see [docs/installation.md](docs/installation.md).

### Usage

```bash
# Initialize a project
forge-cli init

# Validate configuration
forge-cli validate

# Run the pipeline
forge-cli run
```

For more commands and options, see [docs/usage.md](docs/usage.md).

## Documentation

- **[Installation Guide](docs/installation.md)** - Detailed installation instructions for all platforms
- **[Usage Guide](docs/usage.md)** - Complete command reference and workflows
- **[Configuration Reference](docs/configuration.md)** - YAML format and all configuration options
- **[Examples](docs/examples.md)** - Real-world examples for different tech stacks
- **[Architecture](docs/architecture.md)** - System design and component details

## Quick Example

```yaml
version: "1.0"
stages:
  - name: build
    steps:
      - name: Install Dependencies
        command: npm install
        image: node:16-alpine
        working_dir: /app
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
```

More examples in [docs/examples.md](docs/examples.md) and [examples/](examples/) directory.

## Contributing

Thank you for your great contributions!

<div align="center">
  <img src="https://contrib.rocks/image?repo=0xReLogic/Forge" alt="Contributors" />
</div>

**Good First Issues**: Check out our [good first issue](https://github.com/0xReLogic/Forge/labels/good%20first%20issue) label for beginner-friendly tasks.

**How to Contribute**:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

Read [CONTRIBUTING.md](CONTRIBUTING.md) for detailed guidelines.

## License

This project is licensed under the [MIT License](LICENSE).

## Credits

Inspired by [GitHub Actions](https://github.com/features/actions), [GitLab CI](https://docs.gitlab.com/ee/ci/), and [Jenkins](https://www.jenkins.io/).
