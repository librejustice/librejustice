"""What the mapping onto `/api` has to hold: one route per tool, four doors on `get_norm`, and the tool arguments sent under their own names."""

import json
from pathlib import Path

import pytest

import lj_mcp

OPENAPI = Path(lj_mcp.__file__).resolve().parent.parent / "packages/lj-web/public/openapi.json"

DOCUMENTED = {
    "/api/search": "search_decisions",
    "/api/search-textes": "search_norms",
    "/api/decision/{id}": "get_decision",
    "/api/norm/{jurisdiction}/{code}": "get_norm",
    "/api/norm/{jurisdiction}/{code}/{article}": "get_norm",
    "/api/article/{uid}": "get_norm",
}


def test_the_served_tools_are_the_ones_mapped():
    listed = {t["name"] for t in lj_mcp.dispatch({"method": "tools/list"}, {})["tools"]}
    assert listed == set(lj_mcp.ROUTES)


def test_the_instructions_come_from_the_shared_surface():
    initialized = lj_mcp.dispatch({"method": "initialize"}, {})
    assert initialized["instructions"] == lj_mcp._surface["instructions"]
    assert initialized["serverInfo"]["name"] == "librejustice"


def test_a_search_sends_its_scalars_and_repeats_its_filters():
    path, query = lj_mcp.search_decisions(
        {
            "query": "trouble de voisinage",
            "date_from": "2020-01-01",
            "jurisdiction_type": ["CA", "TJ"],
            "legal_article": ["code-civil/1240"],
            "limit": 5,
        }
    )
    assert path == "/api/search"
    assert query == [
        ("query", "trouble de voisinage"),
        ("date_from", "2020-01-01"),
        ("limit", "5"),
        ("jurisdiction_type", "CA"),
        ("jurisdiction_type", "TJ"),
        ("legal_article", "code-civil/1240"),
    ]


def test_a_refused_rerank_is_sent_since_the_route_reranks_by_default():
    _, off = lj_mcp.search_decisions({"query": "x", "ai_rerank": False})
    assert ("ai_rerank", "0") in off


def test_a_decision_is_addressed_by_the_url_of_a_hit():
    path, query = lj_mcp.get_decision({"url": "https://librejustice.fr/decision/fr/HrsWxKX8KYyC"})
    assert (path, query) == ("/api/decision/HrsWxKX8KYyC", [])
    assert lj_mcp.get_decision({"url": "/decision/HrsWxKX8KYyC"})[0] == "/api/decision/HrsWxKX8KYyC"
    with pytest.raises(ValueError, match="not a decision url"):
        lj_mcp.get_decision({"url": "/norm/fr/code-civil/1240"})


@pytest.mark.parametrize(
    ("args", "expected"),
    [
        ({"url": "/norm/fr/code-civil/1240"}, "/api/norm/fr/code-civil/1240"),
        ({"url": "https://librejustice.fr/norm/fr/code-civil"}, "/api/norm/fr/code-civil"),
        ({"url": "/norm/fr/code-civil/livre-i/titre-v"}, "/api/norm/fr/code-civil/livre-i/titre-v"),
        (
            {"url": "/norm/fr/code-civil/section/LEGISCTA000006117617"},
            "/api/norm/fr/code-civil/section/LEGISCTA000006117617",
        ),
        # The older shapes still resolve, so the door passes them through.
        ({"url": "/texte/fr/code-civil/1240"}, "/api/norm/fr/code-civil/1240"),
        ({"url": "/texte/code-civil/1240"}, "/api/norm/code-civil/1240"),
        ({"code": "code civil", "article": "L. 121-1"}, "/api/norm/code%20civil/L.%20121-1"),
        ({"code": "cgi"}, "/api/norm/cgi"),
        ({"text_id": "LEGIARTI000006419282"}, "/api/article/LEGIARTI000006419282"),
        ({"url": "LEGIARTI000006419282"}, "/api/article/LEGIARTI000006419282"),
        ({"url": "LEGITEXT000006070721"}, "/api/article/LEGITEXT000006070721"),
    ],
)
def test_the_four_doors_of_a_legal_text(args, expected):
    assert lj_mcp.legal_text_path(args) == expected


def test_a_legal_text_needs_a_designation():
    with pytest.raises(ValueError, match="missing tool argument: url"):
        lj_mcp.legal_text_path({"as_of": "2020-01-01"})


def test_a_date_rides_along_whichever_door_was_used():
    asked = lj_mcp.get_norm({"code": "code-civil", "article": "1240", "as_of": "2015-06-01"})
    assert asked == ("/api/norm/code-civil/1240", [("as_of", "2015-06-01")])


def test_a_call_without_a_key_names_where_to_get_one():
    with pytest.raises(ValueError, match="librejustice.fr/profile"):
        lj_mcp.call(
            {"name": "search_decisions", "arguments": {"query": "x"}},
            {"api_key": "", "base_url": "https://librejustice.fr"},
        )


def test_an_unknown_tool_is_refused_before_the_network():
    with pytest.raises(lj_mcp.UnknownTool, match="Unknown tool: no_such_tool"):
        lj_mcp.call({"name": "no_such_tool"}, {"api_key": "k", "base_url": "x"})


def test_an_unknown_method_is_told_apart_from_a_failed_call():
    with pytest.raises(lj_mcp.MethodNotFound, match="method not found: resources/list"):
        lj_mcp.dispatch({"method": "resources/list"}, {"api_key": "k", "base_url": "x"})


@pytest.mark.parametrize(
    ("asked", "answered"),
    [("2025-06-18", "2025-06-18"), ("2024-11-05", "2024-11-05"), ("1.0.0", None), (None, None)],
)
def test_the_protocol_revision_answers_the_client_when_we_speak_it(asked, answered):
    result = lj_mcp.dispatch(
        {"method": "initialize", "params": {"protocolVersion": asked}},
        {"api_key": "k", "base_url": "x"},
    )
    assert result["protocolVersion"] == (answered or lj_mcp.PROTOCOL_VERSIONS[0])


def test_the_rest_contract_answers_with_the_tool_output_schemas():
    paths = json.loads(OPENAPI.read_text(encoding="utf-8"))["paths"]
    schemas = {t["name"]: t.get("outputSchema") for t in lj_mcp._surface["tools"]}
    assert set(paths) == set(DOCUMENTED)
    assert set(DOCUMENTED.values()) == set(lj_mcp.ROUTES)
    for path, tool in DOCUMENTED.items():
        served = paths[path]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
        assert served == schemas[tool], path


def test_the_rest_searches_take_every_tool_argument_under_its_name():
    paths = json.loads(OPENAPI.read_text(encoding="utf-8"))["paths"]
    tools = {t["name"]: t["inputSchema"]["properties"] for t in lj_mcp._surface["tools"]}
    for path, tool in [("/api/search", "search_decisions"), ("/api/search-textes", "search_norms")]:
        params = {p["name"] for p in paths[path]["get"]["parameters"] if "name" in p}
        assert set(tools[tool]) <= params, path
    assert set(tools["search_decisions"]) <= set(lj_mcp.SEARCH_SCALARS) | set(lj_mcp.SEARCH_LISTS)
    assert set(tools["search_norms"]) <= set(lj_mcp.TEXTS_SCALARS)
