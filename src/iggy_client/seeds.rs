//! Seed addresses: an ordered list of servers to try when building a client.
//!
//! An SDK 0.12 connection string names exactly one server. Once signed in, a
//! client reads the cluster roster and dials every node itself, so one
//! reachable node is enough. That node has to be up when the client is built,
//! though: at startup, and on each reconnect, which builds a fresh client.
//!
//! `IGGY_CONNECTION_STRING` may therefore list several comma-separated
//! addresses:
//!
//! ```text
//! iggy://user:pass@10.29.0.10:8090,10.29.0.11:8090,10.29.0.12:8090
//! ```
//!
//! [`expand`] turns that into one SDK connection string per address, and a
//! [`SeedRing`] hands them out in order, staying on a seed while it works and
//! moving on when it fails (ADR-0001 PR 3). With a single address nothing
//! changes.
//!
//! The SDK's own parser allows exactly one `@` and at most one `:` in the
//! credentials, so splitting the server part on `,` is unambiguous. A string
//! with more than one `@` (a password containing one) is refused here, and
//! every address must look like `host:port` before it is logged or repeated:
//! otherwise part of a password could land in the server list, and from
//! there in the logs.
//!
//! # Reconnection retries
//!
//! Each seed's connection string also caps the SDK's reconnection retries at
//! [`DEFAULT_RECONNECTION_RETRIES`] unless the string sets the option itself
//! (ADR-0003). With the SDK's default, `unlimited`, a full outage never ends
//! a client's reconnecting, so every call waits out the SDK's 30 s deadline
//! and fails with `TransientNotAccepted`, which the circuit breaker
//! deliberately does not count. With a cap, the SDK reports `NotConnected`
//! once its retries run out, the breaker opens, and callers get an immediate
//! 503 until the server is back. The SDK's keepalive restarts a client that
//! ran out, with a fresh budget, so the client still recovers on its own.

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use iggy::prelude::{Client, IggyClient, IggyError};
use tracing::warn;

use super::helpers::classify_connect_error;
use crate::error::{AppError, AppResult};

/// One seed: its address, and the SDK connection string that dials it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Seed {
    address: String,
    connection_string: String,
}

impl Seed {
    /// The `host:port` this seed dials. Safe to log.
    pub(crate) fn address(&self) -> &str {
        &self.address
    }

    /// The SDK connection string for this seed. Holds the credentials: never
    /// log it.
    pub(crate) fn connection_string(&self) -> &str {
        &self.connection_string
    }
}

/// Only the address: the connection string holds credentials.
impl fmt::Debug for Seed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Seed")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

/// The SDK reconnection retries each seed uses when the connection string
/// sets none: the attempts after the first, with jittered backoff from 1 s,
/// doubling, capped at 10 s. Three ride out a server restart of a few seconds
/// and give up after about 7.5 s on average, 15 s at most, so a call never
/// waits the full 30 s request timeout on a server that is down (ADR-0003).
pub(crate) const DEFAULT_RECONNECTION_RETRIES: u32 = 3;

/// The option that caps reconnection retries for a connection string
/// scheme, or `None` for a transport without reconnection (HTTP) or one the
/// SDK does not know.
fn reconnection_option(scheme: &str) -> Option<&'static str> {
    match scheme {
        "iggy" | "iggy+tcp" | "iggy+ws" => Some("reconnection_retries"),
        "iggy+quic" => Some("reconnection_max_retries"),
        _ => None,
    }
}

/// The `?options` part of each seed's connection string: the given options,
/// plus the default reconnection cap when the transport has one and the
/// options do not set it.
fn seed_options(scheme: &str, options: Option<&str>) -> String {
    let options = options.filter(|options| !options.is_empty());
    let default = reconnection_option(scheme).filter(|key| {
        !options.is_some_and(|options| {
            options
                .split('&')
                .any(|option| option.split_once('=').map_or(option, |(name, _)| name) == *key)
        })
    });
    match (options, default) {
        (Some(options), Some(key)) => {
            format!("?{options}&{key}={DEFAULT_RECONNECTION_RETRIES}")
        }
        (Some(options), None) => format!("?{options}"),
        (None, Some(key)) => format!("?{key}={DEFAULT_RECONNECTION_RETRIES}"),
        (None, None) => String::new(),
    }
}

