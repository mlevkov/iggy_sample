# ADR-0003: Cap the SDK's reconnection retries at 3 by default

- **Status:** Accepted
- **Date:** 2026-11-08
- **Deciders:** Maxim Levkov
- **Resolves:** the option TD-2026-07-02 left open

## Context

Connection-string clients of the Iggy Rust SDK reconnect on their own. The
`reconnection_retries` option bounds how often, and it defaults to
`unlimited`. TD-2026-07-02 left bounding it as a separate decision, because
it changes what callers see during every outage. That record was written
against SDK 0.11, where a retry sweep that gave up ended reconnection for
good.

SDK 0.12 behaves differently. From `links/replicas.rs` and `driver.rs` at
0.12.0:

- **Each link to a node redials in its own slot.** Backoff starts at 1 s
  with full jitter and doubles up to a 10 s cap. A request never dials:
  while a slot is still retrying, a call waits for a link, up to the SDK's
  30 s call deadline.
- **A slot that uses up its retries stops.** Once every slot has stopped,
  the client turns `Disconnected`, and calls fail at once with
  `NotConnected`.
- **The keepalive restarts stopped slots.** It runs every
  `heartbeat_interval`, 5 s by default, and gives each stopped slot a fresh
  budget. So a capped client still recovers on its own.

With `unlimited`, the slots never stop, so the client never reaches
`Disconnected`. During a full outage every call waits out the 30 s deadline
and fails with `TransientNotAccepted`. The gateway maps that to
`503 not_accepted`: retryable, and deliberately not a circuit-breaker
failure, because the SDK has already replayed the request (CLAUDE.md, Error
Handling).

Measured on the compose stack: Iggy stopped for 60 s, then started again,
with a send through the gateway every 0.5 s.

| | `unlimited` (before) | capped at 3 |
|---|---|---|
| Sends during the outage | 116, mean 23.8 s, max 30 s | 115, mean 4.0 s; all immediate after the first few |
| How they failed | `503 not_accepted` after ~30 s; the breaker never opened | first `NotConnected`, then `503 circuit_open` at once |
| First success after the restart | ~4 s (sends held through the restart) | ~7 s |

## Decision

1. **`seeds::expand` adds the cap to each seed's connection string** unless
   the string already sets it. The option is `reconnection_retries=3` for
   `iggy://`, `iggy+tcp://` and `iggy+ws://`, and
   `reconnection_max_retries=3` for `iggy+quic://`. HTTP has no reconnection
   and is left as it is. An explicit value, `unlimited` included, wins.
2. **Three retries.** Their pauses, about 7.5 s on average and 15 s at most,
   ride out a server restart of a few seconds and give up well inside the
   30 s request timeout.
3. **No new environment variable.** The connection string already carries
   SDK options. `?reconnection_retries=unlimited` restores the old
   behaviour.
4. **A regression test.**
   `test_a_call_to_a_stopped_server_ends_as_a_connection_error` stops the
   server under a wrapper built from a string without the option, and
   requires a connection error. With `unlimited` it fails with
   `OperationTimeout` after 25 s.

## Alternatives considered

- **Keep `unlimited`.** Rejected: during an outage, requests hang for 30 s
  each and the breaker never opens, because `TransientNotAccepted` is,
  correctly, not counted.
- **Count `TransientNotAccepted` as a breaker failure.** Rejected: it also
  answers lane and lifecycle refusals on a healthy server, so the breaker
  would open on load, not on loss.
- **`reconnection_retries=0`.** Rejected as the default: it fails fastest,
  but a server restart of even a second or two would fail calls that three
  retries ride out. The keepalive would still restart the link within 5 s.
- **A larger cap, such as 5.** Rejected: its pauses reach 35 s at most,
  past the 30 s request timeout, so most requests during an outage would
  time out much as with `unlimited`.

## Consequences

- **During an outage of more than about 10 s, requests fail fast.** After
  the first few, they get `503 circuit_open` until the breaker's half-open
  probe succeeds.
- **Recovery after the server returns takes a few seconds longer.** The
  client waits for the next keepalive or the breaker's probe, instead of a
  slot that was still redialling.
- **The first requests of an outage can still take up to the request
  timeout.** They wait for the SDK's retries and then for the wrapper's own
  bounded reconnect.
- **A cluster is unaffected.** On the three-node demo with the cap:
  - the cluster tests passed, and the failover script passed twice: new
    leader after 6 s, no failed send, slowest send 4.6 s and 5.4 s;
  - with a follower stopped for 30 s, its link stopped ("Replica 2 stays
    down until a keepalive or a connect restarts it"), and every send went
    through the other two nodes;
  - after the follower restarted, it was back in sync within 2 s, and 60
    sends across the rejoin all succeeded at once.

## References

- SDK 0.12.0: `links/replicas.rs` (`ReconnectPolicy`, `run_slot`,
  `keepalive`), `links/mod.rs` (`backoff`), `driver.rs` (`stopped`),
  `iggy_common` `tcp_connection_string_options.rs` and
  `quic_connection_string_options.rs`
- TD-2026-07-02 (its 2026-11-08 update holds the measurements)
- `src/iggy_client/seeds.rs` (`DEFAULT_RECONNECTION_RETRIES`,
  `seed_options`)
