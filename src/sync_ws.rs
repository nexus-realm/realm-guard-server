//! Réveil temps réel via WebSocket + fan-out Redis pub/sub.
//!
//! # Principe : le WS est un **réveil**, jamais un canal de livraison
//!
//! Le WS ne transporte **pas** les deltas : il envoie seulement un *nudge* (le nouveau
//! `seq`) invitant le client à **tirer** par curseur (`GET /sync/deltas`). La source de
//! vérité reste le log Postgres.
//!
//! C'est délibéré : Redis pub/sub est *fire-and-forget* (un message publié pendant
//! qu'un abonné se reconnecte est perdu). Si le WS livrait les deltas, une perte
//! ferait diverger. En nudge, une perte ne coûte que de la **latence** — le client
//! poll de toute façon en filet, et rattrape au prochain tirage. **Le WS accélère, il
//! ne conditionne jamais la correction.**
//!
//! Donc : le `publish` est **best-effort** (un échec ne casse pas le `push` — le delta
//! est déjà durable en base) ; et le client ne doit jamais supposer qu'un nudge
//! manquant signifie « rien de neuf ».

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use futures_util::StreamExt;
use redis::AsyncCommands;
use uuid::Uuid;

use crate::auth_api::AuthAccount;
use crate::state::AppState;

/// Canal pub/sub d'un compte (les deltas d'un compte ne concernent que ses appareils).
fn channel(account_id: Uuid) -> String {
    format!("sync:{account_id}")
}

/// Routes de réveil temps réel (à monter dans l'app avec l'état).
pub fn routes() -> Router<AppState> {
    Router::new().route("/sync/ws", get(ws))
}

/// Publie un **nudge** sur le canal du compte : le nouveau `seq`. Best-effort — une
/// erreur est ignorée (le delta est déjà en base ; les clients rattraperont au poll).
pub async fn publish_nudge(state: &AppState, account_id: Uuid, seq: i64) {
    let Ok(mut conn) = state.redis().await else {
        return;
    };
    let _: Result<(), _> = conn.publish(channel(account_id), seq).await;
}

/// WebSocket de réveil, **gated par session** : à chaque changement du compte, pousse
/// un nudge (le `seq`) au client, qui tire alors ses deltas.
async fn ws(
    account: AuthAccount,
    upgrade: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    upgrade.on_upgrade(move |socket| handle(socket, account.0, state))
}

async fn handle(mut socket: WebSocket, account_id: Uuid, state: AppState) {
    // Abonnement pub/sub dédié : sans lui, pas de réveil → on ferme (le client
    // basculera sur son poll de secours).
    let Ok(mut pubsub) = state.redis_pubsub().await else {
        return;
    };
    if pubsub.subscribe(channel(account_id)).await.is_err() {
        return;
    }

    let mut messages = pubsub.on_message();
    loop {
        tokio::select! {
            // Un delta a été publié pour ce compte → réveille le client.
            nudge = messages.next() => {
                let Some(nudge) = nudge else { break }; // pub/sub fermé
                let seq: String = nudge.get_payload().unwrap_or_default();
                if socket.send(Message::Text(seq.into())).await.is_err() {
                    break; // client parti
                }
            }
            // Trafic client : on n'attend rien d'utile ; une fermeture rompt la boucle.
            frame = socket.recv() => {
                match frame {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    Some(Ok(_)) => {} // ping/pong gérés par axum ; le reste est ignoré
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_is_account_scoped() {
        let a = channel(Uuid::from_u128(1));
        let b = channel(Uuid::from_u128(2));
        assert!(a.starts_with("sync:"));
        assert_ne!(a, b);
    }
}