/// Expand a connection string that may list several comma-separated server
/// addresses into one seed per address, in the order given. Each seed keeps
/// the string's options and gains [`DEFAULT_RECONNECTION_RETRIES`] when they
/// do not set the transport's reconnection cap.
///
/// # Errors
///
/// A reason the string cannot be expanded. The reason never repeats the
/// string, which holds credentials, and names an address only once it has
/// passed the `host:port` check. Everything past the split (the scheme, the
/// credentials, the port number, the option values) is left to the SDK's own
/// parser when a client is built.
pub(crate) fn expand(connection_string: &str) -> Result<Vec<Seed>, String> {
    let (scheme, rest) = connection_string
        .split_once("://")
        .ok_or("the connection string needs a scheme, e.g. iggy://user:pass@host:8090")?;
    if rest.matches('@').count() > 1 {
        return Err(
            "the connection string has more than one '@'; the SDK allows exactly one, \
                    so credentials cannot contain '@'"
                .into(),
        );
    }
    let (credentials, servers_and_options) = rest
        .split_once('@')
        .ok_or("the connection string needs credentials, e.g. iggy://user:pass@host:8090")?;
    let (servers, options) = match servers_and_options.split_once('?') {
        Some((servers, options)) => (servers, Some(options)),
        None => (servers_and_options, None),
    };

    let mut seeds: Vec<Seed> = Vec::new();
    for address in servers.split(',').map(str::trim) {
        if address.is_empty() {
            return Err("the connection string lists an empty server address".into());
        }
        if !looks_like_host_port(address) {
            return Err("a server address in the connection string is not host:port".into());
        }
        if seeds.iter().any(|seed| seed.address == address) {
            return Err(format!(
                "the connection string lists server {address} twice"
            ));
        }
        let options = seed_options(scheme, options);
        seeds.push(Seed {
            address: address.to_string(),
            connection_string: format!("{scheme}://{credentials}@{address}{options}"),
        });
    }
    Ok(seeds)
}

/// Whether `address` holds only characters a `host:port` can (letters,
/// digits, `.`, `-`, `_`, and `:`/`[`/`]` for the port and IPv6), with a
/// `:`. Safe to log once true.
fn looks_like_host_port(address: &str) -> bool {
    address.contains(':')
        && address
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']'))
}

/// The seeds, with a cursor on the one to try next.
///
/// The cursor stays on a seed that connected and moves to the next one on a
/// failure, wrapping around, so a reconnect starts from the last seed that
/// worked.
pub(crate) struct SeedRing {
    first: Seed,
    rest: Vec<Seed>,
    cursor: AtomicUsize,
}

impl SeedRing {
    /// A ring over `seeds`, starting at the first. `None` when empty.
    pub(crate) fn new(seeds: Vec<Seed>) -> Option<Self> {
        let mut seeds = seeds.into_iter();
        let first = seeds.next()?;
        Some(Self {
            first,
            rest: seeds.collect(),
            cursor: AtomicUsize::new(0),
        })
    }

    /// How many seeds there are; at least one.
    pub(crate) fn len(&self) -> usize {
        1 + self.rest.len()
    }

    /// The seed to try next.
    pub(crate) fn current(&self) -> &Seed {
        let index = self.cursor.load(Ordering::Relaxed) % self.len();
        match index.checked_sub(1) {
            None => &self.first,
            Some(rest_index) => self.rest.get(rest_index).unwrap_or(&self.first),
        }
    }

    /// Move to the next seed, after the current one failed.
    pub(crate) fn advance(&self) {
        self.cursor.fetch_add(1, Ordering::Relaxed);
    }

