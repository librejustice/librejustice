# The AUTHENTICATED path end to end: web session by cookie, then MCP, by OAuth
# and by a static key.
#
# This file presents a VALID identity. The rest of the suite carries no secret
# and tests refusal.
#
# The target must be https, the cookie being `Secure` and so never sent in the
# clear. It does not run alone: the runner injects `base`, `email`, `password`
# and `api_key`. The account is a dedicated test account: it exists already, it
# survives the run, and no real account is borrowed.
#
# PKCE: the verifier and challenge pair is the one of RFC 7636 appendix B, so a
# constant. A client does not have to compute a SHA-256 to prove the server
# checks its own. Same pair as the unit test
# `pkce_s256_challenge_matches_rfc7636_example` of `oauth.rs`.

# ── 1. La connexion pose le cookie et repart vers `next` ─────────────────────
# Formulaire natif : POST → 303, la destination interne du champ caché `next`
# dans le Location.
POST {{base}}/internal/auth/login
[FormParams]
email: {{email}}
password: {{password}}
next: /decision-search
HTTP 303
[Captures]
session: cookie "__Host-lj_session"
[Asserts]
header "Location" == "/decision-search"
cookie "__Host-lj_session" exists
cookie "__Host-lj_session[HttpOnly]" exists
cookie "__Host-lj_session[Secure]" exists
cookie "__Host-lj_session[SameSite]" == "Lax"
# ── 2. Le cookie seul résout le profil ───────────────────────────────────────
# L'identité résolue est CELLE du cookie, pas n'importe laquelle : c'est ce qui
# distingue « la vérification marche » de « le serveur rend un profil ».
GET {{base}}/internal/me
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.sub" isString
jsonpath "$.email" == "{{email}}"
jsonpath "$.createdAt" isString
jsonpath "$.lastSeenAt" isString

# ── 3. La page de connexion refuse un porteur de session ─────────────────────
# La garde serveur répond 303 vers `next` avant tout rendu : un connecté n'a
# rien à faire sur /login.
GET {{base}}/login?next=%2Fdecision-search
HTTP 303
[Asserts]
header "Location" == "/decision-search"

# ── 4. Le consentement MCP se rend avec la session, en privé ─────────────────
# La garde laisse passer le porteur du cookie, l'écran hérite du défaut privé.
GET {{base}}/authorize-mcp
HTTP 200
[Asserts]
header "Cache-Control" == "private, no-store"
body contains "Autoriser"
# `form-action` a quitté cette page : Chrome l'oppose à toute la chaîne de
# redirection du POST, dont nous ne connaissons que le premier saut. La
# régression consiste à la remettre.
header "Content-Security-Policy" not contains "form-action"

# ── 5. Le profil se rend avec la session : compte lu au rendu, en privé ──────
# La zone danger dans le HTML prouve que le rendu serveur est complet, pas un
# squelette.
GET {{base}}/profile
HTTP 200
[Asserts]
header "Cache-Control" == "private, no-store"
body contains "Supprimer mon compte"
body contains "{{email}}"
# ── 6. Une recherche identifiée aboutit ──────────────────────────────────────
# Le 401 n'est pas le seul symptôme possible : une identité perdue dégrade en
# anonyme sans erreur. On assert donc le succès, et `identity` se lit
# dans les spans.
GET {{base}}/internal/search?query=trouble+anormal+de+voisinage&limit=3
HTTP 200
[Asserts]
jsonpath "$.hits" isCollection
jsonpath "$.allHitPaths" isCollection

# ── 7. Enregistrement dynamique du client (RFC 7591) ─────────────────────────
POST {{base}}/oauth/register
Content-Type: application/json
{
  "client_name": "contract-probe",
  "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"]
}
HTTP 201
[Captures]
client_id: jsonpath "$.client_id"

# ── 8. Approbation : l'étape sans laquelle aucun connecteur ne s'installe ────
# Elle exige une session, et répond en 303 : le code sort dans la query du
# Location.
POST {{base}}/oauth/approve
Content-Type: application/x-www-form-urlencoded
[FormParams]
action: approve
client_id: {{client_id}}
redirect_uri: https://claude.ai/api/mcp/auth_callback
code_challenge: E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM
HTTP 303
[Captures]
code: header "Location" regex "[?&]code=([A-Za-z0-9_-]{43})"
[Asserts]
header "Location" startsWith "https://claude.ai/api/mcp/auth_callback"

# ── 8 bis. Le même défi rend le même code ────────────────────────────────────
# A mobile browser replays the submission when the return fails, so every replay
# must yield the one code.
POST {{base}}/oauth/approve
Content-Type: application/x-www-form-urlencoded
[FormParams]
action: approve
client_id: {{client_id}}
redirect_uri: https://claude.ai/api/mcp/auth_callback
code_challenge: E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM
HTTP 303
[Asserts]
header "Location" contains "code={{code}}"

