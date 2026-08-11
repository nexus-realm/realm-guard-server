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
- L'app **n'émet pas** de `process_*` (l'exporter Rust ne mesure pas son propre
  CPU/RAM/FD). Les ressources sont couvertes séparément par les **exporters
  infra** (§3) : cAdvisor voit la conso du conteneur `app`, node-exporter l'hôte.
- Protection optionnelle : si `RG_METRICS_TOKEN` est défini, `/metrics` exige
  `Authorization: Bearer <token>` (401 sinon). **Non posé en compose** → l'endpoint
  est ouvert en dev (cf. §6, *Durcissement production*).

### Métriques métier (P4)

Émises aux points d'événement (handlers sync / auth / ws) via des *recorders*
centralisés dans `src/observability.rs`. **Zero-knowledge** : préfixe `rg_`, et
**aucun label sensible** — pas de compte, username, IP ni `syncId`. Seul
`rg_auth_logins_total` porte un label borné (`outcome`).

| Métrique | Type | Labels | Émise quand |
|---|---|---|---|
| `rg_sync_deltas_pushed_total` | counter | — | delta poussé (`POST /sync/deltas`) |
| `rg_sync_deltas_pulled_total` | counter | — | deltas servis à un tirage |
| `rg_sync_snapshots_created_total` | counter | — | snapshot publié + log compacté |
| `rg_auth_registrations_total` | counter | — | inscription finalisée |
| `rg_auth_logins_total` | counter | `outcome` (`success`/`failure`) | login finalisé |
| `rg_auth_lockouts_total` | counter | — | refus pour compte verrouillé (anti-abus) |
| `rg_ws_connections` | **gauge** | — | connexions WebSocket actives (garde RAII) |

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

Prometheus (`scrape_interval` + `evaluation_interval` = 15s) scrape six cibles,
toutes résolues par nom de service sur le réseau Compose (`prometheus.yml`) :

| Job | Cible | Fournit |
|---|---|---|
| `prometheus` | `localhost:9090` | méta-supervision (P5) |
| `realm-guard-server` | `app:8080` | métriques applicatives (§1) |
| `postgres` | `postgres-exporter:9187` | connexions, taille DB, `pg_up`, stats requêtes |
| `redis` | `redis-exporter:9121` | mémoire, clients, `redis_up`, hits/miss |
| `node` | `node-exporter:9100` | CPU / RAM / **disque hôte** |
| `cadvisor` | `cadvisor:8080` | ressources **par conteneur** |

- Les exporters (P2) sont des services du `docker-compose.yml`, sans port publié
  sur l'hôte. `postgres-exporter` se connecte avec le compte applicatif **en dev
  seulement** ; en prod → rôle `pg_monitor` dédié via secret monté (cf. §6).
- Prometheus scrape l'app **directement** sur le réseau interne (`app:8080`), pas
  via Caddy.
- Caddy (`reverse_proxy app:8080`) proxie **tout**, `/metrics` compris → en prod,
  sans `RG_METRICS_TOKEN` ni règle de blocage au proxy, la volumétrie serait
  publique.
- **Docker Desktop (Windows)** : node-exporter mesure la VM Linux sous-jacente
  (pas Windows), et cAdvisor n'a qu'un support partiel du runtime — les deux
  fonctionnent en prod sur hôte Linux.
- **Images épinglées** (P5) : Prometheus `v2.53.2` (LTS), et les exporters/Grafana
  à une version fixe — plus de `:latest`, builds reproductibles.
- **Rétention TSDB = 30 j** (`--storage.tsdb.retention.time`, P5), sur un volume
  nommé `prom_data` ; recharge à chaud via `POST /-/reload` (`--web.enable-lifecycle`).
- Prometheus **se scrape lui-même** (job `prometheus`, P5).

### Grafana (P5)

Service `grafana` (`:3000`), **entièrement provisionné as-code** — aucun clic,
tout est versionné :

- `grafana/provisioning/datasources/prometheus.yml` — datasource Prometheus
  (`uid: prometheus`, référencé par le dashboard → **stable**).
- `grafana/provisioning/dashboards/dashboards.yml` — provider qui charge tous les
  `.json` de `grafana/dashboards/`.
- `grafana/dashboards/realm-guard-overview.json` — dashboard *Overview* (11
  panneaux) : santé dépendances (`pg_up`/`redis_up`), **RED** (requêtes par statut,
  taux 5xx, latence p50/p95/p99 via les buckets P1), **sync** (deltas poussés/tirés,
  snapshots), **auth** (logins par issue, verrouillages), connexions PG et WS.

