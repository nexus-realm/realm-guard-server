# Observabilité — realm-guard-server

Deux piliers, distincts et complémentaires :

- **Métriques** (Prometheus) — volumétrie, latence, taux d'erreur. Endpoint
  `/metrics`, agrégées, sans donnée personnelle.
- **Erreurs** (Sentry) — exceptions et panics, DSN-gated, PII désactivée.

Contrainte transverse : le serveur est **zero-knowledge**. Ni les métriques ni les
traces ne doivent exposer de contenu de coffre, de mot de passe, ni d'identifiant
de compte en clair. L'instrumentation étiquette par **motif de route** (pas l'URL
brute) et Sentry **scrub** le contexte requête et le hostname (cf. `SECURITY.md`).

Ce document décrit l'existant, ce qu'il permet **aujourd'hui**, ses **lacunes**, et
la **feuille de route** vers une stack Grafana d'alerting. À tenir à jour avec le
code (`src/observability.rs`, `src/main.rs`, `prometheus.yml`, `docker-compose.yml`).

---

## 1. Ce qui est exposé

L'endpoint `/metrics` (`src/lib.rs`) rend le format texte Prometheus. Le
middleware `track_metrics` (`src/observability.rs`) instrumente **toutes** les
routes.

| Métrique | Type | Labels | Sens |
|---|---|---|---|
| `http_requests_total` | counter | `method`, `path`, `status` | Nombre de requêtes par route et code HTTP |
| `http_request_duration_seconds` | **histogram** | `method`, `path` | Latence par route (buckets, cf. §2) |

- **`path` = motif de route** (`MatchedPath`, ex. `/sync/deltas`), jamais l'URL
  concrète — la cardinalité des séries reste bornée.
- Pas de métrique `process_*` : l'exporter Rust (`metrics-exporter-prometheus`)
  n'émet **pas** de CPU/RAM/FD du process. Pour ça → exporters dédiés (§6, P2).
- Protection optionnelle : si `RG_METRICS_TOKEN` est défini, `/metrics` exige
  `Authorization: Bearer <token>` (401 sinon). **Non posé en compose** → l'endpoint
  est ouvert en dev (cf. §5, durcissement prod).

---

## 2. Format des histogrammes : **buckets**, pas summary

> ⚠️ Point structurant. Une régression ici casse silencieusement tout l'alerting
> de latence.

Par défaut, `metrics-exporter-prometheus` rend un histogramme en **summary** :
des quantiles calculés côté processus, sur une fenêtre glissante. C'est un
mauvais support d'alerting — les quantiles d'un summary ne sont **pas
requêtables** par `histogram_quantile()`, **pas agrégeables** entre instances, et
ne permettent pas les SLO (« part des requêtes sous X ms »).

Le serveur configure donc des **buckets explicites** pour
`http_request_duration_seconds` (`set_buckets_for_metric` dans
`prometheus_builder()`), ce qui le rend en **vrai histogramme** Prometheus
(séries `_bucket{le=…}`, `_sum`, `_count`).

Bornes (secondes), les valeurs par défaut de Prometheus, adaptées à une API web :

```
0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10
```

**Règle de maintenance** — les buckets sont posés **par métrique** (`Matcher::Full`).
Tout **nouvel** histogramme non déclaré retomberait en summary : lui enregistrer
ses propres bornes (une distribution de tailles d'octets, p. ex., n'a rien à voir
avec des secondes). Un test (`latency_histogram_renders_as_buckets_not_summary`)
verrouille le format sur la config de prod.

---

## 3. Scrape & topologie

`prometheus.yml` :

```yaml
global:
  scrape_interval: 15s
scrape_configs:
  - job_name: "realm-guard-server"
    metrics_path: /metrics
    static_configs:
      - targets: ["app:8080"]
```

- Prometheus scrape l'app **directement** sur le réseau interne (`app:8080`), pas
  via Caddy.
- Caddy (`reverse_proxy app:8080`) proxie **tout**, `/metrics` compris → en prod,
  sans `RG_METRICS_TOKEN` ni règle de blocage au proxy, la volumétrie serait
  publique.
- L'image `prom/prometheus:latest` n'est **pas épinglée** (à figer en prod).
- Aucun flag `--storage.tsdb.retention*` → **rétention par défaut (15 j)**.
- Prometheus ne se scrape **pas** lui-même (pas de méta-supervision).

---

## 4. Alerting — ce qui est possible aujourd'hui

Avec le seul `http_requests_total` et le `up` intégré de Prometheus, l'essentiel
est déjà couvert. Ces requêtes sont exploitables telles quelles (règles d'alerte
ou panneaux Grafana) :

| Objectif | PromQL |
|---|---|
| Service indisponible | `up{job="realm-guard-server"} == 0` |
| Taux d'erreurs 5xx | `sum(rate(http_requests_total{status=~"5.."}[5m])) / sum(rate(http_requests_total[5m]))` |
| Abus d'auth (401 en rafale) | `rate(http_requests_total{path="/auth/login",status="401"}[5m])` |
| Rate-limit atteint (429) | `rate(http_requests_total{status="429"}[5m])` |
| Chute / pic de trafic | `sum(rate(http_requests_total[5m]))` |

Depuis le passage aux buckets (§2), la **latence** est pleinement requêtable —
ces deux formes étaient **impossibles** avec un summary :

```promql
# p95 de latence par route
histogram_quantile(0.95, sum by (le, path) (rate(http_request_duration_seconds_bucket[5m])))

# SLO : part des requêtes /readyz servies sous 100 ms
sum(rate(http_request_duration_seconds_bucket{path="/readyz",le="0.1"}[5m]))
  / sum(rate(http_request_duration_seconds_count{path="/readyz"}[5m]))
```

