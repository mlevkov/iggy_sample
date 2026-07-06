# Tech Debt Registry

Deferred findings from code reviews, each with a binding trigger — the
condition under which the record MUST be resolved (not "someday").

| ID | Title | Source | Trigger | Status |
|----|-------|--------|---------|--------|
| [TD-2026-07-01](TD-2026-07-01.md) | `with_reconnect` composition test matrix | Review session 01 (tests #2) | Any behavioral change to `with_reconnect`/`retry_once` | resolved (session 02) |
| [TD-2026-07-02](TD-2026-07-02.md) | DiagnosticEvents-driven connection state | Review session 01 (architect #2) | Next iggy SDK minor bump | open (parked; trigger re-checked 2026-07-05, not fired) |
| [TD-2026-07-03](TD-2026-07-03.md) | Half-open probe limiting | Review session 01 (architect #9, types F5) | First production incident involving breaker recovery, or breaker config exposure | resolved (session 02) |
| [TD-2026-07-04](TD-2026-07-04.md) | X-Request-Timeout enforcement | Review session 01 (silentfail M4) | Before advertising the header in any client-facing docs beyond CLAUDE.md | resolved (session 02) |
| [TD-2026-07-05](TD-2026-07-05.md) | Metrics exporter smoke test | Review session 01 (tests #7) | Next metrics-exporter-prometheus major/minor bump | resolved (session 02) |
| [TD-2026-07-06](TD-2026-07-06.md) | Durable-storage guide config re-validation | Review session 01 (consistency #10) | Next server image bump past 0.8.x | resolved (session 02) |
| [TD-2026-07-07](TD-2026-07-07.md) | Pin third-party GitHub Actions to commit SHAs | Security review on v0.2.0 release PR | Next CI-focused change, or any new repo secret | resolved (session 02) |
