//! `realm-guard-server` — serveur de synchronisation E2EE (Axum).
//!
//! Le serveur ne voit **jamais** de données en clair : il ne fait que stocker et
//! relayer des deltas CRDT chiffrés (cf. `realm-guard-core`). Ce module expose la
//! construction du routeur ([`build_app`]) séparément du démarrage réseau, pour
//! des tests d'intégration déterministes.

pub mod accounts;
pub mod config;
pub mod health;
pub mod observability;
pub mod state;

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
        .route_layer(middleware::from_fn(observability::track_metrics))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
