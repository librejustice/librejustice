//! Hydratation DB (metadata + facettes), denylist procédurale, construction des
//! snippets et assemblage des `SearchHit` d'une page.
//!
//! Depuis l'ADR 0146, les facettes et les tags des hits sont résolus depuis le
//! référentiel DB ([`crate::referential::Referential`]) : les compteurs se
//! groupent par colonnes `_uid`/`jurisdiction_code`, les libellés viennent du
//! cache — plus aucune table de labels compilée.

use std::collections::HashMap;

use deadpool_postgres::Client;
use lj_core::truecase;
use lj_store::error::StoreError;

use crate::error::{ApiError, Result};
use crate::referential::{uid_suffix, Referential};
use crate::snippets;
use crate::snippets::highlight;

use lj_dtos::{
    BestChunk, FacetChoice, FacetTag, JurisdictionType, LegalInstrumentFacet, SearchFacets,
    SearchHit,
};

use super::legs::LegHit;

const SNIPPET_MAX_CHARS: usize = 280;
const TITLE_SNIPPET_CHARS: usize = 500;
const LEGAL_INSTRUMENT_FACET_LIMIT: usize = 30;
const LEGAL_ARTICLE_FACET_LIMIT: usize = 20;

// ── Facettes ─────────────────────────────────────────────────────────────

/// Tri d'un compteur en `(clé, count)` : count décroissant, tie-break par clé.
fn ranked(counter: &HashMap<String, i64>) -> Vec<(String, i64)> {
    let mut items: Vec<(String, i64)> = counter.iter().map(|(k, v)| (k.clone(), *v)).collect();
    items.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    items
}

/// Choix d'une facette plate keyée par **uid complet** : `value` = suffixe
/// d'uid, `label` résolu depuis le référentiel (ADR 0146 §4).
fn uid_choices(counter: &HashMap<String, i64>, refs: &Referential) -> Vec<FacetChoice> {
    ranked(counter)
        .into_iter()
        .map(|(uid, count)| FacetChoice {
            value: uid_suffix(&uid).to_string(),
            label: refs.label(&uid).to_string(),
            count,
            parent: None,
        })
        .collect()
}

// ── DB hydration / facets ────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub(crate) struct DecisionMeta {
    pub(crate) public_id: String,
    pub(crate) jurisdiction_type: String,
    pub(crate) jurisdiction_code: Option<String>,
    pub(crate) date_lecture: Option<String>,
    pub(crate) solution_uid: Option<String>,
    pub(crate) procedure_uid: Option<String>,
    pub(crate) office_uid: Option<String>,
    pub(crate) legal_domain_uid: Option<String>,
    pub(crate) publication_codes: Vec<String>,
    pub(crate) docket_numbers: Option<Vec<String>>,
    pub(crate) summary: Option<String>,
    pub(crate) chars: Option<i64>,
    /// Axes formation (ADR 0170) : position recomposée + type de formation,
    /// pour le siège du titre des hits.
    pub(crate) chamber_position: Option<String>,
    pub(crate) formation_uid: Option<String>,
    pub(crate) chamber_uid: Option<String>,
    pub(crate) publication_uid: Option<String>,
}

