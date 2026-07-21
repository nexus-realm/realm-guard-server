//! Log append-only de deltas CRDT chiffrés, par compte.
//!
//! Le serveur ne stocke que des **blobs opaques** : il ne déchiffre ni ne fusionne
//! rien. Le `seq` est un curseur monotone de rattrapage — la convergence ne dépend
//! ni de l'ordre ni de l'unicité de livraison (garanti par les property-tests du
//! cœur), donc pas de dédoublonnage ici.

use anyhow::Context;
use sqlx::PgPool;
use uuid::Uuid;

/// Ajoute un delta au log du compte. Renvoie son `seq`.
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn append(db: &PgPool, account_id: Uuid, payload: &[u8]) -> anyhow::Result<i64> {
    let (seq,): (i64,) =
        sqlx::query_as("INSERT INTO deltas (account_id, payload) VALUES ($1, $2) RETURNING seq")
            .bind(account_id)
            .bind(payload)
            .fetch_one(db)
            .await
            .context("ajout d'un delta")?;
    Ok(seq)
}

/// Deltas du compte dont le `seq` est **strictement supérieur** à `since`, par ordre
/// de `seq` croissant, bornés à `limit`.
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn since(
    db: &PgPool,
    account_id: Uuid,
    since: i64,
    limit: i64,
) -> anyhow::Result<Vec<(i64, Vec<u8>)>> {
    let rows = sqlx::query_as(
        "SELECT seq, payload FROM deltas
         WHERE account_id = $1 AND seq > $2
         ORDER BY seq
         LIMIT $3",
    )
    .bind(account_id)
    .bind(since)
    .bind(limit)
    .fetch_all(db)
    .await
    .context("lecture des deltas")?;
    Ok(rows)
}

/// Plus grand `seq` du compte (`0` si le log est vide) — permet à un client de savoir
/// s'il est à jour sans tirer une page.
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn latest_seq(db: &PgPool, account_id: Uuid) -> anyhow::Result<i64> {
    let (seq,): (Option<i64>,) =
        sqlx::query_as("SELECT max(seq) FROM deltas WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(db)
            .await
            .context("lecture du dernier seq")?;
    Ok(seq.unwrap_or(0))
}
