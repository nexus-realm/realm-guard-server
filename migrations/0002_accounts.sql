-- Comptes OPAQUE. Le serveur ne stocke JAMAIS le mot de passe : seulement le
-- « password file » OPAQUE (≈ sensibilité d'un hash, inutilisable pour se
-- connecter sans compléter le protocole). `username` = identifiant de credential
-- OPAQUE (constant entre register et login).
CREATE TABLE IF NOT EXISTS accounts (
    id uuid PRIMARY KEY,
    username text NOT NULL UNIQUE,
    password_file bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- Secret serveur OPAQUE (graine OPRF + clé), singleton généré au premier boot.
-- SENSIBLE : un dump combiné avec les password files faciliterait une attaque
-- dictionnaire hors-ligne → à isoler en production (KMS / secret dédié). Suivi P1.
CREATE TABLE IF NOT EXISTS opaque_server_setup (
    id smallint PRIMARY KEY DEFAULT 1,
    setup bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT opaque_server_setup_singleton CHECK (id = 1)
);
