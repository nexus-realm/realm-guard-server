# Changelog

Toutes les modifications notables de ce projet sont documentées ici.
## [1.1.0] - 2026-08-18

### Build
- **deps** : Sync Cargo.lock with the core's ed25519-dalek 3.0 bump

### Corrections
- **observability** : Make the Discord embed readable and its source link reachable
- **deps** : Re-sync Cargo.lock with core 1.0.1 and patch the h2 advisory

### Divers
- **coverage** : Add cargo-llvm-cov aliases and a non-blocking CI report
- **deps-dev** : Bump @commitlint/config-conventional
- **deps** : Bump actions/checkout from 4 to 7
- **deps** : Bump softprops/action-gh-release from 2 to 3

### Documentation
- **coverage** : Correct the server figures and run integration tests in every report
- **observability** : Document metrics, alerting and the Grafana roadmap
- **deps** : Document the server lock re-sync after core dependency bumps
- **deps** : Cover the lock re-sync after a core release and deny's two failure modes

### Fonctionnalités
- **observability** : Render http latency as prometheus buckets, not a summary
- **observability** : Scrape postgres, redis, host and container metrics
- **observability** : Add the Prometheus alert rules and Alertmanager pipeline
- **observability** : Emit zero-knowledge business metrics for sync, auth and ws
- **observability** : Pin and harden Prometheus, add provisioned Grafana
- **observability** : Wire the Discord receiver and a Watchdog dead-man's-switch

### Intégration continue
- Lint prometheus rules and alertmanager config with promtool/amtool
- **deps** : Set up monthly grouped Dependabot updates
- **deps** : Ignore the blocked getrandom bump and the rust-toolchain pseudo-tag

### Tests
- **config** : Cover env parsing and secret redaction
## [1.0.0] - 2026-07-21

### Build
- Add Docker Compose stack (app, Postgres, Redis, Caddy, Prometheus)
- **deps** : Pin realm-guard-core 1.0.0 in the lockfile
- **deps** : Pin realm-guard-core 1.0.0 in the lockfile

### Corrections
- **ci** : Use mirror branch pattern to build with paired core branch
- **ci** : Resolve the core sibling checkout on push events

### Divers
- **auth** : Per-account login rate limiting with Redis lockout

### Documentation
- **server** : Add README + AGENTS.md

### Fonctionnalités
- Scaffold the Axum sync server
- Add observability — Prometheus metrics, request tracing, Sentry
- Add persistence layer — Postgres (sqlx) and Redis
- **auth** : Persist OPAQUE accounts and server setup
- **auth** : Opaque register/login endpoints with Redis sessions
- **vault** : Store and fetch the wrapped vault key (session-gated)
- **auth** : Source OPAQUE server setup from env, not the database
- **pairing** : Device pairing FFI + service (relay transport)
- **devices** : Device registry with register/list/revoke endpoints
- **device-auth** : Device-key challenge-response sessions
- **devices** : Expose the device key in the listing and support renaming
- **sync** : Append-only delta log with cursor-based pull
- **sync** : Snapshots with log compaction

### Intégration continue
- Add PR pipeline (GitFlow, quality, cargo-deny, docker) + husky
- Check out realm-guard-core at the PR base branch, not main
- **release** : Add the git-cliff release pipeline

### Performance
- **sessions** : Reuse a pooled Redis connection manager

### Tests
- **sync** : Real-time WebSocket smoke against a live stack
