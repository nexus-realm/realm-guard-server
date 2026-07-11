//! Comptes OPAQUE : persistance des password files et du secret serveur OPAQUE.
//!
//! Le serveur ne détient jamais de mot de passe — uniquement le password file
//! OPAQUE (opaque, lié au secret serveur). `username` sert d'identifiant de
//! credential OPAQUE.

use anyhow::Context;
use realm_guard_core::auth;
use sqlx::PgPool;
use uuid::Uuid;

/// Charge le secret serveur OPAQUE (singleton), en le générant au premier boot.
///
/// **Sensible** : à isoler des password files en production (KMS / secret dédié).
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn bootstrap_opaque_setup(db: &PgPool) -> anyhow::Result<Vec<u8>> {
    let existing: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT setup FROM opaque_server_setup WHERE id = 1")
            .fetch_optional(db)
            .await
            .context("lecture du server setup OPAQUE")?;
    if let Some(setup) = existing {
        return Ok(setup);
    }

    let setup = auth::generate_server_setup();
    sqlx::query(
        "INSERT INTO opaque_server_setup (id, setup) VALUES (1, $1) ON CONFLICT (id) DO NOTHING",
    )
    .bind(&setup)
    .execute(db)
    .await
    .context("écriture du server setup OPAQUE")?;

    // Relit : en cas de course, un autre process a pu insérer le sien en premier.
    let stored: Vec<u8> = sqlx::query_scalar("SELECT setup FROM opaque_server_setup WHERE id = 1")
        .fetch_one(db)
        .await
        .context("relecture du server setup OPAQUE")?;
    Ok(stored)
}

/// Insère un compte. Échoue si le `username` est déjà pris (contrainte unique).
///
/// # Errors
/// Username déjà pris, ou erreur d'accès à la base.
pub async fn insert_account(
    db: &PgPool,
    username: &str,
    password_file: &[u8],
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO accounts (id, username, password_file) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(username)
        .bind(password_file)
        .execute(db)
        .await
        .context("insertion du compte")?;
    Ok(id)
}

/// Récupère le password file OPAQUE d'un compte par `username` (`None` si inconnu).
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn password_file(db: &PgPool, username: &str) -> anyhow::Result<Option<Vec<u8>>> {
    sqlx::query_scalar("SELECT password_file FROM accounts WHERE username = $1")
        .bind(username)
        .fetch_optional(db)
        .await
        .context("lecture du password file")
}

/// Le `username` est-il déjà pris ?
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn username_taken(db: &PgPool, username: &str) -> anyhow::Result<bool> {
    let found: Option<i32> = sqlx::query_scalar("SELECT 1 FROM accounts WHERE username = $1")
        .bind(username)
        .fetch_optional(db)
        .await
        .context("vérification du username")?;
    Ok(found.is_some())
}

/// Récupère `(id, password_file)` d'un compte par `username` (`None` si inconnu).
///
/// # Errors
/// Erreur d'accès à la base.
pub async fn account_credentials(
    db: &PgPool,
    username: &str,
) -> anyhow::Result<Option<(Uuid, Vec<u8>)>> {
    let row: Option<(Uuid, Vec<u8>)> =
        sqlx::query_as("SELECT id, password_file FROM accounts WHERE username = $1")
            .bind(username)
            .fetch_optional(db)
            .await
            .context("lecture des credentials du compte")?;
    Ok(row)
}
