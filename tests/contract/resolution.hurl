# ============================================================================
# CONTRACT TESTS — résolution d'un texte : ce qui doit rester joignable
#   GET /texte/{slug}[/{article}] → 308 vers l'adresse d'aujourd'hui
# {{base}} = host root, no secret. A lost redirect answers 404 to every indexed
# URL with no page changing its look, so these run in the subset every deploy replays.
# ============================================================================

# ── 1. Renommage de slug : l'ancien redirige, l'article aussi ────────────────
# One pair per form of renaming: a build artefact dropped, a dated homonym, a
# numbered edition. The table `legal_text_slug_redirect` carries the others.
GET {{base}}/texte/code-general-des-impots-cgi
HTTP 308
[Asserts]
header "Location" endsWith "/norm/fr/code-general-des-impots"

GET {{base}}/texte/code-penal-2
HTTP 308
[Asserts]
header "Location" endsWith "/norm/fr/code-penal-ancien"

GET {{base}}/texte/code-des-marches-publics-3
HTTP 308
[Asserts]
header "Location" endsWith "/norm/fr/code-des-marches-publics-edition-2001"

# The redirect works at article level, otherwise every article URL of the CGI
# falls, and that is where the indexed volume sits.
GET {{base}}/texte/code-general-des-impots-cgi/286
HTTP 308
[Asserts]
header "Location" endsWith "/norm/fr/code-general-des-impots/286"

# ── 2. Le slug vivant sert, lui, sans redirection ────────────────────────────
GET {{base}}/internal/norm/code-general-des-impots/286
HTTP 200
[Asserts]
jsonpath "$.code" == "code-general-des-impots"
jsonpath "$.numKey" == "286"

# ── 3. Un texte sans articles se lit à sa racine ─────────────────────────────
# Many texts of the fund have no article: their body is all there is to serve.
# Without the `body` field they become empty pages.
GET {{base}}/internal/norm/circulaire-imig0800044c-du-20-novembre-2008-relative-a-l-accord-franco-gabonais
HTTP 200
[Asserts]
jsonpath "$.articleCount" == 0
jsonpath "$.body" isString

# ── 4. Un code, lui, rend son identité et son compte d'articles ──────────────
GET {{base}}/internal/norm/code-du-travail
HTTP 200
[Asserts]
jsonpath "$.code" == "code-du-travail"
jsonpath "$.articleCount" > 20000
