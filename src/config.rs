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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    const SETUP_B64: &str = "c2VjcmV0"; // "secret"

    /// L'environnement est global au processus alors que `cargo test` exécute en
    /// parallèle : ces tests s'excluent mutuellement.
    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            // Un test qui panique empoisonne le verrou : on continue quand même,
            // chaque test repart d'un environnement vierge.
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Repart d'un environnement vierge pour toutes les variables lues ici.
    fn clear_env() {
        for key in [
            "RG_SERVER_ADDR",
            "DATABASE_URL",
            "REDIS_URL",
            "RG_OPAQUE_SETUP",
            "RG_OPAQUE_SETUP_FILE",
        ] {
            // SAFETY : mutation d'env sérialisée par `env_lock`, mono-thread ici.
            unsafe { env::remove_var(key) };
        }
    }

    fn set(key: &str, value: &str) {
        // SAFETY : idem — sous `env_lock`.
        unsafe { env::set_var(key, value) };
    }

    #[test]
    fn defaults_apply_when_only_the_secret_is_set() {
        let _guard = env_lock();
        clear_env();
        set("RG_OPAQUE_SETUP", SETUP_B64);

        let config = Config::from_env().expect("config");

        assert_eq!(config.addr.to_string(), "0.0.0.0:8080");
        assert_eq!(config.database_url, DEFAULT_DATABASE_URL);
        assert_eq!(config.redis_url, DEFAULT_REDIS_URL);
        assert_eq!(config.opaque_setup, b"secret");
    }

    #[test]
    fn env_overrides_addr_and_urls() {
        let _guard = env_lock();
        clear_env();
        set("RG_OPAQUE_SETUP", SETUP_B64);
        set("RG_SERVER_ADDR", "127.0.0.1:9999");
        set("DATABASE_URL", "postgres://u:p@db:5432/x");
        set("REDIS_URL", "redis://cache:6379");

        let config = Config::from_env().expect("config");

        assert_eq!(config.addr.to_string(), "127.0.0.1:9999");
        assert_eq!(config.database_url, "postgres://u:p@db:5432/x");
        assert_eq!(config.redis_url, "redis://cache:6379");
    }

    #[test]
    fn malformed_addr_is_rejected() {
        let _guard = env_lock();
        clear_env();
        set("RG_OPAQUE_SETUP", SETUP_B64);
        set("RG_SERVER_ADDR", "pas-une-adresse");

        let error = Config::from_env().expect_err("adresse invalide");

        assert!(error.to_string().contains("RG_SERVER_ADDR"));
    }

    /// Fail-closed : sans secret, on démarre pas — régénérer en silence
    /// invaliderait tous les comptes existants.
    #[test]
    fn missing_opaque_setup_fails_closed() {
        let _guard = env_lock();
        clear_env();

        let error = Config::from_env().expect_err("secret absent");

        assert!(error.to_string().contains("OPAQUE"));
    }

    #[test]
    fn invalid_base64_secret_is_rejected() {
        let _guard = env_lock();
        clear_env();
        set("RG_OPAQUE_SETUP", "pas du base64 !!");

        let error = Config::from_env().expect_err("base64 invalide");

        assert!(error.to_string().contains("base64"));
    }

    #[test]
    fn empty_secret_is_rejected() {
        let _guard = env_lock();
        clear_env();
        set("RG_OPAQUE_SETUP", "");

        assert!(Config::from_env().is_err());
    }

    #[test]
    fn setup_file_wins_over_inline_and_is_trimmed() {
        let _guard = env_lock();
        clear_env();
        let path = env::temp_dir().join(format!("rg-opaque-{}.b64", std::process::id()));
        // Retour à la ligne final : ce qu'écrit un secret monté par l'orchestrateur.
        std::fs::write(&path, format!("{SETUP_B64}\n")).expect("écriture");
        set("RG_OPAQUE_SETUP", "YXV0cmU="); // "autre" — doit être ignoré
        set("RG_OPAQUE_SETUP_FILE", &path.to_string_lossy());

        let config = Config::from_env().expect("config");

        assert_eq!(config.opaque_setup, b"secret");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn unreadable_setup_file_is_an_error() {
        let _guard = env_lock();
        clear_env();
        set("RG_OPAQUE_SETUP_FILE", "/chemin/qui/n/existe/pas.b64");

        let error = Config::from_env().expect_err("fichier illisible");

        assert!(error.to_string().contains("RG_OPAQUE_SETUP_FILE"));
    }

    /// Un `{:?}` de la config part dans les logs : ni les URLs (porteuses de mot
    /// de passe) ni le secret OPAQUE ne doivent y apparaître.
    #[test]
    fn debug_redacts_every_secret() {
        let config = Config {
            addr: "127.0.0.1:8080".parse().expect("adresse"),
            database_url: "postgres://user:motdepasse@db/x".to_string(),
            redis_url: "redis://:motdepasse@cache".to_string(),
            opaque_setup: b"secret".to_vec(),
        };

        let rendered = format!("{config:?}");

        assert!(!rendered.contains("motdepasse"));
        assert!(!rendered.contains("secret"), "octets du setup exposés");
        assert!(rendered.contains("127.0.0.1:8080"), "adresse non secrète");
        assert!(rendered.contains("6 octets"));
    }
}
