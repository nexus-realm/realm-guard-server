# Changelog

Toutes les modifications notables de ce projet sont documentées ici.
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
