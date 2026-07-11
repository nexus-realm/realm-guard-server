//! `realm-guard-server` — serveur de synchronisation E2EE (Axum).
//!
//! Le serveur ne voit **jamais** de données en clair : il ne fait que stocker et
//! relayer des deltas CRDT chiffrés (cf. `realm-guard-core`). Ce module expose la
//! construction du routeur ([`build_app`]) séparément du démarrage réseau, pour
//! des tests d'intégration déterministes.

pub mod config;
pub mod health;

use axum::Router;
use axum::routing::get;

pub use config::Config;

/// Construit le routeur de l'application, sans ouvrir de socket — utilisable tel
/// quel dans les tests d'intégration (via `tower::ServiceExt::oneshot`).
pub fn build_app() -> Router {
    Router::new().route("/healthz", get(health::healthz))
}
