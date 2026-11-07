# ============================================================================
# CONTRACT TESTS — contexte de lecture d'un article
#   GET /internal/norm/{code}/{num}[/{date}] → $.context
# {{base}} = host root. La sélection est adaptative et se fait en SQL sur
# l'ordre de lecture persisté (`reading_rank`) : ce fichier exerce ses TROIS
# branches sur des textes stables, un par branche, plus la vue à date. Il
# tient lieu de verrou de régression de la règle — elle n'est plus exprimable
# en test unitaire depuis qu'elle vit en SQL.
# ============================================================================

# Branche « texte court » (≤ CONTEXT_FULL_MAX = 40 articles) : tout le texte,
# the current article marked once. 34 articles here, the four annexes included.
GET {{base}}/internal/norm/reglement-ue-n-1025-2012-du-parlement-europeen-et-du-conseil-du-25-octobre-2012/2
HTTP 200
[Asserts]
jsonpath "$.context" count == 34
jsonpath "$.context[?(@.current == true)]" count == 1
jsonpath "$.context[?(@.current == true)].num" nth 0 == "2"
jsonpath "$.context[0].num" isString
jsonpath "$.context[0].etat" isString

# Enclosing-division branch (long text, division ≤ CONTEXT_DIVISION_MAX = 80):
# the section's 8 articles, and none of the rest of the code du travail.
GET {{base}}/internal/norm/code-du-travail/L1234-5
HTTP 200
[Asserts]
jsonpath "$.context" count == 8
jsonpath "$.context[?(@.current == true)]" count == 1
jsonpath "$.context[?(@.current == true)].numKey" nth 0 == "l1234-5"
jsonpath "$.context[0].num" == "L1234-1"
jsonpath "$.context[7].num" == "L1234-8"

# Branche « fenêtre » (texte long, pas de division exploitable) : exactement
# 2 × CONTEXT_WINDOW + 1 = 13 articles, le courant au centre. L'accord EEE :
# 129 articles, aucun arbre structurel.
GET {{base}}/internal/norm/accord-sur-l-espace-economique-europeen/65
HTTP 200
[Asserts]
jsonpath "$.context" count == 13
jsonpath "$.context[?(@.current == true)]" count == 1
jsonpath "$.context[6].current" == true
jsonpath "$.context[6].num" == "65"

# Vue à date : la division est celle des versions D'ÉPOQUE, y
# compris ses membres abrogés/transférés depuis — dispersés hors de la plage
# de rangs du jour, ils n'existent que par l'agrégation globale. En 2003,
# L4413-1 voisine avec L4413-2/3/4, tous sortis de l'arbre actuel.
GET {{base}}/internal/norm/code-de-la-sante-publique/L4413-1/2003-06-03
HTTP 200
[Asserts]
jsonpath "$.context" count == 4
jsonpath "$.context[?(@.current == true)]" count == 1
jsonpath "$.context[0].numKey" == "l4413-1"
jsonpath "$.context[1].etat" == "TRANSFERE"
jsonpath "$.context[3].numKey" == "l4413-4"

# Article inexistant dans le texte : pas de contexte inventé.
GET {{base}}/internal/norm/code-du-travail/L9999-99
HTTP 404
