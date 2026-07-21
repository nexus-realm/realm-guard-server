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
use axum::routing::{post, put};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use crate::auth_api::AuthAccount;
use crate::state::AppState;
use crate::{deltas, snapshots};

/// Borne d'un delta (garde-fou anti-abus ; un delta de coffre est petit).
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
/// Borne d'un snapshot : c'est un coffre entier, donc bien plus gros qu'un delta.
const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
/// Taille de page par défaut / maximale au tirage.
const DEFAULT_LIMIT: i64 = 500;
const MAX_LIMIT: i64 = 1000;

/// Routes de synchronisation (à monter dans l'app avec l'état).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sync/deltas", post(push).get(pull))
        .route("/sync/snapshot", put(put_snapshot).get(get_snapshot))
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

#[derive(Deserialize)]
struct SnapshotReq {
    payload: String,
    covers_seq: i64,
}

#[derive(Serialize)]
struct SnapshotPutResp {
    /// Deltas devenus redondants et purgés par la compaction.
    purged: u64,
}

#[derive(Serialize)]
struct SnapshotResp {
    payload: String,
    covers_seq: i64,
}

fn internal<E>(_error: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Décode + borne un blob opaque (base64 → octets). `max` diffère selon l'usage :
/// un delta est petit, un snapshot est un coffre entier.
fn decode_bounded(value: &str, max: usize) -> Result<Vec<u8>, StatusCode> {
    let payload = STANDARD
        .decode(value)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if payload.is_empty() || payload.len() > max {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(payload)
}

/// Décode + borne un delta.
fn decode_payload(value: &str) -> Result<Vec<u8>, StatusCode> {
    decode_bounded(value, MAX_PAYLOAD_BYTES)
}

/// Normalise le curseur : un `since` négatif n'a pas de sens (le log part à 1).
fn normalize_since(since: Option<i64>) -> i64 {
    since.unwrap_or(0).max(0)
}

/// Normalise la taille de page dans `1..=MAX_LIMIT`.
fn normalize_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// Pousse un delta au log du compte authentifié, puis réveille les autres appareils.
async fn push(
    account: AuthAccount,
    State(state): State<AppState>,
    Json(body): Json<PushReq>,
) -> Result<(StatusCode, Json<PushResp>), StatusCode> {
    let payload = decode_payload(&body.payload)?;
    let seq = deltas::append(&state.db, account.0, &payload)
        .await
        .map_err(internal)?;
    // Réveil temps réel best-effort : le delta est déjà durable, un échec de nudge
    // ne fait que retarder les autres appareils jusqu'à leur prochain poll.
    crate::sync_ws::publish_nudge(&state, account.0, seq).await;
    Ok((StatusCode::CREATED, Json(PushResp { seq })))
}

/// Tire les deltas du compte authentifié postérieurs au curseur `since`.
///
/// **410 Gone** si le curseur est antérieur au snapshot : les deltas de cette période
/// ont été compactés, et se contenter des suivants ferait **rater le reste en
/// silence**. Le client doit repartir de `GET /sync/snapshot` — repli correct, garanti
/// par le cœur (un join d'état complet équivaut à l'échange de deltas).
async fn pull(
    account: AuthAccount,
    State(state): State<AppState>,
    Query(query): Query<PullQuery>,
) -> Result<Json<PullResp>, StatusCode> {
    let since = normalize_since(query.since);
    let limit = normalize_limit(query.limit);

    // À vérifier **avant** de servir : un curseur trop ancien doit échouer bruyamment.
    let covers = snapshots::covers_seq(&state.db, account.0)
        .await
        .map_err(internal)?;
    if covers > since {
        return Err(StatusCode::GONE);
    }

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

/// Publie le snapshot du compte et compacte le log.
///
/// **409** si `covers_seq` régresse (snapshot plus ancien que celui en place) :
/// compacter sur un état antérieur perdrait les deltas intermédiaires. **400** si le
/// snapshot prétend couvrir un `seq` qui n'existe pas encore.
async fn put_snapshot(
    account: AuthAccount,
    State(state): State<AppState>,
    Json(body): Json<SnapshotReq>,
) -> Result<Json<SnapshotPutResp>, StatusCode> {
    let payload = decode_bounded(&body.payload, MAX_SNAPSHOT_BYTES)?;
    if body.covers_seq < 0 {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Un snapshot ne peut pas couvrir plus loin que le log : sinon il masquerait des
    // deltas à venir (le tirage les jugerait « compactés » et les sauterait).
    //
    // Le repère haut n'est **pas** le dernier delta restant : après compaction, le
    // log est vide (`latest_seq` = 0) alors que l'historique va jusqu'à `covers_seq`.
    // S'en tenir aux deltas restants rendrait toute republication impossible.
    let latest_delta = deltas::latest_seq(&state.db, account.0)
        .await
        .map_err(internal)?;
    let already_covered = snapshots::covers_seq(&state.db, account.0)
        .await
        .map_err(internal)?;
    if body.covers_seq > latest_delta.max(already_covered) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let result = snapshots::put(&state.db, account.0, &payload, body.covers_seq)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::CONFLICT)?;

    Ok(Json(SnapshotPutResp {
        purged: result.purged,
    }))
}

/// Récupère le snapshot du compte (**404** s'il n'y en a pas encore).
async fn get_snapshot(
    account: AuthAccount,
    State(state): State<AppState>,
) -> Result<Json<SnapshotResp>, StatusCode> {
    let (payload, covers_seq) = snapshots::get(&state.db, account.0)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(SnapshotResp {
        payload: STANDARD.encode(payload),
        covers_seq,
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
    fn snapshot_bound_is_larger_than_delta_bound() {
        // Un snapshot est un coffre entier : il doit passer là où un delta serait
        // refusé, sans quoi la compaction serait impossible sur un gros coffre.
        let big = STANDARD.encode(vec![0u8; MAX_PAYLOAD_BYTES + 1]);
        assert_eq!(decode_payload(&big).unwrap_err(), StatusCode::BAD_REQUEST);
        assert_eq!(
            decode_bounded(&big, MAX_SNAPSHOT_BYTES).unwrap().len(),
            MAX_PAYLOAD_BYTES + 1
        );

        let too_big = STANDARD.encode(vec![0u8; MAX_SNAPSHOT_BYTES + 1]);
        assert_eq!(
            decode_bounded(&too_big, MAX_SNAPSHOT_BYTES).unwrap_err(),
            StatusCode::BAD_REQUEST
        );
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
