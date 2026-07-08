# LibreJustice

Moteur de recherche libre sur les **décisions de justice françaises** — ordre
administratif (TA/CAA/CE) et ordre judiciaire (Cour de cassation, cours d'appel,
tribunaux judiciaires, tribunaux de commerce) — adossé à un nœud mono-serveur
**Postgres + ParadeDB + VectorChord**, exposé via une UI web et un endpoint
**MCP**.

## Stack

Workspace Cargo **pur Rust**, scindé en deux racines :

- `packages/` — bibliothèques :
  - `lj-core` — cœur pur : parsing / normalisation / extraction, résumé, refs légales.
  - `lj-sources` — I/O sources (Judilibre JSON, ZIP/XML opendata).
  - `lj-store` — accès Postgres (tokio-postgres + deadpool) + migrations.
  - `lj-llm` — backends embedding + cache + quantisation + client Mistral (chat/OCR).
  - `lj-dtos` — contrats API ↔ web (serde).
  - `lj-telemetry` — tracing + export OTLP.
  - `lj-api` — couche API (Axum + MCP rmcp + OAuth).
  - `lj-web` — front Leptos (SSR + hydratation WASM), Tailwind.
- `apps/` — binaires livrables :
  - `lj-server` — déploiement unique : API + MCP/OAuth + SSR + TLS.
  - `lj-ingest` — CLI d'ingestion + runner cron.

## Build & dev

La toolchain Rust est épinglée par `rust-toolchain.toml`. On utilise
[`mise`](https://mise.jdx.dev) pour les tâches :

```bash
mise run test      # rustfmt --check + clippy -D warnings + cargo test --workspace
mise run dev       # lj-server fusionné (cargo leptos watch, :3000)
```

Pour une base locale : `podman compose -f infra/docker-compose.yml up -d postgres`.

## Déploiement

Mono-serveur via **podman compose** (`infra/docker-compose.yml`) : Postgres
(ParadeDB + VectorChord), le binaire fusionné `lj-server` et le cron
d'ingestion. Renseigner `.env` (voir `.env.example`), puis :

```bash
podman compose -f infra/docker-compose.yml --profile prod up -d --build
podman compose -f infra/docker-compose.yml exec -T cron lj-ingest migrate   # migrations
```

`lj-server` sert l'API, le MCP/OAuth, le SSR et les assets (TLS in-process).

## Licence

[Apache-2.0](LICENSE).
