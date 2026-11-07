//! The MCP tool surface: names, descriptions, input and output schemas.
//!
//! Shared so the remote server and any standalone build expose one definition
//! of every tool, never two that drift.

use serde_json::{json, Value};

use lj_dtos::Domain;

pub mod redact;
pub mod rpc;

pub const SERVER_INSTRUCTIONS: &str =
    "Search French and European case law: all French courts plus, among \
others, the CNDA, the Conseil constitutionnel, the CNIL, the \
Défenseur des droits, the CEDH and the CJUE; and read legal texts at any date: codes, laws, \
Journal officiel, collective agreements, EU law, treaties, foreign \
codes. The engine understands meaning, not just keywords: \
phrase the query as a natural question or a list of descriptive terms; \
synonyms and reformulations are handled. Typical workflow: \
search_decisions for a shortlist, then get_decision to read candidates \
in full. Previews, facets and chronologies are machine-made and only \
orient: never judge relevance, quote, or state what a decision holds \
from them. Iterate by refining the \
query or filters. Open any `url` with the matching tool: /decision/ \
links with get_decision, /norm/ links with get_norm. When \
mentioning a decision, hyperlink its `title` (or a conventional \
citation) to its `url`.";

/// Newest first: `negotiate_protocol_version` falls back to the head. Every
/// revision listed carries the tools surface unchanged, which is all we serve.
pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The spec has the server answer with the revision the client asked for when
/// it speaks it, and with its own newest otherwise.
pub fn negotiate_protocol_version(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|asked| PROTOCOL_VERSIONS.iter().find(|known| **known == asked))
        .copied()
        .unwrap_or(PROTOCOL_VERSIONS[0])
}

// ── Tools: input schemas ─────────────────────────────────────────────────────

/// Every schema is inlined, with no `title`, and `additionalProperties: false`
/// at the root.
pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "search_decisions",
            "title": "Search Decisions",
            "description": SEARCH_DECISIONS_DESC,
            "inputSchema": search_decisions_schema(),
            "outputSchema": search_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "get_decision",
            "title": "Get Decision",
            "description": GET_DECISION_DESC,
            "inputSchema": get_decision_schema(),
            "outputSchema": get_decision_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "get_norm",
            "title": "Get Norm",
            "description": GET_NORM_DESC,
            "inputSchema": get_norm_schema(),
            "outputSchema": get_norm_output_schema(),
            "annotations": read_only_annotations(),
        }),
        json!({
            "name": "search_norms",
            "title": "Search Norms",
            "description": SEARCH_NORMS_DESC,
            "inputSchema": search_norms_schema(),
            "outputSchema": search_norms_output_schema(),
            "annotations": read_only_annotations(),
        }),
    ]
}

/// Names an MCP client may still call a tool by, with the tool each now names.
/// A client freezes the tool list when the connector is installed and the model
/// only emits a name from it, so a rename is recoverable by an alias and nothing
/// else; an alias leaves when no call arrives under it any more.
pub const RETIRED_NAMES: [(&str, &str); 6] = [
    ("get_legal_text", "get_norm"),
    ("get_legal_article", "get_norm"),
    ("get_law_article", "get_norm"),
    ("search_legal_texts", "search_norms"),
    ("search_legal_articles", "search_norms"),
    ("search_law_articles", "search_norms"),
];

/// Tools withdrawn with nothing to stand for them: a call gets an error naming
/// the removal rather than an unknown tool.
pub const REMOVED_NAMES: [&str; 1] = ["list_my_activity"];

/// The tool `name` calls: itself, or the one a retired name now stands for.
pub fn canonical_tool_name(name: &str) -> &str {
    RETIRED_NAMES
        .iter()
        .find(|(retired, _)| *retired == name)
        .map_or(name, |(_, current)| current)
}

/// No tool writes anything, so every tool is read-only.
pub fn read_only_annotations() -> Value {
    json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false,
    })
}

pub fn enum_values(values: &[&str]) -> Value {
    json!(values)
}

/// Single source: the `ALL` variants of `lj_dtos::schema`, never a copied list.
fn facet_enum_codes<T: serde::Serialize>(all: &[T]) -> Value {
    json!(all
        .iter()
        .filter_map(|v| serde_json::to_value(v).ok())
        .collect::<Vec<Value>>())
}

