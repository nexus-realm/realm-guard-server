//! Synchronisation CRDT (`/sync/deltas`), **gated par session**.
//!
//! Modèle **delta-interval** : un appareil *pousse* les deltas qu'il produit et *tire*
//! ceux des autres depuis un curseur `seq`. Le serveur est un **log opaque** — il ne
//! déchiffre ni ne fusionne rien ; tout le merge est côté client.
//!
//! Le cœur garantit que la convergence survit au désordre et aux doublons : le
//! transport peut donc être **at-least-once** sans dédoublonnage. Un client qui rejoue
//! une page déjà vue converge quand même.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use crate::auth_api::AuthAccount;
use crate::deltas;
use crate::state::AppState;

/// Borne d'un delta (garde-fou anti-abus ; un delta de coffre est petit).
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
/// Taille de page par défaut / maximale au tirage.
const DEFAULT_LIMIT: i64 = 500;
const MAX_LIMIT: i64 = 1000;

/// Routes de synchronisation (à monter dans l'app avec l'état).
pub fn routes() -> Router<AppState> {
    Router::new().route("/sync/deltas", post(push).get(pull))
}

#[derive(Deserialize)]
struct PushReq {
    payload: String,
}

#[derive(Serialize)]
struct PushResp {
    seq: i64,
}

#[derive(Deserialize)]
struct PullQuery {
    since: Option<i64>,
    limit: Option<i64>,
}

#[derive(Serialize)]
struct DeltaResp {
    seq: i64,
    payload: String,
}

#[derive(Serialize)]
struct PullResp {
    deltas: Vec<DeltaResp>,
    /// Plus grand `seq` du compte : le client sait s'il reste des pages à tirer.
    latest: i64,
}

fn internal<E>(_error: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Décode + borne un delta (base64 → octets opaques).
fn decode_payload(value: &str) -> Result<Vec<u8>, StatusCode> {
    let payload = STANDARD
        .decode(value)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if payload.is_empty() || payload.len() > MAX_PAYLOAD_BYTES {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(payload)
}

/// Normalise le curseur : un `since` négatif n'a pas de sens (le log part à 1).
fn normalize_since(since: Option<i64>) -> i64 {
    since.unwrap_or(0).max(0)
}

/// Normalise la taille de page dans `1..=MAX_LIMIT`.
fn normalize_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// Pousse un delta au log du compte authentifié.
async fn push(
    account: AuthAccount,
    State(state): State<AppState>,
    Json(body): Json<PushReq>,
) -> Result<(StatusCode, Json<PushResp>), StatusCode> {
    let payload = decode_payload(&body.payload)?;
    let seq = deltas::append(&state.db, account.0, &payload)
        .await
        .map_err(internal)?;
    Ok((StatusCode::CREATED, Json(PushResp { seq })))
}

/// Tire les deltas du compte authentifié postérieurs au curseur `since`.
async fn pull(
    account: AuthAccount,
    State(state): State<AppState>,
    Query(query): Query<PullQuery>,
) -> Result<Json<PullResp>, StatusCode> {
    let since = normalize_since(query.since);
    let limit = normalize_limit(query.limit);

    let rows = deltas::since(&state.db, account.0, since, limit)
        .await
        .map_err(internal)?;
    let latest = deltas::latest_seq(&state.db, account.0)
        .await
        .map_err(internal)?;

    Ok(Json(PullResp {
        deltas: rows
            .into_iter()
            .map(|(seq, payload)| DeltaResp {
                seq,
                payload: STANDARD.encode(payload),
            })
            .collect(),
        latest,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_or_oversized_payload() {
        assert_eq!(decode_payload("").unwrap_err(), StatusCode::BAD_REQUEST);
        let huge = STANDARD.encode(vec![0u8; MAX_PAYLOAD_BYTES + 1]);
        assert_eq!(decode_payload(&huge).unwrap_err(), StatusCode::BAD_REQUEST);
        let ok = STANDARD.encode([1u8, 2, 3]);
        assert_eq!(decode_payload(&ok).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn rejects_non_base64_payload() {
        assert_eq!(decode_payload("###").unwrap_err(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn since_cursor_is_clamped_to_zero() {
        assert_eq!(normalize_since(None), 0);
        assert_eq!(normalize_since(Some(-5)), 0);
        assert_eq!(normalize_since(Some(42)), 42);
    }

    #[test]
    fn limit_is_bounded() {
        assert_eq!(normalize_limit(None), DEFAULT_LIMIT);
        assert_eq!(normalize_limit(Some(0)), 1);
        assert_eq!(normalize_limit(Some(-3)), 1);
        assert_eq!(normalize_limit(Some(10)), 10);
        assert_eq!(normalize_limit(Some(99_999)), MAX_LIMIT);
    }
}
