# Contributing to Iggy Sample

Thank you for considering contributing to Iggy Sample! This document provides guidelines and information for contributors.

## Code of Conduct

By participating in this project, you agree to maintain a respectful and inclusive environment for everyone.

## How to Contribute

### Reporting Issues

- Check existing issues before creating a new one
- Use the issue templates when available
- Provide clear reproduction steps for bugs
- Include relevant environment information (OS, Rust version, etc.)

### Pull Requests

1. **Fork the repository** and create your branch from `main`
2. **Follow the commit convention** (see below)
3. **Add tests** for new functionality
4. **Ensure all tests pass**: `cargo test`
5. **Run lints**: `cargo clippy -- -D warnings`
6. **Format code**: `cargo fmt`
7. **Update documentation** if needed

### Commit Convention

This project uses [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>

[optional body]

[optional footer(s)]
```

**Types:**
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation changes
- `refactor`: Code refactoring (no functional change)
- `test`: Adding or updating tests
- `chore`: Maintenance tasks
- `perf`: Performance improvements
- `ci`: CI/CD changes

**Examples:**
```
feat(messages): add batch message validation
fix(auth): handle empty API key gracefully
docs(readme): update configuration examples
```

## Development Setup

### Prerequisites

- Rust 1.90+ (edition 2024)
- Docker & Docker Compose
- cargo-deny (for license/security checks)

### Getting Started

```bash
# Clone your fork
git clone https://github.com/YOUR_USERNAME/iggy_sample.git
cd iggy_sample

# Start Iggy server
docker-compose up -d iggy

# Run tests
cargo test

# Run the application
cargo run
```

### Running Tests

```bash
# Unit tests
cargo test --lib

# Integration tests (requires Docker)
cargo test --test integration_tests

# All tests with verbose output
cargo test -- --nocapture

# Run specific test
cargo test test_health_endpoint
```

### Code Quality

This project enforces strict code quality:

```bash
# Format code
cargo fmt

# Run lints (must pass with no warnings)
cargo clippy -- -D warnings

# Check for security vulnerabilities
cargo audit

# Check licenses
cargo deny check
```

### Fuzz Testing

```bash
# Install cargo-fuzz (requires nightly)
cargo +nightly install cargo-fuzz

# Run fuzz tests
cargo +nightly fuzz run fuzz_validation -- -max_total_time=60
```

## Code Style

### Rust Guidelines

- **No `unwrap()` or `expect()` in production code** - Use proper error handling
- **Zero clippy warnings** - All lints must pass
- **Document public APIs** - Use `///` doc comments with examples
- **Explicit types** - Prefer clarity over brevity
- **Descriptive names** - Even if longer

### Error Handling

Use `thiserror` for custom error types:

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MyError {
    #[error("operation failed: {0}")]
    OperationFailed(String),
}
```

### Testing

- Write tests alongside implementation
- Test edge cases and error paths
- Use descriptive test names:

```rust
#[test]
fn test_parse_returns_error_on_empty_input() { }  // Good
#[test]
fn test_parse() { }  // Too vague
```

## Project Structure

```
src/
├── main.rs           # Entry point
├── lib.rs            # Library exports
├── config.rs         # Configuration
├── error.rs          # Error types
├── handlers/         # HTTP handlers
├── middleware/       # Axum middleware
├── models/           # Domain models
└── services/         # Business logic
```

## Questions?

- Open a [Discussion](https://github.com/mlevkov/iggy_sample/discussions) for questions
- Check existing issues for similar problems
- Review the [README](README.md) and [architecture.md](architecture.md)

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
