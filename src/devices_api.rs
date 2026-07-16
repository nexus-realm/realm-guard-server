//! Endpoints du registre d'appareils (`/devices`), **gated par session**.
//!
//! Modèle : lors du pairing, l'appareil **source** (déjà en session) enregistre la
//! clé Ed25519 du nouvel appareil (`POST /devices`) — il « vouche » pour elle. Le
//! propriétaire du compte peut lister (`GET /devices`) et révoquer
//! (`DELETE /devices/{id}`). Le serveur ne stocke que des clés publiques + des noms.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth_api::AuthAccount;
use crate::devices;
use crate::state::AppState;

const DEVICE_PK_LEN: usize = 32;
const MAX_NAME_LEN: usize = 100;

/// Routes du registre d'appareils (à monter dans l'app avec l'état).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/devices", get(list).post(register))
        .route("/devices/{id}", delete(revoke).patch(rename))
}

#[derive(Deserialize)]
struct RegisterReq {
    device_pk: String,
    name: String,
}

#[derive(Serialize)]
struct RegisterResp {
    id: String,
}

#[derive(Deserialize)]
struct RenameReq {
    name: String,
}

#[derive(Serialize)]
struct DeviceResp {
    id: String,
    name: String,
    /// Clé publique (base64) — permet à un appareil de se reconnaître dans la liste.
    device_pk: String,
    created_at: i64,
    revoked: bool,
}

fn internal<E>(_error: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Décode + valide la clé publique d'appareil (base64 → 32 octets).
fn parse_device_pk(value: &str) -> Result<Vec<u8>, StatusCode> {
    let pk = STANDARD
        .decode(value)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if pk.len() != DEVICE_PK_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(pk)
}

/// Normalise + valide le nom d'appareil (non vide, ≤ [`MAX_NAME_LEN`] caractères).
fn validate_name(name: &str) -> Result<String, StatusCode> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_NAME_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(trimmed.to_string())
}

fn parse_id(value: &str) -> Result<Uuid, StatusCode> {
    Uuid::parse_str(value).map_err(|_| StatusCode::BAD_REQUEST)
}

/// Enregistre la clé d'un appareil sous le compte authentifié (la source vouche).
/// **409** si la clé appartient déjà à un autre compte.
async fn register(
    account: AuthAccount,
    State(state): State<AppState>,
    Json(body): Json<RegisterReq>,
) -> Result<(StatusCode, Json<RegisterResp>), StatusCode> {
    let device_pk = parse_device_pk(&body.device_pk)?;
    let name = validate_name(&body.name)?;
    let id = devices::register(&state.db, account.0, &device_pk, &name)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::CONFLICT)?;
    Ok((
        StatusCode::CREATED,
        Json(RegisterResp { id: id.to_string() }),
    ))
}

/// Liste les appareils du compte authentifié.
async fn list(
    account: AuthAccount,
    State(state): State<AppState>,
) -> Result<Json<Vec<DeviceResp>>, StatusCode> {
    let rows = devices::list(&state.db, account.0)
        .await
        .map_err(internal)?;
    let items = rows
        .into_iter()
        .map(|(id, name, device_pk, created_at, revoked)| DeviceResp {
            id: id.to_string(),
            name,
            device_pk: STANDARD.encode(device_pk),
            created_at,
            revoked,
        })
        .collect();
    Ok(Json(items))
}

/// Renomme un appareil du compte authentifié (**404** si inconnu).
async fn rename(
    account: AuthAccount,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RenameReq>,
) -> Result<StatusCode, StatusCode> {
    let device_id = parse_id(&id)?;
    let name = validate_name(&body.name)?;
    if devices::rename(&state.db, account.0, device_id, &name)
        .await
        .map_err(internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// Révoque un appareil du compte authentifié (**404** si inconnu / déjà révoqué).
async fn revoke(
    account: AuthAccount,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let device_id = parse_id(&id)?;
    if devices::revoke(&state.db, account.0, device_id)
        .await
        .map_err(internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_base64_device_pk() {
        assert_eq!(parse_device_pk("###").unwrap_err(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn rejects_wrong_length_device_pk() {
        let short = STANDARD.encode([0u8; 16]);
        assert_eq!(
            parse_device_pk(&short).unwrap_err(),
            StatusCode::BAD_REQUEST
        );
        let ok = STANDARD.encode([0u8; DEVICE_PK_LEN]);
        assert_eq!(parse_device_pk(&ok).unwrap().len(), DEVICE_PK_LEN);
    }

    #[test]
    fn validates_and_trims_name() {
        assert_eq!(validate_name("  iPhone  ").unwrap(), "iPhone");
        assert_eq!(validate_name("   ").unwrap_err(), StatusCode::BAD_REQUEST);
        let too_long = "a".repeat(MAX_NAME_LEN + 1);
        assert_eq!(
            validate_name(&too_long).unwrap_err(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn rejects_bad_uuid() {
        assert_eq!(parse_id("not-a-uuid").unwrap_err(), StatusCode::BAD_REQUEST);
    }
}
