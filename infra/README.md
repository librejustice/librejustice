# infra

Stack conteneurisée du projet :
- `postgres`
- `app` (`staging` / `prod`) — binaire unique `lj-server` : API + MCP + OAuth + SSR + TLS (ADR 0061)
- `cron`

Les volumes runtime vivent sous `LIBREJUSTICE_STATE_DIR`
(`$HOME/.local/share/librejustice` par défaut), pas dans le repo.

## Local

Infra partagée seulement :

```bash
just up
```

Stack staging Docker complète :

```bash
just deploy-staging
```

Reset local :

```bash
podman compose -f infra/docker-compose.yml down -v
just up
```

Logs :

```bash
just logs
```

## Serveur

Bootstrap :

```bash
./scripts/bootstrap-server.sh your-server
```

Secrets :

```bash
bw login            # une fois
mise run secrets-pull
```

Préparer les volumes :

```bash
mkdir -p \
  "${LIBREJUSTICE_STATE_DIR:-$HOME/.local/share/librejustice}/pgdata" \
  "${LIBREJUSTICE_STATE_DIR:-$HOME/.local/share/librejustice}/tls/origin" \
  "${LIBREJUSTICE_STATE_DIR:-$HOME/.local/share/librejustice}/lib"
```

Le cert Cloudflare Origin (`tls/origin/{cert,key}.pem`) est posé par
`mise run secrets-pull` ; lj-server termine le TLS in-process (rustls, ADR 0061).

Déploiement (mono-serveur fusionné : `lj-server` API+MCP+SSR+TLS + cron, ADR 0061) :

```bash
mise run deploy
```

Ingest manuel dans le conteneur cron :

```bash
cd infra
podman compose exec cron librejustice <cmd>
```

## Observabilité

L'app (`api` + `cron`) exporte directement vers Grafana Cloud en OTLP/HTTP :
traces, logs et métriques, auth Basic via
`LIBREJUSTICE_GRAFANA_OTLP_ENDPOINT` / `…_OTLP_USER` / `…_CLOUD_API_KEY`.
- `host.name` vient du hostname système
- l'app ajoute `deployment.environment`
- l'app utilise un `application_name` Postgres distinct par environnement

Conséquences de la suppression du collector :
- les logs des **autres** conteneurs (Postgres) ne remontent plus dans
  Loki ; seuls `app` et `cron` poussent leurs logs.
- les métriques Postgres sont rapatriées par un scraper Rust embarqué dans
  l'API, plus par le `postgresqlreceiver` du collector.