/// Optional field, serialised `{"anyOf": [inner, {"type": "null"}],
/// "default": null, <extra…>}`. `extra` carries the sibling keys of `anyOf`:
/// `description` on the inputSchema side, `format` on the outputSchema side.
pub fn nullable(inner: Value, extra: Value) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("anyOf".into(), json!([inner, {"type": "null"}]));
    m.insert("default".into(), Value::Null);
    if let Value::Object(e) = extra {
        m.extend(e);
    }
    Value::Object(m)
}

fn search_decisions_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["query"],
        "properties": {
            "query": {
                "type": "string",
                "minLength": 1,
                "maxLength": lj_dtos::QUERY_MAX_CHARS,
                "description": SEARCH_QUERY_DESC,
            },
            "solution": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::Solution::ALL)}}),
                json!({"description": "Filter by the ruling of the operative part (référentiel solution). REJET / IRRECEVABILITE / DESISTEMENT / NON_LIEU_A_STATUER are procedural or negative endings; CONFIRMATION / INFIRMATION* / REFORMATION are appeal outcomes; CASSATION* is cassation-specific; ANNULATION covers administrative annulment; SATISFACTION_TOTALE / SATISFACTION_PARTIELLE cover first-instance civil rulings granting the claim."}),
            ),
            "legal_domain": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&Domain::ALL)}}),
                json!({"description": "Filter by legal domain (curated domain tree): 9 roots and their leaves (e.g. CIVIL_DROIT_LOCATIF). Selecting a root also matches all its leaves."}),
            ),
            "publication": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": enum_values(PUBLICATION_CODES)}}),
                json!({"description": "Filter by publication, at the grain the Cour de cassation itself uses (any-of, membership is multiple: a decision matches every value whose code it carries). Judicial order: PUBLIE_RAPPORT (annual report, the strongest signal), PUBLIE_BULLETIN, LETTRE_CHAMBRE, COMMUNIQUE, INEDIT_BULLETIN. Administrative order: PUBLIE_LEBON, MENTIONNE_LEBON, INEDIT_LEBON. Lower courts carry no publication statement and match no value."}),
            ),
            "seat": nullable(
                json!({"type": "array", "maxItems": 10, "items": {"type": "string", "maxLength": 60}}),
                json!({"description": "Restrict to one or more seats, as a dotted path from the legal order down to the room: \"FR\" (an order), \"FR.CA\" (a type), \"FR.CA.PARIS\" (a court), \"FR.CC.C1_CIVILE\", \"FR.CA.PARIS.P5.C3\", \"FR.TJ.PARIS.REFERES\" (a room), \"EU.CJUE\", \"INTL.CEDH\". A path matches itself and everything below it, so start at the order and go deeper. Types: TJ (tribunal judiciaire), CA (cour d'appel), CC (Cour de cassation), TCOM (tribunal des activités économiques), CPH (conseil de prud'hommes), TA (tribunal administratif), CAA (cour administrative d'appel), CE (Conseil d'État), CNDA (asylum), CONSTIT (Conseil constitutionnel), TC (Tribunal des conflits), CNIL, DDD, CEDH and CJUE. Take the paths from facets.seat."}),
            ),
            "formation": nullable(
                json!({"type": "array", "maxItems": 10, "items": {"type": "string", "maxLength": 60}}),
                json!({"description": "Restrict to the formation that decided, as a path under the court type: \"FR.TJ.JEX\", \"FR.TJ.JAF\", \"FR.TJ.JLD\", \"FR.TA.JUGE_REFERES\", \"FR.CC.RESTREINTE\". Take the paths from facets.formation."}),
            ),
            "legal_instrument": nullable(
                json!({"type": "array", "maxItems": 10, "items": {"type": "string", "maxLength": 100}}),
                json!({"description": "Restrict to decisions citing one or more given codes or statutes. Accepts a slug from facets.legal_instrument (e.g. \"code-civil\") or an exact text name resolved server-side (e.g. \"Code civil\")."}),
            ),
            "legal_article": nullable(
                json!({"type": "array", "maxItems": 20, "items": {"type": "string", "maxLength": 140}}),
                json!({"description": "Restrict to decisions citing a specific article of a specific code, as a composite key \"<instrument>|<article>\" where <instrument> is a slug or an exact text name, resolved like legal_instrument (e.g. \"code-civil|1240\", \"code-de-justice-administrative|L761-1\"). The instrument prefix is required: the same article number exists in several codes. It can be used, for instance, to recover a kind of référé with \"code-de-justice-administrative|L521-3\" or a QPC with \"constitution-du-4-octobre-1958|61-1\"."}),
            ),
            "date_from": nullable(
                json!({"type": "string"}),
                json!({"description": "Earliest decision date, inclusive (YYYY-MM-DD)."}),
            ),
            "date_to": nullable(
                json!({"type": "string"}),
                json!({"description": "Latest decision date, inclusive (YYYY-MM-DD)."}),
            ),
            "search_in": nullable(
                json!({"type": "array", "items": {"type": "string", "enum": facet_enum_codes(&lj_dtos::DecisionPart::ALL)}}),
                json!({"description": "Parts searched, all when absent: headnote (the court's sommaire or analyse), judgment (the decision without its annexes), annexes (the moyens annexés of Cour de cassation judgments)."}),
            ),
            "mode": {
                "type": "string",
                "enum": ["auto", "lexical", "semantic", "reference"],
                "default": "auto",
                "description": SEARCH_MODE_DESC,
            },
            "sort": {
                "type": "string",
                "enum": ["relevance", "date_desc", "date_asc"],
                "default": "relevance",
                "description": "Result ordering. Use 'relevance' (default) unless the user wants chronological order. A date order excludes mode 'semantic'.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 20,
                "default": 10,
                "description": "Maximum number of results (default 10).",
            },
            "ai_rerank": {
                "type": "boolean",
                "default": true,
                "description": "When enabled (default), reorders results by actual relevance to the query using an LLM reranker. Keep on for agentic use, shortlist quality is significantly higher. Cost: a few seconds of extra latency. Disable only for high-rate exploratory searches where latency matters more than ranking quality.",
            },
        },
    })
}

