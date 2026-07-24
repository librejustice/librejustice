# ============================================================================
# CONTRACT TESTS — route group: loi / comparateur de versions (ADR 0193)
#   GET /api/texte/{code}/{num}/compare/{de}/{a}
# {{base}} = host root. Bornes = dates ISO de fenêtre de version (l'article
# 16-13 du Code civil a deux versions stables : 2002-03-05 et 2004-08-07).
# Assertions STRUCTURELLES (présence / type / enum), pas de valeurs live.
# ============================================================================

GET {{base}}/api/texte/code-civil/16-13/compare/2002-03-05/2004-08-07
HTTP 200
[Asserts]
header "Cache-Control" exists
jsonpath "$.code" == "code-civil"
jsonpath "$.numKey" == "16-13"
jsonpath "$.from.legiarti" isString
jsonpath "$.from.dateDebut" isString
jsonpath "$.to.legiarti" isString
jsonpath "$.segments" count > 0
jsonpath "$.segments[0].op" matches /^(equal|insert|delete)$/
jsonpath "$.segments[0].text" isString
jsonpath "$.versions" count >= 2

# Les deux bornes dans la même fenêtre ⇒ même version des deux côtés, diff
# entièrement `equal` (aucun insert/delete).
GET {{base}}/api/texte/code-civil/16-13/compare/2004-08-07/2004-08-07
HTTP 200
[Asserts]
jsonpath "$.segments[?(@.op != 'equal')]" count == 0

# borne non-date → 422
GET {{base}}/api/texte/code-civil/16-13/compare/pas-une-date/2004-08-07
HTTP 422

# code inconnu → 404
GET {{base}}/api/texte/inconnu-xyz/16-13/compare/2002-03-05/2004-08-07
HTTP 404

# ============================================================================
# Chronolégi (ADR 0193 §5) — sommaire et section à date (`?date=YYYY-MM-DD`)
# ============================================================================

GET {{base}}/api/texte/code-civil/sommaire?date=1990-01-01
HTTP 200
[Asserts]
header "Cache-Control" exists
jsonpath "$.tree" count > 0

# date invalide → 422
GET {{base}}/api/texte/code-civil/sommaire?date=pas-une-date
HTTP 422

# ============================================================================
# Vue-lecture intégrale des textes courts (≤ 150 articles) : `reading` porte
# les corps, `entries` (chips) reste vide. Un gros code (code-civil ci-dessus)
# sert `tree` sans `reading`.
# ============================================================================

GET {{base}}/api/texte/l-interpretation-par-la-jurisprudence-de-la-cjue-des-libertes-fondamentales/sommaire
HTTP 200
[Asserts]
jsonpath "$.reading" count > 0
jsonpath "$.reading[0].kind" matches /^(section|article)$/
jsonpath "$.reading[?(@.kind == 'article')].texte" count > 0
jsonpath "$.entries" count == 0

# ============================================================================
# Commentaires de norme (ADR 0212) et travaux parlementaires (ADR 0215) sur la
# réponse article. Art. 8 CEDH : bundle curé (note de cabinet) ancré en base.
# Art. L311-13 CESEDA : lois modificatrices → liens Légifrance composés.
# ============================================================================

GET {{base}}/api/texte/convention-de-sauvegarde-des-droits-de-l-homme-et-des-libertes-fondamentales/8
HTTP 200
[Asserts]
jsonpath "$.commentaires" count > 0
jsonpath "$.commentaires[0].kind" == "note"
jsonpath "$.commentaires[0].url" isString

GET {{base}}/api/texte/code-de-l-entree-et-du-sejour-des-etrangers-et-du-droit-d-asile/l311-13
HTTP 200
[Asserts]
jsonpath "$.travauxParlementaires" count > 0
jsonpath "$.travauxParlementaires[0].label" contains "LOI"
jsonpath "$.travauxParlementaires[0].href" matches /^https:\/\/www\.legifrance\.gouv\.fr\/jorf\/id\/JORFTEXT/

# ============================================================================
# Renvois hyperliés du corps (ADR 0217). Art. 1242 du Code civil (version 2016,
# stable) : renvoie aux art. 1733 et 1734 du même code — spans codepoints
# demi-ouverts sur `texte`, hrefs internes /texte/{slug}/{numKey}.
# ============================================================================

GET {{base}}/api/texte/code-civil/1242
HTTP 200
[Asserts]
jsonpath "$.texteSpans" count > 0
jsonpath "$.texteSpans[0].targets[0].href" matches /^\/texte\//
jsonpath "$.texteSpans[0].targets[0].label" isString
