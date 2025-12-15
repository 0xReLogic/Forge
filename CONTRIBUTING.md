# Contribution Guidelines

Thank you for considering contributing to FORGE! This project is open to contributions from anyone, whether it's fixing bugs, adding features, or improving documentation.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [How to Contribute](#how-to-contribute)
  - [Reporting Bugs](#reporting-bugs)
  - [Suggesting Features](#suggesting-features)
  - [Pull Requests](#pull-requests)
- [Development Guidelines](#development-guidelines)
  - [Environment Setup](#environment-setup)
  - [Code Structure](#code-structure)
  - [Coding Conventions](#coding-conventions)
  - [Testing](#testing)
- [Release Process](#release-process)
- [Communication](#communication)

## Code of Conduct

This project and all participants are expected to uphold a code of conduct. We expect all contributors to:

- Show respect and empathy towards other contributors
- Accept constructive criticism gracefully
- Focus on what's best for the community
- Show empathy towards other community members

## How to Contribute

### Reporting Bugs

Bugs are part of any software project, and we appreciate your reports!

1. Use the issue template for bug reports
2. Explain the steps to reproduce the bug
3. Include information about your environment (OS, Rust version, Docker version)
4. If possible, include error logs or screenshots

### Suggesting Features

We're always open to new ideas!

1. Use the issue template for feature requests
2. Describe the feature you want in detail
3. Explain why this feature would be useful to other users
4. If possible, provide examples of how this feature would work

### Pull Requests

1. Fork the repository
2. Create a new branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

To ensure your PR is accepted quickly, make sure:

- Your code follows the project's coding conventions
- All tests pass
- You've added tests for new features or bug fixes
- Documentation has been updated if necessary
- Your commit messages are clear and descriptive

## Development Guidelines

### Environment Setup

1. Install Rust (1.91.0 or newer)
2. Install Docker (20.10.0 or newer)
3. Clone the repository
4. Run `cargo build` to build the project

### Code Structure

The FORGE project is currently implemented as a small set of modules:

- `src/main.rs`: CLI entry point, configuration parsing/validation, and orchestration
- `src/container.rs`: Docker integration (container setup, mounts, logs, cleanup)

In future development, the code will be refactored into separate modules:
- Configuration module for parsing and validation
- Docker module for Docker API interaction
- Runner module for pipeline orchestration
- Cache module for cache management
- Secrets module for secrets management

### Coding Conventions

- Use `rustfmt` to format code
- Use `clippy` for static analysis
- Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- Add documentation for public functions and structs
- Use proper error handling with `Result` and the `?` operator
- Avoid using `unwrap()` and `expect()` except in tests

### Testing

We use several types of tests:

1. **Unit Tests**: Tests for individual functions and components
2. **Integration Tests**: Tests for interactions between components
3. **End-to-End Tests**: Tests that run a complete pipeline

To run all tests:

```bash
cargo test
```

To run a specific test:

```bash
cargo test <test_name>
```

## Release Process

FORGE follows [Semantic Versioning](https://semver.org/):

- **MAJOR**: Changes that are not backward compatible with previous versions
- **MINOR**: Feature additions that are backward compatible
- **PATCH**: Bug fixes that are backward compatible

Release process:

1. Update version in `Cargo.toml`
2. Update CHANGELOG.md
3. Create a git tag with the new version
4. Build and publish to crates.io
5. Create a GitHub release with pre-compiled binaries

## Communication

- **Issues**: For reporting bugs or suggesting features
- **Discussions**: For general discussion and questions
- **Pull Requests**: For proposing code changes

## Priority Areas for Contribution

We're particularly interested in contributions in these areas:

1. **Code Refactoring**: Help us break down the monolithic codebase into modular components
2. **Test Coverage**: Add more unit and integration tests
3. **Documentation**: Improve examples and usage documentation
4. **Error Handling**: Enhance error messages and recovery mechanisms
5. **Performance**: Optimize pipeline execution and resource usage

See the [Roadmap](README.md#roadmap) in the README for more details on our development plans.

## Thank You!

Thank you for contributing to FORGE! Your contributions are greatly appreciated and help make this project better for everyone.