fn get_decision_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["url"],
        "properties": {
            "url": {
                "type": "string",
                "minLength": 1,
                "description": "A librejustice.fr decision URL, from a search_decisions hit or an inline citation link; search_decisions first if you have neither.",
            },
        },
    })
}

/// Three doors to an article, plus `as_of`. No root combinator says which one is
/// required: that is checked in `law_target` instead.
fn get_norm_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "url": {
                "type": "string",
                "description": LAW_URL_DESC,
            },
            "code": {
                "type": "string",
                "description": "Code slug, when you hold the code and the article apart rather than as a url (\"code-civil\"). Alone, addresses the text itself.",
            },
            "article": {
                "type": "string",
                "description": "Lowercase article key, alongside `code` (\"l761-1\" for L. 761-1, \"1240\" for 1240).",
            },
            "text_id": {
                "type": "string",
                "description": "Légifrance id: an article (LEGIARTI, JORFARTI, KALIARTI), a text (LEGITEXT, JORFTEXT) or a section (LEGISCTA); or a BOFiP id (BOI-…), with `article` for one paragraph.",
            },
            "as_of": nullable(
                json!({"type": "string"}),
                json!({"description": "Consultation date (YYYY-MM-DD): returns the version in force at that date. Omit for the version currently in force."}),
            ),
        },
    })
}

fn search_norms_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "query": {
                "type": "string",
                "minLength": 1,
                "maxLength": lj_dtos::QUERY_MAX_CHARS,
                "description": "French query over legal articles. Matches article titles (boosted) and bodies; alias expansion handles acronyms and usual names.",
            },
            "code": nullable(
                json!({"type": "string"}),
                json!({"description": "Restrict to one code/text by its URL slug (\"code-civil\", as in facets.code) or exact name. Omit to search the whole navigable referential."}),
            ),
            "jurisdiction": nullable(
                json!({"type": "string"}),
                json!({"description": "Filter by country/legal order, as an ISO 3166 alpha-2 country code: \"FR\" (France, the bulk of the corpus) or a foreign code (\"SN\", \"DZ\", \"MA\", \"VN\", \"PE\", …); plus \"EU\" for EU law and \"INTL\" for treaties/international law. A dotted path narrows to a type of text: \"FR.LOI\", \"FR.DECRET\", \"EU.DIRECTIVE\", \"INTL.TREATY\", as in facets.jurisdiction."}),
            ),
            "valid_from": nullable(
                json!({"type": "string"}),
                json!({"description": "Earliest entry into force (YYYY-MM-DD) of the versions served, which are those in force at `as_of`, else today. A text's publication counts as its first version."}),
            ),
            "as_of": nullable(
                json!({"type": "string"}),
                json!({"description": "Consultation date (YYYY-MM-DD): searches the versions valid at that date (point-in-time, same semantics as get_norm). Omit to search the versions currently in force."}),
            ),
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 20,
                "default": 10,
                "description": "Maximum number of results (default 10).",
            },
        },
    })
}

