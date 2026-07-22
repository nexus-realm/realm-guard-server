# realm-guard-server

Serveur de **synchronisation chiffrée de bout en bout** de Realm Guard (Axum).
Le serveur ne voit **jamais** de données en clair : il **stocke et relaie** des
deltas CRDT chiffrés (cf. [`realm-guard-core`](https://github.com/nexus-realm/realm-guard-core))
et authentifie les comptes en **zero-knowledge** (OPAQUE). Le mobile reste
pleinement utilisable **sans** ce serveur (synchronisation optionnelle).

## Rôle

- **Authentification OPAQUE** (le mot de passe ne quitte jamais le client) +
  sessions révocables.
- **Journal de deltas** append-only par compte (indexé par `seq`) + **snapshots /
  compaction**.
- **Appairage** d'appareils (relais du handshake), **registre d'appareils** +
  device-auth (Ed25519), **sauvegarde** de la clé de coffre scellée.
- **WebSocket** de réveil temps réel (*nudge* uniquement — jamais de livraison de
  données : le client tire par curseur).

## Stack

| Domaine | Choix |
|---|---|
| Framework | **Axum 0.8** (Tokio) |
| Base de données | **Postgres** via `sqlx` (migrations `migrations/`) |
| Cache / pub-sub / sessions | **Redis** |
| Crypto / OPAQUE / CRDT | `realm-guard-core` (dépendance Cargo) |
| Observabilité | Prometheus (`/metrics`) + traces `tower-http` |

Édition Rust **2024**, MSRV **1.88**. Endpoints : `auth_api`, `pairing_api`,
`vault_api` (backup de clé), `devices_api` + `device_auth_api`, `sync_api`
(deltas + snapshots), `sync_ws` (nudge), plus `/healthz`, `/readyz`, `/metrics`.

## Démarrage (dev)

```bash
docker compose up -d postgres redis      # dépendances
export DATABASE_URL=postgres://…          # voir docker-compose.yml
export REDIS_URL=redis://…
export RG_OPAQUE_SETUP_FILE=/chemin/opaque_setup.bin   # secret OPAQUE hors base
cargo run                                 # applique les migrations puis sert
```

La stack complète (Postgres + Redis + serveur + Caddy + Prometheus) est décrite
dans `docker-compose.yml`. Depuis un émulateur Android, l'app joint l'hôte via
`10.0.2.2:8080`.

## Configuration (env)

- `DATABASE_URL`, `REDIS_URL` — services.
- `RG_OPAQUE_SETUP_FILE` **ou** `RG_OPAQUE_SETUP` — graine OPAQUE (**hors base**,
  jamais co-localisée avec les `password_file` ; **ne jamais régénérer**).
- `RG_METRICS_TOKEN` (optionnel) — protège `/metrics` par Bearer.

## Développement

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test        # tests d'intégration via build_app + oneshot (Postgres requis pour certains)
cargo deny check
```

### Couverture de tests

```bash
rustup component add llvm-tools-preview && cargo install cargo-llvm-cov  # une fois
cargo cov         # unitaires seuls (34 %) — machine nue, sans base
cargo cov-full    # + tests d'intégration : nécessite Postgres + Redis
cargo cov-html    # rapport navigable (intégration comprise)
cargo cov-lcov    # export lcov.info (intégration comprise)
```

⚠️ `cargo cov` seul **sous-estime largement** la couverture : les tests
d'intégration sont `#[ignore]` (ils exigent Postgres + Redis), et les modules DB
— `accounts`, `deltas`, `sessions`, `snapshots`, `vault_keys`, `devices` —
n'ont pas d'autre exercice. Le chiffre qui compte est celui de `cargo cov-full` :
**95,2 % de lignes**.

Comme partout avec `cargo-llvm-cov`, ce chiffre **inclut les modules
`#[cfg(test)]`** (couverts à ~100 % par construction). Sur le seul code de
production, la couverture est de **94,6 %**.

Les ports de la stack Compose ne sont pas publiés sur l'hôte ; pour lancer
`cov-full` en local, exposer une base jetable puis pointer dessus :

```bash
docker run -d --rm --name rg-cov-pg -e POSTGRES_USER=realmguard -e POSTGRES_PASSWORD=realmguard -e POSTGRES_DB=realmguard -p 55432:5432 postgres:16-alpine
docker run -d --rm --name rg-cov-redis -p 56379:6379 redis:7-alpine
DATABASE_URL="postgres://realmguard:realmguard@localhost:55432/realmguard?sslmode=disable" REDIS_URL="redis://localhost:56379" cargo cov-full
docker stop rg-cov-pg rg-cov-redis
```

Les tests appliquent eux-mêmes les migrations. Seul `src/main.rs` est exclu du
rapport (racine de composition). La CI publie le résumé à chaque PR — avec les
services Postgres/Redis, donc le chiffre réel — **sans jamais bloquer**.

## Sécurité & déploiement

- **TLS obligatoire en production** (le token Bearer transite sinon en clair) —
  terminaison au reverse proxy.
- Modèle de menace et compromis acceptés : [`SECURITY.md`](docs/SECURITY.md).
- Signaler une vulnérabilité **en privé** (pas d'issue publique).
