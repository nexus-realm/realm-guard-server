-- Registre des appareils d'un compte. Un appareil est identifié par sa clé
-- publique Ed25519 (`device_pk`) ; il est enregistré par un appareil SOURCE déjà
-- authentifié lors du pairing (« la source vouche »). `revoked_at` non nul =
-- accès coupé (auth par clé d'appareil refusée). La révocation cryptographique
-- (rotation de la VaultKey) est un suivi séparé.
CREATE TABLE IF NOT EXISTS devices (
    id uuid PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_pk bytea NOT NULL UNIQUE,
    name text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz
);

CREATE INDEX IF NOT EXISTS devices_account_id_idx ON devices (account_id);
