//! Test d'intégration de la sonde `/healthz` (sans ouvrir de socket).

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // apporte `oneshot`

#[tokio::test]
async fn healthz_returns_ok_and_core_version() {
    let state = realm_guard_server::AppState::connect(
        "postgres://realmguard:realmguard@localhost/realmguard",
        "redis://localhost:6379",
        Vec::new(),
    )
    .unwrap();
    let app = realm_guard_server::build_app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        body.contains("\"status\":\"ok\""),
        "corps inattendu : {body}"
    );
    // Le champ est renseigné par realm_guard_core → valide le lien de dépendance.
    assert!(
        body.contains("\"core_version\""),
        "corps inattendu : {body}"
    );
}
