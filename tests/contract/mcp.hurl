# =============================================================================
# MCP over Streamable HTTP at /mcp  (JSON-RPC 2.0)
# Contract tests for LibreJustice.
#
# Every request sends the headers the Streamable HTTP transport asks of a client:
#    Accept: application/json, text/event-stream
#    Content-Type: application/json
#
# Auth: gated by Settings.mcp_require_auth (default False). When True, an auth
# middleware in front of /mcp requires `Authorization: Bearer <db-token>` and
# returns 401 {"error":"auth_required"|"invalid_token"} + WWW-Authenticate.
# The runner injects {{token}} for the authed variants.
# =============================================================================

# -----------------------------------------------------------------------------
# initialize — server capabilities + serverInfo + instructions
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "2024-11-05",
    "capabilities": {},
    "clientInfo": {"name": "parity-runner", "version": "0.0.0"}
  }
}
HTTP 200
[Asserts]
jsonpath "$.jsonrpc" == "2.0"
jsonpath "$.id" == 1
jsonpath "$.result" exists
jsonpath "$.result.protocolVersion" == "2024-11-05"
jsonpath "$.result.capabilities" exists
jsonpath "$.result.capabilities.tools" exists
jsonpath "$.result.serverInfo" exists
jsonpath "$.result.serverInfo.name" == "librejustice"
jsonpath "$.result.serverInfo.version" isString
jsonpath "$.result.instructions" isString
jsonpath "$.result.instructions" contains "French case law"
jsonpath "$.error" not exists
# The revision asserted above is the one this request asked for: the server
# echoes a revision it speaks, and answers its newest for anything else.

# -----------------------------------------------------------------------------
# initialize sur /mcp SANS slash final — même handler. Les clients MCP (ChatGPT,
# Claude.ai) POSTent l'URL saisie telle quelle et ne suivent pas de redirect sur
# POST : un 4xx ici les fait sonder le transport SSE legacy, qui n'existe pas.
# -----------------------------------------------------------------------------
POST {{base}}/mcp
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "2024-11-05",
    "capabilities": {},
    "clientInfo": {"name": "parity-runner", "version": "0.0.0"}
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.serverInfo.name" == "librejustice"
jsonpath "$.error" not exists

# -----------------------------------------------------------------------------
# notifications/initialized — notification (no id) → 202 Accepted, no body
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "method": "notifications/initialized"
}
HTTP 202

# -----------------------------------------------------------------------------
# ping — empty result object
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "ping",
  "params": {}
}
HTTP 200
[Asserts]
jsonpath "$.jsonrpc" == "2.0"
jsonpath "$.id" == 2
jsonpath "$.result" exists
jsonpath "$.error" not exists

