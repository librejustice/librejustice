# ============================================================================
# CONTRACT TESTS: GET /internal/search-textes  (article search)
# ----------------------------------------------------------------------------
# Asserts HTTP status + JSON STRUCTURAL invariants only.
# Variable: {{base}} = host root (e.g. http://localhost:8300).
# Public route, no auth. An empty `q` browses the corpus, as /internal/search does.
# A search with words answers its hits first; its total and facets come from
# /internal/search-textes/decompte on the same parameters.
# ============================================================================

# ── 1. Baseline : recherche plein-texte d'articles ────────────────────────────
GET {{base}}/internal/search-textes?query=clause%20abusive
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.served" isInteger
jsonpath "$.hits" isCollection

GET {{base}}/internal/search-textes/decompte?query=clause%20abusive
HTTP 200
[Asserts]
jsonpath "$.total" isInteger
jsonpath "$.total" >= 0
# The article facets: code, jurisdiction, source, and the type paths.
# The code axis is capped at its top 20.
jsonpath "$.facets.code" isCollection
jsonpath "$.facets.code" count <= 20
jsonpath "$.facets.jurisdiction" isCollection
jsonpath "$.facets.source" isCollection
jsonpath "$.facets.path" isCollection


# ── 2. Hit shape (assert sur le premier hit) ─────────────────────────────────
GET {{base}}/internal/search-textes?query=d%C3%A9pens&limit=5
HTTP 200
[Asserts]
jsonpath "$.hits" count <= 5
jsonpath "$.hits[0].code" isString
jsonpath "$.hits[0].codeTitle" isString
jsonpath "$.hits[0].num" isString
jsonpath "$.hits[0].numKey" isString
jsonpath "$.hits[0].snippet" isString
jsonpath "$.hits[0].source" isString


# ── 2 bis. Requête citant un numéro d'article ───────────────────────────────
# A cited number opens two SQL clauses nothing else takes: the `num` clause of
# the article leg and the usage leg scoped to the number. Only a query citing an
# article reaches them.
GET {{base}}/internal/search-textes?query=L.%201234-5%20pr%C3%A9avis&limit=5
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection

# Deux numéros, dont un nombre nu annoncé par « article » : la clause reçoit un
# tableau de clés, pas une seule.
GET {{base}}/internal/search-textes?query=article%2018%20loi%2065-557%20syndic&limit=5
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 3. q vide = mode parcours, pas une erreur ────────────────────────────────
# Le total porte sur les articles en vigueur du fonds entier, pas sur une page.
GET {{base}}/internal/search-textes?query=&limit=5
HTTP 200
[Asserts]
jsonpath "$.total" > 1000000
jsonpath "$.hits" count == 5
jsonpath "$.facets.path" isCollection
jsonpath "$.facets.jurisdiction" isCollection


# ── 4. Filtre jurisdiction (FRANCE / UE / INTL) ──────────────────────────────
GET {{base}}/internal/search-textes?query=contrat&jurisdiction=FRANCE
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 5. A type under its order, labelled ─────────────────────────────────────
GET {{base}}/internal/search-textes?query=travail&jurisdiction=FR.CODE&limit=5
HTTP 200
[Asserts]
jsonpath "$.hits" count > 0

GET {{base}}/internal/search-textes/decompte?query=travail&jurisdiction=FR.CODE
HTTP 200
[Asserts]
jsonpath "$.facets.path[?(@.value == 'FR.CODE')].label" contains "Code"


# ── 6. Filtre source ─────────────────────────────────────────────────────────
GET {{base}}/internal/search-textes?query=contrat&source=legifrance
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 7. Scoped à un code (slug) ───────────────────────────────────────────────
GET {{base}}/internal/search-textes?query=d%C3%A9pens&code=code-de-procedure-civile
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection


# ── 8. limit hors bornes ⇒ 4xx ───────────────────────────────────────────────
GET {{base}}/internal/search-textes?query=test&limit=101
HTTP *
[Asserts]
status >= 400
status < 500


# ── 9. Recherche à date : versions valides à cette date ─────────────────────
GET {{base}}/internal/search-textes?query=clause%20abusive&as_of=2005-06-01
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection

GET {{base}}/internal/search-textes/decompte?query=clause%20abusive&as_of=2005-06-01
HTTP 200
[Asserts]
jsonpath "$.total" isInteger


