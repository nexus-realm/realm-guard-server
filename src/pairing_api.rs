//! Relais de pairing : **transport opaque** du blob scellé entre l'appareil source
//! (authentifié) et le nouvel appareil. Le serveur ne voit qu'un blob chiffré ; il
//! ne peut ni le lire ni l'exploiter (scellé vers la clé éphémère du nouvel appareil,
//! cf. `realm_guard_core::crypto::pairing`).
//!
//! - **Dépôt** (`POST /pairing/{id}`) : gated par session — seul un appareil déjà
//!   authentifié (donc porteur de la VaultKey) dépose.
//! - **Récupération** (`GET /pairing/{id}`) : **sans session** — le nouvel appareil
//!   n'en a pas encore ; la *capability* est le `pairing_id` (128 bits, affiché en
//!   QR). **Usage unique** (`GETDEL`), TTL court.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use crate::auth_api::AuthAccount;
use crate::state::AppState;

/// TTL du blob de pairing (secondes) — le pairing est interactif et court.
const PAIRING_TTL_SECS: u64 = 300;

/// Routes du relais de pairing (à monter dans l'app avec l'état).
pub fn routes() -> Router<AppState> {
    Router::new().route("/pairing/{id}", post(deposit).get(fetch))
}

#[derive(Deserialize)]
struct DepositReq {
    response: String,
}

#[derive(Serialize)]
struct FetchResp {
    response: String,
}

fn internal<E>(_error: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Construit la clé Redis en validant le `pairing_id` (borné, charset base64url-safe)
/// pour ne pas fabriquer de clé Redis arbitraire depuis l'URL.
fn redis_key(id: &str) -> Result<String, StatusCode> {
    let valid = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(format!("pairing:{id}"))
    } else {
        Err(StatusCode::BAD_REQUEST)
    }
}

/// Dépose le blob scellé pour `id` (source **authentifiée**). Un dépôt écrase le
/// précédent.
async fn deposit(
    _account: AuthAccount,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<DepositReq>,
) -> Result<StatusCode, StatusCode> {
    let key = redis_key(&id)?;
    let blob = STANDARD
        .decode(&body.response)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut conn = state.redis().await.map_err(internal)?;
    let _: () = conn
        .set_ex(key, blob, PAIRING_TTL_SECS)
        .await
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Récupère **et supprime** (usage unique) le blob de pairing. Pas de session : la
/// capability est le `pairing_id`.
async fn fetch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<FetchResp>, StatusCode> {
    let key = redis_key(&id)?;
    let mut conn = state.redis().await.map_err(internal)?;
    let blob: Option<Vec<u8>> = conn.get_del(key).await.map_err(internal)?;
    let blob = blob.ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(FetchResp {
        response: STANDARD.encode(blob),
    }))
}
