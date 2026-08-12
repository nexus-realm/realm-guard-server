# Mise à jour des dépendances — realm-guard-server

## Automatique — Dependabot (`.github/dependabot.yml`)

- **Alertes de sécurité** (CVE) : activer une fois dans *Settings → Code security →
  Dependabot alerts* **+** *Dependabot security updates*.
- **PR de version** — **mensuelles**, **groupées** minor+patch par écosystème
  (majors en PR séparées) pour `cargo`, `npm`, `github-actions`, `docker` (le
  **Dockerfile**). Cible `develop` depuis des branches `dependabot/*`.
- **Prérequis secret** : ajouter `CORE_REPO_DEPLOY_KEY` dans *Settings → Secrets and
  variables → **Dependabot*** — la CI checkoute le cœur en **sibling** ; sans ce
  secret les PR échouent.
- Supply-chain gardé par **`cargo-deny`** (job `deny`).

### ⚠️ cargo + dépendance `path` vers le cœur

`Cargo.toml` référence le cœur en `path` (`../realm-guard-core`), **hors du repo**.
Dependabot clone le seul repo serveur → il **peut** ne pas résoudre le graphe cargo
et n'ouvrir **aucune PR cargo**. Vérifier le tableau de bord Dependabot ; si c'est
le cas, traiter cargo **en manuel** (ci-dessous — le sibling cœur est présent en
local).

## Manuel — procédure d'appoint

### Cargo (repli si Dependabot bloque, ou hors cadence)

```bash
cargo update                       # lock : semver-compatibles récentes
cargo upgrade --incompatible       # cargo-edit : bumpe les bornes `^` (majors)
```

`Cargo.lock` est versionné → committer le lock. **`--locked`** : ne **pas** mettre à
jour les deps pendant une fenêtre de release du cœur (le lock doit rester cohérent
avec la branche cœur checkoutée en CI — cf. pièges de release).

### Images du `docker-compose.yml` (NON couvertes par Dependabot)

Dependabot ne scanne pas les fichiers compose. Vérifier périodiquement les nouveaux
tags (tous **épinglés**) et bumper à la main :

`postgres:16-alpine` · `redis:7-alpine` · `caddy:2-alpine` ·
`prom/prometheus` (LTS) · `prom/alertmanager` · `postgres-exporter` ·
`oliver006/redis_exporter` · `prom/node-exporter` · `gcr.io/cadvisor/cadvisor` ·
`grafana/grafana`.

Après un bump d'image : `docker compose pull && docker compose up -d`, puis smoke
(cf. `docs/OBSERVABILITY.md`). Attention aux **majors** de Prometheus/Grafana
(changements de flags/dashboards) et de Postgres (migration de volume).

### npm

```bash
npm outdated                       # husky / commitlint
```

## Avant de merger (toute PR de deps)

Le **gate Rust** doit être vert :

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
cargo deny check
```

Le CI ajoute `observability-lint` + `docker-build` + le job `coverage`. Mise à jour
**manuelle** → branche `chore/rg-<N>`. Cadence : revue mensuelle des PR Dependabot ;
majors et images compose appliqués séparément après lecture des changelogs.
