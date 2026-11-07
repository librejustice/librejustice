# Contract of the keyed public surface (`/api`), MCP tool parity in REST. A route
# outside the parity answers 404.

# ── No key: the refusal says where to get one ────────────────────────────────
GET {{base}}/api/search?query=usufruit&limit=1
HTTP 401
[Asserts]
jsonpath "$.detail" == "api_key_required"
jsonpath "$.message" contains "librejustice.fr/api"
jsonpath "$.message" contains "assistant"
header "Link" contains "rel=\"help\""

# ── Unknown key: the reason for the refusal changes ──────────────────────────
GET {{base}}/api/search?query=usufruit&limit=1
Authorization: Bearer ljk_cette-cle-n-existe-pas
HTTP 401
[Asserts]
jsonpath "$.detail" == "api_key_invalid"

# ── The dedicated header is read like the Bearer ─────────────────────────────
GET {{base}}/api/decision/inexistante
x-lj-api-key: ljk_cette-cle-n-existe-pas
HTTP 401
[Asserts]
jsonpath "$.detail" == "api_key_invalid"

# ── The spec is readable without a key, and names the six routes ─────────────
GET {{base}}/openapi.json
HTTP 200
[Asserts]
jsonpath "$.openapi" startsWith "3.1"
jsonpath "$.paths['/api/search'].get.operationId" == "searchDecisions"
jsonpath "$.paths['/api/search-textes'].get.operationId" == "searchLegalTexts"
jsonpath "$.paths['/api/decision/{id}'].get.operationId" == "getDecision"
jsonpath "$.paths['/api/norm/{jurisdiction}/{code}'].get.operationId" == "getLegalTextRoot"
jsonpath "$.paths['/api/norm/{jurisdiction}/{code}/{article}'].get.operationId" == "getLegalArticle"
jsonpath "$.paths['/api/article/{uid}'].get.operationId" == "getLegalArticleById"

# ── Legacy mount closed: outside the parity, `/api` serves nothing ───────────
GET {{base}}/api/suggest?q=usufruit
HTTP 404

GET {{base}}/api/health
HTTP 404

GET {{base}}/api/me
Authorization: Bearer ljk_cette-cle-n-existe-pas
HTTP 404
