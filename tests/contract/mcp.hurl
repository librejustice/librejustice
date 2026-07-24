# =============================================================================
# MCP over Streamable HTTP at /mcp  (JSON-RPC 2.0)
# Contract tests for LibreJustice — Python FastMCP oracle vs Rust axum hand-rolled.
#
# Transport notes (READ FIRST — major parity risk):
#  - Python mounts the official MCP SDK FastMCP app (streamable_http_path="/",
#    stateless_http=True, json_response=True). That transport ENFORCES headers:
#       Accept: application/json, text/event-stream
#       Content-Type: application/json
#    and emits/echoes Mcp-Session-Id semantics. Missing/incorrect Accept on the
#    Python side yields a 406 / protocol error from the SDK BEFORE any handler runs.
#  - Rust mcp_router is a bare `POST /` JSON handler: it ignores Accept entirely,
#    accepts any Content-Type axum can parse as JSON, and never sets Mcp-Session-Id.
#  - Therefore these contract tests send the SDK-correct Accept header so the
#    Python oracle answers; the A/B differ must tolerate transport-envelope/header
#    differences and compare only the JSON-RPC `result`/`error` bodies.
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
jsonpath "$.result.protocolVersion" isString
jsonpath "$.result.capabilities" exists
jsonpath "$.result.capabilities.tools" exists
jsonpath "$.result.serverInfo" exists
jsonpath "$.result.serverInfo.name" == "librejustice"
jsonpath "$.result.serverInfo.version" isString
jsonpath "$.result.instructions" isString
jsonpath "$.result.instructions" contains "French case law"
jsonpath "$.error" not exists
# DIVERGENCE: Python protocolVersion is SDK-negotiated (e.g. "2025-06-18"),
# Rust hardcodes "2024-11-05". serverInfo.version: SDK version vs CARGO_PKG_VERSION.

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
# tools/list — five tools, exact names, ordering, strict inputSchema
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
jsonpath "$.result.tools" count == 5
jsonpath "$.result.tools[0].name" == "search_decisions"
jsonpath "$.result.tools[1].name" == "get_decision"
jsonpath "$.result.tools[2].name" == "list_my_activity"
jsonpath "$.result.tools[3].name" == "get_legal_text"
jsonpath "$.result.tools[4].name" == "search_legal_texts"
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
# Tous les outils sont read-only (le log d'activité est une journalisation
# privée du compte, cf. read_only_annotations dans mcp.rs).
jsonpath "$.result.tools[0].annotations.readOnlyHint" == true
jsonpath "$.result.tools[0].annotations.destructiveHint" == false
jsonpath "$.result.tools[0].annotations.idempotentHint" == true
jsonpath "$.result.tools[0].annotations.openWorldHint" == false
jsonpath "$.result.tools[1].inputSchema.additionalProperties" == false
jsonpath "$.result.tools[1].inputSchema.required" includes "url"
jsonpath "$.result.tools[1].inputSchema.properties.url.type" == "string"
jsonpath "$.result.tools[1].inputSchema.properties.url.minLength" == 1
jsonpath "$.result.tools[2].inputSchema.additionalProperties" == false
jsonpath "$.result.tools[2].inputSchema.properties.kind.enum" includes "searches"
jsonpath "$.result.tools[2].inputSchema.properties.kind.enum" includes "bookmarks"
jsonpath "$.result.tools[2].inputSchema.properties.kind.enum" includes "readingHistory"
jsonpath "$.result.tools[2].inputSchema.properties.kind.enum" includes "all"
jsonpath "$.result.tools[2].inputSchema.properties.limit.maximum" == 100
# DIVERGENCE: Python registers tools with title="Search Decisions" etc.;
# _postprocess only strips `title` INSIDE inputSchema, leaving the tool-level
# `title` field present. Rust omits tool-level `title` entirely. The differ
# must flag tools[].title presence (Python) vs absence (Rust).

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
# hits[].title/url/chars/jurisdictionType always present (camelCase), plus
# exactly one of aiSummary (semantic) / snippet (lexical) per hit,
# optional metadata = filter tokens: jurisdictionCode, chamber, solution,
# procedure, office, legalDomain, publication, dateLecture, docketNumbers
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
  "params": {"name": "get_decision", "arguments": {"url": "https://librejustice.fr/decision/does-not-exist-zzz"}}
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
# Rust: top-level error code -32602 "missing tool argument: url".
# Python: FastMCP validation error → result.isError true (SDK-shaped message).

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
# resources/list — liste vide assumée (primitive application-controlled sans
# consommateur réel côté clients ; cf. ADR 0244).
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
jsonpath "$.result.resources" count == 0
jsonpath "$.error" not exists

# -----------------------------------------------------------------------------
# prompts/list + prompts/get — workflows invocables par l'utilisateur
# -----------------------------------------------------------------------------
POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 10,
  "method": "prompts/list",
  "params": {}
}
HTTP 200
[Asserts]
jsonpath "$.result.prompts" count == 3
jsonpath "$.result.prompts[0].name" == "recherche_jurisprudence"
jsonpath "$.result.prompts[1].name" == "article_a_date"
jsonpath "$.result.prompts[2].name" == "textes_applicables"
jsonpath "$.result.prompts[0].arguments[0].name" == "question"
jsonpath "$.result.prompts[0].arguments[0].required" == true
jsonpath "$.error" not exists

POST {{base}}/mcp/
Accept: application/json, text/event-stream
Content-Type: application/json
{
  "jsonrpc": "2.0",
  "id": 11,
  "method": "prompts/get",
  "params": {"name": "article_a_date", "arguments": {"article": "article 1240 du code civil", "date": "2018-01-01"}}
}
HTTP 200
[Asserts]
jsonpath "$.result.messages[0].role" == "user"
jsonpath "$.result.messages[0].content.type" == "text"
jsonpath "$.result.messages[0].content.text" contains "article 1240 du code civil"
jsonpath "$.result.messages[0].content.text" contains "2018-01-01"
jsonpath "$.error" not exists

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
