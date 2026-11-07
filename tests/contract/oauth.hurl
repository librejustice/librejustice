# =============================================================================
# OAuth 2.1 (Authorization Code + PKCE S256 + DCR) — contract tests
# Oracle: apps/api/src/librejustice_api/oauth.py + main.py (well-known)
# Rust:   packages/lj-api/src/oauth.rs
# Host root via {{base}} (e.g. GET {{base}}/admin/health)
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
jsonpath "$.revocation_endpoint" exists
jsonpath "$.revocation_endpoint" endsWith "/oauth/revoke"
jsonpath "$.revocation_endpoint_auth_methods_supported" count == 1
jsonpath "$.revocation_endpoint_auth_methods_supported[0]" == "none"
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
jsonpath "$.scopes_supported" count == 3
jsonpath "$.scopes_supported[0]" == "mcp"
jsonpath "$.scopes_supported[1]" == "openid"
jsonpath "$.scopes_supported[2]" == "email"
jsonpath "$.userinfo_endpoint" endsWith "/oauth/userinfo"
# The RFC 8414 name serves the OIDC document itself, whole: the OpenAI scanner
# reads this path and never `openid-configuration`, so the OIDC fields have to
# be here or they are nowhere for it.
jsonpath "$.subject_types_supported[0]" == "public"
jsonpath "$.id_token_signing_alg_values_supported[0]" == "ES256"
jsonpath "$.jwks_uri" endsWith "/.well-known/jwks.json"
jsonpath "$.claims_supported" includes "email_verified"

GET {{base}}/.well-known/oauth-authorization-server/mcp
HTTP 200
[Asserts]
jsonpath "$.userinfo_endpoint" endsWith "/oauth/userinfo"
jsonpath "$.jwks_uri" endsWith "/.well-known/jwks.json"

# -----------------------------------------------------------------------------
# 1bis. OpenID Connect Discovery. What a ChatGPT Enterprise workspace domain
#       restriction requires: the document, the scopes, and UserInfo.
# -----------------------------------------------------------------------------
GET {{base}}/.well-known/openid-configuration
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.issuer" isString
jsonpath "$.authorization_endpoint" endsWith "/oauth/authorize"
jsonpath "$.token_endpoint" endsWith "/oauth/token"
jsonpath "$.userinfo_endpoint" endsWith "/oauth/userinfo"
jsonpath "$.registration_endpoint" endsWith "/oauth/register"
jsonpath "$.scopes_supported" count == 3
jsonpath "$.scopes_supported[1]" == "openid"
jsonpath "$.scopes_supported[2]" == "email"
jsonpath "$.claims_supported" includes "email"
jsonpath "$.claims_supported" includes "email_verified"
jsonpath "$.subject_types_supported[0]" == "public"
jsonpath "$.id_token_signing_alg_values_supported[0]" == "ES256"
jsonpath "$.jwks_uri" endsWith "/.well-known/jwks.json"
jsonpath "$.code_challenge_methods_supported[0]" == "S256"

# A scanner trying the resource-prefixed shape first reads the same document,
# never a 404.
GET {{base}}/mcp/.well-known/openid-configuration
HTTP 200
[Asserts]
jsonpath "$.userinfo_endpoint" endsWith "/oauth/userinfo"
jsonpath "$.jwks_uri" endsWith "/.well-known/jwks.json"

GET {{base}}/.well-known/openid-configuration/mcp
HTTP 200
[Asserts]
jsonpath "$.userinfo_endpoint" endsWith "/oauth/userinfo"

# JWKS: one signing key, public material only.
GET {{base}}/.well-known/jwks.json
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.keys" count == 1
jsonpath "$.keys[0].kty" == "EC"
jsonpath "$.keys[0].crv" == "P-256"
jsonpath "$.keys[0].alg" == "ES256"
jsonpath "$.keys[0].use" == "sig"
jsonpath "$.keys[0].kid" isString
jsonpath "$.keys[0].x" isString
jsonpath "$.keys[0].y" isString
jsonpath "$.keys[0].d" not exists

# -----------------------------------------------------------------------------
# 1ter. UserInfo with no usable token: 401, cause in the challenge (RFC 6750 §3).
# -----------------------------------------------------------------------------
GET {{base}}/oauth/userinfo
HTTP 401
[Asserts]
header "WWW-Authenticate" contains "Bearer"
header "WWW-Authenticate" contains "error=\"invalid_request\""

