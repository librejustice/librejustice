#!/usr/bin/env python3
"""LibreJustice MCP server over stdio, backed by the public REST API.

The tool surface is read from `mcp-tools.json`, which the hosted server
serialises from its own definitions, so the two servers describe their tools
with the same words. Each tool maps onto one route of `/api`, which answers in
the shape those definitions declare.

`LIBREJUSTICE_API_KEY` is created from https://librejustice.fr/profile. A
directory can enumerate the tools without one; a call needs it.
"""

import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

SERVER_NAME = "librejustice"
VERSION = "0.1.0"

SURFACE = Path(__file__).resolve().parent.parent / "packages/lj-mcp-tools/data/mcp-tools.json"

_surface = json.loads(SURFACE.read_text(encoding="utf-8"))
PROTOCOL_VERSIONS = _surface["protocolVersions"]


class MethodNotFound(Exception):
    """JSON-RPC -32601, where a bad argument is -32602 and a failed call -32603."""


class UnknownTool(Exception):
    pass

# The query keys are the tool argument names; a list is sent once per value.
SEARCH_LISTS = (
    "jurisdiction_type",
    "jurisdiction_code",
    "legal_domain",
    "legal_instrument",
    "legal_article",
    "solution",
    "seat",
    "formation",
    "publication",
    "search_in",
)
SEARCH_SCALARS = ("query", "date_from", "date_to", "mode", "sort", "limit", "ai_rerank")
TEXTS_SCALARS = ("query", "code", "jurisdiction", "valid_from", "as_of", "limit")


def as_text(value):
    if isinstance(value, bool):
        return "1" if value else "0"
    return str(value)


def scalars(args, names):
    return [(name, as_text(args[name])) for name in names if args.get(name) not in (None, "")]


def search_decisions(args):
    params = scalars(args, SEARCH_SCALARS)
    for name in SEARCH_LISTS:
        for value in args.get(name) or []:
            params.append((name, as_text(value)))
    return "/api/search", params


def search_norms(args):
    return "/api/search-textes", scalars(args, TEXTS_SCALARS)


def get_decision(args):
    url = args.get("url") or ""
    _, sep, tail = url.rpartition("/decision/")
    key = tail.strip("/").rpartition("/")[2]
    if not sep or not key:
        raise ValueError(
            f"not a decision url (expected …/decision/{{jurisdiction}}/{{id}}): {url or '(missing)'}"
        )
    return f"/api/decision/{urllib.parse.quote(key, safe='')}", []


def get_norm(args):
    as_of = [("as_of", args["as_of"])] if args.get("as_of") else []
    return legal_text_path(args), as_of


# Today's prefix first; the two older ones still name a text.
LEGAL_TEXT_PREFIXES = ("/norm/", "/texte/", "/loi/")

LEGIFRANCE_ID = re.compile(r"(LEGIARTI|JORFARTI|KALIARTI|LEGITEXT|JORFTEXT|LEGISCTA)[0-9]+$")


def legal_text_path(args):
    """The four doors of `get_norm`, in the order it reads them."""
    url = args.get("url") or ""
    ident = args.get("text_id") or ""
    if LEGIFRANCE_ID.match(url) or ident:
        return f"/api/article/{urllib.parse.quote(ident or url, safe='')}"
    if url:
        tail = next((url.rpartition(p)[2] for p in LEGAL_TEXT_PREFIXES if p in url), "")
        if not tail.strip("/"):
            raise ValueError(
                f"not a legal text url (expected …/norm/{{jurisdiction}}/{{code}}[/{{article}}]): {url}"
            )
        return "/api/norm/" + tail.strip("/")
    code = args.get("code")
    if not code:
        raise ValueError(
            "missing tool argument: url. Address the text by url "
            "(/norm/{jurisdiction}/{code}/{article}), by the pair {code, article}, or by a "
            "Légifrance id."
        )
    path = "/api/norm/" + urllib.parse.quote(code, safe="")
    article = args.get("article")
    return f"{path}/{urllib.parse.quote(article, safe='')}" if article else path


# A tool absent here is absent from `tools/list`.
ROUTES = {
    "search_decisions": search_decisions,
    "search_norms": search_norms,
    "get_decision": get_decision,
    "get_norm": get_norm,
}


def dispatch(request, settings):
    method = request.get("method", "")
    if method == "initialize":
        asked = (request.get("params") or {}).get("protocolVersion")
        return {
            "protocolVersion": asked if asked in PROTOCOL_VERSIONS else PROTOCOL_VERSIONS[0],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": SERVER_NAME, "version": VERSION},
            "instructions": _surface["instructions"],
        }
    if method == "ping":
        return {}
    if method == "tools/list":
        return {"tools": [t for t in _surface["tools"] if t["name"] in ROUTES]}
    if method == "tools/call":
        return call(request.get("params") or {}, settings)
    raise MethodNotFound(f"method not found: {method}")


def call(params, settings):
    name = params.get("name", "")
    route = ROUTES.get(name)
    if route is None:
        raise UnknownTool(f"Unknown tool: {name}")
    path, query = route(params.get("arguments") or {})
    payload = fetch(path, query, settings)
    return {
        "content": [{"type": "text", "text": json.dumps(payload, ensure_ascii=False)}],
        "structuredContent": payload,
        "isError": False,
    }


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """`urllib` strips only the content headers on a redirect, so it would carry
    the key to whatever host answers. The API serves its routes directly."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


def fetch(path, query, settings):
    if not settings["api_key"]:
        raise ValueError(
            "no API key: set LIBREJUSTICE_API_KEY, created from https://librejustice.fr/profile"
        )
    url = settings["base_url"] + path
    if query:
        url += "?" + urllib.parse.urlencode(query)
    asked = urllib.request.Request(
        url,
        headers={
            "authorization": f"Bearer {settings['api_key']}",
            "accept": "application/json",
            "user-agent": f"lj-mcp/{VERSION}",
        },
    )
    try:
        with OPENER.open(asked, timeout=120) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        raise ValueError(f"{error.code}: {error.read().decode('utf-8', 'replace')}") from error


def main():
    if sys.stdin.isatty():
        sys.exit(
            "lj-mcp speaks JSON-RPC on stdin: set LIBREJUSTICE_API_KEY and let a client drive it"
        )
    settings = {
        "api_key": os.environ.get("LIBREJUSTICE_API_KEY", ""),
        "base_url": os.environ.get("LIBREJUSTICE_API_BASE_URL", "https://librejustice.fr").rstrip(
            "/"
        ),
    }
    for raw in sys.stdin:
        line = raw.strip()
        if not line:
            continue
        request = json.loads(line)
        if "id" not in request:
            continue
        try:
            body = {"jsonrpc": "2.0", "id": request["id"], "result": dispatch(request, settings)}
        except Exception as error:
            code = {MethodNotFound: -32601, UnknownTool: -32602}.get(type(error), -32603)
            body = {
                "jsonrpc": "2.0",
                "id": request["id"],
                "error": {"code": code, "message": str(error)},
            }
        print(json.dumps(body, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
