# ── modules fetched when a document is opened ────────────────────────────────
# Served outside the hydrate bundle, under /redactor: a missing or unreadable
# file only shows at the first document opened, /redact itself answering 200.

HEAD {{base}}/redactor/lj_redact.js
HTTP 200

HEAD {{base}}/redactor/lj_redact_bg.wasm
HTTP 200
[Asserts]
header "content-type" == "application/wasm"

HEAD {{base}}/redactor/htr/lj_htr.js
HTTP 200

# ── licences of the third-party files served ─────────────────────────────────
# One file per licence folder the /open-source page links to.

HEAD {{base}}/open-source
HTTP 200

HEAD {{base}}/fonts/geist-OFL.txt
HTTP 200

HEAD {{base}}/licenses/MIT.txt
HTTP 200

HEAD {{base}}/redactor/NOTICE.txt
HTTP 200

HEAD {{base}}/redactor/fonts/sources/tahoma.sfd
HTTP 200

HEAD {{base}}/redactor/cmaps/LICENSE.md
HTTP 200

HEAD {{base}}/redactor/htr/models/licenses/MIT.txt
HTTP 200

HEAD {{base}}/ocr/worker.min.js.LICENSE.txt
HTTP 200
