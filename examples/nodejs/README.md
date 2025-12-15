# Node.js Example

This example demonstrates how to use FORGE with a typical Node.js project.

## Configuration

The `forge.yaml` file sets up a multi-stage pipeline:

1. **setup**: Install npm dependencies
2. **test**: Run linting and unit tests in parallel
3. **build**: Build the application for production

## Features Demonstrated

- Multi-stage pipeline
- Parallel execution (lint and test run simultaneously)
- Stage dependencies (build depends on test)
- Caching (node_modules is cached between runs)
- Environment variables (NODE_ENV for production build)

## Usage

```bash
# From the nodejs directory
forge run --file forge.yaml

# Or from project root
forge run --file examples/nodejs/forge.yaml
```

## Expected Project Structure

```
your-nodejs-project/
├── package.json
├── package-lock.json
├── src/
├── tests/
└── forge.yaml
```

## Customization

Modify the `forge.yaml` to match your project:
- Change Node.js version (e.g., `node:18-alpine`, `node:20-alpine`)
- Add more test stages (e2e, integration tests)
- Add deployment stages
- Configure additional environment variables