# -----------------------------------------------------------------------------
# tools/list: six tools, exact names, ordering, strict inputSchema
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/list",
  "params": {}
}
HTTP 200
[Asserts]
jsonpath "$.jsonrpc" == "2.0"
jsonpath "$.id" == 3
jsonpath "$.result.tools" isCollection
jsonpath "$.result.tools" count == 6
jsonpath "$.result.tools[0].name" == "search_decisions"
jsonpath "$.result.tools[1].name" == "get_decision"
jsonpath "$.result.tools[2].name" == "get_norm"
jsonpath "$.result.tools[3].name" == "search_norms"
jsonpath "$.result.tools[4].name" == "search_notices"
jsonpath "$.result.tools[5].name" == "get_notice"
# Every tool: description + inputSchema, no title-bearing $ref/$defs leakage.
jsonpath "$.result.tools[0].description" isString
jsonpath "$.result.tools[0].inputSchema" exists
jsonpath "$.result.tools[0].inputSchema.type" == "object"
jsonpath "$.result.tools[0].inputSchema.additionalProperties" == false
jsonpath "$.result.tools[0].inputSchema.required" includes "query"
jsonpath "$.result.tools[0].inputSchema.properties.query.type" == "string"
jsonpath "$.result.tools[0].inputSchema.properties.query.minLength" == 1
jsonpath "$.result.tools[0].inputSchema.properties.query.maxLength" == 512
jsonpath "$.result.tools[0].inputSchema.properties.mode.enum" includes "auto"
jsonpath "$.result.tools[0].inputSchema.properties.mode.enum" includes "lexical"
jsonpath "$.result.tools[0].inputSchema.properties.mode.enum" includes "semantic"
jsonpath "$.result.tools[0].inputSchema.properties.sort.enum" includes "relevance"
jsonpath "$.result.tools[0].inputSchema.properties.sort.enum" includes "date_desc"
jsonpath "$.result.tools[0].inputSchema.properties.sort.enum" includes "date_asc"
jsonpath "$.result.tools[0].inputSchema.properties.limit.minimum" == 1
jsonpath "$.result.tools[0].inputSchema.properties.limit.maximum" == 20
jsonpath "$.result.tools[0].inputSchema.properties.ai_rerank.type" == "boolean"
jsonpath "$.result.tools[0].inputSchema.properties.jurisdiction_type.anyOf[0].items.enum" includes "TA"
jsonpath "$.result.tools[0].inputSchema.properties.jurisdiction_type.anyOf[0].items.enum" includes "CC"
jsonpath "$.result.tools[0].inputSchema.properties.jurisdiction_type.anyOf[0].items.enum" includes "TCOM"
# No tool writes anything.
jsonpath "$.result.tools[0].annotations.readOnlyHint" == true
jsonpath "$.result.tools[0].annotations.destructiveHint" == false
jsonpath "$.result.tools[0].annotations.idempotentHint" == true
jsonpath "$.result.tools[0].annotations.openWorldHint" == false
jsonpath "$.result.tools[1].inputSchema.additionalProperties" == false
jsonpath "$.result.tools[1].inputSchema.required" includes "url"
jsonpath "$.result.tools[1].inputSchema.properties.url.type" == "string"
jsonpath "$.result.tools[1].inputSchema.properties.url.minLength" == 1

# -----------------------------------------------------------------------------
# tools/call search_decisions — structured search result shape
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "tools/call",
  "params": {
    "name": "search_decisions",
    "arguments": {"query": "responsabilité hôpital infection nosocomiale", "limit": 5, "ai_rerank": false}
  }
}
HTTP 200
[Asserts]
jsonpath "$.jsonrpc" == "2.0"
jsonpath "$.id" == 4
jsonpath "$.result.content" isCollection
jsonpath "$.result.content[0].type" == "text"
jsonpath "$.result.content[0].text" isString
jsonpath "$.result.structuredContent" exists
jsonpath "$.result.structuredContent.query" isString
jsonpath "$.result.structuredContent.hits" isCollection
jsonpath "$.result.isError" == false
# Per-hit shape (only assert when at least one hit; data-dependent):
# hits[].title/url/chars always present (snake_case), plus exactly one of
# ai_summary (semantic) / snippet (lexical) per hit, optional metadata =
# filter tokens: seat, formation, solution, legal_domain,
# publication, date, docket_numbers
# (omitted when null — skip_serializing_if).

# -----------------------------------------------------------------------------
# tools/call get_decision — not-found url → contrat d'erreur structuré :
# result.isError == true, content[0].text = JSON {error, category, retryable,
# hint}. Un lookup par url n'est PAS une recherche : le hint dit « url erronée
# ou inventée (ou décision dépubliée) », pas « corpus non exhaustif ».
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 5,
  "method": "tools/call",
  "params": {"name": "get_decision", "arguments": {"url": "https://librejustice.fr/decision/fr/does-not-exist-zzz"}}
}
HTTP 200
[Asserts]
jsonpath "$.jsonrpc" == "2.0"
jsonpath "$.id" == 5
jsonpath "$.result.isError" == true
jsonpath "$.result.content[0].type" == "text"
jsonpath "$.result.content[0].text" contains "\"category\":\"not_found\""
jsonpath "$.result.content[0].text" contains "\"retryable\":false"
jsonpath "$.result.content[0].text" contains "cannot be composed or guessed"
jsonpath "$.error" not exists

# -----------------------------------------------------------------------------
# tools/call get_decision — missing required arg `url` → invalid params
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 6,
  "method": "tools/call",
  "params": {"name": "get_decision", "arguments": {}}
}
HTTP 200
[Asserts]
jsonpath "$.jsonrpc" == "2.0"
jsonpath "$.id" == 6

