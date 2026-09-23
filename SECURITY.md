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
- Dependabot version updates every Monday: one grouped PR for minor/patch
  Cargo updates, a separate PR per major bump, and one grouped PR for
  GitHub Actions
- `cargo deny check` in CI, gating every pull request through the required
  `CI Success` check and also run weekly: vulnerability and unmaintained
  advisories fail it, yanked crates only warn, and unsound advisories are
  checked for direct dependencies only
- `cargo-audit` (`rustsec/audit-check`), whose weekly scheduled run opens a
  GitHub issue for each new advisory, including those the `cargo-deny`
  gate lets through and crates that sit in `Cargo.lock` without being built
- Manual review of security advisories