# ── 9. Échange du code contre une paire, PKCE vérifié ────────────────────────
POST {{base}}/oauth/token
[FormParams]
grant_type: authorization_code
code: {{code}}
client_id: {{client_id}}
redirect_uri: https://claude.ai/api/mcp/auth_callback
code_verifier: dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk
HTTP 200
[Captures]
access: jsonpath "$.access_token"
refresh: jsonpath "$.refresh_token"
[Asserts]
jsonpath "$.token_type" == "Bearer"
jsonpath "$.scope" == "mcp openid email"
jsonpath "$.expires_in" isInteger
jsonpath "$.refresh_token" isString
jsonpath "$.id_token" isString
jsonpath "$.id_token" matches "^[A-Za-z0-9_-]+\\.[A-Za-z0-9_-]+\\.[A-Za-z0-9_-]+$"

# ── 9bis. UserInfo : le jeton rend l'identité de son compte ──────────────────
GET {{base}}/oauth/userinfo
Authorization: Bearer {{access}}
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.sub" isString
jsonpath "$.email" contains "@"
jsonpath "$.email_verified" isBoolean

# ── 10. Le jeton MCP ouvre un appel d'outil réel ─────────────────────────────
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "search_decisions",
    "arguments": { "query": "trouble anormal de voisinage", "limit": 1 }
  }
}
HTTP 200
[Asserts]
jsonpath "$.jsonrpc" == "2.0"
jsonpath "$.result.isError" not == true