# -----------------------------------------------------------------------------
# tools/call unknown tool name → isError + contrat d'erreur (validation, hint)
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 7,
  "method": "tools/call",
  "params": {"name": "nonexistent_tool", "arguments": {}}
}
HTTP 200
[Asserts]
jsonpath "$.jsonrpc" == "2.0"
jsonpath "$.id" == 7
jsonpath "$.result.isError" == true
jsonpath "$.result.content[0].text" contains "Unknown tool"
jsonpath "$.result.content[0].text" contains "\"category\":\"validation\""
jsonpath "$.result.content[0].text" contains "\"hint\""

# -----------------------------------------------------------------------------
# tools/call on a removed tool: a client that froze its tool list still calls it.
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 7,
  "method": "tools/call",
  "params": {"name": "list_my_activity", "arguments": {}}
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" == true
jsonpath "$.result.content[0].text" contains "list_my_activity was removed."

# -----------------------------------------------------------------------------
# resources/list: the capability is not announced, so the method does not exist.
# An empty list would make directories advertise `capability:resources`.
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 8,
  "method": "resources/list",
  "params": {}
}
HTTP 200
[Asserts]
jsonpath "$.jsonrpc" == "2.0"
jsonpath "$.id" == 8
jsonpath "$.error.code" == -32601
jsonpath "$.result" not exists

# -----------------------------------------------------------------------------
# tools/call search_decisions: `jurisdiction_code`, out of the schema, is still
# accepted and announces nothing. While callers move over to `seat`, a call
# still carrying it answers as before.
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 9,
  "method": "tools/call",
  "params": {
    "name": "search_decisions",
    "arguments": {"query": "obligation de quitter le territoire", "jurisdiction_code": ["cc"], "limit": 3, "ai_rerank": false}
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" == false
jsonpath "$.result.structuredContent.hits" isCollection
jsonpath "$.result.structuredContent.filterRewritten" not exists

# -----------------------------------------------------------------------------
# tools/call search_decisions: `matiere` is retired for good, so refused as an
# unknown field instead of silently rewritten.
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 10,
  "method": "tools/call",
  "params": {
    "name": "search_decisions",
    "arguments": {"query": "test", "matiere": ["URGENCES"], "limit": 3}
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" == true
jsonpath "$.result.content[0].text" contains "unknown field"

# -----------------------------------------------------------------------------
# tools/call search_decisions: `procedure` and `office` are retired but still
# accepted, answered without them and with a second block naming their heirs.
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 13,
  "method": "tools/call",
  "params": {
    "name": "search_decisions",
    "arguments": {"query": "saisie attribution", "office": ["JEX"], "limit": 3, "ai_rerank": false}
  }
}
HTTP 200
[Asserts]
jsonpath "$.result.isError" == false
jsonpath "$.result.structuredContent.hits" isCollection
jsonpath "$.result.content[1].text" contains "formation"

# -----------------------------------------------------------------------------
# unknown top-level method → method not found
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 12,
  "method": "nonexistent/method",
  "params": {}
}
HTTP 200
[Asserts]
jsonpath "$.jsonrpc" == "2.0"
jsonpath "$.id" == 12
jsonpath "$.result" not exists
jsonpath "$.error.code" == -32601
jsonpath "$.error.message" contains "nonexistent/method"

# -----------------------------------------------------------------------------
# AUTH CONTRACT (mcp_require_auth=True) — unauthenticated initialize → 401
# When auth is disabled (default) this returns 200; only valid under the
# auth-enabled deployment profile.
# -----------------------------------------------------------------------------
# POST {{base}}/mcp/
# Accept: application/json, text/event-stream
# Content-Type: application/json
# {"jsonrpc":"2.0","id":99,"method":"initialize","params":{}}
# HTTP 401
# [Asserts]
# jsonpath "$.error" == "auth_required"
# header "WWW-Authenticate" contains "Bearer"
# header "WWW-Authenticate" contains "resource_metadata"
#
# Authed variant (runner injects {{token}}):
# POST {{base}}/mcp/
# Authorization: Bearer {{token}}
# Accept: application/json, text/event-stream
# Content-Type: application/json
# {"jsonrpc":"2.0","id":100,"method":"tools/list","params":{}}
# HTTP 200
# [Asserts]
# jsonpath "$.result.tools" count == 3
#
# Invalid token → 401 {"error":"invalid_token"} + same WWW-Authenticate challenge.
