//! `realm-guard-server` — serveur de synchronisation E2EE (Axum).
//!
//! Le serveur ne voit **jamais** de données en clair : il ne fait que stocker et
//! relayer des deltas CRDT chiffrés (cf. `realm-guard-core`). Ce module expose la
//! construction du routeur ([`build_app`]) séparément du démarrage réseau, pour
//! des tests d'intégration déterministes.

pub mod accounts;
pub mod auth_api;
pub mod config;
pub mod health;
pub mod observability;
pub mod sessions;
pub mod state;
pub mod vault_api;
pub mod vault_keys;

use axum::routing::get;
use axum::{Router, middleware};
use tower_http::trace::TraceLayer;

pub use config::Config;
pub use state::AppState;

/// Construit le routeur de l'application, sans ouvrir de socket — utilisable tel
/// quel dans les tests d'intégration (via `tower::ServiceExt::oneshot`).
///
/// Routes : `/healthz` (vivacité), `/readyz` (disponibilité DB+Redis),
/// `/metrics` (Prometheus). Les requêtes sont instrumentées (métriques + trace).
pub fn build_app(state: AppState) -> Router {
    let metrics = observability::metrics_handle();
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route(
            "/metrics",
            get(move || std::future::ready(metrics.render())),
        )
        .merge(auth_api::routes())
        .merge(vault_api::routes())
        .route_layer(middleware::from_fn(observability::track_metrics))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Applique les migrations et bootstrappe le secret serveur OPAQUE (généré au
/// premier boot). Renvoie le setup sérialisé. **Nécessite Postgres joignable.**
///
/// # Errors
/// Base injoignable, migration ou bootstrap en échec.
pub async fn migrate_and_bootstrap(database_url: &str) -> anyhow::Result<Vec<u8>> {
    use anyhow::Context;
    use sqlx::postgres::PgPoolOptions;

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_lazy(database_url)
        .context("pool de démarrage")?;
    sqlx::migrate!()
        .run(&pool)
        .await
        .context("application des migrations")?;
    let setup = accounts::bootstrap_opaque_setup(&pool)
        .await
        .context("bootstrap du secret serveur OPAQUE")?;
    pool.close().await;
    Ok(setup)
}
