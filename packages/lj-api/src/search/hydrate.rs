//! Hydratation DB (metadata + facettes), denylist procédurale, construction des
//! snippets et assemblage des `SearchHit` d'une page.
//!
//! Les facettes et les tags des hits sont résolus depuis le référentiel DB
//! ([`crate::referential::Referential`]) : les compteurs se groupent par colonnes
//! `_uid`/`jurisdiction_code`, les libellés viennent du cache.

use std::collections::{BTreeMap, HashMap};

use deadpool_postgres::Client;
use lj_store::error::StoreError;

use crate::error::{ApiError, Result};
use crate::referential::{uid_suffix, Referential};
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
const LEGAL_ARTICLE_TOP_LIMIT: usize = 10;
/// Cited articles served per hit, the procedural bank dropped: beyond the rarest
/// few they are a list, not a signal.
const HIT_CITED_ARTICLES_LIMIT: usize = 5;

// ── Facettes ─────────────────────────────────────────────────────────────

/// Compteurs bruts d'une réponse de facettes, avant résolution des libellés.
/// Deux chemins les remplissent, le parcours d'un jeu de hits et les agrégats
/// du fonds filtré ; passer par la même structure est ce qui garantit une forme
/// de réponse identique.
#[derive(Default)]
pub(crate) struct FacetCounters {
    /// Unfolded seat paths (`FR`, `FR.CA`, `FR.CA.PARIS`, `FR.CA.PARIS.P5.C3`), the whole
    /// tree: a path carries its own depth, so one map is enough.
    pub(crate) seats: HashMap<String, i64>,
    /// Unfolded formation paths (`FR`, `FR.TJ`, `FR.TJ.JEX`), as the seat's.
    pub(crate) formation: HashMap<String, i64>,
    pub(crate) domain: HashMap<String, i64>,
    pub(crate) solution: HashMap<String, i64>,
    /// Clés déjà préfixées `publication:`.
    pub(crate) publication: HashMap<String, i64>,
    /// Années en texte, telles qu'elles sortent en `value` de facette.
    pub(crate) year: HashMap<String, i64>,
    pub(crate) instrument: InstrumentCounters,
}

/// Compteurs de « Textes cités ». Vides, ils rendent une facette vide : c'est
/// ainsi que le mode parcours s'en dispense.
#[derive(Default)]
pub(crate) struct InstrumentCounters {
    pub(crate) counts: HashMap<String, i64>,
    pub(crate) labels: HashMap<String, String>,
    pub(crate) slugs: HashMap<String, Option<String>>,
    pub(crate) articles: HashMap<String, HashMap<String, i64>>,
}