Accès dev : `http://localhost:3000`, **anonyme en lecture** (pas de login). Les
sous-dossiers de provisioning sont montés **individuellement** (pas le parent),
sinon Grafana journalise des erreurs bénignes « répertoire absent » pour les
dossiers par défaut masqués (`plugins/`, `alerting/`).

> Vérifié en live : Prometheus `retention=30d`, `up{job="prometheus"}=1`,
> `POST /-/reload` → 200 ; Grafana santé `ok`, datasource « Successfully queried
> the Prometheus API », dashboard `rg-overview` (11 panneaux) provisionné, logs de
> provisioning propres.

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

### Alertes ressources & dépendances (via les exporters, P2)

Depuis l'ajout des exporters (§3), les ressources et la santé **directe** des
dépendances sont requêtables. Exemples :

| Objectif | PromQL |
|---|---|
| Postgres injoignable | `pg_up == 0` |
| Redis injoignable | `redis_up == 0` |
| Connexions PG proches du plafond | `sum(pg_stat_activity_count) / on() pg_settings_max_connections > 0.8` |
| Mémoire Redis élevée | `redis_memory_used_bytes / redis_memory_max_bytes > 0.9` (si `maxmemory` posé) |
| Disque hôte presque plein | `node_filesystem_avail_bytes{mountpoint="/"} / node_filesystem_size_bytes{mountpoint="/"} < 0.1` |
| RAM d'un conteneur | `container_memory_usage_bytes{name=~"realm-guard-server-.*"}` |

> Les noms de métriques et de labels dépendent de la version et de l'hôte ;
> vérifier sur `http://localhost:9090` (onglet *Graph*, autocomplétion). En
> particulier, `mountpoint="/"` vise le **root d'un hôte Linux de prod** : sur
> Docker Desktop, node-exporter mesure la VM WSL2 et n'expose pas `/` mais des
> chemins `…/docker-desktop-disk`.

`pg_up == 0` / `redis_up == 0` remplacent la déduction indirecte : `/readyz`
renvoie toujours 503 côté app si une dépendance tombe, mais l'alerte n'attend
plus une montée des 5xx.

> Vérifié en live (stack Compose) : les 4 cibles exporters `up`, `pg_up=1`,
> `redis_up=1`, `redis_memory_used_bytes` peuplé, node-exporter (160 séries CPU),
> et cAdvisor voyant les 7 conteneurs (`container_*` par `name`).

### Le pipeline : règles → Alertmanager (P3)

Les requêtes ci-dessus sont matérialisées en **règles d'alerte** dans
`rules/alerts.yml` (chargées par `rule_files` dans `prometheus.yml`, évaluées
toutes les 15 s). Une règle passe *pending* dès que sa condition est vraie, puis
*firing* après sa durée `for:` (anti-flapping) ; Prometheus pousse alors l'alerte
vers **Alertmanager** (service `alertmanager`, `:9093`), qui la regroupe,
déduplique et route vers un *receiver*.

Règles livrées :

| Alerte | Condition (résumé) | `for` | `severity` |
|---|---|---|---|
| `CibleInjoignable` | `up == 0` (app ou exporter) | 2m | critical |
| `PostgresInjoignable` / `RedisInjoignable` | `pg_up == 0` / `redis_up == 0` | 1m | critical |
| `TauxErreurs5xxEleve` | part de 5xx > 5 % | 5m | warning |
| `LatenceP95Elevee` | p95 > 1 s (via buckets P1) | 10m | warning |
| `ConnexionsPostgresElevees` | connexions PG > 80 % du plafond | 5m | warning |
| `MemoireRedisElevee` | Redis > 90 % de `maxmemory` (gardé `> 0`) | 5m | warning |
| `EspaceDisqueFaible` | disque hôte `/` < 10 % | 5m | warning |
| `Watchdog` | `vector(1)` — fire en permanence (dead-man's-switch) | — | none |

**Notifications — Discord + Watchdog.** `route` (dans `alertmanager.yml`) :

- **`critical`** (cible/pg/redis down) → **Discord**, regroupement réactif (`group_wait: 10s`) ;
- **`warning`** (5xx, latence, seuils) → **Discord** aussi, cadence par défaut ;
- **`Watchdog`** → receiver dédié `watchdog` (webhook vers un **heartbeat externe**),
  **jamais** Discord — sinon spam toutes les minutes.

L'**inhibition** évite le bruit (une panne `critical` masque les `warning` du même
`job`) et le **throttling** (`repeat_interval: 4h`) borne les rappels.

