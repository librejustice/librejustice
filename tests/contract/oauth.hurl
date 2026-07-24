# =============================================================================
# OAuth 2.1 (Authorization Code + PKCE S256 + DCR) — contract tests
# Oracle: apps/api/src/librejustice_api/oauth.py + main.py (well-known)
# Rust:   packages/lj-api/src/oauth.rs
# Host root via {{base}} (e.g. GET {{base}}/api/health)
# =============================================================================

# -----------------------------------------------------------------------------
# 1. AS metadata (RFC 8414) — static JSON, EXACT field parity expected
# -----------------------------------------------------------------------------
GET {{base}}/.well-known/oauth-authorization-server
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.issuer" exists
jsonpath "$.issuer" isString
jsonpath "$.authorization_endpoint" exists
jsonpath "$.authorization_endpoint" endsWith "/oauth/authorize"
jsonpath "$.token_endpoint" exists
jsonpath "$.token_endpoint" endsWith "/oauth/token"
jsonpath "$.registration_endpoint" exists
jsonpath "$.registration_endpoint" endsWith "/oauth/register"
jsonpath "$.response_types_supported" isCollection
jsonpath "$.response_types_supported" count == 1
jsonpath "$.response_types_supported[0]" == "code"
jsonpath "$.grant_types_supported" count == 2
jsonpath "$.grant_types_supported[0]" == "authorization_code"
jsonpath "$.grant_types_supported[1]" == "refresh_token"
jsonpath "$.code_challenge_methods_supported" count == 1
jsonpath "$.code_challenge_methods_supported[0]" == "S256"
jsonpath "$.token_endpoint_auth_methods_supported" count == 1
jsonpath "$.token_endpoint_auth_methods_supported[0]" == "none"
jsonpath "$.scopes_supported" count == 1
jsonpath "$.scopes_supported[0]" == "mcp"

# -----------------------------------------------------------------------------
# 2. Protected Resource metadata (RFC 9728) — root path
# -----------------------------------------------------------------------------
GET {{base}}/.well-known/oauth-protected-resource
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.resource" exists
jsonpath "$.resource" endsWith "/mcp"
jsonpath "$.authorization_servers" isCollection
jsonpath "$.authorization_servers" count == 1
jsonpath "$.authorization_servers[0]" isString
jsonpath "$.scopes_supported" count == 1
jsonpath "$.scopes_supported[0]" == "mcp"
jsonpath "$.bearer_methods_supported" count == 1
jsonpath "$.bearer_methods_supported[0]" == "header"

# -----------------------------------------------------------------------------
# 3. Protected Resource metadata — /mcp path variant (Claude.ai connector)
#    Must serve the SAME document as the root path.
# -----------------------------------------------------------------------------
GET {{base}}/.well-known/oauth-protected-resource/mcp
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.resource" endsWith "/mcp"
jsonpath "$.authorization_servers" count == 1
jsonpath "$.scopes_supported[0]" == "mcp"
jsonpath "$.bearer_methods_supported[0]" == "header"

# -----------------------------------------------------------------------------
# 4. POST /oauth/register — minimal valid body (RFC 7591 DCR)
#    Public client; response is RegisterResponse (snake_case fields).
# -----------------------------------------------------------------------------
POST {{base}}/oauth/register
Content-Type: application/json
{
  "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"],
  "client_name": "Claude Connector"
}
HTTP 201
[Asserts]
jsonpath "$.client_id" exists
jsonpath "$.client_id" isString
jsonpath "$.client_id" startsWith "lj_"
jsonpath "$.client_id_issued_at" exists
jsonpath "$.client_id_issued_at" isInteger
jsonpath "$.redirect_uris" isCollection
jsonpath "$.redirect_uris" count == 1
jsonpath "$.redirect_uris[0]" == "https://claude.ai/api/mcp/auth_callback"
jsonpath "$.client_name" == "Claude Connector"
jsonpath "$.grant_types" count == 2
jsonpath "$.grant_types[0]" == "authorization_code"
jsonpath "$.grant_types[1]" == "refresh_token"
jsonpath "$.response_types" count == 1
jsonpath "$.response_types[0]" == "code"
jsonpath "$.token_endpoint_auth_method" == "none"

# 4b. register — localhost loopback redirect (allowed prefix)
POST {{base}}/oauth/register
Content-Type: application/json
{
  "redirect_uris": ["http://localhost:8080/cb", "http://127.0.0.1:9000/cb"]
}
HTTP 201
[Asserts]
jsonpath "$.client_id" startsWith "lj_"
jsonpath "$.redirect_uris" count == 2
# client_name omitted in request -> null in response (Option<String>=None / None)
jsonpath "$.client_name" == null
jsonpath "$.token_endpoint_auth_method" == "none"

# 4c. register — invalid redirect scheme (http non-loopback) -> 400 invalid_redirect_uri
POST {{base}}/oauth/register
Content-Type: application/json
{
  "redirect_uris": ["http://evil.example.com/cb"]
}
HTTP 400

