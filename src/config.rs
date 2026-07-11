//! Configuration du serveur, lue depuis l'environnement.

use std::env;
use std::net::SocketAddr;

use anyhow::Context;

/// Adresse d'écoute par défaut (toutes interfaces, port 8080).
const DEFAULT_ADDR: &str = "0.0.0.0:8080";
/// URL Postgres par défaut (dev local / stack Docker Compose).
const DEFAULT_DATABASE_URL: &str = "postgres://realmguard:realmguard@localhost:5432/realmguard";
/// URL Redis par défaut (dev local / stack Docker Compose).
const DEFAULT_REDIS_URL: &str = "redis://localhost:6379";

/// Configuration résolue au démarrage.
#[derive(Debug, Clone)]
pub struct Config {
    /// Adresse d'écoute HTTP.
    pub addr: SocketAddr,
    /// URL de connexion Postgres.
    pub database_url: String,
    /// URL de connexion Redis.
    pub redis_url: String,
}

impl Config {
    /// Lit la configuration depuis l'environnement.
    ///
    /// Variables : `RG_SERVER_ADDR` (défaut `0.0.0.0:8080`), `DATABASE_URL`,
    /// `REDIS_URL`.
    ///
    /// # Errors
    /// `RG_SERVER_ADDR` mal formée.
    pub fn from_env() -> anyhow::Result<Self> {
        let addr = env::var("RG_SERVER_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string());
        let addr = addr
            .parse()
            .with_context(|| format!("RG_SERVER_ADDR invalide : {addr}"))?;
        let database_url =
            env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_string());
        let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| DEFAULT_REDIS_URL.to_string());
        Ok(Self {
            addr,
            database_url,
            redis_url,
        })
    }
}
