-- Isolation du secret serveur OPAQUE : il n'est plus stocké en base (il y était
-- co-localisé avec les password files → un seul dump = attaque dictionnaire
-- hors-ligne). Désormais fourni au boot hors base, via RG_OPAQUE_SETUP_FILE /
-- RG_OPAQUE_SETUP (généré par `realm-guard-server generate-setup`). Cf. revue sécu P1.
DROP TABLE IF EXISTS opaque_server_setup;
