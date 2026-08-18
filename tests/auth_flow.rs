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
        // Clé **unique par exécution** : une clé codée en dur rendrait le test
        // non rejouable — au 2ᵉ passage, le compte neuf réclamerait la clé du compte
        // précédent, et le serveur la refuserait (409, garde-fou cross-compte).
        let mut pk = [0u8; 32];
        pk[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        pk[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        let device_pk = b64(&pk);
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
        // La clé publique est renvoyée : un appareil peut se reconnaître dans la
        // liste (« cet appareil ») sans pouvoir se confondre avec un autre.
        assert_eq!(entry["device_pk"], device_pk);

        // Renommage (PATCH) → 204, puis la liste reflète le nouveau nom.
        let renamed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/devices/{device_id}"))
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({ "name": "iPhone de Sacha" })).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(renamed.status(), StatusCode::NO_CONTENT);

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

    // --- Auth par clé d'appareil : register → challenge → sign → verify → session ---
    {
        use realm_guard_core::crypto::{device_sign, generate_device_keypair};

        let keypair = generate_device_keypair().unwrap();

        // La source (session courante) enregistre la clé du nouvel appareil.
        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/devices")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(
                            &json!({ "device_pk": b64(&keypair.public), "name": "Laptop" }),
                        )
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);

        // Défi → signature → vérification → session d'appareil.
        let (status, body) = post_json(
            &app,
            "/auth/device/challenge",
            json!({ "device_pk": b64(&keypair.public) }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let nonce = unb64(&body, "challenge");
        let signature = device_sign(&keypair.secret, &nonce).unwrap();
        let (status, body) = post_json(
            &app,
            "/auth/device/verify",
            json!({ "device_pk": b64(&keypair.public), "signature": b64(&signature) }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let device_token = body["session_token"].as_str().unwrap().to_string();

        // La session de l'appareil est utilisable.
        let me = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/me")
                    .header("authorization", format!("Bearer {device_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(me.status(), StatusCode::OK);

        // Nouveau défi + mauvaise signature → 401 (usage unique côté serveur).
        let (_, body) = post_json(
            &app,
            "/auth/device/challenge",
            json!({ "device_pk": b64(&keypair.public) }),
        )
        .await;
        let _ = unb64(&body, "challenge");
        let (status, _) = post_json(
            &app,
            "/auth/device/verify",
            json!({ "device_pk": b64(&keypair.public), "signature": b64(&[0u8; 64]) }),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    // --- Log de deltas : push, pull par curseur, pagination, gating ---
    {
        // Sans session → 401 (le log est privé au compte).
        let (status, _) =
            post_json(&app, "/sync/deltas", json!({ "payload": b64(&[1, 2, 3]) })).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Pousse trois deltas opaques.
        let mut seqs = Vec::new();
        for payload in [vec![1u8, 1], vec![2u8, 2], vec![3u8, 3]] {
            let pushed = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/sync/deltas")
                        .header("authorization", format!("Bearer {token}"))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            serde_json::to_vec(&json!({ "payload": b64(&payload) })).unwrap(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(pushed.status(), StatusCode::CREATED);
            let bytes = to_bytes(pushed.into_body(), usize::MAX).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            seqs.push(body["seq"].as_i64().unwrap());
        }
        // Le curseur est strictement monotone.
        assert!(seqs[0] < seqs[1] && seqs[1] < seqs[2]);

        // Tirage depuis le début du compte : on retrouve les trois, dans l'ordre.
        let pulled = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/sync/deltas?since={}", seqs[0] - 1))
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(pulled.status(), StatusCode::OK);
        let bytes = to_bytes(pulled.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let items = body["deltas"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(unb64(&items[0], "payload"), vec![1u8, 1]);
        assert_eq!(items[2]["seq"].as_i64().unwrap(), seqs[2]);
        assert_eq!(body["latest"].as_i64().unwrap(), seqs[2]);

        // Curseur au dernier seq → rien de neuf (le client est à jour).
        let empty = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/sync/deltas?since={}", seqs[2]))
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(empty.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["deltas"].as_array().unwrap().is_empty());
        assert_eq!(body["latest"].as_i64().unwrap(), seqs[2]);

        // Pagination : `limit` borne la page, `latest` dit qu'il reste à tirer.
        let page = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/sync/deltas?since={}&limit=1", seqs[0] - 1))
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(page.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["deltas"].as_array().unwrap().len(), 1);
        assert_eq!(body["latest"].as_i64().unwrap(), seqs[2]);

        // Payload vide → 400 (garde-fou).
        let (status, _) = {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/sync/deltas")
                        .header("authorization", format!("Bearer {token}"))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            serde_json::to_vec(&json!({ "payload": "" })).unwrap(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            (resp.status(), ())
        };
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // --- Snapshot + compaction : le curseur trop ancien doit échouer bruyamment ---
    {
        // Curseur du compte à ce stade (les deltas du bloc précédent sont là).
        let cursor_before = {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/sync/deltas?since=0")
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            body["latest"].as_i64().unwrap()
        };

        // Pas encore de snapshot → 404.
        let none = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/sync/snapshot")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(none.status(), StatusCode::NOT_FOUND);

        let snapshot = vec![42u8; 64];
        let put_snapshot = |payload: Vec<u8>, covers: i64| {
            let app = app.clone();
            let token = token.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method("PUT")
                        .uri("/sync/snapshot")
                        .header("authorization", format!("Bearer {token}"))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            serde_json::to_vec(
                                &json!({ "payload": b64(&payload), "covers_seq": covers }),
                            )
                            .unwrap(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };

        // Prétendre couvrir au-delà du log → 400 (masquerait des deltas à venir).
        let too_far = put_snapshot(snapshot.clone(), cursor_before + 100).await;
        assert_eq!(too_far.status(), StatusCode::BAD_REQUEST);

        // Publication légitime → compacte les deltas couverts.
        let published = put_snapshot(snapshot.clone(), cursor_before).await;
        assert_eq!(published.status(), StatusCode::OK);
        let bytes = to_bytes(published.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["purged"].as_u64().unwrap() >= 3, "log compacté");

        // Relecture du snapshot.
        let got = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/sync/snapshot")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(got.status(), StatusCode::OK);
        let bytes = to_bytes(got.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(unb64(&body, "payload"), snapshot);
        assert_eq!(body["covers_seq"].as_i64().unwrap(), cursor_before);

        // **Le point crucial** : un curseur antérieur au snapshot → 410, jamais une
        // page tronquée. Sans ça, l'appareil raterait tout le passé en silence.
        let stale = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/sync/deltas?since=0")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::GONE);

        // Curseur à jour (= covers_seq) → tirage normal.
        let fresh = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/sync/deltas?since={cursor_before}"))
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(fresh.status(), StatusCode::OK);

        // Republier au **même** covers_seq reste possible une fois le log compacté :
        // le repère haut tient compte de l'historique déjà couvert, pas seulement
        // des deltas restants (qui viennent d'être purgés).
        let republished = put_snapshot(vec![43u8; 64], cursor_before).await;
        assert_eq!(republished.status(), StatusCode::OK);

        // Régression de covers_seq → 409 (compacter sur un état antérieur perdrait
        // les deltas intermédiaires).
        if cursor_before > 0 {
            let older = put_snapshot(snapshot.clone(), cursor_before - 1).await;
            assert_eq!(older.status(), StatusCode::CONFLICT);
        }

        // Un delta postérieur au snapshot survit à la compaction et reste tirable.
        let (status, body) = {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/sync/deltas")
                        .header("authorization", format!("Bearer {token}"))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            serde_json::to_vec(&json!({ "payload": b64(&[7u8, 7]) })).unwrap(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = resp.status();
            let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap())
        };
        assert_eq!(status, StatusCode::CREATED);
        let new_seq = body["seq"].as_i64().unwrap();

        let after = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/sync/deltas?since={cursor_before}"))
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(after.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let items = body["deltas"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["seq"].as_i64().unwrap(), new_seq);
        assert_eq!(unb64(&items[0], "payload"), vec![7u8, 7]);
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

    // --- Métriques métier (P4) : les opérations ci-dessus ont dû incrémenter les
    // compteurs. On scrape `/metrics` (même recorder global que les handlers) et on
    // vérifie que chaque compteur est présent et non nul — preuve que les handlers
    // appellent bien les recorders (le test unitaire, lui, ne couvre que les
    // recorders isolés). ---
    {
        let response = app
            .clone()
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
        let metrics = String::from_utf8(bytes.to_vec()).unwrap();

        // Valeur d'une ligne Prometheus `<needle> <valeur>` (les commentaires
        // `# TYPE`/`# HELP` commencent par `#`, jamais par le nom nu → ignorés).
        let value = |needle: &str| -> f64 {
            metrics
                .lines()
                .find(|l| l.starts_with(needle))
                .and_then(|l| l.split_whitespace().last())
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or_else(|| panic!("métrique absente : {needle}\n{metrics}"))
        };

        assert!(value("rg_auth_registrations_total") >= 1.0);
        assert!(value("rg_auth_logins_total{outcome=\"success\"}") >= 1.0);
        assert!(value("rg_auth_logins_total{outcome=\"failure\"}") >= 1.0);
        assert!(value("rg_auth_lockouts_total") >= 1.0);
        assert!(value("rg_sync_deltas_pushed_total") >= 1.0);
        assert!(value("rg_sync_deltas_pulled_total") >= 1.0);
        assert!(value("rg_sync_snapshots_created_total") >= 1.0);
    }
}
