# LibreJustice — image applicative unique. Les binaires et les assets SSR sont
# compilés sur l'hôte par `mise run build-app` ; ce Dockerfile ne fait que les
# copier dans une base alignée sur la glibc de l'hôte de build. Deux services
# compose en sortent : `app` sert `lj-server`, `cron` lance supercronic.

FROM docker.io/library/ubuntu:24.04

WORKDIR /app
ENV LEPTOS_SITE_ROOT=/app/site

# Dépendances runtime minimales : les CA roots, que rustls consulte pour les
# certificats sortants, et supercronic. reqwest est en rustls et tokio-postgres
# est pur Rust, donc ni libssl ni postgresql-client.
ARG SUPERCRONIC_VERSION=v0.2.45
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl poppler-utils qpdf \
    && curl -fsSL "https://github.com/aptible/supercronic/releases/download/${SUPERCRONIC_VERSION}/supercronic-linux-$(dpkg --print-architecture)" \
       -o /usr/local/bin/supercronic \
    && chmod +x /usr/local/bin/supercronic \
    && apt-get purge -y curl && apt-get autoremove -y && rm -rf /var/lib/apt/lists/*

# Les assets SSR arrivent déjà précompressés br/gzip, servis tels quels par
# `ServeDir.precompressed_*`. Le contexte de build est la racine du repo.
COPY target/release/lj-server /usr/local/bin/lj-server
COPY target/release/lj-ingest /usr/local/bin/lj-ingest
# Leptos résout les noms content-hashés du bundle via `hash.txt`, qu'il lit à
# côté du binaire et non dans `site_root`.
COPY target/release/hash.txt /usr/local/bin/hash.txt
COPY target/site /app/site
COPY infra/crontab /etc/librejustice/crontab

# Le service `cron` remplace cette commande par supercronic.
CMD ["lj-server"]
