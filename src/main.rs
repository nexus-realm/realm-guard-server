//! Point d'entrée du serveur : config, tracing, démarrage HTTP avec arrêt gracieux.

use anyhow::Context;
use realm_guard_server::{Config, build_app};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = Config::from_env()?;
    let app = build_app();

    let listener = tokio::net::TcpListener::bind(config.addr)
        .await
        .with_context(|| format!("écoute sur {}", config.addr))?;
    tracing::info!(
        addr = %config.addr,
        core = realm_guard_core::core_version(),
        "serveur démarré"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("erreur du serveur HTTP")?;
    Ok(())
}

/// Initialise le tracing (filtre via `RUST_LOG`, défaut `info`).
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).init();
}

/// Attend un signal d'arrêt (Ctrl-C) pour un arrêt propre.
async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "impossible d'installer le gestionnaire Ctrl-C");
        return;
    }
    tracing::info!("arrêt demandé");
}
