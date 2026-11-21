# LibreJustice

**[librejustice.fr](https://librejustice.fr)**: a free search engine for French-language and European law, plus current public notices. It links case law from France's administrative and judicial courts to consolidated legal texts and makes them searchable in plain language on the web and through **MCP**.

Free and open source. **~3.9 million decisions, ~2.1 million legal articles in force and ~1.8 million historical versions** in one database, linked article by article, resynchronised daily. That is the whole point: texts and case law live together, so there is no second server to plug in for the codes and no third one for the citation graph.

## What you search

- **Case law**: Conseil d'État, Cour de cassation, cours d'appel, administrative and judicial first-instance courts, Conseil constitutionnel, CNIL, CNDA (publicly released decisions only), Défenseur des droits, Autorité de la concurrence, Autorité des marchés financiers, Tribunal des conflits, ECHR, CJEU, European Commission competition decisions. Filters by court, date, outcome and cited articles.
- **Legal texts**: codes and statutes **as they stood on any date** (consolidated versions with per-article history), the full Journal officiel, EU law, treaties and bilateral accords, collective agreements, BOFiP, circulaires, and the codes of 59 foreign countries.
- **Public notices**: open public procurement opportunities from BOAMP and buyer platforms, deduplicated with submission links and deadlines.
- **Directory**: lawyers, companies and courts, linked to their litigation.

Corpus refreshed daily from open sources (Judilibre, DILA/Légifrance, EUR-Lex...).

## What the official sources do not offer

Public databases are silos, each with a search engine bounded by its own fonds (Judilibre covers the judicial courts only; the administrative open data searches CE/CAA/TA by keyword, court and date; Légifrance holds the texts). LibreJustice consolidates them and adds the missing layer:

- **plain-language search** (hybrid lexical + semantic) with AI reranking, dynamic facets and similar decisions;
- **the graph**: citations extracted and resolved, from a decision to the exact articles it cites at the right version, and from an article back to the decisions that cite it;
- **cross-source deduplication** (one decision, one page) and recovery of decisions missing from the official channels;
- **AI summaries** per decision, always kept distinct from verbatim excerpts;
- **a remote MCP endpoint** for AI assistants.

## Using LibreJustice from an AI assistant

The public MCP server is `https://librejustice.fr/mcp` (OAuth 2.1 with dynamic client registration: no API key to configure). Four tools: `search_decisions`, `get_decision`, `search_norms`, `get_norm`.

With Claude Code, the plugin installs the connector and the usage skills:

```
/plugin marketplace add librejustice/librejustice
/plugin install librejustice@librejustice
```

The skills alone (without the MCP connector) install into any compatible agent through [skills](https://skills.sh):

```
npx skills add librejustice/librejustice
```

On claude.ai, LibreJustice is listed in the [connector directory](https://claude.ai/directory/connectors/librejustice). One click, then the OAuth authorisation. With ChatGPT, Le Chat or Perplexity, add a custom connector pointing at `https://librejustice.fr/mcp`.

## Using LibreJustice from your own code

The same corpus in REST, in beta, described by [`openapi.json`](https://librejustice.fr/openapi.json) (OpenAPI 3.1). The key is free: create an account, then a key from your profile. Guide at [librejustice.fr/api-guide](https://librejustice.fr/api-guide).

```bash
curl -H 'Authorization: Bearer ljk_your_key' \
  'https://librejustice.fr/api/search?query=trouble+anormal+de+voisinage&limit=3'
```

Six routes: `/api/search`, `/api/search-textes`, `/api/decision/{id}`, `/api/norm/{jurisdiction}/{code}`, `/api/norm/{jurisdiction}/{code}/{article}` and `/api/article/{uid}`, the article routes taking `?as_of=` for the version in force that day. Parameters and fields carry the names of the MCP tools' arguments and outputs.

## Stack

A single-server node, **Postgres + ParadeDB + VectorChord** (hybrid BM25 + vector search), API and front end in Rust.

A **pure Rust** Cargo workspace, split in two roots:

- `packages/` libraries:
  - `lj-core` pure core: the decision model and its normalisation.
  - `lj-parse` source parsers into the decision model.
  - `lj-norm` referential of the legal texts: article keys, aliases, applicability.
  - `lj-extract` field and citation extraction from decisions.
  - `lj-sources` source I/O (Judilibre JSON, opendata ZIP/XML).
  - `lj-store` Postgres access (tokio-postgres + deadpool) and migrations.
  - `lj-llm` embedding backends, cache, quantisation, Mistral client (chat/OCR).
  - `lj-dtos` api to web contracts (serde).
  - `lj-telemetry` tracing and OTLP export.
  - `lj-api` API layer (Axum + MCP rmcp + OAuth).
  - `lj-web` Leptos front end (SSR + WASM hydration), Tailwind.
- `apps/` shippable binaries:
  - `lj-server` single deployment: API + MCP/OAuth + SSR.
  - `lj-ingest` ingest CLI and cron runner.

## Build and dev

The Rust toolchain is pinned by `rust-toolchain.toml`. Tasks run through [`mise`](https://mise.jdx.dev):

```bash
mise run test      # rustfmt --check + clippy -D warnings + cargo test --workspace
mise run dev       # merged lj-server (cargo leptos watch, :3000)
```

For a local database: `mise run dbs`. The task prepares the Postgres state under `LIBREJUSTICE_STATE_DIR`, owning the data directory and copying the configuration mounted by the container, before starting it.

## Deployment

Single server through **podman compose** (`infra/docker-compose.yml`): Postgres (ParadeDB + VectorChord), the merged `lj-server` binary, a `caddy` front end and the ingest cron. Fill in `.env` (see `.env.example`), then:

```bash
mise run ensure-pg                                                          # Postgres state and conf
podman compose -f infra/docker-compose.yml --profile prod up -d --build
podman compose -f infra/docker-compose.yml exec -T cron lj-ingest migrate   # migrations
```

`lj-server` serves the API, MCP/OAuth, SSR and the assets. It listens in clear; `caddy` holds `:443` and terminates TLS, so the socket stays bound when the application container is recreated.

The shipped Postgres configuration (`infra/pg-conf/`) carries what the application requires (extensions, ParadeDB settings, compression) sized for a development machine. A production instance raises `shared_buffers` and `effective_cache_size` to match its hardware.

## Licence

[Apache-2.0](LICENSE).
