//! Endpoints d'authentification **OPAQUE** + session (Bearer).
//!
//! Register/login se déroulent en deux temps (start/finish). L'état serveur
//! transitoire de connexion vit dans Redis (usage unique). Un login réussi émet
//! un token de session (Redis). Les champs binaires transitent en **base64**.

use axum::extract::{FromRequestParts, State};
use axum::http::StatusCode;
use axum::http::header::{AUTHORIZATION, RETRY_AFTER};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use realm_guard_core::{auth, codec};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::AppState;
use crate::{accounts, observability, rate_limit, sessions};

/// Routes d'authentification (à monter dans l'app avec l'état).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/register/start", post(register_start))
        .route("/auth/register/finish", post(register_finish))
        .route("/auth/login/start", post(login_start))
        .route("/auth/login/finish", post(login_finish))
        .route("/auth/me", get(me))
}

#[derive(Deserialize)]
struct RegisterStartReq {
    username: String,
    request: String,
}

#[derive(Serialize)]
struct MessageResp {
    response: String,
}

#[derive(Deserialize)]
struct RegisterFinishReq {
    username: String,
    upload: String,
}

#[derive(Deserialize)]
struct LoginStartReq {
    username: String,
    request: String,
}

#[derive(Serialize)]
struct LoginStartResp {
    response: String,
    flow_id: String,
}

#[derive(Deserialize)]
struct LoginFinishReq {
    flow_id: String,
    finalization: String,
}

#[derive(Serialize)]
struct LoginFinishResp {
    session_token: String,
}

#[derive(Serialize)]
struct MeResp {
    account_id: String,
}

/// État de connexion transitoire stocké en Redis (sérialisé postcard).
#[derive(Serialize, Deserialize)]
struct LoginFlow {
    state: Vec<u8>,
    account_id: Option<Uuid>,
    /// Username tel qu'envoyé au `login/start` — sert à clé le rate-limit au
    /// `finish` (le corps du `finish` ne porte que `flow_id` + `finalization`).
    username: String,
}

fn decode(field: &str) -> Result<Vec<u8>, StatusCode> {
    STANDARD.decode(field).map_err(|_| StatusCode::BAD_REQUEST)
}

fn internal<E>(_error: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

async fn register_start(
    State(state): State<AppState>,
    Json(body): Json<RegisterStartReq>,
) -> Result<Json<MessageResp>, StatusCode> {
    let request = decode(&body.request)?;
    // Le 409 révèle l'existence d'un username (énumération de comptes à
    // l'inscription) — **compromis accepté** : l'inscription doit bien dire « nom
    // déjà pris ». La vitesse d'énumération est bornée par le rate-limit par-IP au
    // proxy ; le *login*, lui, reste anti-énumération. Cf. `SECURITY.md`.
    if accounts::username_taken(&state.db, &body.username)
        .await
        .map_err(internal)?
    {
        return Err(StatusCode::CONFLICT);
    }
    let response =
        auth::server_register_start(&state.opaque_setup, &request, body.username.as_bytes())
            .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Json(MessageResp {
        response: STANDARD.encode(response),
    }))
}

async fn register_finish(
    State(state): State<AppState>,
    Json(body): Json<RegisterFinishReq>,
) -> Result<StatusCode, StatusCode> {
    let upload = decode(&body.upload)?;
    let password_file =
        auth::server_register_finish(&upload).map_err(|_| StatusCode::BAD_REQUEST)?;
    // La contrainte d'unicité tranche une éventuelle course sur le username.
    accounts::insert_account(&state.db, &body.username, &password_file)
        .await
        .map_err(|_| StatusCode::CONFLICT)?;
    observability::record_registration();
    Ok(StatusCode::CREATED)
}

async fn login_start(
    State(state): State<AppState>,
    Json(body): Json<LoginStartReq>,
) -> Result<Response, StatusCode> {
    let request = decode(&body.request)?;
    let conn = state.redis().await.map_err(internal)?;

    // Rate-limit **par compte** (complément du par-IP au proxy) : si le compte est
    // verrouillé, on refuse tôt (429 + Retry-After) sans travail OPAQUE.
    if let Some(retry) = rate_limit::locked_for(conn.clone(), &body.username)
        .await
        .map_err(internal)?
    {
        observability::record_login_lockout();
        return Ok((
            StatusCode::TOO_MANY_REQUESTS,
            [(RETRY_AFTER, retry.to_string())],
        )
            .into_response());
    }

    // Utilisateur inconnu → (None, None) : réponse fabriquée (anti-énumération).
    let (account_id, password_file) = match accounts::account_credentials(&state.db, &body.username)
        .await
        .map_err(internal)?
    {
        Some((id, file)) => (Some(id), Some(file)),
        None => (None, None),
    };
    let started = auth::server_login_start(
        &state.opaque_setup,
        password_file.as_deref(),
        &request,
        body.username.as_bytes(),
    )
    .map_err(|_| StatusCode::BAD_REQUEST)?;

    let flow_id = Uuid::new_v4().simple().to_string();
    let flow = LoginFlow {
        state: started.state,
        account_id,
        username: body.username,
    };
    let blob = codec::encode(&flow).map_err(internal)?;
    sessions::store_login_flow(conn, &flow_id, &blob)
        .await
        .map_err(internal)?;

    Ok(Json(LoginStartResp {
        response: STANDARD.encode(started.response),
        flow_id,
    })
    .into_response())
}

async fn login_finish(
    State(state): State<AppState>,
    Json(body): Json<LoginFinishReq>,
) -> Result<Json<LoginFinishResp>, StatusCode> {
    let finalization = decode(&body.finalization)?;
    let conn = state.redis().await.map_err(internal)?;
    let blob = sessions::take_login_flow(conn.clone(), &body.flow_id)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let flow: LoginFlow = codec::decode(&blob).map_err(internal)?;

    // Valide la finalisation : échoue si mot de passe faux ou user inconnu → on
    // compte l'échec pour ce compte (rate-limit par compte).
    if auth::server_login_finish(&flow.state, &finalization).is_err() {
        rate_limit::record_failure(conn, &flow.username)
            .await
            .map_err(internal)?;
        observability::record_login(false);
        return Err(StatusCode::UNAUTHORIZED);
    }
    let account_id = flow.account_id.ok_or(StatusCode::UNAUTHORIZED)?;

    // Succès : on remet le compteur d'échecs à zéro, puis on émet la session.
    rate_limit::reset(conn.clone(), &flow.username)
        .await
        .map_err(internal)?;
    observability::record_login(true);
    let token = sessions::create_session(conn, account_id)
        .await
        .map_err(internal)?;
    Ok(Json(LoginFinishResp {
        session_token: token,
    }))
}

async fn me(account: AuthAccount) -> Json<MeResp> {
    Json(MeResp {
        account_id: account.0.to_string(),
    })
}

/// Extracteur : exige un token de session valide (`Authorization: Bearer <token>`).
pub struct AuthAccount(pub Uuid);

impl FromRequestParts<AppState> for AuthAccount {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, StatusCode> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let account_id = sessions::session_account(state.redis().await.map_err(internal)?, token)
            .await
            .map_err(internal)?
            .ok_or(StatusCode::UNAUTHORIZED)?;
        Ok(Self(account_id))
    }
}
