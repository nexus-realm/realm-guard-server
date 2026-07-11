//! Sonde de vivacité (`/healthz`).

use axum::Json;
use serde::Serialize;

/// Corps de réponse de la sonde de santé.
#[derive(Debug, Serialize)]
pub struct Health {
    /// État global du service.
    status: &'static str,
    /// Version du cœur partagé lié (valide la dépendance `realm-guard-core`).
    core_version: &'static str,
}

/// Renvoie l'état du service (toujours 200 tant que le processus répond).
pub async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok",
        core_version: realm_guard_core::core_version(),
    })
}