// ── Tools: output schemas ────────────────────────────────────────────────────

/// Mirrors `McpSearchResponse`.
pub fn search_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["query", "total", "hits"],
        "properties": {
            "query": {"type": "string"},
            "total": {"type": "integer"},
            "hits": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "url", "chars"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                        "official_url": nullable(json!({"type": "string"}), json!({})),
                        "ai_summary": nullable(json!({"type": "string"}), json!({})),
                        "court_summary": nullable(json!({"type": "string"}), json!({})),
                        "snippet": nullable(json!({"type": "string"}), json!({})),
                        "chars": {"type": "integer"},
                        "date": nullable(json!({"type": "string"}), json!({"format": "date"})),
                        "docket_numbers": nullable(json!({"items": {"type": "string"}, "type": "array"}), json!({})),
                        "seat": nullable(json!({"type": "string"}), json!({})),
                        "solution": nullable(json!({"type": "string"}), json!({})),
                        "formation": {"type": "array", "items": {"type": "string"}},
                        "legal_domain": nullable(json!({"type": "string"}), json!({})),
                        "publication": {"type": "array", "items": {"type": "string"}},
                        "cited_articles": {"type": "object", "additionalProperties": {"type": "array", "items": {"type": "string"}}},
                    },
                },
            },
            "pinned": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "url"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                    },
                },
            },
            "facets": {
                "type": "object",
                "additionalProperties": true,
            },
        },
    })
}

/// Mirrors `McpDecisionDetail`.
pub fn get_decision_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["title", "url", "jurisdiction_type", "text"],
        "properties": {
            "title": {"type": "string"},
            "url": {"type": "string"},
            "official_url": nullable(json!({"type": "string"}), json!({})),
            "date": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "docket_numbers": nullable(json!({"items": {"type": "string"}, "type": "array"}), json!({})),
            "jurisdiction_type": {"type": "string"},
            "jurisdiction_code": nullable(json!({"type": "string"}), json!({})),
            "seat": nullable(json!({"type": "string"}), json!({})),
            "solution": nullable(json!({"type": "string"}), json!({})),
            "formation": {"type": "array", "items": {"type": "string"}},
            "legal_domain": nullable(json!({"type": "string"}), json!({})),
            "publication": {"type": "array", "items": {"type": "string"}},
            "case_chronology": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "current"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": nullable(json!({"type": "string"}), json!({})),
                        "current": {"type": "boolean"},
                        "solution": nullable(json!({"type": "string"}), json!({})),
                        "link_to_next": nullable(json!({"type": "string"}), json!({})),
                    },
                },
            },
            "commentaries": {
                "type": "array",
                "items": commentaire_schema(),
            },
            "text": {"type": "string"},
        },
    })
}

/// Mirrors `lj_dtos::Commentaire`, shared by `get_decision` and
/// `get_norm`.
fn commentaire_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["kind"],
        "properties": {
            "kind": {"type": "string", "enum": ["summary", "conclusions", "note"]},
            "author": nullable(json!({"type": "string"}), json!({})),
            "date": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "title": nullable(json!({"type": "string"}), json!({})),
            "publisher": nullable(json!({"type": "string"}), json!({})),
            "volume": nullable(json!({"type": "string"}), json!({})),
            "issue": nullable(json!({"type": "string"}), json!({})),
            "pages": nullable(json!({"type": "string"}), json!({})),
            "url": nullable(json!({"type": "string"}), json!({})),
            "body": nullable(json!({"type": "string"}), json!({})),
        },
    })
}

/// Mirrors `LinkedTextRef`: a text named by an edge of the norm graph.
fn linked_text_ref_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["label"],
        "properties": {
            "label": {"type": "string"},
            "href": nullable(json!({"type": "string"}), json!({})),
            "date": nullable(json!({"type": "string"}), json!({"format": "date"})),
        },
    })
}