    /// Every seed's address, comma-separated, for logs and errors.
    pub(crate) fn addresses(&self) -> String {
        std::iter::once(&self.first)
            .chain(&self.rest)
            .map(Seed::address)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Build a client on the first seed that accepts a connection, trying each
/// once from the ring's current seed, and leave the ring on that seed.
///
/// `total` is shared equally between the seeds, so startup stays bounded by
/// it however many are listed. With one seed this is a single bounded
/// connect, as before seeds existed.
///
/// # Errors
///
/// - `IncompatibleServer` at once if a seed runs another protocol version:
///   every node of a cluster runs the same release, so the next seed would
///   refuse too.
/// - `ConnectionFailed` listing each seed's failure if none connected.
pub(crate) async fn connect_first_reachable(
    seeds: &SeedRing,
    total: Duration,
) -> AppResult<IggyClient> {
    let per_seed = total / u32::try_from(seeds.len()).unwrap_or(u32::MAX);
    let mut failures = Vec::with_capacity(seeds.len());

    for _ in 0..seeds.len() {
        let seed = seeds.current();
        let client =
            IggyClient::from_connection_string(seed.connection_string()).map_err(|error| {
                AppError::ConnectionFailed(format!("seed {}: {error}", seed.address()))
            })?;
        // Bounded: the SDK's own reconnection would otherwise retry inside
        // connect() indefinitely.
        let failure = match tokio::time::timeout(per_seed, client.connect()).await {
            Ok(Ok(())) => return Ok(client),
            Ok(Err(error @ IggyError::IncompatibleProtocolVersion(..))) => {
                let _ = client.shutdown().await;
                return Err(classify_connect_error(error));
            }
            Ok(Err(error)) => error.to_string(),
            Err(_) => format!("no connection within {per_seed:?}"),
        };
        warn!(seed = seed.address(), %failure, "Seed did not accept a connection");
        // connect() may already have spawned the SDK's keepalive task, which
        // Drop alone does not stop.
        let _ = client.shutdown().await;
        failures.push(format!("{}: {failure}", seed.address()));
        seeds.advance();
    }

    // A server from another release can also end here: SDK 0.11 waited
    // forever for a 0.8 server's sign-in reply. SDK 0.12 fails fast against
    // 0.9 ("Invalid format"), and a newer server refuses it with
    // IncompatibleProtocolVersion, but say so here too, since a mismatch is
    // the likeliest cause once the address is known to be right.
    Err(AppError::ConnectionFailed(format!(
        "No Iggy server accepted a connection ({}). If the servers are reachable, \
         check that they run Iggy server 0.10.x: this build's SDK (0.12) speaks \
         only the 0.10 wire protocol",
        failures.join("; ")
    )))
}

impl fmt::Debug for SeedRing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SeedRing")
            .field("addresses", &self.addresses())
            .field("current", &self.current().address())
            .finish()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn addresses(seeds: &[Seed]) -> Vec<&str> {
        seeds.iter().map(Seed::address).collect()
    }

    #[test]
    fn test_expand_keeps_a_single_address_and_caps_its_retries() {
        let seeds = expand("iggy://iggy:secret@localhost:8090").unwrap();
        assert_eq!(addresses(&seeds), ["localhost:8090"]);
        assert_eq!(
            seeds[0].connection_string(),
            "iggy://iggy:secret@localhost:8090?reconnection_retries=3"
        );
    }

    #[test]
    fn test_expand_appends_the_retry_cap_to_other_options() {
        let seeds = expand("iggy://iggy:secret@a:1?heartbeat_interval=2s&nodelay=true").unwrap();
        assert_eq!(
            seeds[0].connection_string(),
            "iggy://iggy:secret@a:1?heartbeat_interval=2s&nodelay=true&reconnection_retries=3"
        );
    }

    #[test]
    fn test_expand_keeps_an_explicit_retry_setting_including_unlimited() {
        for value in ["unlimited", "0", "10"] {
            let seeds = expand(&format!(
                "iggy://iggy:secret@a:1?reconnection_retries={value}&nodelay=true"
            ))
            .unwrap();
            assert_eq!(
                seeds[0].connection_string(),
                format!("iggy://iggy:secret@a:1?reconnection_retries={value}&nodelay=true")
            );
        }
    }

    #[test]
    fn test_expand_uses_each_transports_own_retry_option() {
        let cases = [
            (
                "iggy+tcp://u:p@a:1",
                "iggy+tcp://u:p@a:1?reconnection_retries=3",
            ),
            (
                "iggy+ws://u:p@a:1",
                "iggy+ws://u:p@a:1?reconnection_retries=3",
            ),
            (
                "iggy+quic://u:p@a:1",
                "iggy+quic://u:p@a:1?reconnection_max_retries=3",
            ),
            (
                "iggy+quic://u:p@a:1?reconnection_max_retries=unlimited",
                "iggy+quic://u:p@a:1?reconnection_max_retries=unlimited",
            ),
            // HTTP has no reconnection, and `retries` is its request retry
            ("iggy+http://u:p@a:1", "iggy+http://u:p@a:1"),
            (
                "iggy+http://u:p@a:1?retries=2",
                "iggy+http://u:p@a:1?retries=2",
            ),
        ];
        for (input, expected) in cases {
            let seeds = expand(input).unwrap();
            assert_eq!(seeds[0].connection_string(), expected, "{input}");
        }
    }

    #[test]
    fn test_expanded_connection_strings_parse_in_the_sdk() {
        use iggy::prelude::IggyClient;
        for input in [
            "iggy://iggy:secret@127.0.0.1:8090",
            "iggy://iggy:secret@127.0.0.1:8090?heartbeat_interval=2s",
            "iggy+quic://iggy:secret@127.0.0.1:8080",
            "iggy+ws://iggy:secret@127.0.0.1:8092",
            "iggy+http://iggy:secret@127.0.0.1:3000",
        ] {
            let seeds = expand(input).unwrap();
            assert!(
                IggyClient::from_connection_string(seeds[0].connection_string()).is_ok(),
                "{input}"
            );
        }
    }

    #[test]
    fn test_expand_gives_each_address_the_credentials_and_options() {
        let seeds = expand(
            "iggy://iggy:secret@10.29.0.10:8090, 10.29.0.11:8090,10.29.0.12:8090?reconnection_retries=5",
        )
        .unwrap();
        assert_eq!(
            addresses(&seeds),
            ["10.29.0.10:8090", "10.29.0.11:8090", "10.29.0.12:8090"]
        );
        assert_eq!(
            seeds[1].connection_string(),
            "iggy://iggy:secret@10.29.0.11:8090?reconnection_retries=5"
        );
    }

    #[test]
    fn test_expand_keeps_a_personal_access_token() {
        let seeds = expand("iggy+tcp://iggypat-token@a:1,b:2").unwrap();
        assert_eq!(
            seeds[1].connection_string(),
            "iggy+tcp://iggypat-token@b:2?reconnection_retries=3"
        );
    }

    #[test]
    fn test_expand_refuses_malformed_lists_without_echoing_credentials() {
        for raw in [
            "localhost:8090",
            "iggy://localhost:8090",
            "iggy://iggy:secret@a:1,,b:2",
            "iggy://iggy:secret@a:1,",
            "iggy://iggy:secret@a:1,a:1",
        ] {
            let error = expand(raw).expect_err(raw);
            assert!(!error.contains("secret"), "{raw}: {error}");
        }
    }

    #[test]
    fn test_expand_refuses_an_at_sign_in_the_password_without_leaking_it() {
        // Split at the first '@', "ss@a:1" would become a "server address"
        // and reach the logs.
        let error = expand("iggy://iggy:p@ss@a:1").expect_err("two '@'");
        assert!(!error.contains("ss"), "{error}");
    }

    #[test]
    fn test_expand_refuses_addresses_that_are_not_host_port() {
        for raw in [
            "iggy://u:p@a:1/path",
            "iggy://u:p@a:1,not an address",
            "iggy://u:p@hostonly",
        ] {
            let error = expand(raw).expect_err(raw);
            assert!(error.contains("not host:port"), "{raw}: {error}");
        }
        assert!(expand("iggy://u:p@[::1]:8090,my-host_1.local:8090").is_ok());
    }

    #[test]
    fn test_seed_debug_hides_the_connection_string() {
        let seeds = expand("iggy://iggy:secret@a:1").unwrap();
        let debug = format!("{:?}", seeds[0]);
        assert!(
            debug.contains("a:1") && !debug.contains("secret"),
            "{debug}"
        );
    }

    #[test]
    fn test_ring_stays_on_a_working_seed_and_wraps_on_failure() {
        let ring = SeedRing::new(expand("iggy://u:p@a:1,b:2,c:3").unwrap()).unwrap();
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.current().address(), "a:1");
        assert_eq!(ring.current().address(), "a:1");
        ring.advance();
        assert_eq!(ring.current().address(), "b:2");
        ring.advance();
        ring.advance();
        assert_eq!(ring.current().address(), "a:1");
        assert_eq!(ring.addresses(), "a:1, b:2, c:3");
        assert!(!format!("{ring:?}").contains(":p@"));
    }

    #[test]
    fn test_ring_needs_a_seed() {
        assert!(SeedRing::new(Vec::new()).is_none());
    }
}
