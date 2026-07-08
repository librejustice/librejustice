# Smoke test — endpoints sans DB
# Usage : hurl --variable api_url=http://localhost:8300 tests/smoke/health.hurl --test

GET {{api_url}}/api/health
HTTP 200
[Asserts]
jsonpath "$.status" == "ok"
jsonpath "$.version" isString
