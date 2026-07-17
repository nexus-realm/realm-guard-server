//! Snapshots d'état complet (opaques) + **compaction** du log de deltas.
//!
//! Un client qui a fusionné le log publie l'état complet en déclarant le `covers_seq`
//! couvert ; les deltas ≤ `covers_seq` sont alors purgés dans la **même transaction**
//! (sinon une purge sans snapshot visible perdrait des données).

use anyhow::Context;
use sqlx::PgPool;
use uuid::Uuid;

/// Résultat d'une publication de snapshot.
pub struct SnapshotPut {
    /// Nombre de deltas purgés par la compaction.
    pub purged: u64,
}

/// Publie le snapshot du compte et compacte le log (deltas ≤ `covers_seq`).
///
/// Renvoie `None` si `covers_seq` **régresse** (snapshot plus ancien que celui en
/// place) : on refuse, car compacter sur un état plus ancien perdrait les deltas
/// intermédiaires.
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn put(
    db: &PgPool,
    account_id: Uuid,
    payload: &[u8],
    covers_seq: i64,
) -> anyhow::Result<Option<SnapshotPut>> {
    let mut tx = db.begin().await.context("transaction de snapshot")?;

    // Le garde `WHERE` rejette une régression : aucune ligne touchée ⇒ on abandonne.
    let updated = sqlx::query(
        "INSERT INTO snapshots (account_id, payload, covers_seq) VALUES ($1, $2, $3)
         ON CONFLICT (account_id) DO UPDATE
           SET payload = EXCLUDED.payload,
               covers_seq = EXCLUDED.covers_seq,
               updated_at = now()
           WHERE snapshots.covers_seq <= EXCLUDED.covers_seq",
    )
    .bind(account_id)
    .bind(payload)
    .bind(covers_seq)
    .execute(&mut *tx)
    .await
    .context("publication du snapshot")?;

    if updated.rows_affected() == 0 {
        tx.rollback().await.context("annulation du snapshot")?;
        return Ok(None);
    }

    // Compaction : les deltas couverts sont redondants avec le snapshot.
    let purged = sqlx::query("DELETE FROM deltas WHERE account_id = $1 AND seq <= $2")
        .bind(account_id)
        .bind(covers_seq)
        .execute(&mut *tx)
        .await
        .context("compaction du log")?
        .rows_affected();

    tx.commit().await.context("validation du snapshot")?;
    Ok(Some(SnapshotPut { purged }))
}

/// Snapshot du compte : `(payload, covers_seq)`, ou `None` s'il n'y en a pas.
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn get(db: &PgPool, account_id: Uuid) -> anyhow::Result<Option<(Vec<u8>, i64)>> {
    let row = sqlx::query_as("SELECT payload, covers_seq FROM snapshots WHERE account_id = $1")
        .bind(account_id)
        .fetch_optional(db)
        .await
        .context("lecture du snapshot")?;
    Ok(row)
}

/// `covers_seq` du snapshot du compte (`0` s'il n'y en a pas) — sert au tirage de
/// deltas pour détecter un curseur devenu **trop ancien**.
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn covers_seq(db: &PgPool, account_id: Uuid) -> anyhow::Result<i64> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT covers_seq FROM snapshots WHERE account_id = $1")
            .bind(account_id)
            .fetch_optional(db)
            .await
            .context("lecture du covers_seq")?;
    Ok(row.map_or(0, |(seq,)| seq))
}
