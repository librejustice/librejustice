# LibreJustice — image applicative Rust unique (ADR 0061 mono-process, ADR 0065
# build hôte). Image FINE : les binaires (`lj-server` + `lj-ingest`) et les
# assets SSR sont compilés SUR L'HÔTE (`cargo leptos build --release`, cf.
# `mise run build-app`) — aucune compilation en conteneur. Ce Dockerfile ne fait
# que copier les artefacts dans une base `ubuntu:24.04` alignée sur la glibc de
# l'hôte de build (2.39), + supercronic pour l'ordonnanceur cron.
#
# Sert DEUX services compose depuis cette même image :
#   - `app`  : `lj-server` (API + MCP + OAuth + SSR Leptos + assets + TLS in-process).
#   - `cron` : `supercronic` + `lj-ingest` (override de CMD côté compose).

FROM docker.io/library/ubuntu:24.04

WORKDIR /app
ENV LEPTOS_SITE_ROOT=/app/site

# Deps runtime minimales : CA roots (rustls vérifie les certs TLS sortants via le
# magasin système — Workers AI, Mistral, Judilibre) + supercronic (binaire statique
# Go) pour le cron. reqwest est en rustls → pas de libssl ; tokio-postgres est pur
# Rust → pas de postgresql-client.
ARG SUPERCRONIC_VERSION=v0.2.45
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl poppler-utils \
    && curl -fsSL "https://github.com/aptible/supercronic/releases/download/${SUPERCRONIC_VERSION}/supercronic-linux-$(dpkg --print-architecture)" \
       -o /usr/local/bin/supercronic \
    && chmod +x /usr/local/bin/supercronic \
    && apt-get purge -y curl && apt-get autoremove -y && rm -rf /var/lib/apt/lists/*

# Artefacts compilés sur l'hôte (cf. `mise run build-app`) : les deux binaires +
# les assets SSR (wasm/js/css déjà précompressés br/gzip, servis tels quels par
# `ServeDir.precompressed_*`). Le contexte de build est la racine du repo (`..`).
COPY target/release/lj-server /usr/local/bin/lj-server
COPY target/release/lj-ingest /usr/local/bin/lj-ingest
# `hash-files = true` : leptos résout les noms content-hashés du bundle via
# `hash.txt`, lu à CÔTÉ du binaire (`current_exe().parent()`), pas dans site_root.
COPY target/release/hash.txt /usr/local/bin/hash.txt
COPY target/site /app/site
COPY infra/crontab /etc/librejustice/crontab

# Service `app` : sert tout en un process (TLS, cert/clé via env, cf. compose).
# Le service `cron` override ce CMD avec supercronic dans docker-compose.yml.
CMD ["lj-server"]