# ── 10. date invalide ⇒ 4xx ──────────────────────────────────────────────────
GET {{base}}/internal/search-textes?query=test&as_of=hier
HTTP *
[Asserts]
status >= 400
status < 500


# ── 11. Reference mode ───────────────────────────────────────────────────────
GET {{base}}/internal/search-textes?query=code%20civil%201240
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "reference"
jsonpath "$.total" == 1
jsonpath "$.hits[0].num" == "1240"

GET {{base}}/internal/search-textes?query=code%20civil%201240&mode=lexical
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"

GET {{base}}/internal/search-textes/decompte?query=code%20civil%201240&mode=lexical
HTTP 200
[Asserts]
jsonpath "$.total" > 1

# A bare key lists every text carrying it, within the searched corpora.
GET {{base}}/internal/search-textes?query=L752-2
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "reference"
jsonpath "$.total" > 5
jsonpath "$.facets.code" count > 5
jsonpath "$.jurisdictions" count == 3
jsonpath "$.pinned" not exists

GET {{base}}/internal/search-textes?query=article%201240
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "reference"
jsonpath "$.hits[*].code" contains "code-civil"
jsonpath "$.hits[*].code" contains "code-de-procedure-civile"

# Several keys after a named text: one article each.
GET {{base}}/internal/search-textes?query=articles%201240%20et%201241%20du%20code%20civil
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "reference"
jsonpath "$.total" == 2

# A key the named text does not carry: the reference refuses instead of ranking.
GET {{base}}/internal/search-textes?query=code%20civil%20l112
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "reference"
jsonpath "$.total" == 0
jsonpath "$.hits" count == 0

# A key followed by words asks the content; the card pins the carriers, three at
# most, and points at the full list.
GET {{base}}/internal/search-textes?query=L752-2%20licenciement&limit=5
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"
jsonpath "$.pinned" count == 3
jsonpath "$.pinnedMore" == true


# ── 12. Legal orders on the axis ─────────────────────────────────────────────
# The axis names the orders the reader's places search: states first, then the
# Union, then treaties.
GET {{base}}/internal/search-textes/decompte?query=responsabilite
HTTP 200
[Asserts]
jsonpath "$.facets.jurisdiction" count == 3
jsonpath "$.facets.jurisdiction[0].value" == "FR"
jsonpath "$.facets.jurisdiction[1].value" == "EU"
jsonpath "$.facets.jurisdiction[2].value" == "INTL"

# Read from Belgium and Luxembourg, each country is a position of its own.
GET {{base}}/internal/search-textes/decompte?query=responsabilite&place=BE&place=LU
HTTP 200
[Asserts]
jsonpath "$.facets.jurisdiction" count == 4
jsonpath "$.facets.jurisdiction[0].value" == "BE"
jsonpath "$.facets.jurisdiction[1].value" == "LU"
jsonpath "$.facets.jurisdiction[2].value" == "EU"

# An order filtered outside the reader's places comes after theirs, which count
# zero. ParadeDB counts the total and SQL serves the hits: a zero total over
# served hits is the symptom of the two filters parting.
GET {{base}}/internal/search-textes?query=responsabilite&jurisdiction=BE&limit=3
HTTP 200
[Asserts]
jsonpath "$.hits[0].jurisdiction" == "BE"
jsonpath "$.jurisdictions[0]" == "BE"

GET {{base}}/internal/search-textes/decompte?query=responsabilite&jurisdiction=BE
HTTP 200
[Asserts]
jsonpath "$.total" > 0
jsonpath "$.facets.jurisdiction[0].count" == 0
jsonpath "$.facets.jurisdiction[3].value" == "BE"

# Orders stated together are searched together, each counted in the facet.
GET {{base}}/internal/search-textes?query=responsabilite&jurisdiction=CH&jurisdiction=LU&limit=20
HTTP 200
[Asserts]
jsonpath "$.jurisdictions" count == 2
jsonpath "$.jurisdictions[0]" == "CH"
jsonpath "$.jurisdictions[1]" == "LU"
jsonpath "$.hits[*].jurisdiction" not contains "FR"