**Embed Discord.** Titre `[FIRING] <alerte> (<sévérité>)` + couleur (rouge *firing*
/ vert *resolved*) ; corps = `summary` (gras) + `description` + la cible + un lien
**cliquable** vers la métrique. Ce lien (`generatorURL`) n'est joignable que si
Prometheus a un **`--web.external-url`** — sinon il pointe sur le hostname *interne*
du conteneur (injoignable depuis un téléphone). Posé à `http://localhost:9090` en
dev ; **à remplacer par l'URL publique en prod** (idem pour Alertmanager).

**Secrets.** Le webhook Discord et l'URL de heartbeat sont lus depuis des **fichiers
montés** (`webhook_url_file` / `url_file`, cf. `secrets/README.md`), jamais en clair
dans la config. Absents → Alertmanager démarre quand même, l'envoi échoue et est
journalisé (dégradation gracieuse). Le `webhook_url_file` Discord impose
**Alertmanager ≥ 0.28** (d'où le pin `v0.28.1`).

**Dead-man's-switch.** `Watchdog` fire en continu ; un moniteur **externe**
(healthchecks.io, Dead Man's Snitch) surveille ce battement. Si Prometheus,
Alertmanager ou le serveur tombe, le ping s'arrête → le moniteur externe alerte.
C'est ce qui « surveille le surveillant » (heartbeat optionnel : sans `heartbeat_url`,
le Watchdog reste inerte mais ne casse rien).

> Vérifié en live de bout en bout (Alertmanager v0.28.1, secrets pointés sur un
> sink HTTP) : `RedisInjoignable`/`CibleInjoignable` livrées à Discord en **embed
> lisible** (summary + description + cible + **lien source joignable**
> `http://localhost:9090/…`) ; le `Watchdog` routé vers le heartbeat (**pas**
> Discord) ; démarrage sain **sans** les fichiers secrets.

### Alertes métier & anti-abus (via les métriques P4)

Les métriques `rg_*` (§1) ouvrent un axe **produit / anti-abus** que les seules
métriques HTTP ne permettaient pas. Ces requêtes ne sont **pas** dans
`rules/alerts.yml` (jeu volontairement centré dispo/ressources) — ce sont des
candidates à y ajouter selon les seuils souhaités :

| Objectif | PromQL |
|---|---|
| Rafale de verrouillages (attaque distribuée par compte) | `sum(rate(rg_auth_lockouts_total[5m])) > 1` |
| Taux d'échec de login anormal | `sum(rate(rg_auth_logins_total{outcome="failure"}[5m])) / sum(rate(rg_auth_logins_total[5m])) > 0.5` |
| Fuite de connexions WebSocket (jauge qui ne redescend pas) | `rg_ws_connections > 500` |
| Sync au point mort malgré du trafic (aucun delta poussé) | `sum(rate(rg_sync_deltas_pushed_total[15m])) == 0` |

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
- **P2 — visibilité infra.** ✅ **Fait** (§3) : `postgres-exporter`,
  `redis-exporter`, `node-exporter` et `cadvisor` en services compose +
  `scrape_configs`. Débloque les alertes ressource (CPU, RAM, disque, connexions
  PG, mémoire Redis) et la supervision **directe** des dépendances (`pg_up`,
  `redis_up`). Reste de niveau prod : rôle `pg_monitor` dédié, épinglage de
  l'image Prometheus.
- **P3 — pipeline d'alerte.** ✅ **Fait** (§4, *Le pipeline*) : **Alertmanager**
  (Prometheus-natif) + `rules/alerts.yml` + bloc `alerting`, **receiver Discord**
  (secret fichier) + routage par sévérité + **Watchdog** (dead-man's-switch). Reste
  à l'exploitant : déposer le webhook Discord et l'URL de heartbeat (`secrets/`).
- **P4 — métriques métier.** ✅ **Fait** (§1, *Métriques métier*) : compteurs sync
  (`rg_sync_*`), auth (`rg_auth_*`, dont `rg_auth_lockouts_total` anti-abus) et
  jauge `rg_ws_connections`, tous zero-knowledge. Câblage vérifié de bout en bout
  par le test d'intégration. Ouvre l'alerting produit/anti-abus (exemples §4).
- **P5 — durcissement + Grafana.** ✅ **Fait** (§3, durcissement + *Grafana*) :
  images épinglées, rétention 30 j, auto-scrape Prometheus, et **Grafana
  provisionné as-code** (datasource + dashboard *Overview*). Reste de niveau prod :
  voir la checklist *Durcissement production* ci-dessous.

Chaque étape P2–P5 est de l'ajout d'infra/config **sans toucher au code
applicatif** (à l'exception de P4, qui instrumente le code).

### Durcissement production (à faire hors dev)

Le `docker-compose.yml` est une stack **dev** volontairement ouverte sur le réseau
interne. Avant un déploiement exposé :

- **Protéger `/metrics`** : poser `RG_METRICS_TOKEN` sur l'app **et** l'envoyer
  depuis Prometheus (`authorization: { credentials: <token> }` dans le
  `scrape_config` de `realm-guard-server`), ou bloquer `/metrics` au niveau Caddy.
- **Rôle Postgres dédié** : `postgres-exporter` doit utiliser un rôle `pg_monitor`
  en lecture seule (pas le compte applicatif), fourni via secret monté.
- **Grafana** : désactiver l'accès anonyme (`GF_AUTH_ANONYMOUS_ENABLED=false`) et
  poser un `GF_SECURITY_ADMIN_PASSWORD` fort via secret.
- **Alertmanager** : déposer les secrets réels (`secrets/discord_webhook`,
  `secrets/heartbeat_url` — cf. `secrets/README.md`) ; brancher le heartbeat sur un
  moniteur externe pour activer le Watchdog.
- **URL externes** : remplacer les `--web.external-url=http://localhost:{9090,9093}`
  (Prometheus/Alertmanager) par les URL **publiques** — sinon les liens des
  notifications Discord (source de l'alerte) restent injoignables hors de l'hôte.
- **TLS** : terminer au reverse proxy (le token Bearer transite sinon en clair).
- **Secrets** : ne pas réutiliser le `RG_OPAQUE_SETUP` de dev.

---

## 7. Recettes opérationnelles

### Voir les métriques via la stack Compose

```bash
docker compose up -d              # app + pg + redis + caddy + prometheus + exporters + alertmanager + grafana
curl -s localhost:8080/metrics    # métriques app, via Caddy
# Prometheus  : http://localhost:9090  (onglet Alerts pour l'état des règles)
# Alertmanager: http://localhost:9093
# Grafana     : http://localhost:3000  (anonyme ; dashboard « Realm Guard — Overview »)
# Cibles scrapées (self + app + 4 exporters, doivent être UP) :
curl -s 'http://localhost:9090/api/v1/targets?state=active' \
  | python -c "import sys,json;[print(t['labels']['job'], t['health']) for t in json.load(sys.stdin)['data']['activeTargets']]"
```

### Activer les notifications Discord

```bash
cp secrets/discord_webhook.example secrets/discord_webhook   # puis y coller le webhook
# (optionnel) cp secrets/heartbeat_url.example secrets/heartbeat_url   # active le Watchdog
docker compose up -d alertmanager                            # relit les secrets au vol
```

Sans ces fichiers, la stack démarre quand même — Alertmanager journalise juste
l'échec d'envoi (cf. `secrets/README.md`).

### Déclencher une alerte (vérifier le pipeline)

```bash
docker compose stop redis         # redis_up passe à 0
# ~1 min plus tard (for: 1m), RedisInjoignable fire, arrive dans Alertmanager
# et — si le webhook est posé — est livrée à Discord en embed :
curl -s 'http://localhost:9093/api/v2/alerts' \
  | python -c "import sys,json;[print(a['labels']['alertname'], a['status']['state']) for a in json.load(sys.stdin)]"
docker compose start redis        # l'alerte se résout au prochain scrape
```

Valider règles et config d'alerte hors ligne (sur Git Bash, préfixer
`MSYS_NO_PATHCONV=1` pour les chemins internes au conteneur) :

```bash
docker run --rm --entrypoint promtool -v "$PWD/rules:/rules:ro" \
  prom/prometheus:v2.53.2 check rules /rules/alerts.yml
docker run --rm --entrypoint amtool -v "$PWD/alertmanager.yml:/am.yml:ro" \
  prom/alertmanager:v0.28.1 check-config /am.yml
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
  `src/health.rs` (`/healthz`, `/readyz`).
- Config observabilité : `prometheus.yml` (scrape + `rule_files` + `alerting`),
  `rules/alerts.yml` (règles + Watchdog), `alertmanager.yml` (routage Discord +
  Watchdog), `secrets/` (webhook Discord + heartbeat, hors-git), `grafana/`
  (datasource + dashboards provisionnés, P5), `docker-compose.yml` (app +
  exporters + prometheus + alertmanager + grafana).
- Variables d'env : `RG_METRICS_TOKEN` (protège `/metrics`), `SENTRY_DSN`
  (active Sentry), `RUST_LOG` (niveau de trace).
