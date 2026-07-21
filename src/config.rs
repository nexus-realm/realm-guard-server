//! Configuration du serveur, lue depuis l'environnement.

use std::env;
use std::fmt;
use std::net::SocketAddr;

use anyhow::Context;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// Adresse d'écoute par défaut (toutes interfaces, port 8080).
const DEFAULT_ADDR: &str = "0.0.0.0:8080";
/// URL Postgres par défaut (dev local / stack Docker Compose).
const DEFAULT_DATABASE_URL: &str = "postgres://realmguard:realmguard@localhost:5432/realmguard";
/// URL Redis par défaut (dev local / stack Docker Compose).
const DEFAULT_REDIS_URL: &str = "redis://localhost:6379";

/// Configuration résolue au démarrage.
#[derive(Clone)]
pub struct Config {
    /// Adresse d'écoute HTTP.
    pub addr: SocketAddr,
    /// URL de connexion Postgres.
    pub database_url: String,
    /// URL de connexion Redis.
    pub redis_url: String,
    /// Secret serveur OPAQUE (décodé). **Fourni hors base** (env / fichier secret),
    /// jamais persisté avec les password files → un dump de la base ne suffit pas à
    /// monter une attaque dictionnaire hors-ligne (cf. revue sécu P1).
    pub opaque_setup: Vec<u8>,
}

impl Config {
    /// Lit la configuration depuis l'environnement.
    ///
    /// Variables : `RG_SERVER_ADDR` (défaut `0.0.0.0:8080`), `DATABASE_URL`,
    /// `REDIS_URL`, et le **secret serveur OPAQUE** via `RG_OPAQUE_SETUP_FILE`
    /// (chemin d'un secret monté — **recommandé en prod**) ou `RG_OPAQUE_SETUP`
    /// (base64 inline — dev). Générer avec `realm-guard-server generate-setup`.
    ///
    /// # Errors
    /// `RG_SERVER_ADDR` mal formée, ou secret OPAQUE absent/illisible.
    pub fn from_env() -> anyhow::Result<Self> {
        let addr = env::var("RG_SERVER_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_string());
        let addr = addr
            .parse()
            .with_context(|| format!("RG_SERVER_ADDR invalide : {addr}"))?;
        let database_url =
            env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_string());
        let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| DEFAULT_REDIS_URL.to_string());
        let opaque_setup = load_opaque_setup()?;
        Ok(Self {
            addr,
            database_url,
            redis_url,
            opaque_setup,
        })
    }
}

/// Charge le secret serveur OPAQUE depuis un fichier (`RG_OPAQUE_SETUP_FILE`,
/// prioritaire — un fichier monté fuit moins qu'une variable d'env) ou une valeur
/// inline (`RG_OPAQUE_SETUP`), toutes deux en base64. **Fail-closed** : absent → erreur
/// (on ne régénère jamais en silence, cela invaliderait tous les comptes).
fn load_opaque_setup() -> anyhow::Result<Vec<u8>> {
    if let Ok(path) = env::var("RG_OPAQUE_SETUP_FILE") {
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("lecture de RG_OPAQUE_SETUP_FILE ({path})"))?;
        return decode_setup(contents.trim());
    }
    if let Ok(inline) = env::var("RG_OPAQUE_SETUP") {
        return decode_setup(inline.trim());
    }
    anyhow::bail!(
        "secret serveur OPAQUE absent : définir RG_OPAQUE_SETUP_FILE (chemin, \
         recommandé) ou RG_OPAQUE_SETUP (base64). Générer : `realm-guard-server generate-setup`"
    )
}

fn decode_setup(b64: &str) -> anyhow::Result<Vec<u8>> {
    let bytes = STANDARD
        .decode(b64)
        .context("secret serveur OPAQUE : base64 invalide")?;
    anyhow::ensure!(!bytes.is_empty(), "secret serveur OPAQUE vide");
    Ok(bytes)
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Redacte les secrets (URLs porteuses de mot de passe, secret OPAQUE).
        f.debug_struct("Config")
            .field("addr", &self.addr)
            .field("database_url", &"<redacted>")
            .field("redis_url", &"<redacted>")
            .field(
                "opaque_setup",
                &format_args!("<{} octets>", self.opaque_setup.len()),
            )
            .finish()
    }
}
