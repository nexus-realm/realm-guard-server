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

// ── Métriques métier (P4 — cf. docs/OBSERVABILITY.md) ──────────────────────
// Émises aux points d'événement (handlers sync / auth / ws). **Zero-knowledge** :
// aucun label sensible (compte, username, IP, syncId) — uniquement des libellés
// bornés. Centralisées ici pour que les noms de métriques aient une source unique.

/// Un delta de sync poussé (`POST /sync/deltas` réussi).
pub fn record_delta_pushed() {
    metrics::counter!("rg_sync_deltas_pushed_total").increment(1);
}

/// Deltas servis à un tirage (`GET /sync/deltas`). `count` peut être 0 (la série
/// existe alors dès le premier tirage vide).
pub fn record_deltas_pulled(count: u64) {
    metrics::counter!("rg_sync_deltas_pulled_total").increment(count);
}

/// Un snapshot publié et le log compacté (`PUT /sync/snapshot` réussi).
pub fn record_snapshot_created() {
    metrics::counter!("rg_sync_snapshots_created_total").increment(1);
}

/// Une inscription de compte finalisée (`POST /auth/register/finish` réussi).
pub fn record_registration() {
    metrics::counter!("rg_auth_registrations_total").increment(1);
}

/// Une tentative de login finalisée, étiquetée par issue (`success`/`failure`).
pub fn record_login(success: bool) {
    let outcome = if success { "success" } else { "failure" };
    metrics::counter!("rg_auth_logins_total", "outcome" => outcome).increment(1);
}

/// Un refus pour compte verrouillé (rate-limit par compte atteint sur
/// `/auth/login/start`). Signal anti-abus.
pub fn record_login_lockout() {
    metrics::counter!("rg_auth_lockouts_total").increment(1);
}

/// Suit une connexion WebSocket active via la jauge `rg_ws_connections` : à
/// appeler quand la connexion est établie. Le garde **RAII** décrémente au drop,
/// donc la jauge reste juste quel que soit le chemin de sortie (fermeture client,
/// erreur, pub/sub coupé).
#[must_use]
pub fn track_ws_connection() -> WsConnectionGuard {
    metrics::gauge!("rg_ws_connections").increment(1.0);
    WsConnectionGuard
}

/// Garde de connexion WebSocket : décrémente `rg_ws_connections` au drop. Obtenu
/// via [`track_ws_connection`].
pub struct WsConnectionGuard;

impl Drop for WsConnectionGuard {
    fn drop(&mut self) {
        metrics::gauge!("rg_ws_connections").decrement(1.0);
    }
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

    /// Verrouille les **noms** des métriques métier (P4) et l'absence de label
    /// sensible : un renommage casserait silencieusement règles d'alerte et
    /// dashboards. Le garde WS doit incrémenter puis décrémenter au drop.
    #[test]
    fn business_metrics_render_with_expected_names() {
        let recorder = prometheus_builder().build_recorder();
        let handle = recorder.handle();

        metrics::with_local_recorder(&recorder, || {
            record_delta_pushed();
            record_deltas_pulled(3);
            record_snapshot_created();
            record_registration();
            record_login(true);
            record_login(false);
            record_login_lockout();
            {
                let _conn = track_ws_connection(); // +1
                assert!(
                    handle.render().contains("rg_ws_connections 1"),
                    "jauge WS doit être à 1 pendant la connexion"
                );
            } // drop → -1
        });

        let out = handle.render();
        for name in [
            "rg_sync_deltas_pushed_total",
            "rg_sync_deltas_pulled_total",
            "rg_sync_snapshots_created_total",
            "rg_auth_registrations_total",
            "rg_auth_logins_total",
            "rg_auth_lockouts_total",
            "rg_ws_connections",
        ] {
            assert!(out.contains(name), "métrique {name} absente :\n{out}");
        }
        // Issue de login étiquetée, deux valeurs distinctes.
        assert!(out.contains("rg_auth_logins_total{outcome=\"success\"} 1"));
        assert!(out.contains("rg_auth_logins_total{outcome=\"failure\"} 1"));
        // Deltas tirés = 3 (increment par lot).
        assert!(out.contains("rg_sync_deltas_pulled_total 3"));
        // Jauge revenue à 0 après le drop du garde.
        assert!(
            out.contains("rg_ws_connections 0"),
            "la jauge WS doit revenir à 0 au drop :\n{out}"
        );
    }
}
