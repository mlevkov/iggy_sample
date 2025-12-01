# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

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
- **Dependency auditing** via `cargo-audit` in CI
- **License compliance** via `cargo-deny`

## Dependency Updates

Dependencies are monitored via:
- Dependabot (weekly updates)
- `cargo-audit` in CI pipeline
- Manual review of security advisories
