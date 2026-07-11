//! Test d'intégration du flow d'auth OPAQUE complet (register → login → session).
//!
//! **Nécessite Postgres + Redis joignables** → marqué `#[ignore]`. Surcharger
//! `DATABASE_URL` / `REDIS_URL` vers une base **exposée sur l'hôte** (attention aux
//! conflits de port avec un Postgres/Redis natif). `sslmode=disable` par défaut :
//! les tests n'installent pas le provider crypto rustls (contrairement à `main`).
//! Ex. : `DATABASE_URL=postgres://realmguard:realmguard@localhost:55432/realmguard?sslmode=disable
//! REDIS_URL=redis://localhost:56379 cargo test --test auth_flow -- --ignored`.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use realm_guard_core::auth;
use serde_json::{Value, json};
use tower::ServiceExt;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

async fn build_app() -> Router {
    let db_url = env_or(
        "DATABASE_URL",
        "postgres://realmguard:realmguard@localhost:5432/realmguard?sslmode=disable",
    );
    let redis_url = env_or("REDIS_URL", "redis://localhost:6379");
    let setup = realm_guard_server::migrate_and_bootstrap(&db_url)
        .await
        .expect("migrations + bootstrap (Postgres joignable ?)");
    let state = realm_guard_server::AppState::connect(&db_url, &redis_url, setup).expect("état");
    realm_guard_server::build_app(state)
}

async fn post_json(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn unb64(value: &Value, field: &str) -> Vec<u8> {
    STANDARD.decode(value[field].as_str().unwrap()).unwrap()
}

#[tokio::test]
#[ignore = "nécessite Postgres + Redis (stack Docker)"]
async fn full_opaque_auth_flow() {
    let app = build_app().await;
    let username = format!("it-{}", uuid::Uuid::new_v4());
    let password = b"correct horse battery staple";

    // --- Enregistrement ---
    let reg = auth::client_register_start(password).unwrap();
    let (status, body) = post_json(
        &app,
        "/auth/register/start",
        json!({ "username": username, "request": b64(&reg.request) }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "register/start: {body:?}");
    let reg_finish =
        auth::client_register_finish(&reg.state, password, &unb64(&body, "response")).unwrap();
    let (status, _) = post_json(
        &app,
        "/auth/register/finish",
        json!({ "username": username, "upload": b64(&reg_finish.upload) }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // --- Connexion (bon mot de passe) ---
    let login = auth::client_login_start(password).unwrap();
    let (status, body) = post_json(
        &app,
        "/auth/login/start",
        json!({ "username": username, "request": b64(&login.request) }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let flow_id = body["flow_id"].as_str().unwrap().to_string();
    let login_finish =
        auth::client_login_finish(&login.state, password, &unb64(&body, "response")).unwrap();
    let (status, body) = post_json(
        &app,
        "/auth/login/finish",
        json!({ "flow_id": flow_id, "finalization": b64(&login_finish.finalization) }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = body["session_token"].as_str().unwrap().to_string();

    // --- Session : /auth/me avec le token ---
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/me")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // --- /auth/me sans token → 401 ---
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // --- Finalisation invalide (mauvais mdp / user inconnu) → 401 ---
    let login = auth::client_login_start(b"mauvais-mot-de-passe").unwrap();
    let (_, body) = post_json(
        &app,
        "/auth/login/start",
        json!({ "username": username, "request": b64(&login.request) }),
    )
    .await;
    let flow_id = body["flow_id"].as_str().unwrap().to_string();
    let (status, _) = post_json(
        &app,
        "/auth/login/finish",
        json!({ "flow_id": flow_id, "finalization": b64(&[0u8; 64]) }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
