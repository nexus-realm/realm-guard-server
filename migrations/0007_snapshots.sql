-- Snapshot d'état complet, chiffré et **opaque** au serveur : un par compte.
--
-- Sert à **compacter** le log de deltas, qui grossit sans fin par nature. Un client
-- qui a fusionné tout ce qu'il a pu tirer publie l'état complet en déclarant le
-- `covers_seq` qu'il couvre ; les deltas ≤ `covers_seq` deviennent alors redondants
-- et sont purgés.
--
-- `covers_seq` est **le contrat** : un appareil dont le curseur est antérieur ne doit
-- **pas** se contenter des deltas restants (il raterait tout le reste en silence) —
-- il doit repartir du snapshot. Le cœur garantit que ce repli est correct : un join
-- d'état complet équivaut à l'échange de deltas (`delta_exchange_matches_full_state_exchange`).
--
-- Le serveur ne peut pas valider le contenu (E2EE) : il fait confiance au client du
-- compte, qui ne peut de toute façon corrompre que ses propres données.
CREATE TABLE IF NOT EXISTS snapshots (
    account_id uuid PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    payload bytea NOT NULL,
    -- Plus grand `seq` de delta inclus dans ce snapshot.
    covers_seq bigint NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
