//! Observabilité : métriques Prometheus (`/metrics`) et instrumentation des requêtes.
//!
//! Le recorder Prometheus est **global** (une seule installation par processus,
//! gardée par un `OnceLock`) — `build_app` reste ainsi appelable plusieurs fois
//! (tests). L'instrumentation étiquette par **motif de route** (pas l'URL brute)
//! pour borner la cardinalité des séries.

use std::sync::OnceLock;
use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

/// Handle du recorder Prometheus global (installé au plus une fois par processus).
pub fn metrics_handle() -> PrometheusHandle {
    static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            PrometheusBuilder::new()
                .install_recorder()
                .expect("installation du recorder Prometheus")
        })
        .clone()
}

/// Middleware : compte les requêtes et mesure leur latence, étiquetées par méthode,
/// motif de route et statut.
pub async fn track_metrics(request: Request, next: Next) -> Response {
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
        .unwrap_or_else(|| "unknown".to_owned());
    let method = request.method().clone();

    let start = Instant::now();
    let response = next.run(request).await;
    let latency = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    metrics::counter!(
        "http_requests_total",
        "method" => method.to_string(),
        "path" => path.clone(),
        "status" => status
    )
    .increment(1);
    metrics::histogram!(
        "http_request_duration_seconds",
        "method" => method.to_string(),
        "path" => path
    )
    .record(latency);

    response
}
