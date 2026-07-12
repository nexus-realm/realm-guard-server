-- VaultKey enrobée pour la récupération multi-appareils : le blob stocké est
-- `export_key`-wrap(KEK-wrap(VaultKey)) + le sel Argon2id. Le serveur ne peut rien
-- en faire sans que le client complète OPAQUE (pour obtenir l'export_key). Un blob
-- par compte ; supprimé avec le compte.
CREATE TABLE IF NOT EXISTS vault_keys (
    account_id uuid PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    wrapped_key bytea NOT NULL,
    salt bytea NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
