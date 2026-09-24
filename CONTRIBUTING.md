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
<type>(<scope>)!: <description>

[optional body]

[optional footer(s)]
```

The scope and the `!` are both optional. Use `!` to mark a breaking change —
CI checks only the subject line, so a `BREAKING CHANGE:` footer alone will not
be recognized (and would not appear in the generated release notes either).

**Types:**
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation changes
- `style`: Formatting only, no code change
- `refactor`: Code refactoring (no functional change)
- `test`: Adding or updating tests
- `chore`: Maintenance tasks
- `perf`: Performance improvements
- `ci`: CI/CD changes
- `revert`: Reverting a previous commit

**Examples:**
```
feat(messages): add batch message validation
fix(auth): handle empty API key gracefully
docs(readme): update configuration examples
refactor(circuit-breaker)!: narrow the public surface
```

Keep the subject line at 72 characters or fewer (`.commitlintrc.json`).

## Development Setup

### Prerequisites

- Rust 1.93+ (edition 2024; the MSRV in `Cargo.toml`)
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

# Check dependency policy (advisories, bans, licenses, sources)
cargo deny --locked check
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

## Releasing

Pushing a `vX.Y.Z` tag (or `vX.Y.Z-<suffix>` for a pre-release) runs
`.github/workflows/release.yml`, which builds the binaries, creates the GitHub
Release and deploys the docs to GitHub Pages; only admins can create tags.
Before tagging, set `version` in `Cargo.toml` (the validate job requires the
tag to match it), move the `[Unreleased]` changelog entries under the new
version, and resolve or re-bind every record in `docs/tech-debt/` whose
trigger is the next release.

A green run does not mean every step worked, since a step under
`continue-on-error` fails silently. So `.github/workflows/verify-release.yml`
runs `scripts/verify-release.sh` after every green Release run; until
TD-2026-09-04 is resolved it fails on every stable release, by design. To
check a run by hand:

```bash
scripts/verify-release.sh v0.4.2
```

It checks the tag's newest run at its latest attempt; `VERIFY_RUN_ID` and
`VERIFY_RUN_ATTEMPT` pick another run of the tag or an earlier attempt.

To exercise a `release.yml` change before a real release, push a pre-release
tag of the current `Cargo.toml` version, such as `v0.4.1-ci.1` (the validate
job compares only the part before the hyphen), and verify its run the same
way. Two things outlive the exercise:

- The docs job deploys the tagged commit's docs to Pages even for a
  pre-release, and deleting the tag does not undo that. To restore the latest
  stable release's docs, re-run the Deploy Documentation job of its Release
  run (a new attempt that succeeds runs Verify Release again), with two
  limits:
  - GitHub re-runs jobs only within 30 days of a run. Past that, tag the
    stable release's commit as a pre-release instead
    (`git tag v0.4.1-docs.1 'v0.4.1^{}'`) and push it: its run deploys that
    commit's docs. Delete the tag afterwards like any exercise tag.
  - deploy-pages refuses a run that holds more than one `github-pages`
    artifact: v0.2.0's second re-run failed with `Multiple artifacts named
    "github-pages"` beside the first attempt's, which had not expired yet
    (whether an expired one also counts is untested). If that happens,
    delete the run's `github-pages` artifacts and re-run the job again. `gh api repos/mlevkov/iggy_sample/actions/runs/<run-id>/artifacts`
    lists them, and
    `gh api -X DELETE repos/mlevkov/iggy_sample/actions/artifacts/<id>`
    deletes one.
- The tag. Delete it with `gh release delete v0.4.1-ci.1 --cleanup-tag --yes`
  from your clone (this also deletes the local tag). Left behind, it becomes
  where `release.yml` starts the changelog it writes for the next release
  (`git describe --tags`), which drops every commit before it.

## Questions?

- Open a [Discussion](https://github.com/mlevkov/iggy_sample/discussions) for questions
- Check existing issues for similar problems
- Review the [README](README.md) and [architecture.md](architecture.md)

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
