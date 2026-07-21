-- Log **append-only** de deltas CRDT chiffrés, par compte.
--
-- Le serveur ne voit que des octets **opaques** : il ne déchiffre rien et ne fusionne
-- rien — le merge est entièrement côté client (`realm_guard_core::model`).
--
-- Le cœur garantit (property-tests) que la convergence survit à une livraison
-- **désordonnée et dupliquée** : ce log n'a donc ni à dédoublonner, ni à garantir un
-- ordre causal. `seq` n'est qu'un **curseur de rattrapage** monotone.
--
-- Croissance : le log grossit sans fin par nature → compaction par snapshot (lot
-- suivant), qui purgera les deltas couverts.
CREATE TABLE IF NOT EXISTS deltas (
    seq bigserial PRIMARY KEY,
    account_id uuid NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    payload bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- Le seul accès : « les deltas de ce compte après ce curseur ».
CREATE INDEX IF NOT EXISTS deltas_account_seq_idx ON deltas (account_id, seq);