/// Mirrors `McpLawArticle`.
pub fn get_norm_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["code", "title", "url", "versions"],
        "properties": {
            "code": {"type": "string"},
            "number": {"type": "string"},
            "article_count": {"type": "integer"},
            "articles": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["kind", "number", "url"],
                    "properties": {
                        "kind": {"type": "string", "enum": ["article", "section"]},
                        "number": {"type": "string"},
                        "url": {"type": "string"},
                        "section": {"type": "string"},
                        "text": {"type": "string"},
                    },
                },
            },
            "omitted": {"type": "integer"},
            "title": {"type": "string"},
            "url": {"type": "string"},
            "date": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "status": nullable(json!({"type": "string"}), json!({})),
            "valid_from": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "valid_to": nullable(json!({"type": "string"}), json!({"format": "date"})),
            "text": nullable(json!({"type": "string"}), json!({})),
            "source_url": nullable(json!({"type": "string"}), json!({})),
            "note": nullable(json!({"type": "string"}), json!({})),
            "section": {
                "additionalProperties": false,
                "type": "object",
                "required": ["label", "url"],
                "properties": {
                    "label": {"type": "string"},
                    "url": {"type": "string"},
                },
            },
            "commentaries": {
                "type": "array",
                "items": commentaire_schema(),
            },
            "modified_by": {
                "description": "Texts that created, modified or repealed this article.",
                "type": "array",
                "items": linked_text_ref_schema(),
            },
            "legislative_history": {
                "type": "array",
                "items": linked_text_ref_schema(),
            },
            "versions": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["status", "valid_from"],
                    "properties": {
                        "status": {"type": "string"},
                        "valid_from": {"type": "string", "format": "date"},
                        "valid_to": nullable(json!({"type": "string"}), json!({"format": "date"})),
                    },
                },
            },
        },
    })
}

/// Mirrors `McpLawSearchResponse`.
pub fn search_norms_output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "type": "object",
        "required": ["query", "total", "hits", "facets"],
        "properties": {
            "query": {"type": "string"},
            "total": {"type": "integer"},
            "hits": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "url", "snippet", "source"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                        "snippet": {"type": "string"},
                        "title_path": nullable(json!({"type": "string"}), json!({})),
                        "source": {"type": "string"},
                        "valid_from": nullable(json!({"type": "string"}), json!({"format": "date"})),
                        "valid_to": nullable(json!({"type": "string"}), json!({"format": "date"})),
                    },
                },
            },
            "pinned": {
                "type": "array",
                "items": {
                    "additionalProperties": false,
                    "type": "object",
                    "required": ["title", "url"],
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                    },
                },
            },
            "facets": {
                "type": "object",
                "additionalProperties": true,
            },
        },
    })
}

// ── Enum codes, aligned on the lj-core serde renames ─────────────────────────

/// Suffixes of the `publication:*` uids, in menu order.
const PUBLICATION_CODES: &[&str] = &[
    "PUBLIE_RAPPORT",
    "PUBLIE_BULLETIN",
    "LETTRE_CHAMBRE",
    "COMMUNIQUE",
    "INEDIT_BULLETIN",
    "PUBLIE_LEBON",
    "MENTIONNE_LEBON",
    "INEDIT_LEBON",
];

// ── Tool descriptions ────────────────────────────────────────────────────────

const SEARCH_DECISIONS_DESC: &str = "Search decisions by meaning and keywords combined. Returns a \
shortlist: `title`, `url`, an overview (`ai_summary`: AI-written \
summary of what the decision is about; `court_summary`: the court's own \
summary; or `snippet`: the verbatim passage \
where your keywords matched) and metadata; get_decision reads the \
full text. Put constraints in the structured filters (jurisdiction, \
dates, articles, codes), keep the query for the legal issue. Values \
within one filter are OR'd; different filters are AND'd. The response \
carries a `facets` block: per filter name, a map of filter value to \
decision count under the current query. Reuse those keys verbatim to \
refine. \
Hit metadata fields carry the same tokens under the same names: a \
hit's `seat`, `legal_domain` or `solution` passes back \
verbatim into the matching filter.";

const GET_DECISION_DESC: &str = "Fetch the full text and metadata of a decision by its `url`. The \
text carries inline markdown links to cited articles (/norm/, open with \
get_norm) and cited decisions (/decision/, open with \
get_decision); a citation spanning several articles (« articles 3 à 6 \
», « et suivants ») links its first article and appends the others as \
labelled links right after the span. `case_chronology` lists the prior AND \
subsequent decisions of the same case (appeal, pourvoi, renvoi), each \
with its solution. `commentaries` lists documents around the decision \
(the court's summary, conclusions, reports…). `official_url` is the \
decision at its official publisher, proof of its authenticity.";

