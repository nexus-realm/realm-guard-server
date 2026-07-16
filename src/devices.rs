//! Registre d'appareils : une clé publique Ed25519 (`device_pk`) par appareil,
//! rattachée à un compte. Enregistrement idempotent (ré-appairage) ; révocation
//! logique (`revoked_at`). La révocation cryptographique (rotation de la VaultKey)
//! est un suivi séparé.

use anyhow::Context;
use sqlx::PgPool;
use uuid::Uuid;

/// Enregistre (ou ré-active) un appareil pour `account_id`. Idempotent sur
/// `device_pk` **au sein du même compte** (met à jour le nom, lève la révocation).
/// Renvoie l'`id` de l'appareil, ou `None` si le `device_pk` appartient déjà à un
/// **autre** compte.
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn register(
    db: &PgPool,
    account_id: Uuid,
    device_pk: &[u8],
    name: &str,
) -> anyhow::Result<Option<Uuid>> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO devices (id, account_id, device_pk, name) VALUES ($1, $2, $3, $4)
         ON CONFLICT (device_pk) DO UPDATE
           SET name = EXCLUDED.name, revoked_at = NULL
           WHERE devices.account_id = EXCLUDED.account_id
         RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(account_id)
    .bind(device_pk)
    .bind(name)
    .fetch_optional(db)
    .await
    .context("enregistrement d'appareil")?;
    Ok(row.map(|(id,)| id))
}

/// Liste les appareils d'un compte : `(id, name, device_pk, created_at epoch,
/// revoked)`. La clé publique permet à un appareil de **se reconnaître** dans la
/// liste (« cet appareil ») — elle est publique et le compte est authentifié.
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn list(
    db: &PgPool,
    account_id: Uuid,
) -> anyhow::Result<Vec<(Uuid, String, Vec<u8>, i64, bool)>> {
    let rows = sqlx::query_as(
        "SELECT id, name, device_pk, extract(epoch FROM created_at)::bigint,
                (revoked_at IS NOT NULL)
         FROM devices WHERE account_id = $1 ORDER BY created_at",
    )
    .bind(account_id)
    .fetch_all(db)
    .await
    .context("liste des appareils")?;
    Ok(rows)
}

/// Renomme un appareil du compte (`false` si inconnu). Le nom est cosmétique.
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn rename(
    db: &PgPool,
    account_id: Uuid,
    device_id: Uuid,
    name: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query("UPDATE devices SET name = $1 WHERE id = $2 AND account_id = $3")
        .bind(name)
        .bind(device_id)
        .bind(account_id)
        .execute(db)
        .await
        .context("renommage d'appareil")?;
    Ok(result.rows_affected() > 0)
}

/// Révoque un appareil du compte (idempotent : `false` si inconnu ou déjà révoqué).
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn revoke(db: &PgPool, account_id: Uuid, device_id: Uuid) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "UPDATE devices SET revoked_at = now()
         WHERE id = $1 AND account_id = $2 AND revoked_at IS NULL",
    )
    .bind(device_id)
    .bind(account_id)
    .execute(db)
    .await
    .context("révocation d'appareil")?;
    Ok(result.rows_affected() > 0)
}

/// `account_id` du compte propriétaire d'un appareil **enregistré et non révoqué**
/// (`None` sinon). Utilisé par l'authentification par clé d'appareil.
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn active_account_for_pk(db: &PgPool, device_pk: &[u8]) -> anyhow::Result<Option<Uuid>> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT account_id FROM devices WHERE device_pk = $1 AND revoked_at IS NULL",
    )
    .bind(device_pk)
    .fetch_optional(db)
    .await
    .context("recherche d'appareil actif")?;
    Ok(row.map(|(id,)| id))
}