GET {{base}}/oauth/userinfo
Authorization: Bearer no-such-token
HTTP 401
[Asserts]
header "WWW-Authenticate" contains "error=\"invalid_token\""

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
# The three scopes here too: ChatGPT reads this document, and a resource that
# lists `mcp` alone reads as a provider without identity.
jsonpath "$.scopes_supported" count == 3
jsonpath "$.scopes_supported[0]" == "mcp"
jsonpath "$.scopes_supported[1]" == "openid"
jsonpath "$.scopes_supported[2]" == "email"
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
# 3bis. Resource-prefixed discovery, `/mcp/.well-known/*`: the shape some MCP
#       clients try first. Same documents as the root paths.
# -----------------------------------------------------------------------------
GET {{base}}/mcp/.well-known/oauth-protected-resource
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.resource" endsWith "/mcp"
jsonpath "$.authorization_servers" count == 1
jsonpath "$.scopes_supported[0]" == "mcp"
jsonpath "$.bearer_methods_supported[0]" == "header"

GET {{base}}/mcp/.well-known/oauth-authorization-server
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.token_endpoint" endsWith "/oauth/token"
jsonpath "$.registration_endpoint" endsWith "/oauth/register"
jsonpath "$.grant_types_supported" count == 2

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
[Captures]
approve_client_id: jsonpath "$.client_id"
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

# 4d. register: empty redirect_uris -> non-2xx.
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

# 5d. missing required query param (no code_challenge) -> non-2xx.
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

# 6c. missing required form field (no redirect_uri) -> non-2xx.
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
# 7. POST /oauth/approve — formulaire de consentement.
#    Sans session, approve renvoie à la connexion, le consentement complet en
#    next ; deny retourne au client validé sans exiger de session ; un client
#    inconnu est refusé avant tout.
# -----------------------------------------------------------------------------
# 7a. approve sans session -> 303 /login?next=%2Fauthorize-mcp…
POST {{base}}/oauth/approve
Content-Type: application/x-www-form-urlencoded
[FormParams]
action: approve
client_id: {{approve_client_id}}
code_challenge: E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM
redirect_uri: https://claude.ai/api/mcp/auth_callback
state: xyz
HTTP 303
[Asserts]
header "Location" startsWith "/login?next=%2Fauthorize-mcp"

# 7b. deny -> 303 vers le redirect_uri validé, error=access_denied, state gardé
POST {{base}}/oauth/approve
Content-Type: application/x-www-form-urlencoded
[FormParams]
action: deny
client_id: {{approve_client_id}}
code_challenge: E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM
redirect_uri: https://claude.ai/api/mcp/auth_callback
state: xyz
HTTP 303
[Asserts]
header "Location" startsWith "https://claude.ai/api/mcp/auth_callback"
header "Location" contains "error=access_denied"
header "Location" contains "state=xyz"

# 7c. client inconnu -> 400 invalid_client_or_redirect_uri
POST {{base}}/oauth/approve
Content-Type: application/x-www-form-urlencoded
[FormParams]
action: approve
client_id: lj_does_not_exist
code_challenge: abc
redirect_uri: https://claude.ai/cb
HTTP 400

# -----------------------------------------------------------------------------
# 8. POST /oauth/revoke — form-urlencoded; RFC 7009. Public client, no client
#    auth: holding the token IS the credential.
# -----------------------------------------------------------------------------
# 8a. unknown token -> 200 with EMPTY body (RFC 7009 2.2: the response must not
#     tell the caller whether the token existed).
POST {{base}}/oauth/revoke
Content-Type: application/x-www-form-urlencoded
[FormParams]
token: nonexistent_access_token_value
HTTP 200
[Asserts]
bytes count == 0

# 8b. malformed token + token_type_hint (accepted and ignored) -> 200 empty
POST {{base}}/oauth/revoke
Content-Type: application/x-www-form-urlencoded
[FormParams]
token: not-a-token !!
token_type_hint: refresh_token
HTTP 200
[Asserts]
bytes count == 0

# 8c. missing token -> 400 invalid_request (RFC 6749 5.2)
POST {{base}}/oauth/revoke
Content-Type: application/x-www-form-urlencoded
[FormParams]
token_type_hint: access_token
HTTP 400
[Asserts]
jsonpath "$.detail" == "invalid_request"

# Le chemin approuvé (session + client enregistré -> 303 avec code, puis
# échange PKCE) est couvert par authed.hurl §4-5, qui porte la session.
