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
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};

/// Bornes hautes (secondes) des buckets de latence HTTP — les valeurs par défaut
/// de Prometheus, adaptées à une API web (5 ms → 10 s).
///
/// Sans buckets, l'exporter rend l'histogramme en **summary** (quantiles calculés
/// côté processus, sur une fenêtre glissante, non agrégeables entre instances). En
/// vrais buckets, on débloque côté Prometheus/Grafana `histogram_quantile()`,
/// l'agrégation multi-instances et l'alerting SLO (« part des requêtes sous X ms »).
const HTTP_LATENCY_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// Builder Prometheus partagé par le recorder de prod et le test de format, pour
/// que ce dernier prouve la configuration **réellement** installée.
///
/// ⚠️ Les buckets sont posés **par métrique** (`Matcher::Full`) : tout nouvel
/// histogramme non déclaré ici retomberait en *summary*. En ajouter un ⇒ lui
/// enregistrer ses propres buckets.
fn prometheus_builder() -> PrometheusBuilder {
    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full("http_request_duration_seconds".to_owned()),
            HTTP_LATENCY_BUCKETS,
        )
        .expect("buckets de latence non vides")
}

/// Handle du recorder Prometheus global (installé au plus une fois par processus).
pub fn metrics_handle() -> PrometheusHandle {
    static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            prometheus_builder()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Verrouille le format de sortie : l'histogramme de latence doit sortir en
    /// **buckets** Prometheus (`_bucket{le=…}`) et non en *summary* (`quantile=…`).
    /// Une régression ici casse silencieusement `histogram_quantile()` et tout
    /// l'alerting SLO côté Grafana (cf. P1 observabilité).
    #[test]
    fn latency_histogram_renders_as_buckets_not_summary() {
        // Recorder LOCAL (pas d'installation globale, donc pas de conflit avec les
        // autres tests) construit avec la configuration exacte de la prod.
        let recorder = prometheus_builder().build_recorder();
        let handle = recorder.handle();

        metrics::with_local_recorder(&recorder, || {
            for latency in [0.001_f64, 0.03, 0.4, 2.0] {
                metrics::histogram!(
                    "http_request_duration_seconds",
                    "method" => "GET",
                    "path" => "/readyz"
                )
                .record(latency);
            }
        });

        let rendered = handle.render();
        assert!(
            rendered.contains("# TYPE http_request_duration_seconds histogram"),
            "type attendu = histogram, rendu :\n{rendered}"
        );
        assert!(
            rendered.contains("http_request_duration_seconds_bucket"),
            "séries de buckets attendues, rendu :\n{rendered}"
        );
        assert!(
            rendered.contains("le=\"0.05\""),
            "borne le=0.05 des buckets par défaut attendue"
        );
        assert!(
            !rendered.contains("quantile="),
            "l'histogramme ne doit plus sortir en summary :\n{rendered}"
        );
    }
}
