# Security Policy

## Supported Versions

Fixes land on the latest minor release line only.

| Version | Supported          |
| ------- | ------------------ |
| 0.5.x   | :white_check_mark: |
| < 0.5   | :x:                |

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

- **Constant-time comparison** for the API key and for the HTTP sink
  receiver's bearer token (timing attack resistant)
- **Input validation** on all user-provided data
- **Rate limiting** to prevent abuse
- **Brute-force blocking** of verified client IPs after repeated
  authentication failures. `API_KEY` therefore requires `TRUSTED_PROXIES`
  (startup refuses it otherwise, unless `ALLOW_UNVERIFIED_CLIENT_IPS=true`
  for local development)
- **No secrets in code** - credentials come from environment variables,
  or a file (`SINK_INBOUND_TOKEN_FILE`); the connectors demo generates its
  secrets into Docker volumes on first start
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
  unmaintained and unsound advisories in any crate it builds fail it, and so
  does a yanked crate. The job starts from an empty cargo cache so the yank
  status comes from a fresh index, and an index entry it cannot read
  fails the check too. Two limits:
  - an advisory in `deny.toml`'s `ignore` list is accepted, each with its
    reason and when to drop it (now only RUSTSEC-2026-0440,
    `astral-tokio-tar`, unmaintained and dev-only);
  - its `[graph]` targets are the four native release targets, so a crate
    that only wasm32 builds use is not checked (RUSTSEC-2026-0441 came in
    that way, #94).
- `cargo-audit` (`rustsec/audit-check`) on every CI run. On pushes and pull
  requests a vulnerability advisory fails the job, and with it `CI
  Success`, including one in a crate that sits in `Cargo.lock` without being
  built, which the `cargo-deny` gate never sees; unsound, unmaintained and
  yanked findings only warn there. It skips the IDs in its `ignore` input
  (`ci.yml`), which is kept equal to `deny.toml`'s list. The weekly
  scheduled run does not fail on advisories: it opens a GitHub issue per new
  advisory instead, skipping any advisory ID already named in an issue or PR
  title, open or closed
- A manual check for newer releases, major versions included, with
  `cargo outdated --root-deps-only` and
  `cargo upgrade --incompatible allow --dry-run` (`cargo-outdated` and
  `cargo-edit`; see CONTRIBUTING.md)
- Manual review of security advisories

Dependabot alerts and security updates are not relied on for Rust crates:
the GitHub Advisory Database does not mirror every RustSec advisory, and it
carried none of the four fixed in 0.4.1.
