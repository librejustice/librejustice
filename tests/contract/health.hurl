# ── health route contract tests ──────────────────────────────────────────────
# Source oracle: apps/api/src/librejustice_api/main.py:308 (GET /api/health)
# Source Rust:   packages/lj-api/src/routes.rs:141 (async fn health)
# DTO: HealthResponse { status: Literal["ok"], version: str } camelCase.
# Public route (no auth). Single branch, no inputs.

# Happy path: 200 + {status:"ok", version:<string>}
GET {{base}}/api/health
HTTP 200
[Asserts]
header "Content-Type" contains "application/json"
jsonpath "$.status" exists
jsonpath "$.status" isString
jsonpath "$.status" == "ok"
jsonpath "$.version" exists
jsonpath "$.version" isString

# Idempotent: a second call returns the same structural contract.
GET {{base}}/api/health
HTTP 200
[Asserts]
jsonpath "$.status" == "ok"
jsonpath "$.version" isString

# Trailing slash: route is registered without trailing slash on both stacks.
# Documented divergence candidate (see notes) — assert status only, no body shape.
GET {{base}}/api/health/
HTTP *

# Wrong method on a GET-only route: both stacks should yield 405 Method Not Allowed.
POST {{base}}/api/health
HTTP 405

# HEAD is auto-derived from GET on both frameworks (200, empty body).
HEAD {{base}}/api/health
HTTP 200
