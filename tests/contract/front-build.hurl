# ── identité du bundle front ─────────────────────────────────────────────────
# Source oracle: apps/lj-server/src/main.rs (front_build_id + la couche
# `x-lj-front-build`), lu par `packages/lj-web/js/auth.js` et le client hydrate.
#
# Un onglet mémorise la valeur de sa première réponse et propose de recharger
# quand une suivante en porte une autre. D'où les deux moitiés du contrat, dont
# la seconde est celle qui garantit qu'il n'annonce rien à tort : une réponse
# cachable peut être resservie longtemps après le déploiement suivant, et la
# valeur qu'elle porterait serait périmée sans que l'onglet le soit.

GET {{base}}/internal/me
HTTP 401
[Asserts]
header "x-lj-front-build" matches "^[A-Za-z0-9_-]+\\.[A-Za-z0-9_-]+$"

GET {{base}}/internal/search?query=responsabilite&limit=1
HTTP 200
[Asserts]
header "cache-control" contains "public"
header "x-lj-front-build" not exists
