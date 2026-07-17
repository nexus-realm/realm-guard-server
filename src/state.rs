//! État applicatif partagé injecté dans les handlers : pools de connexions +
//! secret serveur OPAQUE.
//!
//! Les connexions sont **paresseuses** — construire l'état n'ouvre aucune socket,
//! donc `AppState` est constructible en test (et au boot) sans serveur vivant. La
//! connexion Redis (multiplexée, auto-reconnect) est **mutualisée** : initialisée
//! à la première demande via [`AppState::redis`] puis réutilisée (clone bon marché),
//! au lieu d'ouvrir une connexion par opération.

use std::sync::Arc;

use anyhow::Context;
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::OnceCell;

/// Nombre maximum de connexions Postgres du pool.
const DB_MAX_CONNECTIONS: u32 = 10;

/// État partagé (cloneable : pools `Arc` en interne, connexion Redis mutualisée).
#[derive(Clone)]
pub struct AppState {
    /// Pool Postgres (connexion paresseuse).
    pub db: PgPool,
    /// Fabrique de connexions Redis (ne se connecte pas à la construction).
    redis_client: redis::Client,
    /// Connexion Redis mutualisée, initialisée à la première demande.
    redis_manager: Arc<OnceCell<ConnectionManager>>,
    /// Secret serveur OPAQUE sérialisé (chargé au boot, immuable ensuite).
    pub opaque_setup: Arc<Vec<u8>>,
}

impl AppState {
    /// Prépare l'état sans ouvrir de connexion (URLs seulement validées).
    ///
    /// # Errors
    /// URL Postgres ou Redis invalide.
    pub fn connect(
        database_url: &str,
        redis_url: &str,
        opaque_setup: Vec<u8>,
    ) -> anyhow::Result<Self> {
        let db = PgPoolOptions::new()
            .max_connections(DB_MAX_CONNECTIONS)
            .connect_lazy(database_url)
            .context("configuration du pool Postgres")?;
        let redis_client =
            redis::Client::open(redis_url).context("configuration du client Redis")?;
        Ok(Self {
            db,
            redis_client,
            redis_manager: Arc::new(OnceCell::new()),
            opaque_setup: Arc::new(opaque_setup),
        })
    }

    /// Renvoie la connexion Redis **mutualisée** (multiplexée, auto-reconnect),
    /// initialisée à la première demande puis réutilisée. Le `ConnectionManager`
    /// se clone à bas coût et partage la connexion sous-jacente.
    ///
    /// # Errors
    /// Redis injoignable lors de l'initialisation.
    pub async fn redis(&self) -> anyhow::Result<ConnectionManager> {
        let manager = self
            .redis_manager
            .get_or_try_init(|| ConnectionManager::new(self.redis_client.clone()))
            .await
            .context("connexion Redis")?;
        Ok(manager.clone())
    }

    /// Ouvre une connexion Redis **dédiée pub/sub**. Contrairement à [`Self::redis`]
    /// (multiplexée), le pub/sub monopolise sa connexion — une par abonné WebSocket.
    /// Simple et suffisant pour un déploiement auto-hébergé ; à multiplexer si le
    /// nombre d'appareils connectés simultanément le justifie.
    ///
    /// # Errors
    /// Redis injoignable.
    pub async fn redis_pubsub(&self) -> anyhow::Result<redis::aio::PubSub> {
        self.redis_client
            .get_async_pubsub()
            .await
            .context("connexion Redis pub/sub")
    }
}
