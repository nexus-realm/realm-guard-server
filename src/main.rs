//! Point d'entrée du serveur : Sentry, tracing, config, démarrage HTTP (arrêt gracieux).

use std::sync::Arc;

use anyhow::Context;
use realm_guard_server::{Config, build_app};

fn main() -> anyhow::Result<()> {
    // Sentry doit être initialisé avant le runtime async ; le guard vit toute la
    // durée du processus (flush des événements au drop).
    let _sentry = init_sentry();
    init_tracing();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("construction du runtime tokio")?
        .block_on(run())
}

/// Charge la config, ouvre le socket et sert jusqu'à l'arrêt.
async fn run() -> anyhow::Result<()> {
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

/// Initialise Sentry. DSN via `SENTRY_DSN` (absent → client désactivé, aucun
/// réseau). PII désactivée + scrubbing défensif : ni contexte requête ni hostname.
fn init_sentry() -> sentry::ClientInitGuard {
    let dsn = std::env::var("SENTRY_DSN")
        .ok()
        .and_then(|s| s.parse().ok());
    sentry::init(sentry::ClientOptions {
        dsn,
        release: sentry::release_name!(),
        send_default_pii: false,
        before_send: Some(Arc::new(|mut event: sentry::protocol::Event<'static>| {
            event.request = None;
            event.server_name = None;
            Some(event)
        })),
        ..Default::default()
    })
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
