//! Limitation du brute-force **par compte** sur `/auth/login` (côté serveur).
//!
//! **Complémentaire** d'un rate-limit **par IP** au reverse proxy (Caddy/Nginx,
//! cf. `Caddyfile`) : le proxy ne voit ni le `username` (dans le corps de la
//! requête) ni l'**échec** d'authentification. Un attaquant distribué peut donc
//! répartir ses tentatives sur de nombreuses IP tout en ciblant **un** compte —
//! invisible au par-IP. On compte donc les `login/finish` **échoués** par username
//! dans Redis (fenêtre glissante) et on verrouille au-delà d'un seuil, pendant que
//! le proxy encaisse le volumétrique. Miroir serveur du `UnlockService` mobile.
//!
//! Le comptage est fait **par username tel qu'envoyé** (compte existant ou non) :
//! le verrouillage est ainsi identique dans les deux cas → pas de canal
//! d'énumération via la latence/le comportement du rate-limit.

use anyhow::Context;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;

/// Nombre d'échecs dans la fenêtre avant verrouillage du compte.
const MAX_FAILURES: u64 = 10;
/// Fenêtre glissante = durée de verrouillage effective (secondes).
const WINDOW_SECS: u64 = 900; // 15 min

fn key(username: &str) -> String {
    format!("rl:login:{username}")
}

/// Décision de verrouillage à partir de l'état Redis brut (logique **pure**,
/// testable sans Redis). Renvoie `Some(secondes_avant_réessai)` si verrouillé.
fn lock_decision(failures: Option<u64>, ttl_secs: i64) -> Option<u64> {
    match failures {
        Some(count) if count >= MAX_FAILURES => {
            // TTL attendu > 0 ; repli sur la fenêtre s'il est illisible (-1/-2).
            Some(if ttl_secs > 0 {
                ttl_secs as u64
            } else {
                WINDOW_SECS
            })
        }
        _ => None,
    }
}

/// Renvoie `Some(secondes_avant_réessai)` si le compte est verrouillé, `None` sinon.
///
/// # Errors
/// Erreur Redis.
pub async fn locked_for(
    mut conn: ConnectionManager,
    username: &str,
) -> anyhow::Result<Option<u64>> {
    let k = key(username);
    let failures: Option<u64> = conn.get(&k).await.context("lecture du compteur d'échecs")?;
    match failures {
        Some(count) if count >= MAX_FAILURES => {
            let ttl: i64 = conn.ttl(&k).await.context("TTL du compteur d'échecs")?;
            Ok(lock_decision(failures, ttl))
        }
        _ => Ok(None),
    }
}

/// Enregistre un échec d'authentification pour `username` (fenêtre glissante :
/// chaque échec repousse l'expiration).
///
/// # Errors
/// Erreur Redis.
pub async fn record_failure(mut conn: ConnectionManager, username: &str) -> anyhow::Result<()> {
    let k = key(username);
    let _: () = conn
        .incr(&k, 1)
        .await
        .context("incrément du compteur d'échecs")?;
    let _: () = conn
        .expire(&k, WINDOW_SECS as i64)
        .await
        .context("expiration du compteur d'échecs")?;
    Ok(())
}

/// Réinitialise le compteur d'échecs d'un compte (connexion réussie).
///
/// # Errors
/// Erreur Redis.
pub async fn reset(mut conn: ConnectionManager, username: &str) -> anyhow::Result<()> {
    let _: () = conn
        .del(key(username))
        .await
        .context("réinitialisation du compteur d'échecs")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_threshold_is_not_locked() {
        assert_eq!(lock_decision(None, 0), None);
        assert_eq!(lock_decision(Some(1), 300), None);
        assert_eq!(lock_decision(Some(MAX_FAILURES - 1), 300), None);
    }

    #[test]
    fn at_or_above_threshold_is_locked_with_retry() {
        assert_eq!(lock_decision(Some(MAX_FAILURES), 300), Some(300));
        assert_eq!(lock_decision(Some(MAX_FAILURES + 5), 42), Some(42));
    }

    #[test]
    fn unreadable_ttl_falls_back_to_window() {
        // TTL -1 (pas d'expiration) / -2 (clé absente) → repli sur la fenêtre.
        assert_eq!(lock_decision(Some(MAX_FAILURES), -1), Some(WINDOW_SECS));
        assert_eq!(lock_decision(Some(MAX_FAILURES), -2), Some(WINDOW_SECS));
    }
}