GET {{base}}/internal/search-textes/decompte?query=responsabilite&jurisdiction=CH&jurisdiction=LU
HTTP 200
[Asserts]
jsonpath "$.facets.jurisdiction[3].value" == "CH"
jsonpath "$.facets.jurisdiction[3].count" > 0
jsonpath "$.facets.jurisdiction[4].value" == "LU"
jsonpath "$.facets.jurisdiction[4].count" > 0


# ── 14. Jurisdiction scope ───────────────────────────────────────────────────
# Naming no country searches the law applicable in France, and the response
# says so.
GET {{base}}/internal/search-textes?query=responsabilite%20du%20fait%20des%20choses&limit=1
HTTP 200
[Asserts]
jsonpath "$.jurisdictions" count == 3
jsonpath "$.jurisdictions[0]" == "FR"

# Naming one searches that country alone, under its own name.
GET {{base}}/internal/search-textes?query=divorce%20au%20senegal&limit=3
HTTP 200
[Asserts]
jsonpath "$.jurisdictions" count == 1
jsonpath "$.jurisdictions[0]" == "SN"
jsonpath "$.hits[0].jurisdiction" == "SN"

GET {{base}}/internal/search-textes/decompte?query=divorce%20au%20senegal
HTTP 200
[Asserts]
jsonpath "$.facets.jurisdiction[*].value" contains "SN"

# An instrument word joins INTL: a bilateral treaty lives in neither signatory's
# own corpus.
GET {{base}}/internal/search-textes?query=convention%20fiscale%20France%20Senegal&limit=1
HTTP 200
[Asserts]
jsonpath "$.jurisdictions" contains "INTL"
jsonpath "$.jurisdictions" contains "SN"


# ── 13. A division is not a name ─────────────────────────────────────────────
# The formed title carries the leaf division after the code and the number, and
# article 1240 of the Code civil sits under « la responsabilité
# extracontractuelle »: reaching it that way is not a citation.
GET {{base}}/internal/search-textes?query=responsabilite%201240
HTTP 200
[Asserts]
jsonpath "$.queryMode" == "lexical"

GET {{base}}/internal/search-textes/decompte?query=responsabilite%201240
HTTP 200
[Asserts]
jsonpath "$.total" > 1


# ── 15. Places read from ─────────────────────────────────────────────────────
# Naming no place reads from the default one, whose answer the others narrow.
GET {{base}}/internal/search-textes/decompte?query=bail%20commercial%20renouvellement
HTTP 200
[Captures]
metropole: jsonpath "$.total"

# A place drops what the coverage knows does not apply there: from Polynesia,
# the national tax law.
GET {{base}}/internal/search-textes?query=bail%20commercial%20renouvellement&place=PF&limit=1
HTTP 200
[Asserts]
jsonpath "$.readerPlace.uid" == "PF"

GET {{base}}/internal/search-textes/decompte?query=bail%20commercial%20renouvellement&place=PF
HTTP 200
[Asserts]
jsonpath "$.total" > 0
jsonpath "$.total" < {{metropole}}

# `place` repeats, and what applies in any of the places is served, from the
# corpora they belong to; several places name no one to open an article at.
GET {{base}}/internal/search-textes?query=responsabilite&place=BE&place=LU&limit=5
HTTP 200
[Asserts]
jsonpath "$.jurisdictions" contains "BE"
jsonpath "$.jurisdictions" contains "LU"
jsonpath "$.jurisdictions" contains "EU"
jsonpath "$.jurisdictions" not contains "FR"
jsonpath "$.hits[*].jurisdiction" not contains "FR"
jsonpath "$.readerPlace" not exists

# Each place the day has not read yet costs a pass over the coverage: 20 at most.
GET {{base}}/internal/search-textes?query=responsabilite&place=AM&place=AO&place=AT&place=BE&place=BF&place=BG&place=BI&place=BJ&place=CD&place=CF&place=CG&place=CH&place=CI&place=CM&place=DE&place=DJ&place=DO&place=DZ&place=EG&place=ES&place=GA
HTTP 422
[Asserts]
jsonpath "$.detail[0].loc[1]" == "place"
jsonpath "$.detail[0].msg" contains "at most 20"

# The browse keeps to the same corpora.
GET {{base}}/internal/search-textes?query=&place=BE&limit=3
HTTP 200
[Asserts]
jsonpath "$.total" > 0
jsonpath "$.hits[*].jurisdiction" not contains "FR"
