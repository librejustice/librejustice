# ============================================================================
# CONTRACT TESTS — route group: loi / comparateur de versions
#   GET /internal/norm/{code}/{num}/compare/{de}/{a}
# Bornes = dates ISO de fenêtre de version. Le 373-2-9 du Code civil gagne un
# alinéa entre 2002-03-05 et 2007-03-06 ; le 16-13 a deux versions au texte
# identique. Assertions STRUCTURELLES, pas de valeurs live.
# ============================================================================

GET {{base}}/internal/norm/code-civil/373-2-9/compare/2002-03-05/2007-03-06
HTTP 200
[Asserts]
header "Cache-Control" exists
jsonpath "$.code" == "code-civil"
jsonpath "$.numKey" == "373-2-9"
jsonpath "$.from.legiarti" isString
jsonpath "$.from.dateDebut" isString
jsonpath "$.to.legiarti" isString
jsonpath "$.rows" count > 0
jsonpath "$.rows[0].kind" matches /^(equal|changed|added|removed)$/
jsonpath "$.rows[?(@.kind != 'equal')]" count > 0
jsonpath "$.versions" count >= 2

# L'invariant du vis-à-vis : la colonne gauche n'a jamais d'insertion, la
# droite jamais de suppression.
jsonpath "$.rows[*].left[?(@.op == 'insert')]" count == 0
jsonpath "$.rows[*].right[?(@.op == 'delete')]" count == 0

# Deux rédactions identiques : aucune ligne modifiée.
GET {{base}}/internal/norm/code-civil/16-13/compare/2002-03-05/2004-08-07
HTTP 200
[Asserts]
jsonpath "$.rows[?(@.kind != 'equal')]" count == 0

# borne non-date → 422
GET {{base}}/internal/norm/code-civil/16-13/compare/pas-une-date/2004-08-07
HTTP 422

# code inconnu → 404
GET {{base}}/internal/norm/inconnu-xyz/16-13/compare/2002-03-05/2004-08-07
HTTP 404

# ============================================================================
# Chronolégi — sommaire et section à date (`?date=YYYY-MM-DD`)
# ============================================================================

GET {{base}}/internal/norm/code-civil/sommaire?date=1990-01-01
HTTP 200
[Asserts]
header "Cache-Control" exists
jsonpath "$.tree" count > 0

# date invalide → 422
GET {{base}}/internal/norm/code-civil/sommaire?date=pas-une-date
HTTP 422

# ============================================================================
# Vue-lecture intégrale des textes courts (≤ 150 articles) : `reading` porte
# les corps, `entries` (chips) reste vide. Un gros code (code-civil ci-dessus)
# sert `tree` sans `reading`.
# ============================================================================

GET {{base}}/internal/norm/l-interpretation-par-la-jurisprudence-de-la-cjue-des-libertes-fondamentales/sommaire
HTTP 200
[Asserts]
jsonpath "$.reading" count > 0
jsonpath "$.reading[0].kind" matches /^(section|article)$/
jsonpath "$.reading[?(@.kind == 'article')].texte" count > 0
jsonpath "$.entries" count == 0

# ============================================================================
# Commentaries and parliamentary work on the article answer. Art. 8 ECHR: a
# curated note stored in the base. Art. L311-13 CESEDA: amending laws, as
# composed Légifrance links.
# ============================================================================

GET {{base}}/internal/norm/convention-de-sauvegarde-des-droits-de-l-homme-et-des-libertes-fondamentales/8
HTTP 200
[Asserts]
jsonpath "$.commentaires" count > 0
jsonpath "$.commentaires[0].kind" == "note"
jsonpath "$.commentaires[0].url" isString

GET {{base}}/internal/norm/code-de-l-entree-et-du-sejour-des-etrangers-et-du-droit-d-asile/l311-13
HTTP 200
[Asserts]
jsonpath "$.travauxParlementaires" count > 0
jsonpath "$.travauxParlementaires[0].label" contains "LOI"
jsonpath "$.travauxParlementaires[0].href" matches /^https:\/\/www\.legifrance\.gouv\.fr\/jorf\/id\/JORFTEXT/

# ============================================================================
# Renvois hyperliés du corps. Art. 1242 du Code civil (version 2016,
# stable) : renvoie aux art. 1733 et 1734 du même code — spans codepoints
# demi-ouverts sur `texte`, hrefs internes /norm/{juris}/{slug}/{numKey}.
# ============================================================================

GET {{base}}/internal/norm/code-civil/1242
HTTP 200
[Asserts]
jsonpath "$.texteSpans" count > 0
jsonpath "$.texteSpans[0].targets[0].href" matches /^\/norm\//
jsonpath "$.texteSpans[0].targets[0].label" isString
