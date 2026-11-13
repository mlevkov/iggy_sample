//! # Iggy Sample Application
//!
//! A comprehensive, production-ready demonstration of Apache Iggy message
//! streaming with Axum, featuring:
//!
//! - **High Performance**: True batch sending, one shared SDK client
//! - **Resilience**: Reconnection with exponential backoff, a circuit
//!   breaker, and SDK 0.12 lane settling
//! - **Security**: API key authentication, rate limiting, input validation
//! - **Observability**: Request IDs, structured logging, health endpoints
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      Axum HTTP Server                       │
//! ├─────────────────────────────────────────────────────────────┤
//! │  Middleware (Rate Limit → Auth → Request ID → Timeout →     │
//! │              Trace → CORS → Body Limit)                     │
//! ├─────────────────────────────────────────────────────────────┤
//! │  Handlers (health, cluster, messages, streams, topics, sink)│
//! ├─────────────────────────────────────────────────────────────┤
//! │  Services (ProducerService, ConsumerService, SinkInbox)     │
//! ├─────────────────────────────────────────────────────────────┤
//! │  IggyClientWrapper (seeds, lanes, breaker, reconnect)       │
//! ├─────────────────────────────────────────────────────────────┤
//! │  Apache Iggy 0.10, one node or a cluster (TCP/QUIC/HTTP)    │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use iggy_sample::{Config, IggyClientWrapper, AppState, build_router};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = Config::from_env()?;
//!     let iggy_client = IggyClientWrapper::new(config.clone()).await?;
//!     iggy_client.initialize_defaults().await?;
//!
//!     let state = AppState::new(iggy_client, config);
//!     let app = build_router(state);
//!
//!     // Start the server...
//!     Ok(())
//! }
//! ```
//!
//! ## Security Configuration
//!
//! Enable API key authentication:
//! ```bash
//! API_KEY=your-secret-key cargo run
//! ```
//!
//! Enable rate limiting:
//! ```bash
//! RATE_LIMIT_RPS=100 RATE_LIMIT_BURST=50 cargo run
//! ```

pub mod config;
pub mod error;
pub mod handlers;
pub mod iggy_client;
pub mod metrics;
pub mod middleware;
pub mod models;
pub mod routes;
pub mod services;
pub mod state;
pub mod utils;
pub mod validation;

// Re-exports for convenience
pub use config::Config;
pub use error::{AppError, AppResult};
pub use iggy_client::{IggyClientWrapper, PollParams};
pub use routes::build_router;
pub use state::AppState;
