# ============================================================================
# CONTRACT TESTS — pot Annonces
# ----------------------------------------------------------------------------
# GET /internal/journals, /internal/journals/catalogue, /internal/journals/{uid} et les flux
# d'alerte /journal-search.rss + /journal-search.ics.
# Asserts HTTP status + JSON STRUCTURAL invariants only.
# Variable: {{base}} = host root (e.g. http://localhost:3000).
# Routes publiques, sans auth. Aucun `q` requis : le moteur est facetté, la
# fenêtre par défaut est le stock ouvert trié par échéance.
# Le pot sert plusieurs fonds derrière un même DTO : la réponse est une liste
# de groupes (`groupes[]`, un par fond) et une liste d'axes déclarés par les
# backends interrogés (`axes[]`), jamais une carte de facettes figée.
# ============================================================================

# ── 1. Baseline : la fenêtre ouverte, tous fonds ─────────────────────────────
GET {{base}}/internal/journals
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.total" isInteger
jsonpath "$.total" >= 0
jsonpath "$.sort" == "echeance"
jsonpath "$.groupes" isCollection
jsonpath "$.groupes[0].fond" isString
jsonpath "$.groupes[0].label" isString
jsonpath "$.groupes[0].total" isInteger
jsonpath "$.groupes[0].hits" isCollection
# Plusieurs fonds ouverts = une vue d'aperçu : cinq lignes par groupe, le
# « tout voir » du groupe coche le fond et rend la page pleine.
jsonpath "$.groupes[0].hits" count <= 5
# L'axe « Type » (le fond) est toujours servi : c'est ce qui rend le pot
# navigable quand plusieurs fonds répondent.
jsonpath "$.axes[0].key" == "fond"
jsonpath "$.axes[0].choices" isCollection
jsonpath "$.axes[0].choices" count >= 2


# ── 2. Un seul fond : ses axes propres apparaissent ──────────────────────────
GET {{base}}/internal/journals?fond=marche
HTTP 200
[Asserts]
jsonpath "$.groupes" count == 1
jsonpath "$.groupes[0].fond" == "marche"
jsonpath "$.groupes[0].hits" count <= 20
# fond plus the three SQL axes. Nature is not one of them: it is the root level
# of `cpv`, served as a single tree the way `juridiction` is.
jsonpath "$.axes" count == 4
jsonpath "$.axes[1].key" == "zone"
jsonpath "$.axes[2].key" == "cpv"
jsonpath "$.axes[3].key" == "metier"
jsonpath "$.axes[2].choices[0].parent" not exists
jsonpath "$.axes[2].choices[0].value" startsWith "nature_marche:"
[Captures]
uid: jsonpath "$.groupes[0].hits[0].uid"


# ── 3. Hit shape (assert sur le premier hit du fond `marche`) ────────────────
GET {{base}}/internal/journals?fond=marche&limit=5
HTTP 200
[Asserts]
jsonpath "$.groupes[0].hits" count <= 5
jsonpath "$.groupes[0].hits[0].uid" isString
jsonpath "$.groupes[0].hits[0].fond" == "marche"
jsonpath "$.groupes[0].hits[0].fondLabel" isString
jsonpath "$.groupes[0].hits[0].titre" isString
jsonpath "$.groupes[0].hits[0].acteur" isString
jsonpath "$.groupes[0].hits[0].acteurRole" == "Acheteur"
jsonpath "$.groupes[0].hits[0].datePublication" matches /^\d{4}-\d{2}-\d{2}$/
jsonpath "$.groupes[0].hits[0].urlAction" startsWith "http"
jsonpath "$.groupes[0].hits[0].closed" == false
jsonpath "$.groupes[0].hits[0].canaux" isCollection


# ── 4. Le fond `bodacc` est servi en proxy live, mêmes champs ────────────────
GET {{base}}/internal/journals?fond=bodacc&limit=3
HTTP 200
[Asserts]
jsonpath "$.groupes" count == 1
jsonpath "$.groupes[0].fond" == "bodacc"
jsonpath "$.groupes[0].hits" count <= 3
jsonpath "$.groupes[0].hits[0].uid" startsWith "bodacc:"
jsonpath "$.groupes[0].hits[0].acteurRole" == "Entreprise"
jsonpath "$.groupes[0].hits[0].urlAction" startsWith "https://www.bodacc.fr/"
# Ce fond ne sait exécuter que deux axes : on n'offre jamais un filtre que la
# source ne sait pas appliquer.
jsonpath "$.axes" count == 3
jsonpath "$.axes[1].key" == "famille"
jsonpath "$.axes[2].key" == "zone"


