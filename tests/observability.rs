//! Test d'intégration : l'endpoint `/metrics` expose du Prometheus.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // apporte `oneshot`

#[tokio::test]
async fn metrics_endpoint_exposes_prometheus() {
    let app = realm_guard_server::build_app();

    // Une requête préalable pour générer au moins une métrique HTTP.
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        body.contains("http_requests_total"),
        "métriques inattendues : {body}"
    );
}
