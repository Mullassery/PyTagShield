//! PyTagShield HTTP server binary.
//!
//! Usage:
//!   pytagshield-server /path/to/config.yaml
//!   PYTAGSHIELD_CONFIG=/path/to/config.yaml pytagshield-server
//!
//! The process loads and validates the strict YAML configuration, builds the
//! pipeline and bounded delivery workers, then serves the Axum router until
//! SIGINT/SIGTERM.

mod app;
mod handlers;

use std::env;
use std::error::Error;
use std::io;

use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("pytagshield=info,tower_http=info")),
        )
        .init();

    let path = match env::args().nth(1) {
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!(
                "Usage: pytagshield-server <config.yaml>\n\
                 Environment alternative: PYTAGSHIELD_CONFIG=<config.yaml>"
            );
            return Ok(());
        }
        Some(arg) => arg,
        None => env::var("PYTAGSHIELD_CONFIG").map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "configuration path required: pass <config.yaml> or set PYTAGSHIELD_CONFIG",
            )
        })?,
    };

    let config = pytagshield_core::Config::load(&path)?;
    let listen = config.server.listen.clone();
    let state = app::build_state(config);
    let router = app::build_router(state);
    let listener = TcpListener::bind(&listen).await?;

    tracing::info!(listen = %listen, config = %path, "pytagshield listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("pytagshield stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
