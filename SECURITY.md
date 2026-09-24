# Security Policy

## Supported Versions

Fixes land on the latest minor release line only.

| Version | Supported          |
| ------- | ------------------ |
| 0.4.x   | :white_check_mark: |
| < 0.4   | :x:                |

## Reporting a Vulnerability

If you discover a security vulnerability, please report it responsibly:

1. **Do not** open a public issue
2. Email the maintainer directly or use GitHub's private vulnerability reporting
3. Include:
   - Description of the vulnerability
   - Steps to reproduce
   - Potential impact
   - Suggested fix (if any)

You can expect:
- Acknowledgment within 48 hours
- Status update within 7 days
- Credit in the security advisory (unless you prefer anonymity)

## Security Measures

This project implements several security measures:

- **Constant-time comparison** for API key authentication (timing attack resistant)
- **Input validation** on all user-provided data
- **Rate limiting** to prevent abuse
- **No secrets in code** - all credentials via environment variables
- **Dependency auditing** via `cargo-deny` and `cargo-audit` in CI
- **License compliance** via `cargo-deny`

## Dependency Updates

Dependencies are monitored via:
- Dependabot version updates every Monday, for direct and transitive Cargo
  dependencies alike: one grouped PR for minor and patch updates, a
  separate PR per major bump (a pre-1.0 minor bump such as 0.8 to 0.9
  counts as major), and one grouped PR for GitHub Actions. Dependabot
  silently skips a version that needs a newer Rust than this crate's
  `rust-version` (TD-2026-07-02); the checks below still flag it
- `cargo deny --locked check` in CI, gating every pull request through the
  required `CI Success` check and also run weekly: vulnerability,
  unmaintained and unsound advisories in any crate fail it, and so does a
  yanked crate. The job starts from an empty cargo cache so the yank
  status comes from a fresh index, and an index entry it cannot read
  fails the check too
- `cargo-audit` (`rustsec/audit-check`) on every CI run. On pushes and pull
  requests a vulnerability advisory fails the job, and with it `CI
  Success`, including one in a crate that sits in `Cargo.lock` without being
  built, which the `cargo-deny` gate never sees; unsound, unmaintained and
  yanked findings only warn there. The weekly scheduled run does not fail
  on advisories: it opens a GitHub issue per new advisory instead, skipping
  any advisory ID already named in an issue or PR title, open or closed
- Manual review of security advisories

Dependabot alerts and security updates are not relied on for Rust crates:
the GitHub Advisory Database does not mirror every RustSec advisory, and it
carried none of the four fixed in 0.4.1.
