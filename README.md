# LibreJustice

**[librejustice.fr](https://librejustice.fr)** : moteur de recherche libre
sur le droit français et européen. Jurisprudence des deux ordres et textes
consolidés, reliés entre eux et interrogeables en langage naturel, sur le
web comme depuis les assistants IA (**MCP**).

## Ce qu'on y cherche

- **Jurisprudence** : Conseil d'État, Cour de cassation, cours d'appel,
  tribunaux administratifs et judiciaires, CNDA, Conseil constitutionnel,
  CEDH, CJUE — filtres par juridiction, date, issue et articles cités.
- **Textes** : codes et lois **tels qu'en vigueur à n'importe quelle date**
  (versions consolidées et historique des articles), droit de l'UE, traités
  et accords bilatéraux, conventions collectives, BOFiP, circulaires, codes
  étrangers (59 pays).
- **Annuaire** : avocats, entreprises et juridictions, reliés à leur
  contentieux.

Corpus mis à jour quotidiennement depuis les sources ouvertes (Judilibre,
DILA/Légifrance, EUR-Lex…).

## Ce que les sources officielles n'offrent pas

Les bases publiques sont des silos (Judilibre = ordre judiciaire seul, JADE =
ZIP XML sans moteur, Légifrance = consultation document par document).
LibreJustice les consolide et ajoute la couche qui manque :

- **recherche en langage naturel** (hybride lexicale + sémantique) avec
  rerank IA, facettes dynamiques et décisions similaires ;
- **le graphe** : citations extraites et résolues — d'une décision vers les
  articles exacts qu'elle cite (à la bonne version), et d'un article vers
  les décisions qui le citent ;
- **dédoublonnage inter-sources** (une décision, une fiche) et récupération
  de décisions absentes des canaux officiels ;
- **résumés IA** par décision, toujours distingués des extraits verbatim ;
- **un accès MCP remote** pour les assistants IA.

## Utiliser LibreJustice depuis un assistant IA

Le serveur MCP public est `https://librejustice.fr/mcp` (OAuth 2.1 avec
enregistrement dynamique de client : aucune clé à configurer). Cinq outils :
`search_decisions`, `get_decision`, `search_legal_texts`, `get_legal_text`,
`list_my_activity`.

Avec Claude Code, le plugin installe le connecteur et les skills d'usage :

```
/plugin marketplace add librejustice/librejustice
/plugin install librejustice@librejustice
```

Les skills seuls (sans le connecteur MCP) s'installent dans n'importe quel
agent compatible via [skills](https://skills.sh) :

```
npx skills add librejustice/librejustice
```

Avec Le Chat, Perplexity ou claude.ai : ajouter un connecteur custom pointant
sur `https://librejustice.fr/mcp`.

## Stack

Nœud mono-serveur **Postgres + ParadeDB + VectorChord** (recherche hybride
BM25 + vecteurs), API et front en Rust.

Workspace Cargo **pur Rust**, scindé en deux racines :

- `packages/` — bibliothèques :
  - `lj-core` — cœur pur : parsing / normalisation / extraction, résumé, refs légales.
  - `lj-extract` — extraction des champs et citations des décisions.
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
