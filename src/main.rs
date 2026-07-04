use std::net::SocketAddr;
use std::process::ExitCode;

use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use iggy_sample::{AppState, Config, IggyClientWrapper, build_router, utils};

#[tokio::main]
async fn main() -> ExitCode {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .with_thread_ids(true)
        .init();

    info!(
        "Starting Iggy Sample Application v{}",
        env!("CARGO_PKG_VERSION")
    );

    match run().await {
        Ok(()) => ExitCode::from(exitcode::OK as u8),
        Err(exit_code) => ExitCode::from(exit_code as u8),
    }
}

/// Run the application, returning an exit code on error.
async fn run() -> Result<(), exitcode::ExitCode> {
    // Load configuration
    let config = Config::from_env().map_err(|e| {
        error!("Configuration error: {e}");
        exitcode::CONFIG
    })?;
    info!(
        host = %config.host,
        port = %config.port,
        stream = %config.default_stream,
        topic = %config.default_topic,
        "Configuration loaded"
    );

    // Initialize Iggy client
    info!("Connecting to Iggy server...");
    let iggy_client = IggyClientWrapper::new(config.clone()).await.map_err(|e| {
        error!("Failed to connect to Iggy server: {e}");
        exitcode::UNAVAILABLE
    })?;
    info!("Successfully connected to Iggy server");

    // Initialize default stream and topic
    info!("Initializing default stream and topic...");
    iggy_client.initialize_defaults().await.map_err(|e| {
        error!("Failed to initialize defaults: {e}");
        exitcode::UNAVAILABLE
    })?;
    info!(
        "Default stream '{}' and topic '{}' initialized",
        config.default_stream, config.default_topic
    );

    // Start the Prometheus metrics exporter (dedicated listener). A bind
    // failure fails startup: silently missing metrics would defeat alerting.
    if config.metrics_port > 0 {
        let metrics_addr: SocketAddr = format!("{}:{}", config.host, config.metrics_port)
            .parse()
            .map_err(|e| {
            error!("Invalid metrics address: {e}");
            exitcode::CONFIG
        })?;
        iggy_sample::metrics::init_metrics(metrics_addr).map_err(|e| {
            error!("Failed to start metrics exporter: {e}");
            exitcode::UNAVAILABLE
        })?;
    } else {
        info!("Metrics exporter disabled (METRICS_PORT=0)");
    }

    // Build application state and router
    let state = AppState::new(iggy_client, config.clone());
    let app = build_router(state.clone()).map_err(|e| {
        error!("Failed to build router: {e}");
        exitcode::CONFIG
    })?;

    // Start server
    let addr: SocketAddr = config.server_addr().parse().map_err(|e| {
        error!("Invalid server address: {e}");
        exitcode::CONFIG
    })?;
    let listener = TcpListener::bind(addr).await.map_err(|e| {
        error!("Failed to bind to {addr}: {e}");
        exitcode::UNAVAILABLE
    })?;

    info!("Server listening on http://{addr}");
    info!("API endpoints:");
    info!("  GET  /health           - Health check");
    info!("  GET  /ready            - Readiness check");
    info!("  GET  /stats            - Service statistics");
    info!("  POST /messages         - Send a message");
    info!("  GET  /messages         - Poll messages");
    info!("  POST /messages/batch   - Send batch of messages");
    info!("  GET  /streams          - List streams");
    info!("  POST /streams          - Create stream");
    info!("  GET  /streams/{{name}}   - Get stream info");
    info!("  DELETE /streams/{{name}} - Delete stream");

    // Start server with graceful shutdown. ConnectInfo exposes the peer
    // address to the middleware stack, which TRUSTED_PROXIES enforcement
    // needs to decide whether forwarded headers can be honored.
    let serve_result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(utils::shutdown_signal())
    .await;

    // Gracefully shutdown background tasks on BOTH exit paths - a serve
    // error must not leave the stats/health tasks running un-awaited.
    info!("HTTP server stopped, shutting down background tasks...");
    state.shutdown().await;

    serve_result.map_err(|e| {
        error!("Server error: {e}");
        exitcode::SOFTWARE
    })?;

    info!("Server shutdown complete");
    Ok(())
}