# ── 5. Le fond `joafe` est servi en proxy live, mêmes champs ─────────────────
GET {{base}}/internal/journals?fond=joafe&limit=3
HTTP 200
[Asserts]
jsonpath "$.groupes" count == 1
jsonpath "$.groupes[0].fond" == "joafe"
jsonpath "$.groupes[0].hits" count <= 3
jsonpath "$.groupes[0].hits[0].uid" startsWith "joafe:"
jsonpath "$.groupes[0].hits[0].acteurRole" == "Association"
# Deux axes exécutables : le type d'avis (valeurs en casse mixte, transmises
# telles quelles — l'égalité ODSQL est sensible à la casse) et le département.
jsonpath "$.axes" count == 3
jsonpath "$.axes[1].key" == "famille"
jsonpath "$.axes[2].key" == "zone"


# ── 5b. The `jo` fond: the official journals by their words, no axis of its own ─
GET {{base}}/internal/journals?fond=jo&q=sous-pr%C3%A9fet&jurisdiction=FR.JORF&limit=3
HTTP 200
[Asserts]
jsonpath "$.groupes" count == 1
jsonpath "$.groupes[0].fond" == "jo"
jsonpath "$.groupes[0].label" == "Journaux officiels"
jsonpath "$.groupes[0].hits" count == 3
jsonpath "$.groupes[0].hits[0].uid" startsWith "jorf:JORFTEXT"
jsonpath "$.groupes[0].hits[0].jurisdiction" == "FR"
jsonpath "$.groupes[0].hits[0].acteurRole" == "Émetteur"
jsonpath "$.axes" count == 1

# A reader in Belgium reads the Moniteur belge, and no French register.
GET {{base}}/internal/journals?fond=jo&q=arr%C3%AAt%C3%A9&home=be&limit=3
HTTP 200
[Asserts]
jsonpath "$.groupes[0].hits[0].jurisdiction" == "BE"
jsonpath "$.groupes[0].hits[0].uid" startsWith "mb-"
jsonpath "$.axes[0].choices[*].value" not includes "marche"

# A place no journal sits at is refused with the places and journals there are.
GET {{base}}/internal/journals?jurisdiction=XX
HTTP 422

# Its acts name persons: the bare page counts them and lists none.
GET {{base}}/internal/journals
HTTP 200
[Asserts]
jsonpath "$.groupes[*].fond" not includes "jo"
jsonpath "$.axes[0].choices[*].value" includes "jo"


# ── 6. Le statut est dérivé, pas un fond ────────────────────────────────────
# Un marché attribué reste une ligne du fond `marche` : même uid, même acteur,
# `attribue` en plus de `closed`. Le stock clos porte les attributions.
GET {{base}}/internal/journals?fond=marche&closes=true&limit=3
HTTP 200
[Asserts]
jsonpath "$.groupes" count == 1
jsonpath "$.groupes[0].fond" == "marche"
jsonpath "$.groupes[0].label" == "Marchés publics"
jsonpath "$.groupes[0].hits" count <= 3
jsonpath "$.groupes[0].hits[0].uid" startsWith "marche:"
jsonpath "$.groupes[0].hits[0].acteurRole" == "Acheteur"
jsonpath "$.groupes[0].hits[0].attribue" isBoolean


# ── 7. Zone filter: Paris, which always has open consultations ──────────────
GET {{base}}/internal/journals?fond=marche&zone=75
HTTP 200
[Asserts]
jsonpath "$.groupes[0].hits" isCollection
jsonpath "$.groupes[0].hits[0].zones" isCollection
jsonpath "$.groupes[0].hits[0].zones[*].key" includes "75"


# ── 8. Fenêtre d'échéance + tri nouveautés ───────────────────────────────────
GET {{base}}/internal/journals?fond=marche&echeanceJours=30&sort=nouveautes
HTTP 200
[Asserts]
jsonpath "$.sort" == "nouveautes"
jsonpath "$.groupes" isCollection