pub(crate) async fn fetch_pub_ids_and_facets(
    conn: &Client,
    decision_ids: &[i64],
    refs: &Referential,
) -> std::result::Result<(HashMap<i64, String>, SearchFacets), StoreError> {
    if decision_ids.is_empty() {
        return Ok((HashMap::new(), SearchFacets::default()));
    }
    let rows = conn
        .query(
            "SELECT d.id, d.public_id, d.jurisdiction_type, d.jurisdiction_code, \
             d.office_uid, d.legal_domain_uid, d.solution_uid, d.publication_uid, \
             EXTRACT(YEAR FROM d.date_lecture)::int AS y, d.publication_codes, \
             d.chamber_uid \
             FROM decisions d WHERE d.id = ANY($1)",
            &[&decision_ids],
        )
        .await?;
    // Facettes instruments/articles depuis la relation du domaine (ADR 0145 M4) :
    // token = ref_text_uid / ref_num_key, identiques aux valeurs des arrays de
    // filtre legal_instruments/composite (migration 0098) ; libellé = titre
    // catalogue, résolu ici. Seul le lié est facettable — le non-lié
    // (`ref_text_uid` NULL) n'a ni token ni libellé (décision opérateur
    // 2026-06-30 : on ne filtre pas vers ce qui ne mène nulle part).
    let refs_rows = conn
        .query(
            "SELECT el->>2, lt.title, el->>3, \
             COUNT(DISTINCT lc.decision_id) AS n, lt.slug \
             FROM legal_citation lc \
             CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el \
             JOIN legal_text lt ON lt.text_uid = el->>2 \
             WHERE lc.decision_id = ANY($1) \
             GROUP BY el->>2, lt.title, el->>3, lt.slug",
            &[&decision_ids],
        )
        .await?;

    let mut pub_ids: HashMap<i64, String> = HashMap::new();
    // Facette juridiction (ADR 0163) : niveau 1 = tokens `jurisdiction_type`
    // (`TJ`), niveau 2 = codes `jurisdiction` (`parent` = token du type). L'office
    // est un axe séparé (`office:*`), en miroir du filtrage (`office_uid`) —
    // chaque décision compte sous son tribunal ET sous son office éventuel.
    let mut jur_roots: HashMap<String, i64> = HashMap::new();
    let mut jur_children: HashMap<(String, String), i64> = HashMap::new();
    let mut chamber_counter: HashMap<String, i64> = HashMap::new();
    let mut office_counter: HashMap<String, i64> = HashMap::new();
    let mut domain_counter: HashMap<String, i64> = HashMap::new();
    let mut solution_counter: HashMap<String, i64> = HashMap::new();
    let mut significance_counter: HashMap<String, i64> = HashMap::new();
    let mut publication_counter: HashMap<String, i64> = HashMap::new();
    let mut year_counter: HashMap<String, i64> = HashMap::new();

    for row in &rows {
        let id: i64 = row.get(0);
        let public_id: Option<String> = row.get(1);
        if let Some(pid) = &public_id {
            pub_ids.insert(id, pid.clone());
        }
        let jt: String = row.get(2);
        let code: Option<String> = row.get(3);
        let office: Option<String> = row.get(4);
        let domain: Option<String> = row.get(5);
        let solution: Option<String> = row.get(6);
        let publication: Option<String> = row.get(7);
        let year: Option<i32> = row.get(8);
        let publication_codes: Vec<String> = row.get(9);
        let chamber: Option<String> = row.get(10);
        if let Some(uid) = chamber {
            *chamber_counter.entry(uid).or_insert(0) += 1;
        }
        // Portée dérivée des codes (mapping total, ADR 0167) — pas de colonne.
        *significance_counter
            .entry(format!(
                "significance:{}",
                lj_core::publication::significance_key(&publication_codes)
            ))
            .or_insert(0) += 1;
        if let Some(code) = code {
            *jur_children.entry((jt.clone(), code)).or_insert(0) += 1;
        }
        *jur_roots.entry(jt).or_insert(0) += 1;
        for (counter, val) in [
            (&mut office_counter, office),
            (&mut domain_counter, domain),
            (&mut solution_counter, solution),
            (&mut publication_counter, publication),
        ] {
            if let Some(uid) = val {
                *counter.entry(uid).or_insert(0) += 1;
            }
        }
        if let Some(y) = year {
            *year_counter.entry(y.to_string()).or_insert(0) += 1;
        }
    }

    // Agrégation par uid : le token est canonique (plus de variantes de casse
    // à replier — c'était une servitude du token `text_key`). Le libellé
    // (titre) accompagne le token pour l'affichage.
    let mut li_counter: HashMap<String, i64> = HashMap::new();
    let mut li_labels: HashMap<String, String> = HashMap::new();
    let mut li_slugs: HashMap<String, Option<String>> = HashMap::new();
    let mut li_article_counter: HashMap<String, HashMap<String, i64>> = HashMap::new();
    for row in &refs_rows {
        let uid: String = row.get(0);
        let title: String = row.get(1);
        let num_key: Option<String> = row.get(2);
        let n: i64 = row.get(3);
        let slug: Option<String> = row.get(4);
        *li_counter.entry(uid.clone()).or_insert(0) += n;
        li_labels.entry(uid.clone()).or_insert(title);
        li_slugs.entry(uid.clone()).or_insert(slug);
        if let Some(k) = num_key {
            *li_article_counter
                .entry(uid)
                .or_default()
                .entry(k)
                .or_insert(0) += n;
        }
    }

    // legal_instrument : most_common(limit), articles most_common(limit).
    let li_facets: Vec<LegalInstrumentFacet> =
        most_common(&li_counter, LEGAL_INSTRUMENT_FACET_LIMIT)
            .into_iter()
            .map(|(uid, count)| {
                let articles = li_article_counter
                    .get(&uid)
                    .map(|c| {
                        most_common(c, LEGAL_ARTICLE_FACET_LIMIT)
                            .into_iter()
                            .map(|(value, count)| FacetChoice {
                                label: value.clone(),
                                value,
                                count,
                                parent: None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let label = li_labels.get(&uid).cloned().unwrap_or_else(|| uid.clone());
                let slug = li_slugs.get(&uid).cloned().flatten();
                LegalInstrumentFacet {
                    value: uid,
                    label,
                    slug,
                    count,
                    articles,
                }
            })
            .collect();

    let facets = SearchFacets {
        jurisdiction: jurisdiction_choices(&jur_roots, &jur_children, refs),
        chamber: uid_choices(&chamber_counter, refs),
        office: uid_choices(&office_counter, refs),
        legal_domain: domain_choices(&domain_counter, refs),
        solution: uid_choices(&solution_counter, refs),
        significance: uid_choices(&significance_counter, refs),
        publication: uid_choices(&publication_counter, refs),
        date_lecture_year: ranked(&year_counter)
            .into_iter()
            .map(|(y, count)| FacetChoice {
                label: y.clone(),
                value: y,
                count,
                parent: None,
            })
            .collect(),
        legal_instrument: li_facets,
    };
    Ok((pub_ids, facets))
}

/// Facette juridiction : racines (tokens `jurisdiction_type`, `TA`/`CC`…)
/// triées par count, puis enfants (`value` = code `jurisdiction`, `parent` =
/// token racine) — le front reconstruit l'arbre par `parent`.
fn jurisdiction_choices(
    roots: &HashMap<String, i64>,
    children: &HashMap<(String, String), i64>,
    refs: &Referential,
) -> Vec<FacetChoice> {
    let mut out: Vec<FacetChoice> = ranked(roots)
        .into_iter()
        .map(|(token, count)| FacetChoice {
            label: refs
                .jurisdiction_type_label(&token)
                .unwrap_or(&token)
                .to_string(),
            value: token,
            count,
            parent: None,
        })
        .collect();
    let mut kids: Vec<((String, String), i64)> =
        children.iter().map(|(k, v)| (k.clone(), *v)).collect();
    kids.sort_by(|a, b| b.1.cmp(&a.1).then(a.0 .1.cmp(&b.0 .1)));
    out.extend(kids.into_iter().map(|((root_token, code), count)| {
        let label = refs
            .jurisdiction(&code)
            .map(|j| j.label.clone())
            .unwrap_or_else(|| code.clone());
        FacetChoice {
            value: code,
            label,
            count,
            parent: Some(root_token),
        }
    }));
    out
}

/// Facette domaine (arbre 2 niveaux) : le count d'une racine agrège ses
/// annotations directes + celles de ses feuilles ; `value` = suffixe d'uid,
/// `parent` (feuilles) = suffixe de la racine.
fn domain_choices(counter: &HashMap<String, i64>, refs: &Referential) -> Vec<FacetChoice> {
    let mut root_counts: HashMap<String, i64> = HashMap::new();
    let mut leaf_counts: HashMap<String, i64> = HashMap::new();
    for (uid, count) in counter {
        match refs.value(uid).and_then(|e| e.parent_uid.clone()) {
            Some(root_uid) => {
                *root_counts.entry(root_uid).or_insert(0) += count;
                *leaf_counts.entry(uid.clone()).or_insert(0) += count;
            }
            None => *root_counts.entry(uid.clone()).or_insert(0) += count,
        }
    }
    let mut out: Vec<FacetChoice> = ranked(&root_counts)
        .into_iter()
        .map(|(uid, count)| FacetChoice {
            value: uid_suffix(&uid).to_string(),
            label: refs.label(&uid).to_string(),
            count,
            parent: None,
        })
        .collect();
    out.extend(ranked(&leaf_counts).into_iter().map(|(uid, count)| {
        let parent = refs
            .value(&uid)
            .and_then(|e| e.parent_uid.as_deref())
            .map(|p| uid_suffix(p).to_string());
        FacetChoice {
            value: uid_suffix(&uid).to_string(),
            label: refs.label(&uid).to_string(),
            count,
            parent,
        }
    }));
    out
}

/// `Counter.most_common(n)` : tri par count décroissant puis insertion (ici
/// déterminisé par clé pour la reproductibilité, faute d'ordre d'insertion).
fn most_common(counter: &HashMap<String, i64>, n: usize) -> Vec<(String, i64)> {
    let mut items = ranked(counter);
    items.truncate(n);
    items
}

pub(crate) async fn hydrate_decisions(
    conn: &Client,
    decision_ids: &[i64],
) -> std::result::Result<HashMap<i64, DecisionMeta>, StoreError> {
    if decision_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = conn
        .query(
            "SELECT d.id, d.public_id, d.jurisdiction_type, d.jurisdiction_code, \
             d.date_lecture, d.solution_uid, d.procedure_uid, d.office_uid, \
             d.legal_domain_uid, d.publication_codes, \
             d.docket_numbers, d.summary, c.chars, \
             d.chamber_position, d.formation_uid, d.chamber_uid, d.publication_uid \
             FROM decisions d \
             LEFT JOIN LATERAL (SELECT max(char_end) AS chars FROM decision_chunks \
               WHERE decision_id = d.id) c ON true \
             WHERE d.id = ANY($1)",
            &[&decision_ids],
        )
        .await?;
    let mut out = HashMap::new();
    for r in &rows {
        let public_id: Option<String> = r.get(1);
        let Some(public_id) = public_id else {
            continue;
        };
        let date_raw: Option<chrono::NaiveDate> = r.get(4);
        let chars: Option<i32> = r.get(12);
        out.insert(
            r.get::<_, i64>(0),
            DecisionMeta {
                public_id,
                jurisdiction_type: r.get(2),
                jurisdiction_code: r.get(3),
                date_lecture: date_raw.map(|d| d.to_string()),
                solution_uid: r.get(5),
                procedure_uid: r.get(6),
                office_uid: r.get(7),
                legal_domain_uid: r.get(8),
                publication_codes: r.try_get(9).unwrap_or_default(),
                docket_numbers: r.try_get(10).ok(),
                summary: r.get(11),
                chars: chars.map(|c| c as i64),
                chamber_position: r.get(13),
                formation_uid: r.get(14),
                chamber_uid: r.get(15),
                publication_uid: r.get(16),
            },
        );
    }
    Ok(out)
}

/// `full_text` (TOAST pglz) des décisions d'**une page** (≤ `limit`), par
/// `decision_id` : source unique du snippet (ADR 0084). Le moteur de highlight
/// re-tokenise le texte fourni — aucun offset BM25 requis. Lu seulement pour les
/// décisions réellement affichées, sur un miss `page_memo`.
async fn load_full_texts(
    conn: &Client,
    decision_ids: &[i64],
) -> std::result::Result<HashMap<i64, String>, StoreError> {
    if decision_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = conn
        .query(
            "SELECT id, full_text FROM decisions \
             WHERE id = ANY($1) AND full_text IS NOT NULL",
            &[&decision_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1)))
        .collect())
}

/// Spans char `(char_start, char_end)` des chunks gagnants ANN d'une page, par
/// `chunk_id` : fenêtre de fallback du snippet pour un hit purement sémantique
/// (aucun terme requête dans `full_text`) — on montre alors la région du chunk
/// gagnant plutôt que la tête de la décision (ADR 0084).
async fn load_chunk_spans(
    conn: &Client,
    chunk_ids: &[i64],
) -> std::result::Result<HashMap<i64, (i32, i32)>, StoreError> {
    if chunk_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = conn
        .query(
            "SELECT id, char_start, char_end FROM decision_chunks WHERE id = ANY($1)",
            &[&chunk_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get::<_, i64>(0), (r.get::<_, i32>(1), r.get::<_, i32>(2))))
        .collect())
}

// ── Construction des hits de page ─────────────────────────────────────────────

fn make_snippet(text: &str) -> String {
    let count = text.chars().count();
    if count <= SNIPPET_MAX_CHARS {
        return text.to_string();
    }
    let mut s: String = text.chars().take(SNIPPET_MAX_CHARS - 1).collect();
    s.push('…');
    s
}

fn jt_from_str(code: &str) -> Option<JurisdictionType> {
    match code {
        "TA" => Some(JurisdictionType::Ta),
        "CAA" => Some(JurisdictionType::Caa),
        "CE" => Some(JurisdictionType::Ce),
        "CONSTIT" => Some(JurisdictionType::Constit),
        "TC" => Some(JurisdictionType::Tc),
        "CC" => Some(JurisdictionType::Cc),
        "CA" => Some(JurisdictionType::Ca),
        "TJ" => Some(JurisdictionType::Tj),
        "TCOM" => Some(JurisdictionType::Tcom),
        "CEDH" => Some(JurisdictionType::Cedh),
        "CJUE" => Some(JurisdictionType::Cjue),
        "CNDA" => Some(JurisdictionType::Cnda),
        "CNIL" => Some(JurisdictionType::Cnil),
        _ => None,
    }
}

/// Tag référentiel d'un uid optionnel (`solution:REJET` → `{REJET, Rejet}`).
fn opt_tag(uid: &Option<String>, refs: &Referential) -> Option<FacetTag> {
    uid.as_deref().map(|u| refs.tag(u))
}

/// Juridiction résolue d'une décision (nom réel assaini sinon libellé du type).
fn display_jurisdiction(m: &DecisionMeta, refs: &Referential) -> String {
    let jurisdiction_name = m
        .jurisdiction_code
        .as_deref()
        .and_then(|c| refs.jurisdiction(c))
        .map(|j| j.label.as_str());
    crate::titles::decision_jurisdiction(
        refs.jurisdiction_type_label(&m.jurisdiction_type)
            .unwrap_or(&m.jurisdiction_type),
        jurisdiction_name,
    )
}

/// Titre d'affichage des cartes/topbar (ADR 0170) : « <juridiction résolue>,
/// <date FR>, <n°> » — **sans** le siège (rendu à part, [`display_seat`], en 2ᵉ
/// ligne). Une seule ligne, ne wrappe pas. Le titre BM25 (`search_title`), lui,
/// garde le siège pour le matching.
pub(crate) fn display_title(m: &DecisionMeta, refs: &Referential) -> String {
    let jur_display = display_jurisdiction(m, refs);
    lj_core::titles::decision_title(
        &jur_display,
        None,
        m.date_lecture.as_deref(),
        m.docket_numbers
            .as_deref()
            .and_then(|d| d.first())
            .map(String::as_str),
    )
}

/// Siège recomposé depuis les axes structurés (chambre · formation/office),
/// rendu en 2ᵉ ligne sous le titre. `None` si aucun axe.
pub(crate) fn display_seat(m: &DecisionMeta, refs: &Referential) -> Option<String> {
    let jur_display = display_jurisdiction(m, refs);
    crate::titles::decision_seat(
        &jur_display,
        m.chamber_position.as_deref(),
        m.formation_uid.as_deref(),
        m.office_uid.as_deref(),
    )
}

fn make_hit(
    m: &DecisionMeta,
    score: f64,
    chunk: &LegHit,
    title_html: String,
    summary: Option<String>,
    refs: &Referential,
) -> SearchHit {
    SearchHit {
        id: m.public_id.clone(),
        jurisdiction_type: jt_from_str(&m.jurisdiction_type).unwrap_or(JurisdictionType::Ta),
        jurisdiction_code: m.jurisdiction_code.clone(),
        jurisdiction_name: m
            .jurisdiction_code
            .as_deref()
            .and_then(|c| refs.jurisdiction(c))
            .map(|j| j.label.clone()),
        title_html,
        seat: display_seat(m, refs),
        score,
        date_lecture: m.date_lecture.clone(),
        docket_numbers: m.docket_numbers.clone(),
        solution: opt_tag(&m.solution_uid, refs),
        procedure: opt_tag(&m.procedure_uid, refs),
        office: opt_tag(&m.office_uid, refs),
        legal_domain: opt_tag(&m.legal_domain_uid, refs),
        chamber: opt_tag(&m.chamber_uid, refs),
        publication: opt_tag(&m.publication_uid, refs),
        publication_codes: m.publication_codes.clone(),
        best_chunk: BestChunk {
            chunk_index: chunk.chunk_index,
            snippet: chunk.snippet.clone().unwrap_or_default(),
        },
        chars: m.chars,
        summary,
    }
}

/// Assemble les `SearchHit` d'**une seule page** (≤ `limit`) à partir de la
/// metadata déjà hydratée ([`hydrate_decisions`]). Accès DB : le `full_text` des
/// ≤`limit` décisions affichées ([`load_full_texts`]) + les spans char des
/// chunks gagnants ANN ([`load_chunk_spans`], fenêtre de fallback). Suit le
/// highlight tantivy de `full_text` + titres de la page (Tier 2, mémoïsé par
/// page). Le snippet vient désormais du texte décision (ADR 0084), plus du body
/// chunk (dropé).
pub(crate) async fn assemble_page(
    conn: &Client,
    ranked_page: &[(i64, f64, LegHit)],
    meta: &HashMap<i64, DecisionMeta>,
    query_text: &str,
    refs: &Referential,
) -> Result<Vec<SearchHit>> {
    // Hits affichés (présents dans la metadata hydratée) + le chunk gagnant
    // éventuel (jambe ANN, `chunk_id != -1`) qui ancre la fenêtre de fallback.
    let display: Vec<(i64, f64, LegHit)> = ranked_page
        .iter()
        .filter(|(d, _, _)| meta.contains_key(d))
        .cloned()
        .collect();

    let decision_ids: Vec<i64> = display.iter().map(|(d, _, _)| *d).collect();
    let ann_chunk_ids: Vec<i64> = display
        .iter()
        .map(|(_, _, c)| c.chunk_id)
        .filter(|id| *id != -1)
        .collect();
    let full_texts = load_full_texts(conn, &decision_ids)
        .await
        .map_err(ApiError::Store)?;
    // Vieilles décisions opendata intégralement en MAJUSCULES (surtout Cassation) :
    // recasse déterministe pour l'affichage du snippet, en miroir de la vue détail
    // (`decisions.rs`), gardée par `is_caps_lock`. Le highlight re-tokenise insensible
    // casse/accents (ADR 0084) → les `<mark>` se placent toujours correctement. CPU
    // pur (tokenizer + dicos) : offload `spawn_blocking` car le runtime est
    // `current_thread` (SSR Leptos, ADR 0061). La map recasée alimente les deux
    // chemins de snippet (highlight + fallback brut).
    let full_texts = tokio::task::spawn_blocking(move || {
        full_texts
            .into_iter()
            .map(|(id, t)| {
                let t = if truecase::is_caps_lock(&t) {
                    truecase::truecase(&t)
                } else {
                    t
                };
                (id, t)
            })
            .collect::<HashMap<i64, String>>()
    })
    .await
    .map_err(|e| ApiError::Internal(format!("recase task: {e}")))?;
    let chunk_spans = load_chunk_spans(conn, &ann_chunk_ids)
        .await
        .map_err(ApiError::Store)?;

    let body_docs: Vec<(i64, String)> = display
        .iter()
        .filter_map(|(d, _, _)| full_texts.get(d).map(|t| (*d, t.clone())))
        .collect();
    // `highlight` tokenise les textes + sélectionne les fragments : CPU pur (le
    // tokenizer regex domine). On l'offload sur `spawn_blocking` (pool blocking
    // dédié) : le runtime est `current_thread` (contrainte SSR Leptos, ADR 0061),
    // donc ce CPU sur l'unique thread async sérialiserait sinon les requêtes en vol.
    let query_body = query_text.to_string();
    let body_task = tokio::task::spawn_blocking(move || {
        highlight(&body_docs, &query_body, snippets::DEFAULT_MAX_CHARS)
    });

    // Titres composés depuis les référentiels (jamais la colonne `search_title`,
    // qui porte la formation source brute) : highlight puis fallback verbatim.
    let titles: HashMap<i64, String> = display
        .iter()
        .filter_map(|(d, _, _)| meta.get(d).map(|m| (*d, display_title(m, refs))))
        .collect();
    let title_docs: Vec<(i64, String)> = titles.iter().map(|(d, t)| (*d, t.clone())).collect();
    let query_title = query_text.to_string();
    let title_task = tokio::task::spawn_blocking(move || {
        highlight(&title_docs, &query_title, TITLE_SNIPPET_CHARS)
    });

    let body_snippets = body_task
        .await
        .map_err(|e| ApiError::Internal(format!("body snippet task: {e}")))?;
    let title_snippets = title_task
        .await
        .map_err(|e| ApiError::Internal(format!("title snippet task: {e}")))?;

    let mut hits = Vec::with_capacity(display.len());
    for (decision_id, score, mut display_chunk) in display {
        let Some(decision_meta) = meta.get(&decision_id) else {
            continue;
        };
        // Snippet : fragment highlighté de `full_text` si un terme matche, sinon
        // fenêtre brute. Fallback ancré sur le chunk gagnant ANN (`[char_start..
        // char_end]`) — région du match sémantique — ou la tête du texte pour un
        // hit lexical/title-only. `make_hit` rend `""` faute de texte.
        display_chunk.snippet = body_snippets.get(&decision_id).cloned().or_else(|| {
            let full_text = full_texts.get(&decision_id)?;
            let raw = match chunk_spans.get(&display_chunk.chunk_id) {
                Some(&(cs, ce)) => full_text
                    .chars()
                    .skip(cs.max(0) as usize)
                    .take((ce - cs).max(0) as usize)
                    .collect::<String>(),
                None => full_text.clone(),
            };
            Some(make_snippet(&raw))
        });
        let title_html = title_snippets
            .get(&decision_id)
            .cloned()
            .or_else(|| titles.get(&decision_id).cloned())
            .unwrap_or_default();
        // Résumé garanti en base et déjà joint au hot path (ADR 0051) : embarqué
        // dans chaque hit quel que soit le mode → vue « Résumé » instantanée côté
        // front, sans aller-retour `/summary`. Coût DB nul (colonne déjà lue).
        let summary = decision_meta.summary.clone();
        hits.push(make_hit(
            decision_meta,
            score,
            &display_chunk,
            title_html,
            summary,
            refs,
        ));
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_store::repository::FacetValueRow;

    #[test]
    fn snippet_truncation_adds_ellipsis() {
        let long = "a".repeat(400);
        let s = make_snippet(&long);
        assert_eq!(s.chars().count(), SNIPPET_MAX_CHARS);
        assert!(s.ends_with('…'));
    }

    fn fv(uid: &str, label: &str, parent: Option<&str>) -> FacetValueRow {
        FacetValueRow {
            uid: uid.to_string(),
            facet: uid.split(':').next().unwrap().to_string(),
            label: label.to_string(),
            abbr: None,
            parent_uid: parent.map(str::to_string),
            sort: 0,
        }
    }

    #[test]
    fn domain_facet_aggregates_leaves_under_root() {
        let refs = Referential::new(
            vec![
                fv("legal_domain:CIVIL", "Civil", None),
                fv(
                    "legal_domain:CIVIL_DROIT_LOCATIF",
                    "Droit locatif",
                    Some("legal_domain:CIVIL"),
                ),
                fv("legal_domain:FISCAL", "Fiscal", None),
            ],
            Vec::new(),
        );
        let counter = HashMap::from([
            ("legal_domain:CIVIL_DROIT_LOCATIF".to_string(), 3i64),
            ("legal_domain:CIVIL".to_string(), 1),
            ("legal_domain:FISCAL".to_string(), 2),
        ]);
        let choices = domain_choices(&counter, &refs);
        // Racines d'abord (count agrégé), feuilles ensuite (parent = suffixe racine).
        let civil = choices.iter().find(|c| c.value == "CIVIL").unwrap();
        assert_eq!((civil.count, civil.parent.clone()), (4, None));
        let fiscal = choices.iter().find(|c| c.value == "FISCAL").unwrap();
        assert_eq!(fiscal.count, 2);
        let leaf = choices
            .iter()
            .find(|c| c.value == "CIVIL_DROIT_LOCATIF")
            .unwrap();
        assert_eq!(
            (leaf.count, leaf.parent.as_deref(), leaf.label.as_str()),
            (3, Some("CIVIL"), "Droit locatif")
        );
    }

    #[test]
    fn juridiction_facet_nests_codes_under_types() {
        let refs = Referential::new(
            vec![fv("jurisdiction_type:TJ", "Tribunal judiciaire", None)],
            vec![lj_store::repository::JurisdictionRow {
                code: "tj_le_havre".to_string(),
                source_code: "tj76351".to_string(),
                jurisdiction_type: "TJ".to_string(),
                city: Some("Le Havre".to_string()),
                label: "Tribunal judiciaire du Havre".to_string(),
            }],
        );
        let roots = HashMap::from([("TJ".to_string(), 5i64)]);
        let children = HashMap::from([(("TJ".to_string(), "tj_le_havre".to_string()), 4i64)]);
        let choices = jurisdiction_choices(&roots, &children, &refs);
        // Racine : token `jurisdiction_type`, label résolu.
        assert_eq!(choices[0].value, "TJ");
        assert_eq!(choices[0].label, "Tribunal judiciaire");
        // Enfant : code `jurisdiction`, parent = token racine.
        let child = choices.iter().find(|c| c.value == "tj_le_havre").unwrap();
        assert_eq!(child.parent.as_deref(), Some("TJ"));
        assert_eq!(child.label, "Tribunal judiciaire du Havre");
    }
}
