# ── security headers contract ────────────────────────────────────────────────
# Source oracle: apps/lj-server/src/main.rs (async fn security_headers)
#
# These headers are set by a middleware nothing else tests, and the site
# answers 200 with or without them. This contract runs against the live target.

GET {{base}}/admin/health
HTTP 200
[Asserts]
# HSTS un an, sans includeSubDomains ni preload (irrévocables, cf. doc du
# middleware). Posé sans condition côté origine ; le navigateur ne le voit
# jamais qu'en HTTPS.
header "strict-transport-security" contains "max-age=31536000"
header "x-content-type-options" == "nosniff"
header "x-frame-options" == "DENY"
header "referrer-policy" == "strict-origin-when-cross-origin"
header "permissions-policy" contains "geolocation=()"
# Socle CSP des réponses sans shell SSR (API, /pkg, /fonts, exports).
header "content-security-policy" contains "frame-ancestors 'none'"

# Même socle sur une page HTML, où la CSP complète du document est posée par le
# shell SSR avec son nonce : c'est ELLE qui doit répondre ici, pas le socle.
GET {{base}}/
HTTP 200
[Asserts]
header "strict-transport-security" contains "max-age=31536000"
header "x-content-type-options" == "nosniff"
header "content-security-policy" contains "nonce-"
# The identity detour navigates to our own `/internal/auth/google/start`: no
# third-party origin is expected here, and one reappearing is worth explaining.
header "content-security-policy" not contains "accounts.google.com"
header "content-security-policy" contains "connect-src 'self';"
