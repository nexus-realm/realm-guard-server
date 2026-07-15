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
    realm_guard_server::run_migrations(&db_url)
        .await
        .expect("migrations (Postgres joignable ?)");
    // Le secret serveur OPAQUE est fourni hors base : on en génère un pour le test.
    let setup = auth::generate_server_setup();
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

    // --- VaultKey enrobée : upload (PUT) puis fetch (GET), gated par la session ---
    {
        let wrapped = vec![10u8, 11, 12, 13];
        let salt = vec![20u8, 21, 22];
        let put = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/vault/key")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(
                            &json!({ "wrapped_key": b64(&wrapped), "salt": b64(&salt) }),
                        )
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::NO_CONTENT);

        let got = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/vault/key")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(got.status(), StatusCode::OK);
        let bytes = to_bytes(got.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            STANDARD
                .decode(body["wrapped_key"].as_str().unwrap())
                .unwrap(),
            wrapped,
        );
        assert_eq!(
            STANDARD.decode(body["salt"].as_str().unwrap()).unwrap(),
            salt,
        );

        // Sans token → 401.
        let unauth = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/vault/key")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
    }

    // --- Registre d'appareils : enregistrement (source), liste, révocation, gating ---
    {
        let device_pk = b64(&[9u8; 32]);
        let register_body = json!({ "device_pk": device_pk, "name": "iPhone" });

        // Sans session → 401 (seule la source authentifiée enregistre).
        let (status, _) = post_json(&app, "/devices", register_body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Enregistrement authentifié → 201 + id.
        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/devices")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&register_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let bytes = to_bytes(created.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let device_id = body["id"].as_str().unwrap().to_string();

        // Liste → l'appareil est présent et actif.
        let listed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/devices")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let bytes = to_bytes(listed.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let entry = body
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == device_id)
            .expect("appareil listé");
        assert_eq!(entry["name"], "iPhone");
        assert_eq!(entry["revoked"], false);

        // Révocation → 204, puis seconde révocation → 404 (déjà révoqué).
        for (i, expected) in [StatusCode::NO_CONTENT, StatusCode::NOT_FOUND]
            .into_iter()
            .enumerate()
        {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("DELETE")
                        .uri(format!("/devices/{device_id}"))
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "révocation #{i}");
        }
    }

    // --- Relais de pairing : dépôt gated, récupération par capability, usage unique ---
    {
        let pairing_id = "3q2-7wDerb7v3q2-7wDerbc"; // base64url-safe, arbitraire
        let blob = vec![0xDEu8, 0xAD, 0xBE, 0xEF];

        // Sans session → 401 (seul un appareil authentifié dépose).
        let (status, _) = post_json(
            &app,
            &format!("/pairing/{pairing_id}"),
            json!({ "response": b64(&blob) }),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Dépôt authentifié → 204.
        let deposit = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/pairing/{pairing_id}"))
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({ "response": b64(&blob) })).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deposit.status(), StatusCode::NO_CONTENT);

        // Récupération SANS session (capability = pairing_id) → 200, blob intact.
        let got = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/pairing/{pairing_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(got.status(), StatusCode::OK);
        let bytes = to_bytes(got.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(unb64(&body, "response"), blob);

        // Usage unique : une seconde récupération → 404.
        let again = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/pairing/{pairing_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(again.status(), StatusCode::NOT_FOUND);

        // Identifiant hors charset (pas de clé Redis arbitraire) → 400.
        let bad = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/pairing/a%2Fb")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    }

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

    // --- Rate-limit par compte : 10 échecs → verrouillage (429 + Retry-After) ---
    {
        let victim = format!("lock-{}", uuid::Uuid::new_v4());
        for _ in 0..10 {
            let login = auth::client_login_start(b"peu-importe").unwrap();
            let (status, body) = post_json(
                &app,
                "/auth/login/start",
                json!({ "username": victim, "request": b64(&login.request) }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let flow_id = body["flow_id"].as_str().unwrap().to_string();
            // Finalisation bidon → échec compté pour ce username.
            let (status, _) = post_json(
                &app,
                "/auth/login/finish",
                json!({ "flow_id": flow_id, "finalization": b64(&[0u8; 64]) }),
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }

        // 11e tentative : compte verrouillé → 429 + en-tête Retry-After.
        let login = auth::client_login_start(b"peu-importe").unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/login/start")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(
                            &json!({ "username": victim, "request": b64(&login.request) }),
                        )
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(response.headers().contains_key("retry-after"));
    }
}
