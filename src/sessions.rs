//! Sessions et état de connexion transitoire, stockés dans **Redis**.
//!
//! - **État de connexion** (OPAQUE `ServerLogin`) entre `login/start` et
//!   `login/finish` : clé `login:<flow>`, TTL court, **usage unique** (`GETDEL`).
//! - **Session** émise après login : clé `session:<token>` → `account_id`, TTL
//!   long, **révocable** (supprimer la clé).

use anyhow::Context;
use redis::AsyncCommands;
use uuid::Uuid;

/// TTL de l'état de connexion transitoire (secondes).
const LOGIN_FLOW_TTL_SECS: u64 = 60;
/// TTL d'une session (7 jours).
const SESSION_TTL_SECS: u64 = 7 * 24 * 3600;

fn login_key(flow_id: &str) -> String {
    format!("login:{flow_id}")
}

fn session_key(token: &str) -> String {
    format!("session:{token}")
}

/// Stocke l'état de connexion serveur sous un identifiant de flux (TTL court).
///
/// # Errors
/// Erreur Redis.
pub async fn store_login_flow(
    redis: &redis::Client,
    flow_id: &str,
    blob: &[u8],
) -> anyhow::Result<()> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .context("connexion Redis")?;
    let _: () = conn
        .set_ex(login_key(flow_id), blob, LOGIN_FLOW_TTL_SECS)
        .await
        .context("écriture du flux de connexion")?;
    Ok(())
}

/// Récupère **et supprime** (usage unique) l'état de connexion d'un flux.
///
/// # Errors
/// Erreur Redis.
pub async fn take_login_flow(
    redis: &redis::Client,
    flow_id: &str,
) -> anyhow::Result<Option<Vec<u8>>> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .context("connexion Redis")?;
    let blob: Option<Vec<u8>> = conn
        .get_del(login_key(flow_id))
        .await
        .context("lecture du flux de connexion")?;
    Ok(blob)
}

/// Crée une session pour `account_id` et renvoie son token opaque.
///
/// # Errors
/// Erreur Redis.
pub async fn create_session(redis: &redis::Client, account_id: Uuid) -> anyhow::Result<String> {
    let token = Uuid::new_v4().simple().to_string();
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .context("connexion Redis")?;
    let _: () = conn
        .set_ex(
            session_key(&token),
            account_id.to_string(),
            SESSION_TTL_SECS,
        )
        .await
        .context("création de session")?;
    Ok(token)
}

/// Résout un token de session en `account_id` (`None` si invalide/expiré).
///
/// # Errors
/// Erreur Redis.
pub async fn session_account(redis: &redis::Client, token: &str) -> anyhow::Result<Option<Uuid>> {
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .context("connexion Redis")?;
    let value: Option<String> = conn
        .get(session_key(token))
        .await
        .context("lecture de session")?;
    Ok(value.and_then(|s| Uuid::parse_str(&s).ok()))
}
