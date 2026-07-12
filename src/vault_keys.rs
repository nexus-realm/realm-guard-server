//! Persistance de la VaultKey enrobée (opaque pour le serveur) + sel, par compte.

use anyhow::Context;
use sqlx::PgPool;
use uuid::Uuid;

/// Insère ou remplace la VaultKey enrobée + le sel du compte.
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn upsert(
    db: &PgPool,
    account_id: Uuid,
    wrapped_key: &[u8],
    salt: &[u8],
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO vault_keys (account_id, wrapped_key, salt) VALUES ($1, $2, $3)
         ON CONFLICT (account_id) DO UPDATE
           SET wrapped_key = EXCLUDED.wrapped_key, salt = EXCLUDED.salt, updated_at = now()",
    )
    .bind(account_id)
    .bind(wrapped_key)
    .bind(salt)
    .execute(db)
    .await
    .context("upsert de la vault key")?;
    Ok(())
}

/// Récupère `(wrapped_key, salt)` d'un compte (`None` si jamais uploadée).
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn get(db: &PgPool, account_id: Uuid) -> anyhow::Result<Option<(Vec<u8>, Vec<u8>)>> {
    let row: Option<(Vec<u8>, Vec<u8>)> =
        sqlx::query_as("SELECT wrapped_key, salt FROM vault_keys WHERE account_id = $1")
            .bind(account_id)
            .fetch_optional(db)
            .await
            .context("lecture de la vault key")?;
    Ok(row)
}
