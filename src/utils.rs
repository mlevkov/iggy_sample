use tokio::signal;
use tracing::{error, warn};

/// Wait for a shutdown signal (Ctrl+C or SIGTERM).
///
/// # Panics
///
/// Panics if signal handlers cannot be installed. This is a critical
/// initialization failure that should halt the application.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        match signal::ctrl_c().await {
            Ok(()) => {}
            Err(e) => {
                error!("Failed to install Ctrl+C handler: {e}");
                panic!("Critical: cannot install Ctrl+C signal handler");
            }
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                error!("Failed to install SIGTERM handler: {e}");
                panic!("Critical: cannot install SIGTERM signal handler");
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            warn!("Received Ctrl+C, initiating graceful shutdown...");
        }
        _ = terminate => {
            warn!("Received SIGTERM, initiating graceful shutdown...");
        }
    }
}