/// Résout les libellés et l'ordre : compteurs bruts vers réponse de facettes.
pub(crate) fn facets_from_counters(c: &FacetCounters, refs: &Referential) -> SearchFacets {
    let li_facets: Vec<LegalInstrumentFacet> =
        most_common(&c.instrument.counts, LEGAL_INSTRUMENT_FACET_LIMIT)
            .into_iter()
            .map(|(uid, count)| {
                let articles = c
                    .instrument
                    .articles
                    .get(&uid)
                    .map(|a| {
                        most_common(a, LEGAL_ARTICLE_FACET_LIMIT)
                            .into_iter()
                            .map(|(value, count)| FacetChoice {
                                label: lj_norm::article_key::display(&value),
                                value,
                                count,
                                parent: None,
                                hint: None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                LegalInstrumentFacet {
                    label: c
                        .instrument
                        .labels
                        .get(&uid)
                        .cloned()
                        .unwrap_or_else(|| uid.clone()),
                    slug: c.instrument.slugs.get(&uid).cloned().flatten(),
                    value: uid,
                    count,
                    articles,
                }
            })
            .collect();

    let mut article_top: Vec<FacetChoice> = Vec::new();
    for f in &li_facets {
        let slug = f.slug.as_deref().unwrap_or_default();
        for a in &f.articles {
            if !lj_norm::procedural::is_procedural(slug, &a.value) {
                article_top.push(FacetChoice {
                    value: format!("{}|{}", f.value, a.value),
                    label: format!("{} · {}", a.label, f.label),
                    count: a.count,
                    parent: None,
                    hint: None,
                });
            }
        }
    }
    article_top.sort_by_key(|c| std::cmp::Reverse(c.count));
    article_top.truncate(LEGAL_ARTICLE_TOP_LIMIT);

    SearchFacets {
        seat: seat_choices(&c.seats, refs),
        formation: formation_choices(&c.formation, refs),
        legal_domain: group_choices(&c.domain, refs),
        solution: uid_choices(&c.solution, refs),
        publication: scale_choices(&c.publication, refs),
        date_lecture_year: ranked(&c.year)
            .into_iter()
            .map(|(y, count)| FacetChoice {
                label: y.clone(),
                value: y,
                count,
                parent: None,
                hint: None,
            })
            .collect(),
        legal_instrument: li_facets,
        legal_article_top: article_top,
    }
}

/// Clé de tri naturelle d'un libellé, chiffres comparés en nombres : « 2e
/// chambre » avant « 10e chambre », et les formations numérotées avant les
/// nommées, leur première suite de texte étant vide.
fn label_sort_key(label: &str) -> Vec<(String, u64)> {
    let folded = lj_fold::fold_chars(label);
    let mut out: Vec<(String, u64)> = Vec::new();
    let mut rest = folded.as_str();
    while !rest.is_empty() {
        let digit = rest
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(rest.len());
        let (text, tail) = rest.split_at(digit);
        let end = tail
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(tail.len());
        let (digits, tail) = tail.split_at(end);
        out.push((text.to_string(), digits.parse().unwrap_or(0)));
        rest = tail;
    }
    out
}

/// Clé de tri d'une cour : sa ville, obtenue en retirant le libellé du type puis
/// l'article. Sur le libellé entier, l'apostrophe précède toute lettre et range
/// « Cour d'appel d'Orléans » entre Angers et Basse-Terre.
fn court_sort_key(label: &str, type_label: Option<&str>) -> Vec<(String, u64)> {
    let reste = type_label
        .and_then(|t| label.strip_prefix(t))
        .map(str::trim_start)
        .unwrap_or(label);
    let ville = ["de la ", "des ", "du ", "de ", "d'", "l'"]
        .iter()
        .find_map(|a| reste.strip_prefix(a))
        .unwrap_or(reste);
    label_sort_key(ville)
}

/// Tri d'un compteur en `(clé, count)` : count décroissant, tie-break par clé.
fn ranked(counter: &HashMap<String, i64>) -> Vec<(String, i64)> {
    let mut items: Vec<(String, i64)> = counter.iter().map(|(k, v)| (k.clone(), *v)).collect();
    items.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    items
}

/// Choix d'une facette plate keyée par **uid complet** : `value` = suffixe
/// d'uid, `label` résolu depuis le référentiel.
fn uid_choices(counter: &HashMap<String, i64>, refs: &Referential) -> Vec<FacetChoice> {
    ranked(counter)
        .into_iter()
        .map(|(uid, count)| choice(uid, count, refs))
        .collect()
}

/// Les mêmes choix, mais dans l'ordre du référentiel : une facette dont les
/// valeurs forment une échelle se lit du plus fort au plus faible, là où le
/// compteur mettrait l'inédit devant le rapport annuel.
fn scale_choices(counter: &HashMap<String, i64>, refs: &Referential) -> Vec<FacetChoice> {
    let mut uids: Vec<&String> = counter.keys().collect();
    uids.sort_by_key(|uid| (refs.rank(uid), uid.as_str()));
    uids.into_iter()
        .map(|uid| choice(uid.clone(), counter[uid], refs))
        .collect()
}

fn choice(uid: String, count: i64, refs: &Referential) -> FacetChoice {
    FacetChoice {
        value: uid_suffix(&uid).to_string(),
        label: refs.label(&uid).to_string(),
        count,
        parent: None,
        hint: refs.hint(&uid),
    }
}

// ── DB hydration / facets ────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub(crate) struct DecisionMeta {
    pub(crate) public_id: String,
    pub(crate) jurisdiction_type: String,
    pub(crate) jurisdiction_code: Option<String>,
    pub(crate) date_lecture: Option<String>,
    pub(crate) solution_uid: Option<String>,
    pub(crate) formation_paths: Vec<String>,
    pub(crate) office_uid: Option<String>,
    pub(crate) legal_domain_uid: Option<String>,
    pub(crate) publication_codes: Vec<String>,
    pub(crate) docket_numbers: Option<Vec<String>>,
    pub(crate) summary: Option<String>,
    pub(crate) summary_quote: Option<std::ops::Range<usize>>,
    pub(crate) summary_body: Option<std::ops::Range<usize>>,
    pub(crate) chars: Option<i64>,
    /// Axes formation : position recomposée + type de formation, pour le siège du
    /// titre des hits.
    pub(crate) seat: Option<lj_core::chamber::Seat>,
    pub(crate) seat_paths: Vec<String>,
    pub(crate) formation_uid: Option<String>,
    pub(crate) official_url: Option<String>,
    pub(crate) case_name: Option<String>,
    /// `text_uid|num_key`, as the composite array stores them: the slug and the
    /// rank are resolved per page.
    pub(crate) cited_terms: Vec<String>,
}

/// The page path of every decision of the list.
pub(crate) async fn fetch_decision_paths(
    conn: &Client,
    decision_ids: &[i64],
) -> std::result::Result<HashMap<i64, String>, StoreError> {
    if decision_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = conn
        .query(
            "SELECT d.id, d.public_id, d.jurisdiction_type, d.jurisdiction_code \
             FROM decisions d WHERE d.id = ANY($1)",
            &[&decision_ids],
        )
        .await?;
    let mut paths: HashMap<i64, String> = HashMap::new();
    for row in &rows {
        let jt = JurisdictionType::from_code(row.get(2))
            .expect("a stored jurisdiction_type is a known variant");
        paths.insert(
            row.get(0),
            lj_dtos::decision_path(jt, row.get(3), row.get(1)),
        );
    }
    Ok(paths)
}

/// Seat facet: one choice per path, `parent` being the path minus its last
/// segment. Orders, types and courts take their label from the referential,
/// which names them better than their segment does; rooms take theirs from the
/// grammar of the path.
fn seat_choices(seats: &HashMap<String, i64>, refs: &Referential) -> Vec<FacetChoice> {
    let mut rows: Vec<(&String, &i64)> = seats.iter().collect();
    rows.sort_by_cached_key(|(path, _)| {
        let segments: Vec<&str> = path.split('.').collect();
        let libelle = seat_label(path, refs);
        let ordre = match segments.len() {
            1 | 2 => Vec::new(),
            3 => court_sort_key(&libelle, None),
            _ => label_sort_key(&libelle),
        };
        (
            order_rank(segments[0]),
            segments.get(1).map_or(i32::MIN, |kind| {
                refs.rank(&format!("jurisdiction_type:{kind}"))
            }),
            segments.len().min(4),
            ordre,
            path.to_string(),
        )
    });
    rows.into_iter()
        .map(|(path, count)| FacetChoice {
            label: seat_label(path, refs),
            value: path.clone(),
            count: *count,
            parent: path.rsplit_once('.').map(|(parent, _)| parent.to_string()),
            hint: None,
        })
        .collect()
}

/// Formation facet: one choice per path, `parent` being the path minus its last
/// segment, the order and the court type labelled as in the seat facet.
fn formation_choices(paths: &HashMap<String, i64>, refs: &Referential) -> Vec<FacetChoice> {
    let mut rows: Vec<(&String, &i64)> = paths.iter().collect();
    rows.sort_by_cached_key(|(path, _)| {
        let segments: Vec<&str> = path.split('.').collect();
        (
            order_rank(segments[0]),
            segments.get(1).map_or(i32::MIN, |kind| {
                refs.rank(&format!("jurisdiction_type:{kind}"))
            }),
            segments.len(),
            lj_core::formation::formation_rank(path),
        )
    });
    rows.into_iter()
        .map(|(path, count)| FacetChoice {
            label: lj_core::formation::formation_label(path)
                .map(str::to_string)
                .unwrap_or_else(|| seat_label(path, refs)),
            value: path.clone(),
            count: *count,
            parent: path.rsplit_once('.').map(|(parent, _)| parent.to_string()),
            hint: None,
        })
        .collect()
}

/// French law first, then the Union, then international law.
fn order_rank(order: &str) -> u8 {
    match order {
        "FR" => 0,
        "EU" => 1,
        "INTL" => 2,
        _ => 3,
    }
}

/// Label of a seat node, folded back onto its segment when nothing names it.
fn seat_label(path: &str, refs: &Referential) -> String {
    let Some((_, rest)) = path.split_once('.') else {
        return lj_norm::referential_labels::jurisdiction_label(path)
            .unwrap_or(path)
            .to_string();
    };
    if !rest.contains('.') {
        return refs
            .jurisdiction_type_label(rest)
            .unwrap_or(rest)
            .to_string();
    }
    if let Some(label) = refs.court_label_of_path(path) {
        return label.to_string();
    }
    lj_core::chamber::seat_path_label(path)
        .unwrap_or_else(|| path.rsplit('.').next().unwrap_or(path).to_string())
}

/// Facette hiérarchisée (arbre 2 niveaux, domaine et matière) : le count d'un
/// groupe agrège ses annotations directes + celles de ses membres ; `value` =
/// suffixe d'uid, `parent` (membres) = suffixe du groupe.
fn group_choices(counter: &HashMap<String, i64>, refs: &Referential) -> Vec<FacetChoice> {
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
            hint: refs.hint(&uid),
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
            hint: refs.hint(&uid),
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
    // Extractions inside the lateral: read from the outer select, its ORDER BY/LIMIT
    // sort carries whole `source_fields` rows.
    let rows = conn
        .query(
            &format!("SELECT d.id, d.public_id, d.jurisdiction_type, d.jurisdiction_code, \
             d.date_lecture, d.solution_uid, d.formation_paths, d.office_uid, \
             d.legal_domain_uid, d.publication_codes, \
             d.docket_numbers, pt.summary, c.chars, \
             d.seat, d.seat_paths, d.formation_uid, \
             src.source, src.source_uid, src.court_code, src.published_url, \
             src.decision_date, src.first_number, \
             d.case_name, d.legal_article_composite, \
             CASE WHEN pt.summary_quote_start IS NOT NULL \
               THEN ARRAY[pt.summary_quote_start, pt.summary_quote_end] END, \
             CASE WHEN pt.summary_body_start IS NOT NULL \
               THEN ARRAY[pt.summary_body_start, pt.summary_body_end] END \
             FROM decisions d \
             LEFT JOIN LATERAL {text} pt ON true \
             LEFT JOIN LATERAL (SELECT max(char_end) AS chars FROM decision_chunks \
               WHERE decision_id = d.id) c ON true \
             LEFT JOIN LATERAL (SELECT source, source_uid, \
                 source_fields->'Dossier'->>'Code_Juridiction' AS court_code, \
                 COALESCE(source_fields->'META_JURI_CONSTIT'->>'URL_CC', \
                          source_fields->>'fiche_url', \
                          source_fields->>'url_site') AS published_url, \
                 source_fields->>'decision_date' AS decision_date, \
                 source_fields->'numbers'->>0 AS first_number \
               FROM decision_sources WHERE decision_id = d.id AND deleted_at IS NULL \
               ORDER BY source = ANY($2) DESC, source_rank DESC, (lang = 'fra') IS TRUE DESC, (lang = 'eng') IS TRUE DESC, source_uid ASC \
               LIMIT 1) src ON true \
             WHERE d.id = ANY($1)",
                text = lj_store::repository::pick::text(
                    "p.summary, p.summary_quote_start, p.summary_quote_end, \
                     p.summary_body_start, p.summary_body_end",
                    "d"
                )
            ),
            &[&decision_ids, &lj_core::official_url::LEGIFRANCE_SOURCES],
        )
        .await?;
    let mut out = HashMap::new();
    for r in &rows {
        let public_id: String = r.get(1);
        let date_raw: Option<chrono::NaiveDate> = r.get(4);
        let chars: Option<i32> = r.get(12);
        let summary: Option<String> = r.get(11);
        let summary_quote = r
            .get::<_, Option<Vec<i32>>>(24)
            .map(|q| q[0] as usize..q[1] as usize);
        let summary_body = summary.as_deref().and_then(|text| {
            let stored = r
                .get::<_, Option<Vec<i32>>>(25)
                .map(|b| b[0] as usize..b[1] as usize);
            lj_summary::summary_body(text, stored, summary_quote.clone())
        });
        out.insert(
            r.get::<_, i64>(0),
            DecisionMeta {
                public_id,
                jurisdiction_type: r.get(2),
                jurisdiction_code: r.get(3),
                date_lecture: date_raw.map(|d| d.to_string()),
                solution_uid: r.get(5),
                formation_paths: r.get::<_, Option<Vec<String>>>(6).unwrap_or_default(),
                office_uid: r.get(7),
                legal_domain_uid: r.get(8),
                publication_codes: r.get(9),
                docket_numbers: r.get(10),
                summary,
                summary_quote,
                summary_body,
                chars: chars.map(|c| c as i64),
                seat: r
                    .get::<_, Option<tokio_postgres::types::Json<lj_core::chamber::Seat>>>(13)
                    .map(|j| j.0),
                seat_paths: r.get::<_, Option<Vec<String>>>(14).unwrap_or_default(),
                formation_uid: r.get(15),
                official_url: r
                    .get::<_, Option<&str>>(16)
                    .zip(r.get::<_, Option<&str>>(17))
                    .and_then(|(source, source_uid)| {
                        lj_core::official_url::official_url(&lj_core::official_url::SourceRef {
                            source,
                            source_uid,
                            court_code: r.get(18),
                            published_url: r.get(19),
                            ariane_ref: r
                                .get::<_, Option<&str>>(20)
                                .zip(r.get::<_, Option<&str>>(21)),
                        })
                    }),
                case_name: r.get(22),
                cited_terms: r.get::<_, Option<Vec<String>>>(23).unwrap_or_default(),
            },
        );
    }
    Ok(out)
}

/// `full_text` (TOAST pglz) des décisions d'**une page** (≤ `limit`), par
/// `decision_id` : source unique du snippet. Le moteur de highlight re-tokenise
/// le texte fourni — aucun offset BM25 requis. Lu seulement pour les décisions
/// réellement affichées, sur un miss `page_memo`.
async fn load_full_texts(
    conn: &Client,
    decision_ids: &[i64],
) -> std::result::Result<HashMap<i64, String>, StoreError> {
    if decision_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = conn
        .query(
            &format!(
                "SELECT t.decision_id, t.full_text FROM decision_text t \
                 WHERE t.decision_id = ANY($1) AND {read}",
                read = lj_store::repository::pick::read("t")
            ),
            &[&decision_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1)))
        .collect())
}

/// Text slug and corpus-wide citing decision count, for the `text_uid|num_key`
/// terms of **one page**. The count is rebuilt weekly: a brand new article has
/// no row and passes for the rarest, which it is.
async fn load_article_ranks(
    conn: &Client,
    terms: &[String],
) -> std::result::Result<HashMap<String, (String, i64)>, StoreError> {
    if terms.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = conn
        .query(
            "SELECT u.term, t.slug, coalesce(c.decision_count, 0) \
             FROM unnest($1::text[]) AS u(term) \
             JOIN legal_text t ON t.text_uid = split_part(u.term, '|', 1) \
             LEFT JOIN citing_decision_counts c ON c.cited_term = u.term \
             WHERE t.slug IS NOT NULL",
            &[&terms],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get::<_, String>(0), (r.get::<_, String>(1), r.get(2))))
        .collect())
}

