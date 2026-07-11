//! Sondes de vivacité (`/healthz`) et de disponibilité (`/readyz`).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use crate::state::AppState;

/// Corps de réponse de la sonde de vivacité.
#[derive(Debug, Serialize)]
pub struct Health {
    /// État global du service.
    status: &'static str,
    /// Version du cœur partagé lié (valide la dépendance `realm-guard-core`).
    core_version: &'static str,
}

/// Vivacité : le processus répond. **Indépendant des dépendances** (ne doit pas
/// échouer si la DB est momentanément indisponible).
pub async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok",
        core_version: realm_guard_core::core_version(),
    })
}

/// Corps de réponse de la sonde de disponibilité.
#[derive(Debug, Serialize)]
pub struct Ready {
    /// État global.
    status: &'static str,
    /// État de Postgres.
    database: &'static str,
    /// État de Redis.
    redis: &'static str,
}

/// Disponibilité : les dépendances (Postgres, Redis) sont joignables. Renvoie
/// **503** si l'une échoue (sémantique readiness façon Kubernetes).
pub async fn readyz(State(state): State<AppState>) -> Result<Json<Ready>, StatusCode> {
    sqlx::query("SELECT 1")
        .execute(&state.db)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "disponibilité : Postgres injoignable");
            StatusCode::SERVICE_UNAVAILABLE
        })?;

    let mut conn = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|error| {
            tracing::warn!(%error, "disponibilité : Redis injoignable");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
    let _pong: String = redis::cmd("PING")
        .query_async(&mut conn)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "disponibilité : PING Redis échoué");
            StatusCode::SERVICE_UNAVAILABLE
        })?;

    Ok(Json(Ready {
        status: "ready",
        database: "up",
        redis: "up",
    }))
}