const GET_NORM_DESC: &str = "Fetch a legal text, or one of its articles, as it stood on a given \
date. Returns the version in force at `as_of` (omit for today): full \
text, status, validity dates, and the timeline of all versions: say \
which version you quote. The text carries inline markdown links to \
cross-referenced articles (/norm/, open with get_norm; when \
served at a date, the links point to the same date). `commentaries` \
links out to commentary anchored on the article. `number` absent means \
you hold the whole text; \
the `url` of an article's `section`, or of a `section` entry of \
`articles`, returns all its articles at once.";

const SEARCH_NORMS_DESC: &str =
    "Find legal articles from their subject, their wording, or an article \
number with no code named; returns a shortlist with highlighted \
snippets and the exact `total`. Query in French, descriptive terms (« \
délai de recours contentieux refus implicite »); put the code in the \
`code` filter (slug or exact name), keep the query for the subject. \
With no `query`, hits are whole texts, the most recently changed first. \
The response carries a `facets` block (`code`, `jurisdiction`): per \
filter name, a map of filter value to article count, reuse those keys \
verbatim to refine. Chain a hit into get_norm with its \
`url`, plus `as_of` when the dispute is governed by an earlier version.";

const LAW_URL_DESC: &str = "A librejustice.fr article URL \
`/norm/{jurisdiction}/{code}/{article}`: take it from a search_norms hit \
or an inline citation link, or compose it: {jurisdiction} is the enacting state \
as ISO 3166-1 alpha-2 lowercase (\"fr\", \"be\"), \"eu\" for the Union, its \
treaties included, \"intl\" for a treaty signed outside it (the ECHR, a \
bilateral accord), {code} is the code slug (\"code-civil\", as in \
facets.legal_instrument), {article} the lowercase article key (\"l761-1\" for \
L. 761-1, \"1240\" for 1240). `/norm/{jurisdiction}/{code}` alone addresses the \
text itself, which is how a text with no articles (circulaire, publication \
decree) is read.";

const SEARCH_QUERY_DESC: &str = "The primary input, in the language of the decisions \
sought: a question or descriptive keywords on a legal issue (« licenciement discrimination syndicale charge de la preuve »), \
with quoted phrases for a name or a precise formula \
(« \"Société Générale\" responsabilité du banquier »); \
a boolean formula (« \"force majeure\" ET épidémie SAUF grippe »); \
or a reference (« 21PA01295 », « CE 13 juillet 2016 », « Czabaj »).";

const SEARCH_MODE_DESC: &str = "'auto' is the right choice in almost all cases. \
auto: automatically switches among 'semantic', 'lexical' and 'reference'. \
semantic: meaning + keyword hybrid, quoted phrases filter presence. \
lexical: keyword-only, every word required, quoted phrases exact, \
ET / AND / OU / OR / SAUF / NOT / PROCHE5 / NEAR5 / répar*. \
reference: lookup by identity, on the docket number, the ECLI, the case name, \
or the court and the date.";

#[cfg(test)]
mod tests {
    use super::*;

    /// A client reads the schema as sent: no `$ref` to resolve, no root `title`
    /// to strip, and an unknown argument refused rather than ignored.
    #[test]
    fn search_schema_is_strict_and_inlined() {
        let schema = search_decisions_schema();
        assert_eq!(schema["additionalProperties"], json!(false));
        assert!(schema.get("title").is_none());
        assert!(schema["properties"]["query"].get("title").is_none());
        assert_eq!(schema["required"], json!(["query"]));
        let dumped = serde_json::to_string(&schema).unwrap();
        assert!(!dumped.contains("$ref"));
        assert!(!dumped.contains("$defs"));
    }

    /// The standalone server reads `data/mcp-tools.json`; `LIBREJUSTICE_BLESS_MCP_TOOLS=1` rewrites it.
    /// The output schemas are also copied by hand into `packages/lj-web/public/openapi.json`,
    /// which `pytest mcp` holds equal to this file.
    #[test]
    fn the_committed_surface_matches_the_definitions() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/mcp-tools.json");
        let built = serde_json::to_string_pretty(&json!({
            "instructions": SERVER_INSTRUCTIONS,
            "protocolVersions": PROTOCOL_VERSIONS,
            "tools": tool_definitions(),
        }))
        .unwrap()
            + "\n";
        if std::env::var_os("LIBREJUSTICE_BLESS_MCP_TOOLS").is_some() {
            std::fs::write(path, &built).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(path).expect("data/mcp-tools.json");
        assert_eq!(
            committed, built,
            "data/mcp-tools.json is stale: LIBREJUSTICE_BLESS_MCP_TOOLS=1 cargo test -p lj-mcp-tools"
        );
    }
}
