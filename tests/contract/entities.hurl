# ============================================================================
# CONTRACT TESTS — route group: entities (fiche entité + annuaire)
#   GET /internal/entity/{ns}/{id}
#   GET /internal/entity/{ns}/{id}/decisions
#   GET /internal/decision/{id}/parties
#   GET /internal/entities/search
#   GET /internal/entities/directory
#   GET /internal/entities/stats
# All public (auth=none). {{base}} = host root. Anchors from the public
# registers: entity `siren:444608442` (ENEDIS); decision `qlSPCVKPKFgt` has
# resolved parties. STRUCTURAL assertions only (presence / type / enum / order),
# never live values.
# ============================================================================

# ---------------------------------------------------------------------------
# health sanity (anchors {{base}})
# ---------------------------------------------------------------------------
GET {{base}}/admin/health
HTTP 200
[Asserts]
jsonpath "$.status" == "ok"


# ===========================================================================
# GET /internal/entity/{ns}/{id}: fiche, entité réelle connue
# ===========================================================================
GET {{base}}/internal/entity/siren/444608442
HTTP 200
[Asserts]
header "Cache-Control" exists
# header registre
jsonpath "$.header.uid" == "siren:444608442"
jsonpath "$.header.namespace" == "siren"
jsonpath "$.header.nature" matches /^(physique|morale_privee|morale_publique)$/
jsonpath "$.header.denomination" isString
jsonpath "$.header.active" isBoolean
jsonpath "$.header.denominations" isCollection
# stats agrégées
jsonpath "$.stats.decisionCount" isInteger
jsonpath "$.stats.decisionCount" > 0
jsonpath "$.stats.asApplicant" isInteger
jsonpath "$.stats.asDefendant" isInteger
jsonpath "$.stats.byYear" isCollection
jsonpath "$.stats.byJurisdiction" isCollection
# forme des lignes d'agrégat
jsonpath "$.stats.byYear[0].year" isInteger
jsonpath "$.stats.byYear[0].count" isInteger
jsonpath "$.stats.byJurisdiction[0].key" isString
jsonpath "$.stats.byJurisdiction[0].label" isString
jsonpath "$.stats.byJurisdiction[0].count" isInteger


# ---------------------------------------------------------------------------
# fiche: uid inconnu → 404
# ---------------------------------------------------------------------------
GET {{base}}/internal/entity/siren/000000000
HTTP 404


# ===========================================================================
# GET /internal/entity/{ns}/{id}/decisions: page 1, taille bornée
# ===========================================================================
GET {{base}}/internal/entity/siren/444608442/decisions?page=1&page_size=5
HTTP 200
[Asserts]
header "Cache-Control" exists
jsonpath "$.total" isInteger
jsonpath "$.total" > 0
jsonpath "$.page" == 1
jsonpath "$.pageSize" == 5
jsonpath "$.items" isCollection
jsonpath "$.items" count <= 5
# forme d'un item : un vrai SearchHit (rendu unifié recherche) + rôle
jsonpath "$.items[0].hit.id" isString
jsonpath "$.items[0].hit.titleHtml" isString
jsonpath "$.items[0].hit.jurisdictionType" isString
jsonpath "$.items[0].hit.score" isNumber
jsonpath "$.items[0].hit.bestChunk.snippet" isString
jsonpath "$.items[0].quality" matches /^(party|law_firm|counsel_name)$/


# ---------------------------------------------------------------------------
# décisions: page_size hors borne (> 100) → 422
# ---------------------------------------------------------------------------
GET {{base}}/internal/entity/siren/444608442/decisions?page_size=101
HTTP 422


# ===========================================================================
# GET /internal/decision/{id}/parties: décision réelle avec parties résolues
# ===========================================================================
GET {{base}}/internal/decision/qlSPCVKPKFgt/parties
HTTP 200
[Asserts]
header "Cache-Control" exists
jsonpath "$.parties" isCollection
jsonpath "$.parties" count > 0
# forme d'une partie
jsonpath "$.parties[0].quality" matches /^(party|law_firm|counsel_name|intervenor)$/
jsonpath "$.parties[0].value" isString


# ---------------------------------------------------------------------------
# parties: décision inconnue → 404
# ---------------------------------------------------------------------------
GET {{base}}/internal/decision/does-not-exist-xyz-0000/parties
HTTP 404


