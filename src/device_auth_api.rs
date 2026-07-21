//! Authentification par **clé d'appareil** (Ed25519 challenge-response).
//!
//! Un appareil appairé — qui ne connaît **pas** le mot de passe du compte — obtient
//! une session en signant un défi avec sa clé privée. Le serveur vérifie la
//! signature (`realm_guard_core::crypto::device_verify`) puis n'émet une session que
//! si l'appareil est **enregistré et non révoqué** dans le registre (`devices`).
//!
//! - `POST /auth/device/challenge` : **ouvert**. Renvoie un nonce aléatoire (usage
//!   unique, TTL court) lié à la clé publique fournie. Ne révèle pas si l'appareil
//!   est connu (pas d'oracle d'énumération).
//! - `POST /auth/device/verify` : **ouvert**. Signature valide **et** appareil actif
//!   → token de session ; sinon **401** uniforme.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use realm_guard_core::crypto::device_verify;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use crate::state::AppState;
use crate::{devices, sessions};

const DEVICE_PK_LEN: usize = 32;
const NONCE_LEN: usize = 32;
/// TTL du défi (court : la réponse de l'appareil est immédiate).
const CHALLENGE_TTL_SECS: u64 = 60;

/// Routes de l'auth par clé d'appareil (à monter dans l'app avec l'état).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/device/challenge", post(challenge))
        .route("/auth/device/verify", post(verify))
}

#[derive(Deserialize)]
struct ChallengeReq {
    device_pk: String,
}

#[derive(Serialize)]
struct ChallengeResp {
    challenge: String,
}

#[derive(Deserialize)]
struct VerifyReq {
    device_pk: String,
    signature: String,
}

#[derive(Serialize)]
struct VerifyResp {
    session_token: String,
}

fn internal<E>(_error: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Décode + valide la clé publique d'appareil (base64 → 32 octets).
fn decode_pk(value: &str) -> Result<Vec<u8>, StatusCode> {
    let pk = STANDARD
        .decode(value)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if pk.len() != DEVICE_PK_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(pk)
}

/// Clé Redis du défi, dérivée de la clé publique (base64) — charset borné et sûr.
fn challenge_key(device_pk: &[u8]) -> String {
    format!("devauth:{}", STANDARD.encode(device_pk))
}

/// Émet un défi aléatoire lié à la clé publique (usage unique, TTL court). Ouvert :
/// l'appareil n'est pas encore authentifié.
async fn challenge(
    State(state): State<AppState>,
    Json(body): Json<ChallengeReq>,
) -> Result<Json<ChallengeResp>, StatusCode> {
    let device_pk = decode_pk(&body.device_pk)?;
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(internal)?;
    let mut conn = state.redis().await.map_err(internal)?;
    let _: () = conn
        .set_ex(
            challenge_key(&device_pk),
            nonce.as_slice(),
            CHALLENGE_TTL_SECS,
        )
        .await
        .map_err(internal)?;
    Ok(Json(ChallengeResp {
        challenge: STANDARD.encode(nonce),
    }))
}

/// Vérifie la signature du défi ; si valide **et** appareil actif, émet une session.
/// Échec uniforme **401** (signature invalide, défi absent/expiré, appareil inconnu
/// ou révoqué) — pas d'oracle d'existence.
async fn verify(
    State(state): State<AppState>,
    Json(body): Json<VerifyReq>,
) -> Result<Json<VerifyResp>, StatusCode> {
    let device_pk = decode_pk(&body.device_pk)?;
    let signature = STANDARD
        .decode(&body.signature)
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    let mut conn = state.redis().await.map_err(internal)?;
    // Usage unique : le défi est consommé, valide ou non (anti-rejeu).
    let nonce: Option<Vec<u8>> = conn
        .get_del(challenge_key(&device_pk))
        .await
        .map_err(internal)?;
    let nonce = nonce.ok_or(StatusCode::UNAUTHORIZED)?;

    if !device_verify(&device_pk, &nonce, &signature) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // La signature prouve la possession de la clé ; reste à ce que l'appareil soit
    // enregistré et actif (non révoqué) pour émettre une session sur son compte.
    let account_id = devices::active_account_for_pk(&state.db, &device_pk)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let token = sessions::create_session(conn, account_id)
        .await
        .map_err(internal)?;
    Ok(Json(VerifyResp {
        session_token: token,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wrong_length_pk() {
        let short = STANDARD.encode([0u8; 16]);
        assert_eq!(decode_pk(&short).unwrap_err(), StatusCode::BAD_REQUEST);
        let ok = STANDARD.encode([0u8; DEVICE_PK_LEN]);
        assert_eq!(decode_pk(&ok).unwrap().len(), DEVICE_PK_LEN);
    }

    #[test]
    fn challenge_key_is_stable_and_scoped() {
        let key = challenge_key(&[1u8; DEVICE_PK_LEN]);
        assert!(key.starts_with("devauth:"));
        assert_eq!(key, challenge_key(&[1u8; DEVICE_PK_LEN]));
    }
}