# The window keeps its lower bound even when `closes` drops the open predicate:
# "closing within 7 days" only speaks of deadlines that still run.
GET {{base}}/internal/journals?fond=marche&closes=true&echeanceJours=7&limit=3
HTTP 200
[Asserts]
jsonpath "$.groupes[0].hits[0].closed" == false
jsonpath "$.groupes[0].hits[2].closed" == false


# ── 8b. A trade narrows the fund. An axis served as a facet but unread by the
# query parser answers the whole fund to every value, which reads as a working
# filter; only a count smaller than the unfiltered one proves it applies.
GET {{base}}/internal/journals?fond=marche&limit=1
HTTP 200
[Captures]
tous: jsonpath "$.total"

GET {{base}}/internal/journals?fond=marche&metier=batiment-tce&limit=1
HTTP 200
[Asserts]
jsonpath "$.total" < {{tous}}
jsonpath "$.groupes[0].hits[0].metiers[*].key" includes "metier:batiment-tce"


# ── 9. Filtre par mot sur titre + acteur ─────────────────────────────────────
GET {{base}}/internal/journals?q=travaux
HTTP 200
[Asserts]
jsonpath "$.groupes" isCollection


# ── 10. Fond inconnu ⇒ 4xx (un jeton qui filtrerait dans le vide) ─────────────
GET {{base}}/internal/journals?fond=inconnu
HTTP *
[Asserts]
status >= 400
status < 500

# `attribution` a été un fond avant de devenir un statut.
# Le jeton mort est refusé comme n'importe quel inconnu : pas d'alias.
GET {{base}}/internal/journals?fond=attribution
HTTP *
[Asserts]
status >= 400
status < 500


# ── 11. Bornes : limit et offset hors plage ⇒ 4xx ─────────────────────────────
GET {{base}}/internal/journals?limit=101
HTTP *
[Asserts]
status >= 400
status < 500

GET {{base}}/internal/journals?offset=5001
HTTP *
[Asserts]
status >= 400
status < 500

GET {{base}}/internal/journals?sort=pertinence
HTTP *
[Asserts]
status >= 400
status < 500


# ── 12. Catalogue : les chemins de crawl du fond `marche` ────────────────────
GET {{base}}/internal/journals/catalogue
HTTP 200
[Asserts]
jsonpath "$.fond" == "marche"
jsonpath "$.total" isInteger
jsonpath "$.zones" isCollection
jsonpath "$.zones[0].code" isString
jsonpath "$.zones[0].label" isString
jsonpath "$.zones[0].count" isInteger
jsonpath "$.familles" isCollection


# ── 13. Fiche : la consultation capturée en 2 ────────────────────────────────
GET {{base}}/internal/journals/{{uid}}
HTTP 200
[Asserts]
jsonpath "$.annonce.uid" == "{{uid}}"
jsonpath "$.annonce.urlAction" startsWith "http"
jsonpath "$.liens" isCollection
jsonpath "$.canauxDetail" isCollection
jsonpath "$.canauxDetail[0].moteur" isString
jsonpath "$.canauxDetail[0].firstSeenAt" isString
jsonpath "$.entites" isCollection


# ── 14. Fiche inconnue ⇒ 404 ─────────────────────────────────────────────────
GET {{base}}/internal/journals/marche:0000000000000000
HTTP 404


# ── 15. Flux d'alerte RSS : l'état de filtres comme abonnement ───────────────
GET {{base}}/journal-search.rss?fond=marche&zone=75
HTTP 200
[Asserts]
header "Content-Type" contains "application/rss+xml"
header "X-Robots-Tag" contains "noindex"
body contains "<rss version=\"2.0\""
body contains "<channel>"


# ── 16. Flux ICS : une échéance = un VEVENT ──────────────────────────────────
GET {{base}}/journal-search.ics?fond=marche&echeanceJours=30
HTTP 200
[Asserts]
header "Content-Type" contains "text/calendar"
header "X-Robots-Tag" contains "noindex"
body contains "BEGIN:VCALENDAR"
body contains "END:VCALENDAR"


# ── 17. Un filtre invalide se propage aux flux (même frontière) ──────────────
GET {{base}}/journal-search.rss?fond=inconnu
HTTP *
[Asserts]
status >= 400
status < 500