# ===========================================================================
# GET /internal/entities/search: préfixe de dénomination, catégorie entreprises
# ===========================================================================
GET {{base}}/internal/entities/search?q=credit&kind=entreprises&limit=5
HTTP 200
[Asserts]
header "Cache-Control" exists
jsonpath "$.items" isCollection
jsonpath "$.items" count <= 5
# forme d'un item + tri contentieux décroissant (>= 1 hit attendu sur ce préfixe)
jsonpath "$.items[0].uid" isString
jsonpath "$.items[0].namespace" == "siren"
jsonpath "$.items[0].denomination" isString
jsonpath "$.items[0].nature" matches /^(physique|morale_privee|morale_publique)$/
jsonpath "$.items[0].active" isBoolean
jsonpath "$.items[0].decisionCount" isInteger
jsonpath "$.items[0].decisionCount" > 0


# ---------------------------------------------------------------------------
# search: sans kind (toutes catégories), défaut limit
# ---------------------------------------------------------------------------
GET {{base}}/internal/entities/search?q=societe
HTTP 200
[Asserts]
jsonpath "$.items" isCollection
jsonpath "$.items" count <= 10


# ---------------------------------------------------------------------------
# search: q trop courte (< 2) → 422
# ---------------------------------------------------------------------------
GET {{base}}/internal/entities/search?q=a
HTTP 422


# ---------------------------------------------------------------------------
# search: kind inconnu → 422
# ---------------------------------------------------------------------------
GET {{base}}/internal/entities/search?q=credit&kind=martiens
HTTP 422


# ---------------------------------------------------------------------------
# search: limit hors borne (> 50) → 422
# ---------------------------------------------------------------------------
GET {{base}}/internal/entities/search?q=credit&limit=51
HTTP 422


# ===========================================================================
# GET /internal/entities/directory: listing paginé d'une catégorie
# ===========================================================================
GET {{base}}/internal/entities/directory?kind=entreprises&page=1&page_size=10
HTTP 200
[Asserts]
header "Cache-Control" exists
jsonpath "$.total" isInteger
jsonpath "$.total" > 0
jsonpath "$.contentieux" isInteger
jsonpath "$.contentieux" > 0
jsonpath "$.page" == 1
jsonpath "$.pageSize" == 10
jsonpath "$.items" isCollection
jsonpath "$.items" count <= 10
jsonpath "$.items[0].uid" isString
jsonpath "$.items[0].namespace" == "siren"
jsonpath "$.items[0].denomination" isString
jsonpath "$.items[0].decisionCount" isInteger


# ---------------------------------------------------------------------------
# directory: avocats are natural persons, never listed
# ---------------------------------------------------------------------------
GET {{base}}/internal/entities/directory?kind=avocats&barreau=paris&page_size=5
HTTP 422


# ---------------------------------------------------------------------------
# directory: kind manquant → 422
# ---------------------------------------------------------------------------
GET {{base}}/internal/entities/directory?page=1
HTTP 422


# ---------------------------------------------------------------------------
# directory: kind inconnu → 422
# ---------------------------------------------------------------------------
GET {{base}}/internal/entities/directory?kind=martiens
HTTP 422


# ---------------------------------------------------------------------------
# directory: page_size hors borne (> 100) → 422
# ---------------------------------------------------------------------------
GET {{base}}/internal/entities/directory?kind=entreprises&page_size=101
HTTP 422


# ---------------------------------------------------------------------------
# directory: profondeur au-delà du plafond (page × page_size > 10 000) → 422
# ---------------------------------------------------------------------------
GET {{base}}/internal/entities/directory?kind=entreprises&page=501&page_size=20
HTTP 422


# ---------------------------------------------------------------------------
# directory: catégorie cabinets
# ---------------------------------------------------------------------------
GET {{base}}/internal/entities/directory?kind=cabinets&page_size=5
HTTP 200
[Asserts]
jsonpath "$.total" isInteger
jsonpath "$.items" isCollection


# ===========================================================================
# GET /internal/entities/stats: compteurs par catégorie (registre + contentieux)
# ===========================================================================
GET {{base}}/internal/entities/stats
HTTP 200
[Asserts]
header "Cache-Control" exists
jsonpath "$.entreprises.registre" isInteger
jsonpath "$.entreprises.registre" > 0
jsonpath "$.entreprises.contentieux" isInteger
jsonpath "$.entreprises.contentieux" > 0
jsonpath "$.personnesPubliques.registre" isInteger
jsonpath "$.personnesPubliques.contentieux" isInteger
jsonpath "$.associations.registre" isInteger
jsonpath "$.associations.contentieux" isInteger
jsonpath "$.avocats.registre" isInteger
jsonpath "$.avocats.contentieux" isInteger
jsonpath "$.cabinets.registre" isInteger
jsonpath "$.cabinets.contentieux" isInteger
