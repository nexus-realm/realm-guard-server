# Secrets d'alerte

Fichiers montés dans le conteneur Alertmanager (`/etc/alertmanager/secrets/`) et
lus via les options `*_file` de `alertmanager.yml`. **Jamais** de secret en clair
dans la config versionnée.

Les vrais fichiers (`discord_webhook`, `heartbeat_url`) sont **gitignorés** : seuls
les `.example` sont versionnés. Ils sont **optionnels** — si absents, Alertmanager
démarre quand même et journalise l'échec de notification (dégradation gracieuse).

## Discord (obligatoire pour recevoir les alertes)

1. Discord → Paramètres du salon → **Intégrations** → **Webhooks** → *Nouveau
   webhook* → copier l'URL.
2. `cp secrets/discord_webhook.example secrets/discord_webhook`, puis y coller
   l'URL (une seule ligne, rien d'autre).

## Heartbeat (optionnel — active le Watchdog)

Le Watchdog fire en permanence ; il faut un moniteur **externe** qui s'alarme si le
ping s'arrête. Ex. [healthchecks.io](https://healthchecks.io) (gratuit, self-host
possible) ou Dead Man's Snitch.

1. Créer un check « toutes les 1 min » → copier l'URL de ping.
2. `cp secrets/heartbeat_url.example secrets/heartbeat_url`, puis y coller l'URL.

Sans ce fichier, le Watchdog reste inerte (aucune supervision externe) mais ne
casse rien.
