-- Sitemaps servis depuis Postgres (ADR 0064).
--
-- Remplace le stockage objet R2 + le Worker Cloudflare : le cron (`lj-ingest
-- sitemap`) régénère l'ensemble et l'upsert ici, `lj-server` sert
-- `/sitemap.xml` (ligne `sitemap-index.xml`) + `/sitemap-{n}.xml.gz` depuis
-- cette table. ~61 lignes (~35 Mo de blobs gz pour 3 M décisions), cache CDN
-- devant → Postgres touché rarement.
CREATE TABLE IF NOT EXISTS sitemaps (
    filename     text PRIMARY KEY,
    content_type text NOT NULL,
    body         bytea NOT NULL,
    lastmod      date NOT NULL,
    updated_at   timestamptz NOT NULL DEFAULT now()
);
