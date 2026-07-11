//! Configuration du serveur, lue depuis l'environnement.

use std::env;
use std::net::SocketAddr;

use anyhow::Context;

/// Adresse d'écoute par défaut (toutes interfaces, port 8080).
const DEFAULT_ADDR: &str = "0.0.0.0:8080";

/// Configuration résolue au démarrage.
#[derive(Debug, Clone)]
pub struct Config {
    /// Adresse d'écoute HTTP.
    pub addr: SocketAddr,
}

impl Config {
    /// Lit la configuration depuis l'environnement.
    ///
    /// Variables : `RG_SERVER_ADDR` (défaut `0.0.0.0:8080`).
    ///
    /// # Errors
    /// `RG_SERVER_ADDR` mal formée.
    pub fn from_env() -> anyhow::Result<Self> {
        let addr = env::var("RG_SERVER_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string());
        let addr = addr
            .parse()
            .with_context(|| format!("RG_SERVER_ADDR invalide : {addr}"))?;
        Ok(Self { addr })
    }
}
