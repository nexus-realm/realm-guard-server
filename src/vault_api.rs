//! Endpoints de la VaultKey enrobée (`/vault/key`), **gated par session** : un
//! compte ne peut lire/écrire que SA propre VaultKey. Le serveur ne voit qu'un
//! blob opaque + un sel (base64 sur le wire).

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::put;
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use crate::auth_api::AuthAccount;
use crate::state::AppState;
use crate::vault_keys;

/// Routes de la VaultKey (à monter dans l'app avec l'état).
pub fn routes() -> Router<AppState> {
    Router::new().route("/vault/key", put(store).get(fetch))
}

#[derive(Deserialize)]
struct StoreReq {
    wrapped_key: String,
    salt: String,
}

#[derive(Serialize)]
struct FetchResp {
    wrapped_key: String,
    salt: String,
}

fn decode(field: &str) -> Result<Vec<u8>, StatusCode> {
    STANDARD.decode(field).map_err(|_| StatusCode::BAD_REQUEST)
}

fn internal<E>(_error: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Stocke (ou remplace) la VaultKey enrobée du compte authentifié.
async fn store(
    account: AuthAccount,
    State(state): State<AppState>,
    Json(body): Json<StoreReq>,
) -> Result<StatusCode, StatusCode> {
    let wrapped = decode(&body.wrapped_key)?;
    let salt = decode(&body.salt)?;
    vault_keys::upsert(&state.db, account.0, &wrapped, &salt)
        .await
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Récupère la VaultKey enrobée du compte authentifié (404 si absente).
async fn fetch(
    account: AuthAccount,
    State(state): State<AppState>,
) -> Result<Json<FetchResp>, StatusCode> {
    let (wrapped, salt) = vault_keys::get(&state.db, account.0)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(FetchResp {
        wrapped_key: STANDARD.encode(wrapped),
        salt: STANDARD.encode(salt),
    }))
}
