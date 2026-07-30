# AGENTS.md — realm-guard-server

> Agent-oriented context. Dense, factual, scannable. Keep in sync with the code.
> Docstrings/user text are French; this doc is English for AI efficiency.

## 1. What this is

- The **E2EE sync server** for Realm Guard (Axum). It **only stores and relays
  encrypted CRDT deltas** and authenticates accounts **zero-knowledge** (OPAQUE) —
  it never sees the master password or vault plaintext.
- Mobile is **local-first**; this server is **optional** (opt-in sync).
- **Invariant:** the server is a dumb, opaque relay. Never add a code path that
  needs to read delta/vault contents. Metadata (structure, HLC, deviceId, seq) is
  visible in clear; **values are encrypted client-side**.

## 2. Stack & layout

- **Axum 0.8** (Tokio), **Postgres** via `sqlx` (`migrations/`, `sqlx::migrate!`),
  **Redis** (sessions, rate-limit, pub/sub for WS nudges), crypto/OPAQUE/CRDT from
  `realm-guard-core` (Cargo dep). Edition 2024, MSRV 1.88.
- `build_app(state) -> Router` is separated from the network bind so integration
  tests run via `tower::ServiceExt::oneshot` (deterministic). `run_migrations` is
  explicit (needs Postgres).

| Module | Responsibility |
|---|---|
| `auth_api` / `sessions` / `accounts` | OPAQUE register/login (`/auth/*`), opaque session tokens in Redis (revocable, 7-day TTL), account rows |
| `device_auth_api` / `devices` / `devices_api` | device-auth challenge/verify (Ed25519), device registry (register/list/rename/revoke) |
| `pairing_api` | relays the device-to-device pairing handshake |
| `vault_api` / `vault_keys` | sealed **VaultKey backup** (`PUT/GET /vault/key`) |
| `sync_api` / `deltas` / `snapshots` | append-only **delta log per account** (indexed by `seq`) + **snapshot/compaction** |
| `sync_ws` | `/sync/ws` — realtime **nudge** (fan-out via Redis pub/sub) |
| `rate_limit` | per-account brute-force lockout (Redis) |
| `health` / `observability` / `config` / `state` | `/healthz` `/readyz` `/metrics`; Prometheus + traces; `Config::from_env`; `AppState` |

## 3. Sync protocol (durable)

- **Delta log:** `POST /sync/deltas` appends; `GET /sync/deltas?since=<seq>` pulls.
  At-least-once, **no dedup, no ordering guarantee** beyond `seq`. The client
  merges (CRDT is idempotent/commutative).
- **Snapshot/compaction:** `PUT/GET /sync/snapshot` (a client publishes a doc
  snapshot with `covers_seq`); deltas ≤ `covers_seq` are purged. A puller whose
  cursor precedes the snapshot gets **410 Gone** → it restarts from the snapshot.
- **WS = wake only** (LOCKED decision): `/sync/ws` emits the new `seq` as a nudge;
  the client always **pulls by cursor**. Redis pub/sub is fire-and-forget;
  delivering deltas over WS would diverge on a drop. `publish_nudge` best-effort.

## 4. Auth & zero-knowledge

- **OPAQUE** (via core): the server stores a `password_file` unusable without
  completing the protocol; the **OPAQUE ServerSetup seed is supplied out of band**
  (`RG_OPAQUE_SETUP_FILE` / `RG_OPAQUE_SETUP`) — never co-located with password
  files, so a DB dump alone doesn't enable an offline dictionary attack.
- **Login is anti-enumeration** (unknown user ⇒ indistinguishable response;
  per-account rate-limit applied on the sent username either way). **Register
  leaks account presence** (409 on taken username) — an accepted tradeoff bounded
  by the proxy per-IP rate-limit. Full threat model: **`SECURITY.md`**.

## 5. Config (env)

`DATABASE_URL`, `REDIS_URL`, `RG_OPAQUE_SETUP_FILE`|`RG_OPAQUE_SETUP` (required),
`RG_METRICS_TOKEN` (optional — Bearer-protects `/metrics`), `SENTRY_DSN`
(optional — enables Sentry). Metrics, alerting and the Grafana roadmap:
**`OBSERVABILITY.md`**. `http_request_duration_seconds` is a real histogram
(explicit buckets) — new histograms need their own buckets or fall back to a
summary.

## 6. Dev, gate & deploy

```bash
docker compose up -d postgres redis
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings
cargo test && cargo deny check
```
- **Deploy:** TLS mandatory in prod (Bearer token). **Never regenerate the OPAQUE
  ServerSetup** (invalidates every account).
- **⚠️ Docker image embeds migrations at build** → if the DB was migrated further
  (e.g. by an e2e test), a stale `app` image crash-loops (`migration N applied but
  missing in resolved`) → `docker compose build app`.
- **CI:** `ci.yml` (branch check → quality → deny → docker-build → coverage) and
  `release.yml` (git-cliff, `initial_tag = 1.0.0`, build-once-then-promote like
  mobile/core). Both check out the **core as a sibling** (`path:` + deploy key)
  because of the path dependency. GitFlow `develop→staging→main`. v1.0.0 shipped.

**Coverage** — `cargo-llvm-cov`, aliased in `.cargo/config.toml`. Two modes, and
the difference is enormous:

```bash
cargo cov        # unit only — 34 %, what a bare machine gives
cargo cov-full   # + #[ignore]d integration tests — 95.2 %, needs Postgres + Redis
```

Like every `cargo-llvm-cov` figure, that number **includes inline `#[cfg(test)]`
modules** (~100 % covered by construction); production code alone sits at 94.6 %.
`cov-html` / `cov-lcov` also run the integration tests — the `-unit` variants are
the bare-machine ones.

The DB modules (`accounts`, `deltas`, `sessions`, `snapshots`, `vault_keys`,
`devices`) have **no exercise other than the integration tests**, so `cargo cov`
alone reports them at 0 % and is not a meaningful figure. Compose does not publish
the DB ports on the host — run throwaway containers (recipe in `README.md`). The
CI `coverage` job provides Postgres/Redis as services and runs `cov-full`, so the
published number is the real one; it is **non-blocking** (`continue-on-error`, no
threshold). Only `src/main.rs` is excluded (composition root); the exclusion regex
accepts `/` **and** `\` — a `/`-only pattern filters nothing on Windows.

## 7. When you change X

- **New endpoint** → `merge` its `routes()` in `build_app`; add an integration
  test via `oneshot`; keep the zero-knowledge invariant (no plaintext reads).
- **DB schema** → add a `migrations/*.sql` (LF via `.gitattributes`); rebuild the
  Docker `app` image; a modified checksum on an applied migration breaks startup.
- **Anything auth/crypto** → it lives in `realm-guard-core`; the server only wires
  it. Run a security review before merging sensitive changes.
