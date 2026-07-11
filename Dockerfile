# syntax=docker/dockerfile:1
# Build multi-stage. Le contexte de build est le dossier parent `development/`
# (voir docker-compose.yml) afin de résoudre la dépendance `path` vers
# `realm-guard-core`, sibling du serveur.

FROM rust:1.97-bookworm AS builder
WORKDIR /build
# Cœur partagé (dépendance path) : manifeste + sources suffisent.
COPY realm-guard-core/Cargo.toml ./realm-guard-core/
COPY realm-guard-core/src ./realm-guard-core/src/
# Serveur : manifeste, lock, sources, migrations (embarquées par migrate!()).
COPY realm-guard-server/Cargo.toml realm-guard-server/Cargo.lock ./realm-guard-server/
COPY realm-guard-server/src ./realm-guard-server/src/
COPY realm-guard-server/migrations ./realm-guard-server/migrations/
WORKDIR /build/realm-guard-server
RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 realmguard
COPY --from=builder /build/realm-guard-server/target/release/realm-guard-server \
    /usr/local/bin/realm-guard-server
USER realmguard
ENV RG_SERVER_ADDR=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/realm-guard-server"]
