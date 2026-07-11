-- Migration initiale (socle). Les tables métier arrivent plus tard :
-- comptes (P1 · OPAQUE), appareils (P2 · pairing), deltas chiffrés (P3 · sync).
-- Cette table-marqueur valide la chaîne de migrations de bout en bout.
CREATE TABLE IF NOT EXISTS schema_bootstrap (
    id smallint PRIMARY KEY DEFAULT 1,
    applied_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT schema_bootstrap_singleton CHECK (id = 1)
);

INSERT INTO schema_bootstrap (id) VALUES (1) ON CONFLICT (id) DO NOTHING;
