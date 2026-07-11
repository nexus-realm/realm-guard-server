//! État applicatif partagé injecté dans les handlers : pools de connexions.
//!
//! Les connexions sont **paresseuses** — construire l'état n'ouvre aucune socket,
//! donc `AppState` est constructible en test (et au boot) sans serveur vivant.

use anyhow::Context;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

/// Nombre maximum de connexions Postgres du pool.
const DB_MAX_CONNECTIONS: u32 = 10;

/// État partagé (cloneable : les pools sont des `Arc` en interne).
#[derive(Clone)]
pub struct AppState {
    /// Pool Postgres (connexion paresseuse).
    pub db: PgPool,
    /// Client Redis (connexions obtenues à la demande).
    pub redis: redis::Client,
}

impl AppState {
    /// Prépare l'état sans ouvrir de connexion (URLs seulement validées).
    ///
    /// # Errors
    /// URL Postgres ou Redis invalide.
    pub fn connect(database_url: &str, redis_url: &str) -> anyhow::Result<Self> {
        let db = PgPoolOptions::new()
            .max_connections(DB_MAX_CONNECTIONS)
            .connect_lazy(database_url)
            .context("configuration du pool Postgres")?;
        let redis = redis::Client::open(redis_url).context("configuration du client Redis")?;
        Ok(Self { db, redis })
    }
}
