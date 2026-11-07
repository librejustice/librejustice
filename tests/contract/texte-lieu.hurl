# ============================================================================
# CONTRACT TESTS: the reader's place on an article
#   GET /internal/places
#   GET /internal/norm/{code}/{num}?place=PF
# {{base}} = host root. This locks the contract of the setting, not the content
# of the states: while `assertion` is empty, `applicability` is absent and the
# served version is the one served before.
# ============================================================================

GET {{base}}/internal/places
HTTP 200
[Asserts]
jsonpath "$.places" count > 40
jsonpath "$.places[0].uid" not contains ":"
jsonpath "$.places[0].parent" not exists
jsonpath "$.places[?(@.uid == 'FR-M')].label" nth 0 isString
jsonpath "$.places[?(@.uid == 'FR-M')].parent" nth 0 == "FR"
# An exclusion must be able to be an absence, so Alsace-Moselle is the
# metropole's sibling, never its child.
jsonpath "$.places[?(@.uid == 'FR-ALS')].parent" nth 0 == "FR"
# Every place comes under the heading the reader picks it from.
jsonpath "$.places[?(@.uid == 'FR-M')].group" nth 0 == "metropole"
jsonpath "$.places[?(@.uid == 'YT')].group" nth 0 == "overseas_identity"
jsonpath "$.places[?(@.uid == 'PF')].group" nth 0 == "overseas_specialty"
jsonpath "$.places[?(@.uid == 'BE')].group" nth 0 == "other_states"

GET {{base}}/internal/norm/code-civil/1240
HTTP 200
[Captures]
version_defaut: jsonpath "$.dateDebut"

GET {{base}}/internal/norm/code-civil/1240?place=FR-M
HTTP 200
[Asserts]
jsonpath "$.dateDebut" == {{version_defaut}}
jsonpath "$.numKey" == "1240"

GET {{base}}/internal/norm/code-civil/1240?place=NC
HTTP 200
[Asserts]
jsonpath "$.numKey" == "1240"
jsonpath "$.versions" count > 0

GET {{base}}/internal/norm/code-civil/1240?place=XX
HTTP 422
[Asserts]
jsonpath "$.detail[0].loc[0]" == "query"
jsonpath "$.detail[0].loc[1]" == "place"

# A facet that is not a place does not pass for one.
GET {{base}}/internal/norm/code-civil/1240?place=matiere:civil
HTTP 422

GET {{base}}/internal/norm/code-civil/1240/2020-01-01?place=GP
HTTP 200
[Asserts]
jsonpath "$.numKey" == "1240"
jsonpath "$.dateDebut" == "2016-10-01"

# The corpus of a place: what applies there, grouped by journal then nature.
GET {{base}}/internal/corpus
HTTP 200
[Asserts]
jsonpath "$.place.uid" == "FR-M"
jsonpath "$.total" > 0
jsonpath "$.journals" count > 0
jsonpath "$.journals[0].code" isString
jsonpath "$.journals[0].groups[0].total" > 0

GET {{base}}/internal/corpus/2020-01-01?place=NC
HTTP 200
[Asserts]
jsonpath "$.place.uid" == "NC"
jsonpath "$.date" == "2020-01-01"

GET {{base}}/internal/corpus?place=XX
HTTP 422
[Asserts]
jsonpath "$.detail[0].loc[1]" == "place"

GET {{base}}/internal/corpus/2020-13-01
HTTP 422

# Two live states of one article, the acceptance test of the model: the
# metropole reads the current wording, New Caledonia keeps the one a counter
# froze in 2015 and names in its clause.
GET {{base}}/internal/norm/code-de-l-environnement/L218-42?place=FR-M
HTTP 200
[Captures]
version_metropole: jsonpath "$.dateDebut"
[Asserts]
jsonpath "$.applicability.status" == "applicable"
jsonpath "$.applicability.reader.uid" == "FR-M"
jsonpath "$.applicability.places[*].uid" contains "FR-M"

GET {{base}}/internal/norm/code-de-l-environnement/L218-42?place=NC
HTTP 200
[Asserts]
jsonpath "$.applicability.status" == "applicable"
jsonpath "$.applicability.reader.uid" == "NC"
jsonpath "$.applicability.places[*].uid" contains "NC"
jsonpath "$.applicability.startsOn" isString
jsonpath "$.applicability.startsOnSource" contains "compteur"
jsonpath "$.dateDebut" != {{version_metropole}}

# Saint-Pierre-et-Miquelon receives the JORF by default since 1985, and the
# environment is none of the matters it keeps.
GET {{base}}/internal/norm/code-de-l-environnement/L218-42?place=PM
HTTP 200
[Asserts]
jsonpath "$.applicability.status" == "applicable"
jsonpath "$.applicability.places[*].uid" contains "PM"