# 4d. register — empty redirect_uris.
#   DIVERGENCE: Python Field(min_length=1) -> FastAPI 422 (validation).
#   Rust accepts empty Vec then branch returns 400 invalid_redirect_uri.
#   Status differs (422 vs 400); differ should flag. Asserting it is non-2xx.
POST {{base}}/oauth/register
Content-Type: application/json
{
  "redirect_uris": []
}
HTTP *
[Asserts]
status >= 400

# -----------------------------------------------------------------------------
# 5. GET /oauth/authorize — branch coverage on validation, all pre-DB checks
# -----------------------------------------------------------------------------
# 5a. unsupported response_type -> 400 unsupported_response_type
GET {{base}}/oauth/authorize?response_type=token&client_id=lj_x&redirect_uri=https://claude.ai/cb&code_challenge=abc
HTTP 400

# 5b. unsupported code_challenge_method (plain) -> 400 unsupported_code_challenge_method
GET {{base}}/oauth/authorize?response_type=code&client_id=lj_x&redirect_uri=https://claude.ai/cb&code_challenge=abc&code_challenge_method=plain
HTTP 400

# 5c. unknown client / unregistered redirect -> 400 invalid_client_or_redirect_uri
GET {{base}}/oauth/authorize?response_type=code&client_id=lj_does_not_exist&redirect_uri=https://claude.ai/cb&code_challenge=abc
HTTP 400

# 5d. missing required query param (no code_challenge).
#   DIVERGENCE: Python (required query arg) -> 422; Rust Query reject -> 400.
GET {{base}}/oauth/authorize?response_type=code&client_id=lj_x&redirect_uri=https://claude.ai/cb
HTTP *
[Asserts]
status >= 400

# -----------------------------------------------------------------------------
# 6. POST /oauth/token — form-urlencoded; branch coverage (pre-DB + DB lookup)
# -----------------------------------------------------------------------------
# 6a. unsupported grant_type -> 400 unsupported_grant_type
POST {{base}}/oauth/token
Content-Type: application/x-www-form-urlencoded
[FormParams]
grant_type: client_credentials
code: x
code_verifier: x
client_id: lj_x
redirect_uri: https://claude.ai/cb
HTTP 400

# 6b. authorization_code with unknown/expired code -> 400 invalid_grant
POST {{base}}/oauth/token
Content-Type: application/x-www-form-urlencoded
[FormParams]
grant_type: authorization_code
code: nonexistent_code_value
code_verifier: dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk
client_id: lj_x
redirect_uri: https://claude.ai/cb
HTTP 400

# 6c. missing required form field (no redirect_uri).
#   DIVERGENCE: Python Form(...) required -> 422; Rust Form reject -> 400/415.
POST {{base}}/oauth/token
Content-Type: application/x-www-form-urlencoded
[FormParams]
grant_type: authorization_code
code: x
code_verifier: x
client_id: lj_x
HTTP *
[Asserts]
status >= 400

# 6d. refresh_token grant with unknown/expired refresh token -> 400 invalid_grant
POST {{base}}/oauth/token
Content-Type: application/x-www-form-urlencoded
[FormParams]
grant_type: refresh_token
refresh_token: nonexistent_refresh_token_value
client_id: lj_x
HTTP 400

# 6e. refresh_token grant missing refresh_token field -> non-2xx (422 missing).
POST {{base}}/oauth/token
Content-Type: application/x-www-form-urlencoded
[FormParams]
grant_type: refresh_token
client_id: lj_x
HTTP *
[Asserts]
status >= 400

# -----------------------------------------------------------------------------
# 7. POST /oauth/approve — auth=jwt. Unauthenticated 401 contract.
#   Python: HTTPException(401, detail="auth_required") -> {"detail":"auth_required"}
#   Rust:   ApiError::Unauthorized -> 401 plain-text body "unauthorized"
#   DIVERGENCE: status parity OK (401), body shape DIVERGES. Asserting status only.
# -----------------------------------------------------------------------------
POST {{base}}/oauth/approve
Content-Type: application/json
{
  "client_id": "lj_x",
  "code_challenge": "abc",
  "redirect_uri": "https://claude.ai/cb"
}
HTTP 401

# --- AUTHED assertions (runner injects {{token}}) ---------------------------
# With a valid Supabase bearer token AND a registered client whose redirect_uri
# matches, /oauth/approve returns 200 with:
#   jsonpath "$.code" isString          (secrets.token_urlsafe(32) -> 43 chars)
#   jsonpath "$.redirect_uri" == <the request redirect_uri>
# With a valid token but UNREGISTERED client/redirect:
#   HTTP 400  (invalid_client_or_redirect_uri)
# Example (commented; uncomment in authed run):
# POST {{base}}/oauth/approve
# Authorization: Bearer {{token}}
# Content-Type: application/json
# { "client_id": "lj_x", "code_challenge": "abc", "redirect_uri": "https://claude.ai/cb" }
# HTTP 400