/// Spans char `(char_start, char_end)` des chunks gagnants ANN d'une page, par
/// `chunk_id` : fenêtre de fallback du snippet pour un hit purement sémantique
/// (aucun terme requête dans `full_text`) — on montre alors la région du chunk
/// gagnant plutôt que la tête de la décision.
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

/// Tag référentiel d'un uid optionnel (`solution:REJET` → `{REJET, Rejet}`).
pub(crate) fn opt_tag(uid: &Option<String>, refs: &Referential) -> Option<FacetTag> {
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

/// Titre d'affichage des cartes/topbar : « <juridiction résolue>,
/// <date FR>, <n°> » — **sans** le siège (rendu à part, [`display_seat`], en 2ᵉ
/// ligne). Une seule ligne, ne wrappe pas. Le titre BM25 (`search_title`), lui,
/// garde le siège pour le matching.
pub(crate) fn display_title(m: &DecisionMeta, refs: &Referential) -> String {
    let jur_display = display_jurisdiction(m, refs);
    lj_core::titles::decision_title(
        &jur_display,
        None,
        m.date_lecture.as_deref(),
        m.case_name.as_deref(),
        m.docket_numbers
            .as_deref()
            .and_then(lj_core::titles::docket_display)
            .as_deref(),
    )
}

/// Siège recomposé depuis les axes structurés (chambre · formation/office),
/// rendu en 2ᵉ ligne sous le titre. `None` si aucun axe.
pub(crate) fn display_seat(m: &DecisionMeta, refs: &Referential) -> Option<String> {
    let jur_display = display_jurisdiction(m, refs);
    crate::titles::decision_seat(
        &jur_display,
        m.seat
            .as_ref()
            .and_then(lj_core::chamber::Seat::label)
            .as_deref(),
        m.formation_uid.as_deref(),
        m.office_uid.as_deref(),
    )
}

/// The rarest articles the decision cites, grouped under their text: the slug
/// runs to 63 characters and a hit cites the same code four times. The
/// procedural bank is dropped; a term the catalogue does not name goes with it,
/// being neither linkable nor filterable. Sorting by count then by key keeps
/// the answer stable between two weekly rebuilds.
fn cited_articles(
    terms: &[String],
    ranks: &HashMap<String, (String, i64)>,
) -> BTreeMap<String, Vec<String>> {
    let mut kept: Vec<(i64, &str, &str)> = terms
        .iter()
        .filter_map(|term| {
            let (slug, count) = ranks.get(term)?;
            let num = term.split_once('|')?.1;
            (!lj_norm::procedural::is_procedural(slug, num)).then_some((*count, slug.as_str(), num))
        })
        .collect();
    kept.sort_unstable();
    kept.truncate(HIT_CITED_ARTICLES_LIMIT);
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (_, slug, num) in kept {
        out.entry(slug.to_string())
            .or_default()
            .push(num.to_string());
    }
    out
}

/// The docket the query bit on, said only when the title does not name it:
/// `docket_display` titles a decision by its head number alone, and one judgment
/// can join many pourvois. `query_keys` are
/// canonical, like the `docket_keys` field the leg admits on.
fn matched_docket(query_keys: &[String], numbers: &[String]) -> Option<String> {
    let head = lj_core::titles::docket_display(numbers)?;
    numbers
        .iter()
        .map(|n| n.trim())
        .filter(|n| !n.is_empty() && *n != head)
        .find(|n| query_keys.contains(&lj_core::docket_key::docket_key(n)))
        .map(str::to_string)
}

#[allow(clippy::too_many_arguments)]
fn make_hit(
    m: &DecisionMeta,
    score: f64,
    chunk: &LegHit,
    title_html: String,
    summary: Option<String>,
    refs: &Referential,
    ranks: &HashMap<String, (String, i64)>,
    query_keys: &[String],
) -> SearchHit {
    let head = summary
        .as_deref()
        .map(|s| lj_summary::summary_head(s).chars().count());
    let clip =
        |r: &std::ops::Range<usize>| head.filter(|&n| r.start < n).map(|n| r.start..r.end.min(n));
    SearchHit {
        id: m.public_id.clone(),
        jurisdiction_type: JurisdictionType::from_code(&m.jurisdiction_type)
            .unwrap_or(JurisdictionType::Ta),
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
        formation: crate::titles::formation_tags(&m.formation_paths),
        legal_domain: opt_tag(&m.legal_domain_uid, refs),
        seat_path: crate::titles::seat_path_tag(&m.seat_paths),
        publication_codes: m.publication_codes.clone(),
        best_chunk: BestChunk {
            chunk_index: chunk.chunk_index,
            snippet: chunk.snippet.clone().unwrap_or_default(),
        },
        chars: m.chars,
        summary_quote: m.summary_quote.as_ref().and_then(clip),
        summary_body: m.summary_body.as_ref().and_then(clip),
        summary: summary.as_deref().map(lj_summary::summary_teaser),
        official_url: m.official_url.clone(),
        cited_articles: cited_articles(&m.cited_terms, ranks),
        matched_docket: m
            .docket_numbers
            .as_deref()
            .and_then(|numbers| matched_docket(query_keys, numbers)),
    }
}

/// Assemble les `SearchHit` d'**une seule page** (≤ `limit`) à partir de la
/// metadata déjà hydratée ([`hydrate_decisions`]). Accès DB : le `full_text` des
/// ≤`limit` décisions affichées ([`load_full_texts`]) + les spans char des
/// chunks gagnants ANN ([`load_chunk_spans`], fenêtre de fallback). Suit le
/// highlight tantivy de `full_text` + titres de la page (Tier 2, mémoïsé par
/// page). Le snippet vient du texte décision.
pub(crate) async fn assemble_page(
    conn: &Client,
    ranked_page: &[(i64, f64, LegHit)],
    meta: &HashMap<i64, DecisionMeta>,
    query_text: &str,
    refs: &Referential,
    snippet_chars: usize,
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
    let query_keys: Vec<String> = lj_core::docket_key::query_numbers(query_text)
        .into_iter()
        .map(|n| n.key)
        .collect();
    let chunk_spans = load_chunk_spans(conn, &ann_chunk_ids)
        .await
        .map_err(ApiError::Store)?;
    let page_terms: Vec<String> = display
        .iter()
        .filter_map(|(d, _, _)| meta.get(d))
        .flat_map(|m| m.cited_terms.iter().cloned())
        .collect::<std::collections::HashSet<String>>()
        .into_iter()
        .collect();
    let ranks = load_article_ranks(conn, &page_terms)
        .await
        .map_err(ApiError::Store)?;

    let body_docs: Vec<(i64, String)> = display
        .iter()
        .filter_map(|(d, _, _)| full_texts.get(d).map(|t| (*d, t.clone())))
        .collect();
    // `highlight` tokenise les textes + sélectionne les fragments : CPU pur (le
    // tokenizer regex domine), offloadé sur `spawn_blocking` — le runtime est
    // `current_thread` (contrainte SSR Leptos).
    let query_body = query_text.to_string();
    let body_task =
        tokio::task::spawn_blocking(move || highlight(&body_docs, &query_body, snippet_chars));

    // Titres composés depuis les référentiels (jamais la colonne `search_title`,
    // qui porte la formation source brute) : highlight puis fallback verbatim.
    let mut titles: HashMap<i64, String> = display
        .iter()
        .filter_map(|(d, _, _)| meta.get(d).map(|m| (*d, display_title(m, refs))))
        .collect();
    // A query carrying a number does not highlight the title: its digit runs land
    // inside another number, `C-782/19` marking the `19` of `C-776/19`, and a court
    // with its day marks the title whole. Which number was bitten is said beside
    // the card. What is left is prose, short enough to tokenise on the spot.
    if query_keys.is_empty() {
        let docs: Vec<(i64, String)> = titles.iter().map(|(d, t)| (*d, t.clone())).collect();
        titles.extend(highlight(&docs, query_text, TITLE_SNIPPET_CHARS));
    }

    let body_snippets = body_task
        .await
        .map_err(|e| ApiError::Internal(format!("body snippet task: {e}")))?;

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
        let title_html = titles.get(&decision_id).cloned().unwrap_or_default();
        // Résumé garanti en base et déjà joint au hot path : embarqué
        // dans chaque hit quel que soit le mode → vue « Résumé » instantanée côté
        // front, sans aller-retour `/summary`.
        let summary = decision_meta.summary.clone();
        hits.push(make_hit(
            decision_meta,
            score,
            &display_chunk,
            title_html,
            summary,
            refs,
            &ranks,
            &query_keys,
        ));
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_store::repository::FacetValueRow;

    /// A digit run sorts as a number: `2e chambre` comes before `10e chambre`.
    #[test]
    fn label_sort_key_compte_les_chiffres_en_nombres() {
        let mut labels = vec![
            "Étrangers et rétention",
            "10e chambre",
            "Pôle 6, 8e chambre",
            "2e chambre",
            "Chambre sociale",
            "Pôle 1, 11e chambre",
            "1re chambre",
        ];
        labels.sort_by_key(|l| label_sort_key(l));
        assert_eq!(
            labels,
            [
                "1re chambre",
                "2e chambre",
                "10e chambre",
                "Chambre sociale",
                "Étrangers et rétention",
                "Pôle 1, 11e chambre",
                "Pôle 6, 8e chambre",
            ]
        );
    }

    #[test]
    fn matched_docket_names_the_joined_number_and_never_the_head() {
        let key = |n: &str| lj_core::docket_key::docket_key(n);
        let joined: Vec<String> = ["C-776/19", "C-782/19"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            matched_docket(&[key("c 782/19")], &joined).as_deref(),
            Some("C-782/19")
        );
        assert_eq!(matched_docket(&[key("C-776/19")], &joined), None);
        assert_eq!(matched_docket(&[key("C-999/19")], &joined), None);
        assert_eq!(matched_docket(&[key("C-776/19")], &joined[..1]), None);
    }

    #[test]
    fn cited_articles_drop_the_bank_and_serve_the_rarest() {
        let ranks: HashMap<String, (String, i64)> = [
            ("CPC|700", ("code-de-procedure-civile", 1_100_000)),
            ("CPC|1240", ("code-de-procedure-civile", 87_000)),
            ("CIV|1240", ("code-civil", 87_000)),
            ("CIV|1101", ("code-civil", 40_000)),
            ("CIV|815-13", ("code-civil", 900)),
            ("CIV|2224", ("code-civil", 12_000)),
            ("CIV|1109", ("code-civil", 3_000)),
            ("CIV|544", ("code-civil", 24_000)),
            ("CIV|9", ("code-civil", 50)),
            ("CJA|l600-1-1", ("code-de-justice-administrative", 200)),
            ("CJA|l761-1", ("code-de-justice-administrative", 1_000_000)),
        ]
        .into_iter()
        .map(|(term, (slug, n))| (term.to_string(), (slug.to_string(), n)))
        .collect();
        let terms: Vec<String> = [
            "CPC|700",
            "CIV|1240",
            "CIV|1101",
            "CIV|815-13",
            "CIV|2224",
            "CIV|1109",
            "CIV|544",
            "CIV|9",
            "CJA|l600-1-1",
            "CJA|l761-1",
            "ABSENT|1",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            cited_articles(&terms, &ranks),
            BTreeMap::from([
                (
                    "code-civil".to_string(),
                    vec![
                        "9".to_string(),
                        "815-13".to_string(),
                        "1109".to_string(),
                        "2224".to_string()
                    ]
                ),
                (
                    "code-de-justice-administrative".to_string(),
                    vec!["l600-1-1".to_string()]
                ),
            ])
        );
        // The bank keys on the slug: 1240 of the CPC does not exist, and the
        // number alone would have dropped article 1240 of the code civil.
        assert_eq!(
            cited_articles(&["CPC|1240".to_string()], &ranks),
            BTreeMap::from([(
                "code-de-procedure-civile".to_string(),
                vec!["1240".to_string()]
            )])
        );
        assert!(cited_articles(&["CPC|700".to_string()], &ranks).is_empty());
    }

    #[test]
    fn court_sort_key_range_les_cours_par_leur_ville() {
        let mut labels = vec![
            "Cour d'appel de Nîmes",
            "Cour d'appel d'Agen",
            "Cour d'appel de Paris",
            "Cour d'appel d'Orléans",
            "Cour d'appel de Basse-Terre",
        ];
        labels.sort_by_key(|l| court_sort_key(l, Some("Cour d'appel")));
        assert_eq!(
            labels,
            [
                "Cour d'appel d'Agen",
                "Cour d'appel de Basse-Terre",
                "Cour d'appel de Nîmes",
                "Cour d'appel d'Orléans",
                "Cour d'appel de Paris",
            ]
        );
    }

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
            hint: None,
        }
    }

    fn jr(code: &str, jt: &str, label: &str) -> lj_store::repository::JurisdictionRow {
        lj_store::repository::JurisdictionRow {
            code: code.to_string(),
            source_code: code.to_uppercase(),
            jurisdiction_type: jt.to_string(),
            city: None,
            label: label.to_string(),
            place: None,
        }
    }

    /// Une cour seule sous son type et homonyme de lui n'est pas un étage : elle
    /// se replie et rend ses formations à la racine. Une cour parmi d'autres, ou
    /// nommée autrement, tient sa place.
    #[test]
    fn la_cour_unique_homonyme_de_son_type_se_replie() {
        let refs = Referential::new(
            vec![
                FacetValueRow {
                    sort: 1,
                    ..fv("jurisdiction_type:CC", "Cour de cassation", None)
                },
                FacetValueRow {
                    sort: 2,
                    ..fv("jurisdiction_type:CA", "Cour d'appel", None)
                },
                FacetValueRow {
                    sort: 12,
                    ..fv(
                        "jurisdiction_type:CNDA",
                        "Cour nationale du droit d'asile",
                        None,
                    )
                },
            ],
            vec![
                jr("cc", "CC", "Cour de cassation"),
                jr("ca_paris", "CA", "Cour d'appel de Paris"),
                jr("cnda", "CNDA", "CNDA"),
            ],
            Vec::new(),
        );
        let choices = seat_choices(
            &HashMap::from([
                ("FR".to_string(), 300_071i64),
                ("FR.CA".to_string(), 200_000),
                ("FR.CC".to_string(), 100_000),
                ("FR.CNDA".to_string(), 71),
                ("FR.CC.SOCIALE".to_string(), 30_000),
                ("FR.CA.PARIS".to_string(), 40_000),
                ("FR.CA.PARIS.C3".to_string(), 900),
            ]),
            &refs,
        );
        let roots: Vec<&str> = choices
            .iter()
            .filter(|c| c.parent.as_deref() == Some("FR"))
            .map(|c| c.value.as_str())
            .collect();
        assert_eq!(
            roots,
            ["FR.CC", "FR.CA", "FR.CNDA"],
            "l'ordre vient du référentiel"
        );
        let at = |value: &str| {
            choices
                .iter()
                .find(|c| c.value == value)
                .unwrap_or_else(|| panic!("{value} absent"))
        };
        assert!(
            !choices.iter().any(|c| c.value == "FR.CC.CC"),
            "une cour homonyme de son type n'ouvre pas de niveau"
        );
        assert_eq!(at("FR.CC.SOCIALE").parent.as_deref(), Some("FR.CC"));
        assert_eq!(at("FR.CA.PARIS").parent.as_deref(), Some("FR.CA"));
        assert_eq!(at("FR.CA.PARIS").label, "Cour d'appel de Paris");
        assert_eq!(at("FR.CA.PARIS.C3").parent.as_deref(), Some("FR.CA.PARIS"));
        assert_eq!(at("FR.CA.PARIS.C3").label, "3e chambre");
    }

    /// La publication est une échelle : le rang du référentiel gagne sur le
    /// compteur, sinon l'inédit ouvre le menu parce qu'il est le plus nombreux.
    #[test]
    fn publication_facet_reads_in_referential_order() {
        let refs = Referential::new(
            vec![
                FacetValueRow {
                    sort: 1,
                    ..fv("publication:PUBLIE_RAPPORT", "Publié au rapport", None)
                },
                FacetValueRow {
                    sort: 2,
                    ..fv("publication:PUBLIE_BULLETIN", "Publié au bulletin", None)
                },
                FacetValueRow {
                    sort: 5,
                    ..fv("publication:INEDIT_BULLETIN", "Inédit au bulletin", None)
                },
            ],
            vec![],
            Vec::new(),
        );
        let counter = HashMap::from([
            ("publication:INEDIT_BULLETIN".to_string(), 4000i64),
            ("publication:PUBLIE_BULLETIN".to_string(), 300),
            ("publication:PUBLIE_RAPPORT".to_string(), 2),
        ]);
        let scaled = scale_choices(&counter, &refs);
        let values: Vec<&str> = scaled.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(
            values,
            ["PUBLIE_RAPPORT", "PUBLIE_BULLETIN", "INEDIT_BULLETIN"]
        );
        assert_eq!(
            uid_choices(&counter, &refs)[0].value,
            "INEDIT_BULLETIN",
            "les facettes plates gardent l'ordre du compteur"
        );
    }

    #[test]
    fn group_facet_aggregates_members_under_group() {
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
            Vec::new(),
        );
        let counter = HashMap::from([
            ("legal_domain:CIVIL_DROIT_LOCATIF".to_string(), 3i64),
            ("legal_domain:CIVIL".to_string(), 1),
            ("legal_domain:FISCAL".to_string(), 2),
        ]);
        let choices = group_choices(&counter, &refs);
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
    fn juridiction_facet_nests_types_codes_et_formations() {
        let refs = Referential::new(
            vec![fv("jurisdiction_type:TJ", "Tribunal judiciaire", None)],
            vec![lj_store::repository::JurisdictionRow {
                code: "tj_le_havre".to_string(),
                source_code: "tj76351".to_string(),
                jurisdiction_type: "TJ".to_string(),
                city: Some("Le Havre".to_string()),
                label: "Tribunal judiciaire du Havre".to_string(),
                place: None,
            }],
            Vec::new(),
        );
        let choices = seat_choices(
            &HashMap::from([
                ("FR".to_string(), 5i64),
                ("FR.TJ".to_string(), 5),
                ("FR.TJ.LE_HAVRE".to_string(), 4),
                ("FR.TJ.LE_HAVRE.C3".to_string(), 2),
            ]),
            &refs,
        );
        assert_eq!(choices[0].value, "FR");
        assert_eq!(choices[1].value, "FR.TJ");
        assert_eq!(choices[1].label, "Tribunal judiciaire");
        let child = choices
            .iter()
            .find(|c| c.value == "FR.TJ.LE_HAVRE")
            .unwrap();
        assert_eq!(child.parent.as_deref(), Some("FR.TJ"));
        assert_eq!(child.label, "Tribunal judiciaire du Havre");
        let chambre = choices
            .iter()
            .find(|c| c.value == "FR.TJ.LE_HAVRE.C3")
            .unwrap();
        assert_eq!(chambre.parent.as_deref(), Some("FR.TJ.LE_HAVRE"));
        assert_eq!((chambre.label.as_str(), chambre.count), ("3e chambre", 2));
    }

    #[test]
    fn le_referentiel_qualifie_le_libelle_de_chambre() {
        let refs = Referential::new(
            vec![fv("jurisdiction_type:CC", "Cour de cassation", None)],
            vec![lj_store::repository::JurisdictionRow {
                code: "cc".to_string(),
                source_code: "cc".to_string(),
                jurisdiction_type: "CC".to_string(),
                city: None,
                label: "Cour de cassation".to_string(),
                place: None,
            }],
            Vec::new(),
        );
        let choices = seat_choices(
            &HashMap::from([
                ("FR".to_string(), 9i64),
                ("FR.CC".to_string(), 9),
                ("FR.CC.C2_CIVILE".to_string(), 5),
                ("FR.CC.SOCIALE".to_string(), 4),
            ]),
            &refs,
        );
        let label = |v: &str| {
            choices
                .iter()
                .find(|c| c.value == v)
                .map(|c| c.label.as_str())
        };
        assert_eq!(label("FR.CC.C2_CIVILE"), Some("2e chambre civile"));
        assert_eq!(label("FR.CC.SOCIALE"), Some("Chambre sociale"));
    }
}