# No rule brings the JORF to Niger: the article does not apply there, the
# answer says where it does, and the picker offers the reader's own place.
GET {{base}}/internal/norm/code-de-justice-administrative/L761-1?place=NE
HTTP 200
[Asserts]
jsonpath "$.applicability.status" == "not_applicable"
jsonpath "$.applicability.places[0]" not exists
jsonpath "$.applicability.elsewhere" count > 0
jsonpath "$.applicability.elsewhere[0].version" isString
jsonpath "$.applicability.readable[*].place.uid" contains "NE"
jsonpath "$.applicability.readable[*].place.uid" contains "PF"

# The administrative procedure reaches French Polynesia by its own rule, where
# the JORF otherwise needs an express mention.
GET {{base}}/internal/norm/code-de-justice-administrative/L761-1?place=PF
HTTP 200
[Asserts]
jsonpath "$.applicability.status" == "applicable"
jsonpath "$.applicability.places[*].uid" contains "PF"

# Civil law passed to French Polynesia on 2 March 2004: article 1382 is served
# in the wording of that day, which the 2016 reform replaced in the metropole.
GET {{base}}/internal/norm/code-civil/1382?place=PF
HTTP 200
[Asserts]
jsonpath "$.applicability.status.undetermined" == "frozen"
jsonpath "$.applicability.frozenOn" == "2004-03-02"
jsonpath "$.applicability.ruleSource" contains "2004-192"
jsonpath "$.texte" contains "Tout fait quelconque"

# The reader's place sets the corpora searched. Without one, the law applicable
# in France, which is what every caller had before the place was a setting.
GET {{base}}/internal/search-textes?query=code+civil&limit=1
HTTP 200
[Asserts]
jsonpath "$.jurisdictions" count == 3
jsonpath "$.jurisdictions" contains "FR"
jsonpath "$.jurisdictions" contains "EU"
jsonpath "$.readerPlace" not exists

# Placed in Switzerland, the same query searches Swiss law, and the answer says
# where it is read from so a page opened from it keeps the place.
GET {{base}}/internal/search-textes?query=code+civil&limit=1&place=CH
HTTP 200
[Asserts]
jsonpath "$.jurisdictions" contains "CH"
jsonpath "$.jurisdictions" not contains "FR"
jsonpath "$.readerPlace.uid" == "CH"
jsonpath "$.readerPlace.label" == "Suisse"

# A place of the Union is reached by the Union, an overseas country is not: the
# rules of the JOUE say it.
GET {{base}}/internal/search-textes?query=responsabilite&limit=1&place=GP
HTTP 200
[Asserts]
jsonpath "$.jurisdictions" contains "EU"

GET {{base}}/internal/search-textes?query=responsabilite&limit=1&place=PF
HTTP 200
[Asserts]
jsonpath "$.jurisdictions" contains "FR"
jsonpath "$.jurisdictions" not contains "EU"

# A country named in the query still wins over the reader's place.
GET {{base}}/internal/search-textes?query=code+civil+belge&limit=1&place=CH
HTTP 200
[Asserts]
jsonpath "$.jurisdictions" contains "BE"

GET {{base}}/internal/search-textes?query=code+civil&limit=1&place=XX
HTTP 422
[Asserts]
jsonpath "$.detail[0].loc[1]" == "place"

# A place written with its vocabulary prefix, as old links carry it, moves to
# the bare code, on a page as on the API.
GET {{base}}/norm/fr/code-civil/1240?place=geo:NC
[Options]
location: false
HTTP 308
[Asserts]
header "Location" == "/norm/fr/code-civil/1240?place=NC"

GET {{base}}/internal/norm/code-civil/1240?date=2020-01-01&place=geo%3APF
[Options]
location: false
HTTP 308
[Asserts]
header "Location" == "/internal/norm/code-civil/1240?date=2020-01-01&place=PF"

# FR names the Republic, not a place one reads from: it reads as the metropole.
GET {{base}}/internal/norm/code-civil/1240?place=FR
HTTP 200
[Asserts]
jsonpath "$.applicability.reader.uid" == "FR-M"
jsonpath "$.applicability.status" == "applicable"

# A country the query names is searched alone.
GET {{base}}/internal/search-textes?query=code+civil+belge&limit=3
HTTP 200
[Asserts]
jsonpath "$.jurisdictions" count == 1
jsonpath "$.jurisdictions[0]" == "BE"
jsonpath "$.hits[0].codeTitle" contains "Code civil belge"

# A bilateral treaty lives in INTL, which every place searches, and
# « franco-algérien » names no country: the accord comes first read from France
# as from Algeria.
GET {{base}}/internal/search-textes?query=trait%C3%A9+franco+alg%C3%A9rien&limit=3
HTTP 200
[Asserts]
jsonpath "$.jurisdictions" contains "INTL"
jsonpath "$.jurisdictions" not contains "DZ"
jsonpath "$.hits[0].codeTitle" contains "franco-algérien"

GET {{base}}/internal/search-textes?query=trait%C3%A9+franco+alg%C3%A9rien&limit=3&place=DZ
HTTP 200
[Asserts]
jsonpath "$.jurisdictions" contains "INTL"
jsonpath "$.hits[0].codeTitle" contains "franco-algérien"
