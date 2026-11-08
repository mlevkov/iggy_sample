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
//! credentials, so splitting the server part on `,` is unambiguous.

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

/// Expand a connection string that may list several comma-separated server
/// addresses into one seed per address, in the order given.
///
/// # Errors
///
/// A reason the string cannot be expanded. The reason never repeats the
/// string itself, which holds credentials. Everything past the split (the
/// scheme, the credentials, each address, the options) is left to the SDK's
/// own parser when a client is built.
pub(crate) fn expand(connection_string: &str) -> Result<Vec<Seed>, String> {
    let (scheme, rest) = connection_string
        .split_once("://")
        .ok_or("the connection string needs a scheme, e.g. iggy://user:pass@host:8090")?;
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
        if seeds.iter().any(|seed| seed.address == address) {
            return Err(format!(
                "the connection string lists server {address} twice"
            ));
        }
        let options = options
            .map(|options| format!("?{options}"))
            .unwrap_or_default();
        seeds.push(Seed {
            address: address.to_string(),
            connection_string: format!("{scheme}://{credentials}@{address}{options}"),
        });
    }
    Ok(seeds)
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
    fn test_expand_keeps_a_single_address_unchanged() {
        let seeds = expand("iggy://iggy:secret@localhost:8090").unwrap();
        assert_eq!(addresses(&seeds), ["localhost:8090"]);
        assert_eq!(
            seeds[0].connection_string(),
            "iggy://iggy:secret@localhost:8090"
        );
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
        assert_eq!(seeds[1].connection_string(), "iggy+tcp://iggypat-token@b:2");
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
