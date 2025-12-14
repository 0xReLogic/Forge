# FORGE Examples

This directory contains example configurations for different project types and use cases.

## Available Examples

### By Language/Framework

- **[go/](go/)** - Go project with go test
- **[java/](java/)** - Java project with javac
- **[nodejs/](nodejs/)** - Node.js/JavaScript project with npm
- **[python/](python/)** - Python project with pytest and flake8
- **[dotnet/](dotnet/)** - C#/.NET project with dotnet CLI
- **[rust/](rust/)** - Rust project with Cargo

### General Examples

- **[forge.yaml](forge.yaml)** - Basic example configuration
- **[multi-stage-test.yaml](multi-stage-test.yaml)** - Multi-stage pipeline demonstration
- **[test-forge.yaml](test-forge.yaml)** - Simple test configuration
- **[test-forge-v2.yaml](test-forge-v2.yaml)** - Advanced test configuration
- **[test-forge-v3.yaml](test-forge-v3.yaml)** - Complex workflow example
- **[real-test.yaml](real-test.yaml)** - Real-world scenario

## Using Examples

To run an example:

```bash
# Navigate to the example directory
cd examples/nodejs

# Run with FORGE
forge-cli run --file forge.yaml
```

Or run from the project root:

```bash
forge-cli run --file examples/nodejs/forge.yaml
```

## Contributing Examples

Have a cool FORGE configuration? Share it!

1. Create a new directory for your example (e.g., `examples/python/`)
2. Add your `forge.yaml` configuration
3. Include a `README.md` explaining the example
4. Submit a pull request

Examples we'd love to see:
- PHP projects (Laravel, Symfony)
- Ruby projects (Rails)
- Multi-language monorepos
- Database migrations
- Docker Compose integration
- Cloud deployment workflows

## Learning Path

Start with simple examples and progress to more complex ones:

1. **[forge.yaml](forge.yaml)** - Basic single-stage pipeline
2. **[nodejs/forge.yaml](nodejs/forge.yaml)** - Multi-stage with dependencies
3. **[multi-stage-test.yaml](multi-stage-test.yaml)** - Parallel execution
4. **[test-forge-v3.yaml](test-forge-v3.yaml)** - Advanced features

## Need Help?

- Check the [main documentation](../docs/)
- Open an [issue](https://github.com/0xReLogic/Forge/issues)
- Read [CONTRIBUTING.md](../CONTRIBUTING.md)
