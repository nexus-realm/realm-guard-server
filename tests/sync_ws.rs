//! Smoke temps réel du WebSocket de réveil (P3.2b) : un `push` HTTP doit réveiller un
//! abonné WebSocket via le fan-out Redis.
//!
//! **Nécessite Postgres + Redis joignables** → `#[ignore]` (mêmes URLs que
//! `auth_flow`). Exerce le **vrai** handler axum : l'app est servie sur une socket
//! réelle, un client WS s'y connecte, et un delta poussé par le handler HTTP (même
//! `AppState`, donc même Redis) doit produire un nudge sur le WS.

use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::StreamExt;
use realm_guard_server::{AppState, build_app, run_migrations, sessions};
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tower::ServiceExt;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Prépare l'état + un compte et sa session. On **injecte** la session directement
/// (pas besoin du protocole OPAQUE pour ce smoke) : la session est juste une clé Redis
/// `session:<token>` → account_id.
async fn setup() -> (AppState, String) {
    let db_url = env_or(
        "DATABASE_URL",
        "postgres://realmguard:realmguard@localhost:5432/realmguard?sslmode=disable",
    );
    let redis_url = env_or("REDIS_URL", "redis://localhost:6379");
    run_migrations(&db_url)
        .await
        .expect("migrations (Postgres joignable ?)");
    let setup = realm_guard_core::auth::generate_server_setup();
    let state = AppState::connect(&db_url, &redis_url, setup).expect("état");

    let account_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO accounts (id, username, password_file) VALUES ($1, $2, $3)")
        .bind(account_id)
        .bind(format!("ws-{account_id}"))
        .bind(vec![0u8])
        .execute(&state.db)
        .await
        .expect("insertion du compte");
    let conn = state.redis().await.expect("connexion Redis");
    let token = sessions::create_session(conn, account_id)
        .await
        .expect("création de session");
    (state, token)
}

#[tokio::test]
#[ignore = "nécessite Postgres + Redis (stack Docker)"]
async fn push_wakes_ws_subscriber() {
    let (state, token) = setup().await;

    // Le WS a besoin d'une vraie socket : on sert l'app sur un port éphémère.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = build_app(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, served.into_make_service())
            .await
            .unwrap();
    });

    // 1) Sans token → le handshake doit être refusé (gating de session).
    let no_auth = connect_async(format!("ws://{addr}/sync/ws")).await;
    assert!(no_auth.is_err(), "le WS doit être gated par session");

    // 2) Avec token → connexion établie.
    let mut request = format!("ws://{addr}/sync/ws")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
    let (mut ws, _response) = connect_async(request).await.expect("connexion WS");

    // Laisse le handler s'abonner à Redis avant de publier (pub/sub = fire-and-forget :
    // un nudge publié avant l'abonnement serait perdu).
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Pousse un delta via le **vrai handler HTTP** (même AppState → même Redis).
    let push_app = build_app(state.clone());
    let pushed = push_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sync/deltas")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(
                        &serde_json::json!({ "payload": STANDARD.encode([1u8, 2, 3]) }),
                    )
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pushed.status(), StatusCode::CREATED);
    let bytes = to_bytes(pushed.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let seq = body["seq"].as_i64().unwrap();

    // Le WS doit recevoir le nudge : le `seq` en texte.
    let frame = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timeout : aucun nudge reçu sur le WS")
        .expect("flux WS clos prématurément")
        .expect("erreur de trame WS");
    match frame {
        Message::Text(text) => assert_eq!(text.as_str(), seq.to_string()),
        other => panic!("trame inattendue : {other:?}"),
    }
}
