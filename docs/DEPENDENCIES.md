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
- **Ignorées** (`ignore` dans `dependabot.yml`, saut de minor bloqué, patches OK) :
  `getrandom` (API 0.3+ + épinglage rand/opaque-ke, comme le cœur) et
  `dtolnay/rust-toolchain` (son "tag" = version de Rust, pas un tag d'action →
  Dependabot inventait des versions inexistantes ; **bump manuel**).

### ⚠️ cargo + dépendance `path` vers le cœur

`Cargo.toml` référence le cœur en `path` (`../realm-guard-core`), **hors du repo**.
Dependabot clone le seul repo serveur → il **peut** ne pas résoudre le graphe cargo
et n'ouvrir **aucune PR cargo**. Vérifier le tableau de bord Dependabot ; si c'est
le cas, traiter cargo **en manuel** (ci-dessous — le sibling cœur est présent en
local).

### Re-sync du lock après un changement du cœur (bump de dep **ou release**)

Corollaire de la dép `path` : dès que le cœur change sur `develop` — un bump
Dependabot mergé côté cœur (ex. `ed25519-dalek 2 → 3`) **ou un release du cœur qui
bump sa `version`** (ex. `1.0.0 → 1.0.1`) — le `Cargo.lock` du serveur (qui épingle
la **version** et le sous-graphe du cœur) devient **périmé**. Le job **`docker-build`**
(le seul à builder en `--locked`) échoue alors — les autres jobs, sans `--locked`,
régénèrent le lock en silence :

```
error: cannot update the lock file … because --locked was passed
```

Re-synchroniser (le sibling cœur **à jour** doit être présent en local), puis
committer le lock :

```bash
cargo update -p realm-guard-core   # re-résout le sous-graphe du cœur (ciblé)
cargo build --release --locked     # reproduit le check du Dockerfile → doit passer
cargo deny check                   # voir l'avertissement ci-dessous
```

⚠️ **Le job `deny` peut échouer pour deux raisons distinctes** : (1) le même décalage
`--locked`, et (2) une **vulnérabilité réelle** que le lock ré-résolu fait apparaître
(cargo-deny lit la base d'advisories à jour — ex. `RUSTSEC-2026-0258` sur `h2`). Dans
ce cas, patcher aussi le crate visé : `cargo update -p <crate>`.

Ça n'ajoute que la nouvelle génération de crates au lock (pas de churn des autres
deps). **Le mobile n'est pas concerné** : il épingle le cœur par **tag**, pas par
la branche `develop`.

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