> Vérifié de bout en bout contre un Prometheus réel scrutant le serveur :
> `up == 1`, type `histogram` reconnu, `histogram_quantile(0.95)` ≈ 4,75 ms sur
> `/readyz`, ratio SLO = 100 %.

**Note sur la santé des dépendances.** `/readyz` renvoie 503 si Postgres/Redis
est injoignable, mais **n'est pas scrapé** — il n'existe donc pas de métrique
« DB up ». Aujourd'hui, une panne DB se déduit **indirectement** (montée des 5xx,
ou `up == 0` si le process tombe). Une supervision directe des dépendances
demande les exporters de la §6 (P2).

---

## 5. Erreurs (Sentry)

Initialisé dans `src/main.rs` **avant** le runtime async :

- **DSN via `SENTRY_DSN`** — absent → client désactivé, **aucun réseau** (défaut
  dev/CI).
- `send_default_pii: false` + `before_send` qui vide `event.request` et
  `event.server_name` → ni contexte requête ni hostname ne partent.
- `release` renseignée (`sentry::release_name!()`) pour corréler par version.

Sentry couvre les **erreurs**, Prometheus les **métriques** : les deux sont
nécessaires, aucun ne remplace l'autre.

---

## 6. Lacunes connues & feuille de route Grafana

Grafana est **utilisable dès maintenant** pour un tableau de bord RED
(Rate/Errors/Duration) en branchant Prometheus en datasource. Pour en faire une
base d'alerting complète, dans l'ordre de priorité :

- **P1 — buckets de latence.** ✅ **Fait** (§2). Débloque percentiles, agrégation
  et SLO.
- **P2 — visibilité infra.** Ajouter `postgres_exporter`, `redis_exporter`, et
  `cadvisor`/`node_exporter` (services compose + `scrape_configs`). Sans eux,
  **aucune** alerte ressource (CPU, RAM, disque, connexions PG, mémoire Redis) ni
  supervision directe des dépendances.
- **P3 — pipeline d'alerte.** Soit **Alertmanager** (`rule_files` + bloc
  `alerting` dans `prometheus.yml`), soit **alerting natif Grafana** (règles
  gérées par Grafana, un composant de moins). Les deux se valent ; Grafana-managed
  est plus simple pour démarrer.
- **P4 — métriques métier.** Émettre des compteurs/jauges pour la sync
  (`deltas_pushed_total`, `snapshots_created_total`, jauge `ws_connections`),
  l'auth, les hits de rate-limit. Active l'alerting produit / anti-abus.
- **P5 — durcissement.** Poser `RG_METRICS_TOKEN` en prod (ou bloquer `/metrics`
  au niveau Caddy), épingler l'image Prometheus, fixer
  `--storage.tsdb.retention.time`, scraper Prometheus lui-même, et provisionner
  Grafana en *as-code* (`provisioning/datasources` + `provisioning/dashboards`).

Chaque étape P2–P5 est de l'ajout d'infra/config **sans toucher au code
applicatif** (à l'exception de P4, qui instrumente le code).

---

## 7. Recettes opérationnelles

### Voir les métriques via la stack Compose

```bash
docker compose up -d              # app + postgres + redis + caddy + prometheus
curl -s localhost:8080/metrics    # via Caddy
# Prometheus : http://localhost:9090
```

### Vérifier `/metrics` sans la stack (app native)

Le Compose **ne publie pas** les ports Postgres/Redis sur l'hôte. Pour tester
l'app native contre des bases jetables :

```bash
docker run -d --rm --name rg-pg -e POSTGRES_USER=realmguard -e POSTGRES_PASSWORD=realmguard -e POSTGRES_DB=realmguard -p 55432:5432 postgres:16-alpine
docker run -d --rm --name rg-redis -p 56379:6379 redis:7-alpine

DATABASE_URL="postgres://realmguard:realmguard@localhost:55432/realmguard?sslmode=disable" \
REDIS_URL="redis://localhost:56379" \
RG_OPAQUE_SETUP="<setup base64 de dev>" \
cargo run --bin realm-guard-server

# dans un autre shell
curl -s localhost:8080/readyz          # génère de la latence
curl -s localhost:8080/metrics | grep http_request_duration_seconds_bucket

docker stop rg-pg rg-redis             # conteneurs en --rm : disparaissent
```

Attendu : `# TYPE http_request_duration_seconds histogram` et des séries
`_bucket{le=…}`. Si tu vois `quantile=…`, l'histogramme est reparti en summary
(recompiler le binaire après un changement de `observability.rs`).

### Vérifier la chaîne Prometheus (facultatif)

Un Prometheus jetable scrutant l'app native (config en fichier temporaire, cible
`host.docker.internal:8080`) permet de valider `histogram_quantile` :

```bash
curl -s 'http://localhost:9090/api/v1/query' \
  --data-urlencode 'query=histogram_quantile(0.95, sum by (le) (rate(http_request_duration_seconds_bucket[1m])))'
```

---

## 8. Références

- Modèle de menace et exigences de déploiement : [`SECURITY.md`](SECURITY.md).
- Code : `src/observability.rs` (métriques), `src/main.rs` (Sentry, tracing),
  `src/health.rs` (`/healthz`, `/readyz`), `prometheus.yml`, `docker-compose.yml`.
- Variables d'env : `RG_METRICS_TOKEN` (protège `/metrics`), `SENTRY_DSN`
  (active Sentry), `RUST_LOG` (niveau de trace).
