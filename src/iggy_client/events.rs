//! Connection state driven by the SDK's diagnostic events (TD-2026-07-02).
//!
//! The periodic `ping` in `IggyClientWrapper::health_check` is the source of
//! truth for whether the broker is usable, but on its own it leaves
//! `/health`, `/ready` and the connection gauge stale for up to one
//! `HEALTH_CHECK_INTERVAL_SECS` after the SDK has seen a transition.
//! `Client::subscribe_events()` reports transitions as they happen, so
//! [`IggyClientWrapper::follow_connection_events`] applies them:
//!
//! - **Lost** (`Disconnected`, `Shutdown`, `SignedOut`): mark the connection
//!   down at once. Pessimism is safe; the next successful ping restores it.
//! - **Established** (`Connected`, `SignedIn`): run a live ping now instead of
//!   marking the connection up. The TCP client publishes `Connected` before it
//!   signs in, so the event alone does not prove the broker is usable.
//!
//! # What SDK 0.11 actually publishes (TCP)
//!
//! Observed against server 0.9.0 and confirmed in the SDK source
//! (`tcp_client.rs`):
//!
//! - **Recovery always publishes `Connected`**, including when the SDK's own
//!   transport reconnection succeeds. So an outage the ping recorded ends as
//!   soon as the SDK is back, not up to one health-check interval later.
//! - **Loss publishes `Disconnected` only when a reconnect sweep gives up**
//!   (`fail_connect`), or on an explicit disconnect or shutdown. When the
//!   connection drops, the SDK goes straight into its sweep and publishes
//!   nothing while it redials. With connection strings' default
//!   `reconnection_retries=unlimited` the sweep never gives up, so no loss
//!   event arrives and the ping stays the only loss signal. With bounded
//!   retries the event arrives once they are spent.
//!
//! # Why the wrapper keeps its own resilience layer
//!
//! TD-2026-07-02 also asked whether SDK 0.11 lets resilience move into the
//! SDK. It does not: connection-string clients still hard-wire transport
//! reconnection on (`TcpConnectionStringOptions::parse_options` sets
//! `enabled: true`), and the SDK has no circuit breaker or request deadline.
//! So the wrapper's bounded reconnect, breaker and per-request timeouts stay,
//! and events only make the connection state fresher.
//!
//! Bounding the SDK's retries (`reconnection_retries=N` in the connection
//! string) would hand outage detection to the SDK: callers would get
//! `CannotEstablishConnection` instead of blocking until a timeout, the
//! breaker would open on classified connection errors, and the loss event
//! would fire. That changes request behaviour during every outage, so it is
//! left as a separate decision rather than a default set here.
//!
//! # Client swaps
//!
//! A reconnect session replaces the SDK client, and each client has its own
//! event channel. The follower records the client generation when it
//! subscribes and re-subscribes when a swap bumps it. Events still queued on
//! the old channel are discarded, including the `Shutdown` the wrapper itself
//! triggers when it closes the old client, which would otherwise mark a
//! freshly reconnected client down.

use iggy::prelude::Client;
use iggy_common::DiagnosticEvent;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::IggyClientWrapper;

/// What a diagnostic event means for the tracked connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConnectionEffect {
    /// The SDK lost the connection: mark it down immediately.
    Lost,
    /// The SDK (re)established a transport: confirm with a live ping.
    Established,
}

/// Map a diagnostic event to its effect on the connection state.
///
/// Exhaustive on purpose: a variant added by a future SDK fails to compile
/// here instead of being silently ignored.
pub(super) fn connection_effect(event: DiagnosticEvent) -> ConnectionEffect {
    match event {
        DiagnosticEvent::Disconnected | DiagnosticEvent::Shutdown | DiagnosticEvent::SignedOut => {
            ConnectionEffect::Lost
        }
        DiagnosticEvent::Connected | DiagnosticEvent::SignedIn => ConnectionEffect::Established,
    }
}

/// How one wait on the event channel ended.
enum Wakeup {
    Cancelled,
    ClientSwapped,
    Event(DiagnosticEvent),
    /// The receiver fell behind the SDK's bounded channel and missed events.
    Missed,
    /// The channel closed: its client is gone.
    Closed,
}

impl IggyClientWrapper {
    /// Apply the SDK's connection events to the tracked connection state
    /// until `cancel` fires. Spawned once by `AppState`; see the module docs.
    pub async fn follow_connection_events(&self, cancel: CancellationToken) {
        loop {
            // Read the generation BEFORE subscribing: if a swap lands in
            // between, the wait below returns at once and we re-subscribe to
            // the new client instead of following the old one.
            let generation = self.state.client_generation();
            let mut events = self.client.read().await.subscribe_events().await;

            loop {
                let wakeup = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Wakeup::Cancelled,
                    _ = self.state.wait_for_client_swap(generation) => Wakeup::ClientSwapped,
                    received = events.recv() => match received {
                        Ok(event) => Wakeup::Event(event),
                        Err(_) if events.is_closed() => Wakeup::Closed,
                        Err(_) => Wakeup::Missed,
                    },
                };

                match wakeup {
                    Wakeup::Cancelled => {
                        debug!("Connection event follower received cancellation signal");
                        return;
                    }
                    Wakeup::ClientSwapped => {
                        debug!("SDK client replaced; re-subscribing to its events");
                        break;
                    }
                    Wakeup::Event(event) => {
                        // Recheck: the swap may have landed while this event
                        // was queued (the old client's Shutdown, typically).
                        if self.state.client_generation() != generation {
                            debug!(%event, "Discarding an event from a replaced SDK client");
                            break;
                        }
                        self.apply_connection_event(event).await;
                    }
                    Wakeup::Missed => {
                        warn!("Missed SDK connection events; re-checking with a live ping");
                        self.health_check().await;
                    }
                    Wakeup::Closed => {
                        // Nothing more will arrive on this channel. Wait for
                        // the swap that replaces its client rather than
                        // re-subscribing to the same closed channel in a loop.
                        debug!("SDK event channel closed; waiting for a client swap");
                        tokio::select! {
                            biased;
                            _ = cancel.cancelled() => return,
                            _ = self.state.wait_for_client_swap(generation) => break,
                        }
                    }
                }
            }
        }
    }

    async fn apply_connection_event(&self, event: DiagnosticEvent) {
        match connection_effect(event) {
            ConnectionEffect::Lost => {
                if self.state.is_connected() {
                    warn!(%event, "SDK reported the Iggy connection lost");
                }
                self.state.set_connected(false);
                crate::metrics::set_connection_status(false);
            }
            ConnectionEffect::Established => {
                debug!(%event, "SDK reported a connection; confirming with a live ping");
                self.health_check().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loss_events_mark_the_connection_lost() {
        for event in [
            DiagnosticEvent::Disconnected,
            DiagnosticEvent::Shutdown,
            DiagnosticEvent::SignedOut,
        ] {
            assert_eq!(connection_effect(event), ConnectionEffect::Lost, "{event}");
        }
    }

    #[test]
    fn test_establish_events_only_trigger_a_probe() {
        // Connected fires before sign-in, so it must never mark the
        // connection up by itself.
        for event in [DiagnosticEvent::Connected, DiagnosticEvent::SignedIn] {
            assert_eq!(
                connection_effect(event),
                ConnectionEffect::Established,
                "{event}"
            );
        }
    }
}