# A case name inside a question: the list answers the question, the decision sits apart in `pinned`.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 21,
  "method": "tools/call",
  "params": {
    "name": "search_decisions",
    "arguments": { "query": "Danthony vice de procédure", "limit": 1, "ai_rerank": false }
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.pinned[0].title" contains "Danthony"
jsonpath "$.result.structuredContent.pinned[0].url" contains "/decision/"

# A stated mode is served as stated: `pinned` exists in `auto` only.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 22,
  "method": "tools/call",
  "params": {
    "name": "search_decisions",
    "arguments": { "query": "Danthony vice de procédure", "mode": "lexical", "limit": 1, "ai_rerank": false }
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.pinned" not exists

POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 23,
  "method": "tools/call",
  "params": {
    "name": "search_decisions",
    "arguments": { "query": "Danthony vice de procédure", "mode": "semantic", "limit": 1, "ai_rerank": false }
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.pinned" not exists

# The retired key narrows the set and does not open a rank: only a `seat` filter
# does. A caller still on it reads the paths off the hits, where `seat` carries
# the room, and the facet answers with the orders and their types alone.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 20,
  "method": "tools/call",
  "params": {
    "name": "search_decisions",
    "arguments": { "query": "bail commercial", "jurisdiction_code": ["cc"], "limit": 1 }
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
# The total comes out of the deferred cell, which MCP resolves, and the facets
# open only because it overflows the served window.
jsonpath "$.result.structuredContent.total" isInteger
jsonpath "$.result.structuredContent.total" > 1
# `cc` has no court segment: its code names the type, so a room's path holds in
# three segments and the type carries the court's count.
jsonpath "$.result.structuredContent.facets.seat.FR" isInteger
jsonpath "$.result.structuredContent.facets.seat['FR.CC']" isInteger
jsonpath "$.result.structuredContent.facets.seat['FR.CC.COMMERCIALE']" not exists

# A filtered seat opens the rank right below it, whole: MCP serves the `seat`
# facet, whose keys are dotted paths, handed back verbatim to the `seat` filter.
# A room with no number is named by its specialisation.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 21,
  "method": "tools/call",
  "params": {
    "name": "search_decisions",
    "arguments": { "query": "bail commercial", "seat": ["FR.CC"], "limit": 1 }
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.facets.seat['FR.CC']" isInteger
jsonpath "$.result.structuredContent.facets.seat['FR.CC.C3_CIVILE']" isInteger
jsonpath "$.result.structuredContent.facets.seat['FR.CC.COMMERCIALE']" isInteger

# ── 10 bis. La liste servie est enregistrable telle quelle par un client ─────
# A combinator at the root of an `inputSchema` makes the client drop the tool,
# with no error nor trace on the server side.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }
HTTP 200
[Asserts]
jsonpath "$.result.tools" count == 4
jsonpath "$.result.tools[*].name" contains "get_norm"
jsonpath "$.result.tools[0].inputSchema.anyOf" not exists
jsonpath "$.result.tools[1].inputSchema.anyOf" not exists
jsonpath "$.result.tools[2].inputSchema.anyOf" not exists
jsonpath "$.result.tools[3].inputSchema.anyOf" not exists

# ── 10 ter. Les rattrapages de désignation, que seul un vrai jeton atteint ───
# Un client MCP est un modèle : il compose la forme d'appel qui lui paraît
# naturelle, et le serveur accepte ce qui ne peut désigner qu'une cible unique.
# Sans verrou, ces rattrapages repartent en arrière sans bruit.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 11,
  "method": "tools/call",
  "params": {
    "name": "get_norm",
    "arguments": { "code_name": "Code de procédure pénale", "article_number": "706-88" }
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.code" == "code-de-procedure-penale"
jsonpath "$.result.structuredContent.number" == "706-88"

# Un identifiant Légifrance se reconnaît à sa forme, où qu'il soit posé.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 12,
  "method": "tools/call",
  "params": { "name": "get_norm", "arguments": { "text_id": "LEGIARTI000006419282" } }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.code" == "code-civil"

# The root of a text without articles: its body and no `number`, whose absence
# tells the caller it holds a text and not an article.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 13,
  "method": "tools/call",
  "params": {
    "name": "get_norm",
    "arguments": { "url": "/texte/circulaire-imig0800044c-du-20-novembre-2008-relative-a-l-accord-franco-gabonais" }
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.text" isString
jsonpath "$.result.structuredContent.number" not exists

# Racine d'un texte qui a des articles : elle rend sa table des matières, bornée,
# et `omitted` dit ce qui manque. Un code entier ne tient pas dans une réponse ;
# sa table, oui.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 14,
  "method": "tools/call",
  "params": { "name": "get_norm", "arguments": { "url": "/texte/code-du-travail" } }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.article_count" > 1000
jsonpath "$.result.structuredContent.articles" isCollection
jsonpath "$.result.structuredContent.omitted" isInteger
jsonpath "$.result.structuredContent.text" not exists

# A division read by its path, and by its Légifrance id, which lands on the same.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 14,
  "method": "tools/call",
  "params": { "name": "get_norm", "arguments": { "url": "/norm/fr/code-civil/livre-i/titre-v" } }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.title" == "Titre V : Du mariage"
jsonpath "$.result.structuredContent.url" endsWith "/norm/fr/code-civil/livre-i/titre-v"

POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 14,
  "method": "tools/call",
  "params": { "name": "get_norm", "arguments": { "text_id": "LEGISCTA000006117710" } }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.url" endsWith "/norm/fr/code-civil/livre-i/titre-v"

# Nom de texte écrit juste mais autrement ponctué, et code de juridiction au
# tiret : deux rattrapages qui ne réécrivent que ce qui devient connu.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 15,
  "method": "tools/call",
  "params": {
    "name": "search_decisions",
    "arguments": {
      "query": "trouble de voisinage",
      "legal_instrument": ["code-de-la-construction-et-de-lhabitation"],
      "jurisdiction_code": ["ca_aix-en-provence"],
      "limit": 1
    }
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true

# Le CGI renommé se filtre sous son nom, et un code inventé erre toujours.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 16,
  "method": "tools/call",
  "params": {
    "name": "search_norms",
    "arguments": { "code": "code-general-des-impots", "query": "exigibilité de la taxe", "limit": 1 }
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true

POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 17,
  "method": "tools/call",
  "params": {
    "name": "search_norms",
    "arguments": { "code": "code-inexistant-xyz", "query": "peine", "limit": 1 }
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" == true

# A citation naming no code has no other door: the search lists the carrying
# texts instead of ranking the whole fund.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 18,
  "method": "tools/call",
  "params": {
    "name": "search_norms",
    "arguments": { "query": "article 1240", "limit": 10 }
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.total" < 20
jsonpath "$.result.structuredContent.facets.code['code-civil']" isInteger

# ── 10 quater. An article carries its amending laws and their travaux ───────
# `legislative_history` is `modified_by` narrowed to the LOI whose JORF text
# is known, so both lock together: 2279 has two of each, and 350, amended by
# an ordonnance alone, keeps the first and loses the second. The DILA label
# breaks its line inside the law number, the JSON does not.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 19,
  "method": "tools/call",
  "params": { "name": "get_norm", "arguments": { "url": "/texte/code-civil/2279" } }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.modified_by" count == 2
jsonpath "$.result.structuredContent.legislative_history" count == 2
jsonpath "$.result.structuredContent.legislative_history[0].date" == "2015-02-16"
jsonpath "$.result.structuredContent.legislative_history[0].href" == "https://www.legifrance.gouv.fr/jorf/id/JORFTEXT000030248562"
jsonpath "$.result.structuredContent.legislative_history[1].label" == "LOI n°2008-561 du 17 juin 2008"

POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 20,
  "method": "tools/call",
  "params": { "name": "get_norm", "arguments": { "url": "/texte/code-civil/350" } }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.modified_by[0].label" startsWith "Ordonnance n°2022-1292"
jsonpath "$.result.structuredContent.legislative_history" not exists

# ── 10 quinquies. A retired tool name still calls its tool ──────────────────
# A client freezes the tool list when the connector is installed, and keeps
# calling the names of before a rename.
POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 24,
  "method": "tools/call",
  "params": { "name": "get_legal_text", "arguments": { "url": "/norm/fr/code-civil/1240" } }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.number" == "1240"

POST {{base}}/mcp
Authorization: Bearer {{access}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 25,
  "method": "tools/call",
  "params": { "name": "search_legal_texts", "arguments": { "query": "responsabilité du fait des choses", "limit": 1 } }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true
jsonpath "$.result.structuredContent.total" > 0

# ── 11. Le grant refresh tourne, et rote les deux jetons ─────────────────────
# An access token outlives 30 days through this grant alone.
POST {{base}}/oauth/token
[FormParams]
grant_type: refresh_token
refresh_token: {{refresh}}
client_id: {{client_id}}
HTTP 200
[Captures]
access2: jsonpath "$.access_token"
refresh2: jsonpath "$.refresh_token"
[Asserts]
jsonpath "$.access_token" != "{{access}}"
jsonpath "$.refresh_token" != "{{refresh}}"

# ── 12. Le jeton issu du refresh appelle un outil ────────────────────────────
POST {{base}}/mcp
Authorization: Bearer {{access2}}
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "search_decisions",
    "arguments": { "query": "servitude de passage", "limit": 1 }
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" not == true

# ── 13. L'ancien refresh est mort : la rotation tient ────────────────────────
POST {{base}}/oauth/token
[FormParams]
grant_type: refresh_token
refresh_token: {{refresh}}
client_id: {{client_id}}
# Statut seul, comme le fait déjà `oauth.hurl` § 6d : la propriété à verrouiller
# est « l'ancien refresh ne marche plus », et le 400 l'exprime entièrement.
HTTP 400

# ── 14. Un jeton MCP inventé est refusé ──────────────────────────────────────
# Refused with `invalid_token` and the challenge naming the resource metadata.
POST {{base}}/mcp
Authorization: Bearer lj_jeton_qui_nexiste_pas
Accept: application/json, text/event-stream
Content-Type: application/json
{ "jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {} }
HTTP 401
[Asserts]
jsonpath "$.error" == "invalid_token"
header "WWW-Authenticate" contains "resource_metadata="

# ── 14 bis. The public door answers at the grain of the MCP tools ────────────
# A hit carries the url that chains to `get_decision`; the browser DTO does not.
GET {{base}}/api/search?query=usufruit&limit=1&ai_rerank=1
Authorization: Bearer {{api_key}}
HTTP 200
[Asserts]
jsonpath "$.hits[0].url" contains "/decision/"
jsonpath "$.hits[0].title" exists
jsonpath "$.total" exists

# ── 14 bis bis. A parameter the contract does not name is refused ───────────
# Ignored, it would answer another question than the one asked. Every one is named.
GET {{base}}/api/search?query=usufruit&limit=1&foo=1&bar=2
Authorization: Bearer {{api_key}}
HTTP 422
[Asserts]
jsonpath "$.detail[0].type" == "extra_forbidden"
jsonpath "$.detail[0].loc[1]" == "foo"
jsonpath "$.detail[1].loc[1]" == "bar"

# ── 14 bis ter. The former names are still read ─────────────────────────────
GET {{base}}/api/search?q=usufruit&limit=1&aiMode=false
Authorization: Bearer {{api_key}}
HTTP 200
[Asserts]
jsonpath "$.query" == "usufruit"

GET {{base}}/api/norm/code%20civil/Article%201240?date=2020-01-01
Authorization: Bearer {{api_key}}
HTTP 200
[Asserts]
jsonpath "$.code" == "code-civil"

# ── 14 ter. A code is named as a model composes it, an article likewise ──────
GET {{base}}/api/norm/code%20civil/Article%201240
Authorization: Bearer {{api_key}}
HTTP 200
[Asserts]
jsonpath "$.url" contains "/norm/fr/code-civil/1240"
jsonpath "$.code" == "code-civil"

# ── 15. La déconnexion efface le cookie côté client ──────────────────────────
POST {{base}}/internal/auth/logout
HTTP 204
[Asserts]
cookie "__Host-lj_session[Max-Age]" == 0

# ── 16. La valeur déconnectée est refusée, pas ignorée ───────────────────────
# Represente à la main : l'étape 3 l'a retirée du client, et ce qui est vérifié
# ici est le refus côté serveur — un 401 anonyme passerait pour une réussite.
GET {{base}}/internal/me
Cookie: __Host-lj_session={{session}}
HTTP 401
[Asserts]
jsonpath "$.detail" == "session_rejected"
cookie "__Host-lj_session[Max-Age]" == 0
