//! `realm-guard-server` — serveur de synchronisation E2EE (Axum).
//!
//! Le serveur ne voit **jamais** de données en clair : il ne fait que stocker et
//! relayer des deltas CRDT chiffrés (cf. `realm-guard-core`). Ce module expose la
//! construction du routeur ([`build_app`]) séparément du démarrage réseau, pour
//! des tests d'intégration déterministes.

pub mod accounts;
pub mod auth_api;
pub mod config;
pub mod devices;
pub mod devices_api;
pub mod health;
pub mod observability;
pub mod pairing_api;
pub mod rate_limit;
pub mod sessions;
pub mod state;
pub mod vault_api;
pub mod vault_keys;

use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Router, middleware};
use metrics_exporter_prometheus::PrometheusHandle;
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
    // Optionnel : si `RG_METRICS_TOKEN` est défini, `/metrics` exige
    // `Authorization: Bearer <token>` ; sinon l'endpoint reste ouvert (dev/compose).
    // À définir en prod pour ne pas exposer la volumétrie publiquement.
    let metrics_token = std::env::var("RG_METRICS_TOKEN").ok();
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route(
            "/metrics",
            get(move |headers: HeaderMap| {
                std::future::ready(render_metrics(&metrics, metrics_token.as_deref(), &headers))
            }),
        )
        .merge(auth_api::routes())
        .merge(pairing_api::routes())
        .merge(vault_api::routes())
        .merge(devices_api::routes())
        .route_layer(middleware::from_fn(observability::track_metrics))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Rend les métriques Prometheus, éventuellement protégées par un token Bearer
/// (`RG_METRICS_TOKEN`). Renvoie **401** si un token est configuré et absent/erroné.
fn render_metrics(
    metrics: &PrometheusHandle,
    expected_token: Option<&str>,
    headers: &HeaderMap,
) -> Result<String, StatusCode> {
    if let Some(expected) = expected_token {
        let provided = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        if provided != Some(expected) {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(metrics.render())
}

/// Applique les migrations de la base. **Nécessite Postgres joignable.**
///
/// Le secret serveur OPAQUE n'est **plus** bootstrappé ici : il est fourni hors
/// base via la config ([`Config::from_env`]), pour ne pas le co-localiser avec les
/// password files.
///
/// # Errors
/// Base injoignable ou migration en échec.
pub async fn run_migrations(database_url: &str) -> anyhow::Result<()> {
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
    pool.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with_auth(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn metrics_open_when_no_token_configured() {
        let metrics = observability::metrics_handle();
        assert!(render_metrics(&metrics, None, &HeaderMap::new()).is_ok());
    }

    #[test]
    fn metrics_require_correct_bearer_when_token_set() {
        let metrics = observability::metrics_handle();
        // Token configuré mais en-tête absent → 401.
        assert_eq!(
            render_metrics(&metrics, Some("s3cret"), &HeaderMap::new()).unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
        // Mauvais token → 401.
        assert_eq!(
            render_metrics(&metrics, Some("s3cret"), &headers_with_auth("Bearer nope"))
                .unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
        // Bon token → OK.
        assert!(
            render_metrics(
                &metrics,
                Some("s3cret"),
                &headers_with_auth("Bearer s3cret")
            )
            .is_ok()
        );
    }
}
