//! Lecture des décisions (détail, sections, similaires).

use std::collections::{HashMap, HashSet};

use lj_core::decision::Decision;
use lj_core::truecase;
use lj_dtos::{
    CitationSpan, CitationTarget, DecisionDetail, DecisionPreview, DecisionSection,
    DecisionSourceXml, FacetTag, JurisdictionType, LegalRefArticle, LegalReference,
    SimilarDecisionHit, SimilarDecisionsResponse,
};
use lj_parse::FromSourceFields;

use tracing::instrument;

use crate::error::{ApiError, Result};
use crate::referential::{referential, Referential};
use crate::state::AppState;

/// Facteur d'élargissement de la fenêtre ANN interne.
const SIMILAR_INNER_LIMIT_FACTOR: i64 = 24;

/// Sur-fetch du volet espèce : absorbe les exclusions avant troncature à `limit`.
const ESPECE_OVERFETCH_FACTOR: i64 = 3;

/// Probes vchordrq de la jambe KNN pré-filtrée portée notable.
const PRINCIPE_KNN_PROBES: u32 = 100;

/// Head page of the « citée par » block; the total of citing decisions comes with it.
const CITANTES_LIMIT: i64 = 5;

/// Codes de publication « portée notable » = MAJEURE ∪ IMPORTANTE.
const NOTABLE_CODES: [&str; 8] = ["r", "A", "b", "l", "c", "B", "C+", "R"];

/// Jambe IDF : nombre max de termes article retenus sur la décision source.
const IDF_MAX_RARE_TERMS: i64 = 8;

/// Jambe IDF : df au-delà duquel un article est trop cité pour signer la
/// question de droit.
const IDF_RARE_DF_CAP: i64 = 100_000;

/// Titres de sommaire canoniques par `kind`.
fn section_label(kind: &str) -> Option<&'static str> {
    match kind {
        "preamble" => Some("Introduction"),
        "procedure" => Some("Procédure"),
        "visa" => Some("Visa"),
        "expose" => Some("Exposé du litige"),
        "moyens" => Some("Moyens"),
        "chapeau" => Some("Chapeau"),
        "motivations" => Some("Motivations"),
        "dispositif" => Some("Dispositif"),
        "annexes" => Some("Moyens annexés"),
        _ => None,
    }
}

/// Paragraphes du corps : lignes non vides du texte nettoyé.
pub fn decision_paragraphs(decision: &Decision) -> Vec<String> {
    decision
        .texte_integral_clean
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Global range `[start, end)` (codepoints of `texte_integral_clean` =
/// `decisions.full_text`) of each rendered paragraph, index for index with
/// `paragraphs`. A paragraph not found gets the empty range `(cursor, cursor)`,
/// to which no span attaches.
///
/// The cursor carries both the codepoint and the byte offset: only the gap since
/// the cursor is counted, never the text from byte 0.
pub fn paragraph_global_ranges(text: &str, paragraphs: &[String]) -> Vec<(usize, usize)> {
    let mut char_cursor = 0usize;
    let mut byte_cursor = 0usize;
    let mut ranges = Vec::with_capacity(paragraphs.len());
    for paragraph in paragraphs {
        match text[byte_cursor..].find(paragraph.as_str()) {
            Some(gap) => {
                let start = char_cursor + text[byte_cursor..byte_cursor + gap].chars().count();
                let end = start + paragraph.chars().count();
                byte_cursor += gap + paragraph.len();
                char_cursor = end;
                ranges.push((start, end));
            }
            None => ranges.push((char_cursor, char_cursor)),
        }
    }
    ranges
}

fn decision_source_meta(decision: &Decision) -> DecisionSourceXml {
    DecisionSourceXml {
        nom_jurisdiction_type: decision.jurisdiction_name.clone(),
        numero_dossier: decision.numero_dossier.clone(),
        date_lecture: decision.date_lecture.clone(),
        formation_jugement: decision.formation.clone(),
        type_recours: decision.type_recours.clone(),
        solution: decision.solution.clone(),
    }
}

/// Replie la structure autoritative (`decision.sections`) en blocs de
/// paragraphes pour le sommaire/corps front.
///
/// Chaque paragraphe est rattaché — par ses offsets char — à la section
/// canonique qui le couvre, en partition stricte. Les paragraphes sont émis
/// dans l'ordre du texte ; seuls les runs consécutifs de même `kind` sont
/// fusionnés. `None` quand aucune section autoritative (`start_char >= 0`)
/// n'existe.
pub fn sections_from_decision(
    decision: &Decision,
    paragraphs: &[String],
    ranges: &[(usize, usize)],
    citations: &[GlobalCitation],
) -> Option<Vec<DecisionSection>> {
    // (start_char, end_char, kind, label), trié par start_char.
    let mut bounds: Vec<(usize, usize, &str, &str)> = decision
        .sections
        .iter()
        .filter(|s| s.start_char != lj_parse::SECTION_NO_OFFSET)
        .map(|s| (s.start_char, s.end_char, s.kind.as_str(), s.label.as_str()))
        .collect();
    bounds.sort_by_key(|b| b.0);

    if bounds.is_empty() {
        return None;
    }

    let section_for = |pos: usize| -> (&str, String) {
        let mut chosen = &bounds[0];
        for bound in &bounds {
            if bound.0 <= pos {
                chosen = bound;
            } else {
                break;
            }
        }
        let kind = chosen.2;
        let label = section_label(kind)
            .map(str::to_string)
            .unwrap_or_else(|| chosen.3.to_string());
        (kind, label)
    };

    let mut out: Vec<DecisionSection> = Vec::new();
    // Plages globales alignées sur `out[i].paragraphs[j]`.
    let mut out_ranges: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut seen_kinds: HashMap<String, usize> = HashMap::new();

    for (paragraph, &range) in paragraphs.iter().zip(ranges) {
        let pos = range.0;
        let (kind, label) = section_for(pos);

        if let Some(last) = out.last_mut() {
            if last.kind == kind {
                last.paragraphs.push(paragraph.clone());
                out_ranges
                    .last_mut()
                    .expect("out_ranges aligné sur out")
                    .push(range);
                continue;
            }
        }

        let count = seen_kinds.entry(kind.to_string()).or_insert(0);
        *count += 1;
        let anchor = if *count == 1 {
            kind.to_string()
        } else {
            format!("{kind}-{count}")
        };
        out.push(DecisionSection {
            kind: kind.to_string(),
            anchor,
            label,
            paragraphs: vec![paragraph.clone()],
            paragraph_spans: Vec::new(),
        });
        out_ranges.push(vec![range]);
    }

    if out.is_empty() {
        return None;
    }

    for (section, ranges) in out.iter_mut().zip(&out_ranges) {
        let spans: Vec<Vec<CitationSpan>> = ranges
            .iter()
            .map(|&(pstart, pend)| spans_for_range(citations, pstart, pend))
            .collect();
        if spans.iter().any(|s| !s.is_empty()) {
            section.paragraph_spans = spans;
        }
    }

    Some(out)
}

/// Une citation globale prête à projeter : `[global_start, global_end)` en
/// codepoints sur `texte_integral_clean` (= `decisions.full_text`) et ses cibles
/// résolues (une seule pour une mention simple ; toutes les bornes +
/// intermédiaires pour une plage fusionnée).
#[derive(Debug, Clone)]
pub struct GlobalCitation {
    pub start: usize,
    pub end: usize,
    pub targets: Vec<CitationTarget>,
}

/// Une mention liée brute : une ligne `legal_citation` résolue.
struct RawCite {
    start: usize,
    end: usize,
    text_uid: String,
    path: String,
    num_key: Option<String>,
    href: String,
    label: String,
    /// Span « N et suivants » : la cible est la famille TOC de l'ancre, résolue à
    /// l'assemblage.
    suivants: bool,
}

/// A citation address. The reading date serves the version the court applied:
/// without it a link may render a text the court never applied, the article having
/// taken a version since. The place serves the wording of the court's own territory,
/// which overseas decisions need, the Pacific ones most, legislative speciality
/// dropping most of metropolitan law.
fn citation_href(
    path: &str,
    num_key: Option<&str>,
    date: Option<&str>,
    place: Option<&str>,
) -> String {
    let mut url = match num_key {
        Some(k) => format!("{path}/{k}"),
        None => path.to_string(),
    };
    if let Some(d) = date.filter(|_| num_key.is_some()) {
        url.push('/');
        url.push_str(d);
    }
    if let Some(p) = place {
        url.push_str("?place=");
        url.push_str(crate::legi::place_code(p));
    }
    url
}

/// Deux mentions adjacentes forment-elles une plage d'articles
/// (« L. 225-1 à L. 225-9 ») ? Bornes du MÊME texte, toutes deux à article,
/// séparées du seul mot « à ».
fn is_article_range(a: &RawCite, b: &RawCite, text: &[char]) -> bool {
    a.num_key.is_some()
        && b.num_key.is_some()
        && a.text_uid == b.text_uid
        && b.start > a.end
        && b.start - a.end <= 8
        && text
            .get(a.end..b.start)
            .is_some_and(|gap| gap.iter().collect::<String>().trim() == "à")
}

/// Décompose une paire de bornes en `(radical, k1, k2)` quand la plage est
/// énumérable : même radical avant le dernier composant, composants finaux
/// entiers croissants (« L. 225-1 » / « L. 225-9 » → `("L. 225", 1, 9)` ;
/// « 102 » / « 108 » → `("", 102, 108)`). Bornes à suffixe non numérique
/// (« 50 sexies B à H ») ou radicaux différents → `None`, la plage reste aux
/// bornes. Cap à 50.
fn range_stem(num1: &str, num2: &str) -> Option<(String, u32, u32)> {
    fn split(num: &str) -> (&str, Option<u32>) {
        match num.rfind(['-', ' ']) {
            Some(i) => (&num[..i], num[i + 1..].parse().ok()),
            None => ("", num.parse().ok()),
        }
    }
    let (stem1, k1) = split(num1);
    let (stem2, k2) = split(num2);
    let (k1, k2) = (k1?, k2?);
    (stem1 == stem2 && k1 < k2 && k2 - k1 <= 50).then(|| (stem1.to_string(), k1, k2))
}

/// Clés candidates des articles intermédiaires d'une plage : les entiers
/// stricts entre les bornes (`exact`) et les sous-articles insérés de chaque
/// pas (`like`, « L. 225-3-1 » ; le pas de la borne basse est inclus, pas celui
/// de la borne haute).
fn range_candidate_keys(stem: &str, k1: u32, k2: u32) -> (Vec<String>, Vec<String>) {
    let key = |k: u32| {
        if stem.is_empty() {
            k.to_string()
        } else {
            format!("{stem}-{k}")
        }
    };
    let exact: Vec<String> = (k1 + 1..k2).map(key).collect();
    let like: Vec<String> = (k1..k2).map(|k| format!("{}-%", key(k))).collect();
    (exact, like)
}

/// Projette les citations globales tombant dans `[pstart, pend)` en spans LOCAUX
/// au paragraphe (offsets ramenés à `pstart`), demi-ouverts. Une citation n'est
/// retenue que si elle est entièrement contenue dans le paragraphe.
///
/// Chevauchement entre mentions → fusion en leur enveloppe avec union des
/// cibles (dédup par `(href, label)`) : un span pour N cibles, que le front rend
/// en lien simple pour 1 cible et en menu déroulant pour ≥ 2. Les spans rendus
/// sont triés par début, disjoints.
fn spans_for_range(citations: &[GlobalCitation], pstart: usize, pend: usize) -> Vec<CitationSpan> {
    let mut cands: Vec<(usize, usize, &GlobalCitation)> = citations
        .iter()
        .filter(|c| c.start >= pstart && c.end <= pend && c.end > c.start)
        .map(|c| (c.start - pstart, c.end - pstart, c))
        .collect();
    cands.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let mut kept: Vec<CitationSpan> = Vec::new();
    for (ls, le, c) in cands {
        match kept.last_mut() {
            Some(last) if ls < last.end => {
                last.end = last.end.max(le);
                for target in &c.targets {
                    if !last.targets.contains(target) {
                        last.targets.push(target.clone());
                    }
                }
            }
            _ => kept.push(CitationSpan {
                start: ls,
                end: le,
                targets: c.targets.clone(),
            }),
        }
    }
    kept
}

/// Spans cliquables alignés sur [`decision_paragraphs`] (corps plat, sans
/// sections autoritatives).
fn flat_paragraph_spans(
    ranges: &[(usize, usize)],
    citations: &[GlobalCitation],
) -> Vec<Vec<CitationSpan>> {
    let spans: Vec<Vec<CitationSpan>> = ranges
        .iter()
        .map(|&(pstart, pend)| spans_for_range(citations, pstart, pend))
        .collect();
    if spans.iter().any(|s| !s.is_empty()) {
        spans
    } else {
        Vec::new()
    }
}

/// Une référence brute lue de la DB : `(instrument, slug résolu, juridiction,
/// [(num affiché, ref_num_key résolu)])`. `None` = non ancré au catalogue, donc
/// pas de lien.
type RawLegalRef = (
    String,
    Option<String>,
    String,
    Vec<(String, Option<String>)>,
);

/// Construit les `LegalReference` exposées.
///
/// Un instrument cité sans article précis est conservé tel quel. Le `slug` et le
/// `numKey` résolus sont propagés au DTO pour bâtir les liens
/// `/norm/{juris}/{slug}/{numKey}` sans re-slugifier côté front. `None` si la liste
/// résultante est vide.
fn parse_legal_refs(raw: &[RawLegalRef]) -> Option<Vec<LegalReference>> {
    if raw.is_empty() {
        return None;
    }
    let mut refs: Vec<LegalReference> = Vec::new();
    for (instrument, slug, jurisdiction, original) in raw {
        if slug.is_none() {
            continue;
        }
        let mut seen = std::collections::HashSet::new();
        let articles: Vec<LegalRefArticle> = original
            .iter()
            .filter(|(num, _)| seen.insert(num.clone()))
            .map(|(num, num_key)| LegalRefArticle {
                num: num.clone(),
                num_key: num_key.clone().unwrap_or_default(),
            })
            .collect();
        refs.push(LegalReference {
            instrument: instrument.clone(),
            slug: slug.clone(),
            jurisdiction: jurisdiction.clone(),
            articles,
        });
    }
    if refs.is_empty() {
        None
    } else {
        Some(refs)
    }
}

fn parse_jur_type(raw: &str) -> Result<JurisdictionType> {
    JurisdictionType::from_code(raw)
        .ok_or_else(|| ApiError::Internal(format!("jurisdiction_type DB invalide : {raw:?}")))
}

/// Tag référentiel d'un uid optionnel (`solution:REJET` → `{REJET, Rejet}`).
fn opt_tag(uid: &Option<String>, refs: &Referential) -> Option<FacetTag> {
    uid.as_deref().map(|u| refs.tag(u))
}

/// Projette `themes` depuis `source_fields` : casefold + déduplication
/// ordre-stable, entrées vides retirées. Scalaire au lieu de liste, ou clé
/// absente ⇒ `Vec` vide.
fn project_themes(source_fields: &serde_json::Value) -> Vec<String> {
    let Some(arr) = source_fields.get("themes").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for v in arr {
        let Some(s) = v.as_str() else { continue };
        let theme = s.trim();
        if theme.is_empty() {
            continue;
        }
        if seen.insert(theme.to_lowercase()) {
            out.push(theme.to_string());
        }
    }
    out
}

/// Projette `nac` depuis `source_fields` : le littéral chaîne `"null"` et la
/// chaîne vide → `None` ; code brut sinon.
fn project_nac(source_fields: &serde_json::Value) -> Option<String> {
    let s = source_fields.get("nac")?.as_str()?.trim();
    if s.is_empty() || s == "null" {
        return None;
    }
    Some(s.to_string())
}

/// Triplet hydraté depuis la `Decision` reconstruite : paragraphes, sections,
/// méta XML.
type ParsedPayload = (
    Vec<String>,
    Vec<Vec<CitationSpan>>,
    Option<Vec<DecisionSection>>,
    Option<DecisionSourceXml>,
);

/// Détail complet d'une décision par `public_id` (paragraphes, sections, refs).
#[instrument(skip(state), fields(db.system = "postgresql", public_id = %public_id))]
/// The address of a decision under its jurisdiction, `None` for an unknown id.
pub async fn decision_address(state: &AppState, public_id: &str) -> Option<String> {
    let conn = state.pool.get().await.ok()?;
    let (jt, code) = lj_store::repository::DecisionRepository::new(&conn)
        .decision_jurisdiction(public_id)
        .await
        .ok()??;
    let jt = lj_dtos::JurisdictionType::from_code(&jt)?;
    Some(lj_dtos::decision_path(jt, code.as_deref(), public_id))
}

pub async fn get_decision(state: &AppState, public_id: &str) -> Result<DecisionDetail> {
    let refs = referential(state).await?;
    let conn = state.pool.get().await?;

    let row = conn
        .query_opt(
            &format!(
                "
            SELECT
              d.id,
              d.public_id,
              d.jurisdiction_type,
              pt.full_text,
              ds.source_fields,
              d.solution_uid,
              d.formation_paths,
              d.office_uid,
              d.legal_domain_uid,
              d.publication_codes,
              d.date_lecture::text,
              d.date_audience::text,
              d.jurisdiction_code,
              d.docket_numbers,
              d.seat,
              pt.summary,
              d.ecli,
              ds.source,
              d.formation_uid,
              ariane.source_fields,
              notes.files,
              web.notes,
              ds.source_uid,
              d.case_name,
              d.seat_paths,
              j.place,
              legi.source,
              legi.source_uid,
              CASE WHEN pt.summary_quote_start IS NOT NULL
                THEN ARRAY[pt.summary_quote_start, pt.summary_quote_end] END,
              (SELECT ds4.source_fields->>'summary' FROM decision_sources ds4
                WHERE ds4.decision_id = d.id AND ds4.deleted_at IS NULL AND ds4.source = 'judilibre'
                  AND btrim(ds4.source_fields->>'summary') <> ''
                LIMIT 1),
              CASE WHEN pt.summary_body_start IS NOT NULL
                THEN ARRAY[pt.summary_body_start, pt.summary_body_end] END,
              pt.summary_engine
            FROM decisions d
            LEFT JOIN jurisdiction j ON j.code = d.jurisdiction_code
            LEFT JOIN LATERAL {text} pt ON true
            LEFT JOIN LATERAL (
                SELECT source_fields, source, source_uid
                FROM decision_sources
                WHERE decision_id = d.id AND deleted_at IS NULL
                ORDER BY (source_uid = pt.source_uid) IS TRUE DESC, source_rank DESC, (lang = 'fra') IS TRUE DESC, (lang = 'eng') IS TRUE DESC, source_uid ASC
                LIMIT 1
            ) ds ON true
            LEFT JOIN LATERAL (
                SELECT source, source_uid
                FROM decision_sources
                WHERE decision_id = d.id AND deleted_at IS NULL
                  AND source = ANY($2)
                ORDER BY source_rank DESC, source_uid ASC
                LIMIT 1
            ) legi ON true
            LEFT JOIN LATERAL (
                SELECT source_fields
                FROM decision_sources
                WHERE decision_id = d.id AND deleted_at IS NULL
                  AND source = 'ariane-web'
                LIMIT 1
            ) ariane ON true
            LEFT JOIN LATERAL (
                SELECT source_fields->'files' AS files
                FROM decision_sources
                WHERE decision_id = d.id AND deleted_at IS NULL
                  AND source = 'judilibre'
                  AND jsonb_array_length(COALESCE(source_fields->'files', '[]'::jsonb)) > 0
                LIMIT 1
            ) notes ON true
            LEFT JOIN LATERAL (
                SELECT jsonb_agg(c) AS notes
                FROM decision_sources ds3
                CROSS JOIN LATERAL jsonb_array_elements(ds3.source_fields->'commentaires') c
                WHERE ds3.decision_id = d.id AND ds3.deleted_at IS NULL
                  AND ds3.source <> 'ariane-web'
                  AND jsonb_typeof(ds3.source_fields->'commentaires') = 'array'
            ) web ON true
            WHERE d.public_id = $1 AND d.deleted_at IS NULL
            ",
                text = lj_store::repository::pick::text(
                    "p.full_text, p.summary, p.summary_quote_start, p.summary_quote_end, \
                     p.summary_body_start, p.summary_body_end, p.source_uid, p.summary_engine",
                    "d",
                ),
            ),
            &[&public_id, &lj_core::official_url::LEGIFRANCE_SOURCES],
        )
        .await?;

    let row = row.ok_or(ApiError::NotFound)?;

    let decision_id: i64 = row.get(0);
    let id: String = row.get(1);
    let jur_type_raw: String = row.get(2);
    let full_text: Option<String> = row.get(3);
    let source_fields: Option<serde_json::Value> = row.get(4);
    let solution_uid: Option<String> = row.get(5);
    let formation_paths: Vec<String> = row.get::<_, Option<Vec<String>>>(6).unwrap_or_default();
    let office_uid: Option<String> = row.get(7);
    let legal_domain_uid: Option<String> = row.get(8);
    let publication_codes: Vec<String> = row.get(9);
    let date_lecture: Option<String> = row.get(10);
    let date_audience: Option<String> = row.get(11);
    let jurisdiction_code: Option<String> = row.get(12);
    let docket_numbers_raw: Option<Vec<String>> = row.get(13);
    let seat_stored: Option<lj_core::chamber::Seat> = row
        .get::<_, Option<tokio_postgres::types::Json<lj_core::chamber::Seat>>>(14)
        .map(|j| j.0);
    let seat_paths: Vec<String> = row.get::<_, Option<Vec<String>>>(24).unwrap_or_default();
    let court_place: Option<String> = row.get(25);
    let summary: Option<String> = row.get(15);
    let summary_quote = row
        .get::<_, Option<Vec<i32>>>(28)
        .map(|q| q[0] as usize..q[1] as usize);
    let ecli: Option<String> = row.get(16);
    let source: Option<String> = row.get(17);
    let formation_uid: Option<String> = row.get(18);
    let ariane_bundle: Option<serde_json::Value> = row.get(19);
    let judilibre_files: Option<serde_json::Value> = row.get(20);
    let web_notes: Option<serde_json::Value> = row.get(21);
    let source_uid: Option<String> = row.get(22);
    let case_name: Option<String> = row.get(23);
    let legifrance: Option<(String, String)> = row
        .get::<_, Option<String>>(26)
        .zip(row.get::<_, Option<String>>(27));
    let judilibre_summary: Option<String> = row.get(29);
    let twins: Vec<(String, Vec<String>)> = match judilibre_summary.as_deref() {
        Some(summary) if lj_parse::cassation_summary_numbers_arrets(summary) => conn
            .query(
                &format!(
                    "
                    SELECT {head}, t.docket_numbers
                    FROM decisions d
                    JOIN decisions t ON t.jurisdiction_code = d.jurisdiction_code
                      AND t.date_lecture = d.date_lecture
                      AND t.id <> d.id AND t.deleted_at IS NULL
                    JOIN decision_sources s ON s.decision_id = t.id
                      AND s.deleted_at IS NULL AND s.source = 'judilibre'
                    WHERE d.id = $1 AND s.source_fields->>'summary' = $2
                    ",
                    head = lj_store::repository::pick::text("left(p.full_text, 64)", "t"),
                ),
                &[&decision_id, &summary],
            )
            .await?
            .iter()
            .map(|r| {
                (
                    r.get::<_, Option<String>>(0).unwrap_or_default(),
                    r.get::<_, Option<Vec<String>>>(1).unwrap_or_default(),
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    let as_this_arret = |summary: &str| {
        lj_parse::cassation_summary_for(
            summary,
            full_text.as_deref().unwrap_or_default(),
            docket_numbers_raw.as_deref().unwrap_or_default(),
            &twins,
        )
    };
    let cassation_summary = judilibre_summary.as_deref().map(as_this_arret);
    let stored_body = row
        .get::<_, Option<Vec<i32>>>(30)
        .map(|b| b[0] as usize..b[1] as usize);
    let (summary, summary_quote, stored_body) = match (summary, summary_quote) {
        (Some(text), Some(quote)) => {
            let (text, quote, body) = requote(&text, quote, stored_body, as_this_arret);
            (Some(text), Some(quote), body)
        }
        (summary, quote) => (summary, quote, stored_body),
    };
    let pasted_summary = summary
        .clone()
        .filter(|_| row.get::<_, Option<i16>>(31) == Some(lj_summary::COURT_SUMMARY_ENGINE));
    let summary_body = summary
        .as_deref()
        .and_then(|text| lj_summary::summary_body(text, stored_body, summary_quote.clone()));

    let jur_type = parse_jur_type(&jur_type_raw)?;
    let jurisdiction_name = jurisdiction_code
        .as_deref()
        .and_then(|c| refs.jurisdiction(c))
        .map(|j| j.label.clone());

    let ref_rows = conn
        .query(
            "
            SELECT lt.title AS instrument,
                   lt.slug,
                   lt.jurisdiction,
                   lt.text_uid,
                   COALESCE(array_agg(DISTINCT el->>3 ORDER BY el->>3)
                            FILTER (WHERE el->>3 IS NOT NULL),
                            ARRAY[]::text[]) AS num_keys
            FROM legal_citation lc
            CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
            JOIN legal_text lt ON lt.text_uid = el->>2
            WHERE lc.decision_id = $1
            GROUP BY lt.text_uid, lt.title, lt.slug, lt.jurisdiction
            ORDER BY 1
            ",
            &[&decision_id],
        )
        .await?;

    let span_rows = conn
        .query(
            "
            SELECT (el->>0)::int AS char_start,
                   (el->>1)::int AS char_end,
                   lt.text_uid,
                   lt.slug,
                   lt.jurisdiction,
                   el->>3 AS ref_num_key,
                   lt.title AS label,
                   (EXISTS (SELECT 1 FROM legal_article a
                            WHERE a.text_uid = lt.text_uid)
                    OR coalesce(length(lt.body), 0) > 0) AS readable,
                   (el->>4)::bool AS suivants
            FROM legal_citation lc
            CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
            JOIN legal_text lt ON lt.text_uid = el->>2
            WHERE lc.decision_id = $1
            ",
            &[&decision_id],
        )
        .await?;

    let case_rows = conn
        .query(
            "
            SELECT cc.char_start,
                   cc.char_end,
                   t.public_id,
                   t.jurisdiction_code,
                   t.date_lecture::text,
                   t.docket_numbers,
                   t.jurisdiction_type
            FROM case_citation cc
            JOIN decisions t ON t.id = cc.target_decision_id
            WHERE cc.decision_id = $1 AND t.deleted_at IS NULL
            ",
            &[&decision_id],
        )
        .await?;

    let cite_date = date_lecture.clone();
    let cite_place = court_place.clone();
    let read_from = |path: String| citation_href(&path, None, None, cite_place.as_deref());
    let article_href = |path: &str, num_key: &str| {
        citation_href(
            path,
            Some(num_key),
            cite_date.as_deref(),
            cite_place.as_deref(),
        )
    };

    // Per cited text, what its reception is read from and its address.
    let cited_texts: HashMap<String, (String, String, String)> = span_rows
        .iter()
        .filter_map(|r| {
            let slug: Option<String> = r.get(3);
            let jurisdiction: String = r.get(4);
            let path = lj_vocab::norm_path(&jurisdiction, &slug?);
            Some((r.get(2), (jurisdiction, r.get(6), path)))
        })
        .collect();

    let mut raw: Vec<RawCite> = span_rows
        .iter()
        .filter_map(|r| {
            let start: i32 = r.get(0);
            let end: i32 = r.get(1);
            let text_uid: String = r.get(2);
            let slug: Option<String> = r.get(3);
            let jurisdiction: String = r.get(4);
            let num_key: Option<String> = r.get(5);
            let label: String = r.get(6);
            let readable: bool = r.get(7);
            let suivants: bool = r.get(8);
            let path = lj_vocab::norm_path(&jurisdiction, &slug?);
            let num_key = num_key.filter(|k| !k.is_empty());
            let href = match num_key.as_deref() {
                Some(k) => article_href(&path, k),
                None if readable => read_from(path.clone()),
                None => return None,
            };
            Some(RawCite {
                start: start.max(0) as usize,
                end: end.max(0) as usize,
                text_uid,
                path,
                num_key,
                href,
                label,
                suivants,
            })
        })
        .collect();
    raw.sort_by_key(|c| c.start);

    let text_chars: Vec<char> = full_text.as_deref().unwrap_or_default().chars().collect();

    // Plages d'articles (« L. 225-1 à L. 225-9 ») : la capture pose une ligne par
    // borne écrite ; la plage — un span de borne à borne — est reconstituée ici, et
    // ses articles intermédiaires résolus contre le catalogue.
    let single = |c: &RawCite| CitationTarget {
        href: Some(c.href.clone()),
        label: c.label.clone(),
    };
    let mut citations: Vec<GlobalCitation> = Vec::with_capacity(raw.len());
    // Membres des plages recollées (bornes + intermédiaires), par `text_uid` : ils
    // rejoignent la liste « Références juridiques » du texte cité.
    let mut range_members: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut cited_keys: HashSet<(String, String)> = raw
        .iter()
        .filter_map(|c| Some((c.text_uid.clone(), c.num_key.clone()?)))
        .collect();
    let mut it = raw.into_iter().peekable();
    while let Some(a) = it.next() {
        let ranged = it
            .peek()
            .is_some_and(|b| is_article_range(&a, b, &text_chars));
        if !ranged {
            // « N et suivants » : la famille TOC de l'ancre en menu, comme une plage.
            // `_suivants_family_keys` porte la sémantique (section unique, arête en vigueur
            // à la date de lecture de la décision, cap 20 ; NULL sinon, l'ancre reste alors
            // un lien simple). Les clés publiques de la famille repassent en numéro
            // d'affichage (`legal_article.num`).
            let family: Vec<String> = match (a.suivants, a.num_key.as_deref()) {
                (true, Some(nk)) => conn
                    .query(
                        "
                        SELECT coalesce(
                                 (SELECT la.num FROM legal_article la
                                  WHERE la.text_uid = $1 AND la.num_key = t.k
                                  ORDER BY la.date_debut DESC LIMIT 1), t.k)
                        FROM unnest(_suivants_family_keys($1, $2,
                                 (SELECT coalesce(d.date_lecture, CURRENT_DATE)
                                  FROM decisions d WHERE d.id = $3)))
                             WITH ORDINALITY AS t(k, ord)
                        ORDER BY t.ord
                        ",
                        &[
                            &a.text_uid,
                            &lj_norm::article_key::article_key(nk),
                            &decision_id,
                        ],
                    )
                    .await?
                    .iter()
                    .map(|r| r.get(0))
                    .collect(),
                _ => Vec::new(),
            };
            let targets = if family.is_empty() {
                vec![single(&a)]
            } else {
                let members = range_members.entry(a.text_uid.clone()).or_default();
                members.extend(family.iter().cloned());
                family
                    .iter()
                    .map(|nk| CitationTarget {
                        href: Some(article_href(
                            &a.path,
                            &lj_norm::article_key::article_key(nk),
                        )),
                        label: format!("{nk} — {}", a.label),
                    })
                    .collect()
            };
            citations.push(GlobalCitation {
                start: a.start,
                end: a.end,
                targets,
            });
            continue;
        }
        let b = it.next().expect("peek a garanti un suivant");
        // Intermédiaires = clés numériquement entre les bornes ET existantes au
        // catalogue, sous-articles insérés compris. Bornes non énumérables
        // (« 50 sexies B à H ») → la plage garde ses deux bornes.
        let mut inter: Vec<String> = Vec::new();
        if let Some((stem, k1, k2)) = range_stem(
            a.num_key.as_deref().expect("gardé par is_article_range"),
            b.num_key.as_deref().expect("gardé par is_article_range"),
        ) {
            let (exact, like) = range_candidate_keys(&stem, k1, k2);
            inter = conn
                .query(
                    "
                    SELECT DISTINCT num_key FROM legal_article
                    WHERE text_uid = $1 AND (num_key = ANY($2) OR num_key LIKE ANY($3))
                    ",
                    &[&a.text_uid, &exact, &like],
                )
                .await?
                .iter()
                .map(|r| r.get::<_, String>(0))
                .collect();
            inter.sort_by_key(|nk| {
                let tail = nk.strip_prefix(&stem).unwrap_or(nk);
                let k: u32 = tail
                    .trim_start_matches(['-', ' '])
                    .split('-')
                    .next()
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0);
                (k, nk.clone())
            });
        }
        let bound = |c: &RawCite| CitationTarget {
            href: Some(c.href.clone()),
            label: format!(
                "{} — {}",
                lj_norm::article_key::display(
                    c.num_key.as_deref().expect("gardé par is_article_range")
                ),
                c.label
            ),
        };
        let members = range_members.entry(a.text_uid.clone()).or_default();
        members.push(a.num_key.clone().expect("gardé par is_article_range"));
        members.extend(inter.iter().cloned());
        members.push(b.num_key.clone().expect("gardé par is_article_range"));
        let mut targets = vec![bound(&a)];
        targets.extend(inter.into_iter().map(|nk| CitationTarget {
            href: Some(article_href(&a.path, &nk)),
            label: format!(
                "{} \u{2014} {}",
                lj_norm::article_key::display(&nk),
                a.label
            ),
        }));
        targets.push(bound(&b));
        citations.push(GlobalCitation {
            start: a.start,
            end: b.end,
            targets,
        });
    }

    // A court with a place links no article that does not apply there on the
    // day of the decision. One without, national or metropolitan, keeps every
    // link: a national court also judges the territories.
    let not_applicable: HashSet<(String, String)> = match court_place.as_deref() {
        Some(place) => {
            cited_keys.extend(range_members.iter().flat_map(|(uid, keys)| {
                keys.iter()
                    .map(|k| (uid.clone(), lj_norm::article_key::article_key(k)))
            }));
            let cited: Vec<crate::legi::CitedArticle> = cited_keys
                .iter()
                .filter_map(|(uid, key)| {
                    let (jurisdiction, title, _) = cited_texts.get(uid)?;
                    Some(crate::legi::CitedArticle {
                        text_uid: uid.clone(),
                        jurisdiction: jurisdiction.clone(),
                        title: title.clone(),
                        num_key: key.clone(),
                    })
                })
                .collect();
            let day = date_lecture
                .as_deref()
                .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .unwrap_or_else(|| chrono::Utc::now().date_naive());
            let repo = lj_store::repository::DecisionRepository::new(&conn);
            crate::legi::not_applicable_at(state, &repo, place, day, &cited).await?
        }
        None => HashSet::new(),
    };
    let dropped: HashSet<String> = not_applicable
        .iter()
        .filter_map(|(uid, key)| Some(article_href(&cited_texts.get(uid)?.2, key)))
        .collect();
    for target in citations.iter_mut().flat_map(|c| c.targets.iter_mut()) {
        if target.href.as_ref().is_some_and(|h| dropped.contains(h)) {
            target.href = None;
        }
    }

    for r in &case_rows {
        let start: i32 = r.get(0);
        let end: i32 = r.get(1);
        let target_pid: String = r.get(2);
        let code: Option<String> = r.get(3);
        let date: Option<String> = r.get(4);
        let dockets: Option<Vec<String>> = r.get(5);
        let target_type = parse_jur_type(r.get(6))?;
        let mut label = code
            .as_deref()
            .and_then(|c| refs.jurisdiction(c))
            .map(|j| j.label.clone())
            .unwrap_or_else(|| "Décision".to_string());
        if let Some(d) = date {
            label.push_str(&format!(", {d}"));
        }
        if let Some(n) = dockets.as_deref().and_then(|d| d.first()) {
            label.push_str(&format!(", n° {n}"));
        }
        citations.push(GlobalCitation {
            start: start.max(0) as usize,
            end: end.max(0) as usize,
            targets: vec![CitationTarget {
                href: Some(lj_dtos::decision_path(
                    target_type,
                    code.as_deref(),
                    &target_pid,
                )),
                label,
            }],
        });
    }
    citations.sort_by_key(|c| c.start);

    let raw_refs: Vec<RawLegalRef> = ref_rows
        .iter()
        .map(|r| {
            let instrument: String = r.get(0);
            let slug: Option<String> = r.get(1);
            let jurisdiction: String = r.get(2);
            let text_uid: String = r.get(3);
            let mut num_keys: Vec<String> = r.get(4);
            if let Some(members) = range_members.get(&text_uid) {
                num_keys.extend(members.iter().cloned());
                num_keys.sort();
                num_keys.dedup();
            }
            let articles: Vec<(String, Option<String>)> = num_keys
                .into_iter()
                .map(|k| {
                    let key = lj_norm::article_key::article_key(&k);
                    let linked = !not_applicable.contains(&(text_uid.clone(), key.clone()));
                    (lj_norm::article_key::display(&key), linked.then_some(key))
                })
                .collect();
            (instrument, slug, jurisdiction, articles)
        })
        .collect();

    // Chronologie de l'affaire : décisions atteignables par les liens
    // appel/pourvoi/renvoi résolus, traversés dans les deux sens. Profondeur bornée
    // à 8 ; l'UNION coupe les cycles.
    let chrono_rows = conn
        .query(
            "
            WITH RECURSIVE chain(id, depth) AS (
                SELECT $1::bigint, 0
                UNION
                SELECT CASE WHEN dl.decision_id = c.id THEN dl.target_decision_id
                            ELSE dl.decision_id END,
                       c.depth + 1
                FROM chain c
                JOIN decision_links dl
                  ON dl.target_decision_id IS NOT NULL
                 AND (dl.decision_id = c.id OR dl.target_decision_id = c.id)
                WHERE c.depth < 8
            )
            SELECT d.public_id,
                   d.jurisdiction_code,
                   d.jurisdiction_type,
                   d.date_lecture::text,
                   d.id = $1 AS is_current,
                   d.id,
                   d.solution_uid,
                   d.docket_numbers
            FROM decisions d
            JOIN (SELECT DISTINCT id FROM chain) c ON c.id = d.id
            WHERE d.deleted_at IS NULL
            ORDER BY d.date_lecture DESC NULLS LAST, d.public_id
            ",
            &[&decision_id],
        )
        .await?;

    let chain_ids: Vec<i64> = chrono_rows.iter().map(|r| r.get::<_, i64>(5)).collect();
    let chrono_edges = conn
        .query(
            "
            SELECT s.public_id, t.public_id, dl.target_court, dl.target_docket,
                   dl.target_date::text, dl.link_type
            FROM decision_links dl
            JOIN decisions s ON s.id = dl.decision_id
            LEFT JOIN decisions t ON t.id = dl.target_decision_id
            WHERE dl.decision_id = ANY($1)
              AND (dl.target_decision_id = ANY($1) OR dl.target_decision_id IS NULL)
            ",
            &[&chain_ids],
        )
        .await?;

    let (citantes_total, citantes) = lj_store::repository::DecisionRepository::new(&conn)
        .decisions_citing(decision_id, &chain_ids, CITANTES_LIMIT)
        .await?;

    // La connexion est rendue au pool avant le parse offloadé, qui ne touche plus
    // la DB.
    drop(conn);

    let chronology = chronology(&refs, &chrono_rows, &chrono_edges);

    // `source_fields` est un JSONB hétérogène par source : forme inattendue ⇒
    // champ vide, jamais d'erreur.
    let (themes, nac) = match source_fields.as_ref() {
        Some(sf) => (project_themes(sf), project_nac(sf)),
        None => (Vec::new(), None),
    };

    let legifrance_url = legifrance.as_ref().and_then(|(source, source_uid)| {
        lj_core::official_url::official_url(&lj_core::official_url::SourceRef {
            source,
            source_uid,
            court_code: None,
            published_url: None,
            ariane_ref: None,
        })
    });
    let official_url = legifrance_url.or_else(|| {
        source
            .as_deref()
            .zip(source_uid.as_deref())
            .and_then(|(source, source_uid)| {
                let field = |path: [&str; 2]| {
                    source_fields
                        .as_ref()
                        .and_then(|f| f.get(path[0]))
                        .and_then(|f| f.get(path[1]))
                        .and_then(serde_json::Value::as_str)
                };
                let top = |key: &str| {
                    source_fields
                        .as_ref()
                        .and_then(|f| f.get(key))
                        .and_then(serde_json::Value::as_str)
                };
                lj_core::official_url::official_url(&lj_core::official_url::SourceRef {
                    source,
                    source_uid,
                    court_code: field(["Dossier", "Code_Juridiction"]),
                    published_url: field(["META_JURI_CONSTIT", "URL_CC"])
                        .or_else(|| top("fiche_url"))
                        .or_else(|| top("url_site")),
                    ariane_ref: top("decision_date").zip(
                        source_fields
                            .as_ref()
                            .and_then(|f| f.get("numbers"))
                            .and_then(|n| n.get(0))
                            .and_then(serde_json::Value::as_str),
                    ),
                })
            })
    });

    // Reconstruction `(full_text, source_fields)` → `Decision` puis dérivation
    // paragraphes/sections : CPU-bloquant, donc offloadé sur `spawn_blocking`.
    let (paragraphs, paragraph_spans, sections, source_xml) =
        if let (Some(full_text), Some(source_fields)) = (full_text, source_fields) {
            // The scraped fonds take their own parser only under their uid prefix.
            let uid = source_uid.clone().unwrap_or_else(|| public_id.to_string());
            tokio::task::spawn_blocking(move || -> ParsedPayload {
                let decision = Decision::from_source_fields(&full_text, &source_fields, &uid);
                let paragraphs = decision_paragraphs(&decision);
                let ranges = paragraph_global_ranges(&decision.texte_integral_clean, &paragraphs);
                let sections = sections_from_decision(&decision, &paragraphs, &ranges, &citations);
                // Spans on the flat body, the front's fallback without sections.
                let flat_spans = flat_paragraph_spans(&ranges, &citations);
                (
                    paragraphs,
                    flat_spans,
                    sections,
                    Some(decision_source_meta(&decision)),
                )
            })
            .await
            .map_err(|e| ApiError::Internal(format!("payload parse task: {e}")))?
        } else {
            (Vec::new(), Vec::new(), None, None)
        };

    let legal_references = parse_legal_refs(&raw_refs);

    let docket_numbers = docket_numbers_raw.filter(|d| !d.is_empty());

    // Siège recomposé depuis les axes structurés, même composition que
    // `search_title` à l'ingest.
    let jur_display = crate::titles::decision_jurisdiction(
        refs.jurisdiction_type_label(&jur_type_raw)
            .unwrap_or(&jur_type_raw),
        jurisdiction_name.as_deref(),
    );
    let chamber_label = seat_stored.as_ref().and_then(lj_core::chamber::Seat::label);
    let seat = crate::titles::decision_seat(
        &jur_display,
        chamber_label.as_deref(),
        formation_uid.as_deref(),
        office_uid.as_deref(),
    );
    let title = lj_core::titles::decision_title(
        &jur_display,
        seat.as_deref(),
        date_lecture.as_deref(),
        case_name.as_deref(),
        docket_numbers
            .as_deref()
            .and_then(lj_core::titles::docket_display)
            .as_deref(),
    );

    let seat_path = crate::titles::seat_path_tag(&seat_paths);

    Ok(DecisionDetail {
        id,
        jurisdiction_type: jur_type,
        title,
        case_name,
        paragraphs,
        paragraph_spans,
        sections,
        summary,
        summary_quote,
        summary_body,
        jurisdiction_code,
        jurisdiction_name,
        court_place: court_place
            .as_deref()
            .map(|p| crate::legi::place_code(p).to_string()),
        date_lecture,
        solution: opt_tag(&solution_uid, &refs),
        formation: crate::titles::formation_tags(&formation_paths),
        legal_domain: opt_tag(&legal_domain_uid, &refs),
        publication_codes,
        date_audience,
        docket_numbers,
        seat,
        seat_path,
        legal_references,
        source_xml,
        themes,
        nac,
        ecli,
        source,
        official_url,
        chronology,
        commentaires: {
            let mut c: Vec<lj_dtos::Commentaire> = cassation_summary
                .or_else(|| ariane_bundle.as_ref().and_then(analysis_from_bundle))
                .or(pasted_summary)
                .map(court_summary)
                .into_iter()
                .collect();
            if let Some(bundle) = ariane_bundle.as_ref() {
                c.extend(conclusions_from_bundle(bundle));
            }
            if let Some(files) = judilibre_files.as_ref() {
                c.extend(notes_from_judilibre_files(files));
            }
            if let Some(notes) = web_notes.as_ref() {
                c.extend(notes_from_web_bundle(notes));
            }
            c
        },
        citantes: citantes
            .into_iter()
            .map(|r| crate::legi::citing_dto(r, &refs))
            .collect(),
        citantes_total: (citantes_total > CITANTES_LIMIT).then_some(citantes_total),
    })
}

/// Case chronology, most recent first: the decisions of the chain (`rows`) and,
/// for every link left pending, the step its key names without leading
/// anywhere. A lone step is no chronology.
fn chronology(
    refs: &Referential,
    rows: &[tokio_postgres::Row],
    edges: &[tokio_postgres::Row],
) -> Vec<lj_dtos::ChronologyEntry> {
    // Ordering and edge key: `public_id` of a step held by the corpus, the
    // rendered address of a pending one.
    let mut steps: Vec<(String, lj_dtos::ChronologyEntry)> = rows
        .iter()
        .map(|r| {
            let public_id: String = r.get(0);
            let code: Option<String> = r.get(1);
            let jt: String = r.get(2);
            let solution_uid: Option<String> = r.get(6);
            (
                public_id.clone(),
                lj_dtos::ChronologyEntry {
                    id: Some(public_id),
                    jurisdiction_type: lj_dtos::JurisdictionType::from_code(&jt),
                    label: jurisdiction_label(refs, code.as_deref(), &jt),
                    jurisdiction_code: code,
                    date: r.get(3),
                    current: r.get(4),
                    solution: solution_uid.as_deref().map(|u| refs.tag(u).key),
                    docket_numbers: r.get(7),
                    link: None,
                },
            )
        })
        .collect();

    let mut seen: HashSet<(String, Option<String>)> = steps
        .iter()
        .map(|(_, e)| (e.label.clone(), e.date.clone()))
        .collect();
    let mut links: HashMap<(String, String), String> = HashMap::new();
    for row in edges {
        let source: String = row.get(0);
        let target: Option<String> = row.get(1);
        let court: Option<String> = row.get(2);
        let docket: Option<String> = row.get(3);
        let date: Option<String> = row.get(4);
        let link_type: String = row.get(5);
        let Some(target) = target else {
            let Some(step) = crate::chronology::pending_step(
                court.as_deref(),
                docket.as_deref(),
                date.as_deref(),
                refs,
            ) else {
                continue;
            };
            let arete = format!(
                "{}|{}|{}",
                court.unwrap_or_default(),
                docket.unwrap_or_default(),
                date.unwrap_or_default()
            );
            if seen.insert((step.label.clone(), Some(step.date.clone()))) {
                steps.push((
                    arete.clone(),
                    lj_dtos::ChronologyEntry {
                        id: None,
                        jurisdiction_type: None,
                        jurisdiction_code: None,
                        label: step.label,
                        date: Some(step.date),
                        current: false,
                        solution: None,
                        docket_numbers: step.docket.map(|d| vec![d]),
                        link: None,
                    },
                ));
            }
            links.insert((source, arete), link_type);
            continue;
        };
        links.insert((source, target), link_type);
    }

    if steps.len() < 2 {
        return Vec::new();
    }
    steps.sort_by(|a, b| b.1.date.cmp(&a.1.date).then_with(|| a.0.cmp(&b.0)));
    // A link reads from the more recent step to the decision it attacks.
    for i in 0..steps.len() - 1 {
        steps[i].1.link = links
            .get(&(steps[i].0.clone(), steps[i + 1].0.clone()))
            .cloned();
    }
    steps.into_iter().map(|(_, e)| e).collect()
}

/// Nom de la cour, repli sur le libellé du type de juridiction.
fn jurisdiction_label(refs: &Referential, code: Option<&str>, jur_type: &str) -> String {
    code.and_then(|c| refs.jurisdiction(c))
        .map(|j| j.label.clone())
        .unwrap_or_else(|| {
            refs.jurisdiction_type_label(jur_type)
                .unwrap_or(jur_type)
                .to_string()
        })
}

/// Commentaires doctrine web (`source_fields.commentaires[]` des fournisseurs
/// autres qu'ArianeWeb) → DTO. Entrées `note` auto-suffisantes : le lien est
/// stocké tel quel.
fn notes_from_web_bundle(notes: &serde_json::Value) -> Vec<lj_dtos::Commentaire> {
    notes
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|c| c["kind"].as_str() == Some("note") && c["url"].is_string())
                .map(|c| lj_dtos::Commentaire {
                    kind: "note".to_string(),
                    author: c["author"].as_str().map(str::to_string),
                    date: c["date"].as_str().map(str::to_string),
                    title: c["title"].as_str().map(str::to_string),
                    publisher: c["publisher"].as_str().map(str::to_string),
                    volume: c["volume"].as_str().map(str::to_string),
                    issue: c["issue"].as_str().map(str::to_string),
                    pages: c["pages"].as_str().map(str::to_string),
                    url: c["url"].as_str().map(str::to_string),
                    body: None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Libellé public d'un type de document lié Judilibre (taxonomie `filetype`).
/// `None` = type non doctrinal (graphiques, décision annotée) → écarté.
fn judilibre_filetype_label(t: &str) -> Option<&'static str> {
    Some(match t {
        "prep_rapp" => "Rapport du conseiller",
        "prep_raco" => "Rapport complémentaire du conseiller",
        "prep_avpg" => "Avis du procureur général",
        "prep_avis" => "Avis de l'avocat général",
        "prep_oral" => "Avis oral de l'avocat général",
        "prep_avco" => "Avis complémentaire de l'avocat général",
        "comm_comm" => "Communiqué",
        "comm_note" => "Note explicative",
        "comm_nora" => "Notice au rapport annuel",
        "comm_lett" => "Lettre de chambre",
        "comm_trad" => "Arrêt traduit",
        _ => return None,
    })
}

/// Documents liés Judilibre (`files[]`) → commentaires `note` : rapports, avis,
/// communiqués, notes explicatives. Lien direct vers le PDF public (`rawUrl`).
/// Les graphiques et la décision annotée (`datt_*`) sont écartés.
fn notes_from_judilibre_files(files: &serde_json::Value) -> Vec<lj_dtos::Commentaire> {
    files
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    let label = judilibre_filetype_label(f["type"].as_str()?)?;
                    let url = f["rawUrl"].as_str()?.to_string();
                    Some(lj_dtos::Commentaire {
                        kind: "note".to_string(),
                        author: None,
                        date: f["date"].as_str().map(str::to_string),
                        title: Some(label.to_string()),
                        publisher: Some("Cour de cassation".to_string()),
                        volume: None,
                        issue: None,
                        pages: None,
                        url: Some(url),
                        body: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The court's own summary, as its source publishes it: the Judilibre
/// `summary`, the body of the ArianeWeb analysis, titrage included, or the summary
/// pasted on the text read. One published in capitals is recased for display.
fn court_summary(text: String) -> lj_dtos::Commentaire {
    let text = text.trim();
    let body = truecase::recased_summary(text).unwrap_or_else(|| text.to_string());
    lj_dtos::Commentaire {
        kind: "summary".to_string(),
        author: None,
        date: None,
        title: None,
        publisher: None,
        volume: None,
        issue: None,
        pages: None,
        url: None,
        body: Some(body),
    }
}

/// A stored summary with its quoted court words read anew, its codepoint bounds moved.
fn requote(
    summary: &str,
    quote: std::ops::Range<usize>,
    body: Option<std::ops::Range<usize>>,
    read: impl Fn(&str) -> String,
) -> (
    String,
    std::ops::Range<usize>,
    Option<std::ops::Range<usize>>,
) {
    let chars: Vec<char> = summary.chars().collect();
    let Some(quoted) = chars.get(quote.clone()) else {
        return (summary.to_string(), quote, body);
    };
    let read: Vec<char> = read(&String::from_iter(quoted)).chars().collect();
    let shift = |i: usize| {
        if i >= quote.end {
            i + read.len() - quote.len()
        } else {
            i
        }
    };
    let text = chars[..quote.start]
        .iter()
        .chain(&read)
        .chain(&chars[quote.end..])
        .collect();
    (
        text,
        quote.start..quote.start + read.len(),
        body.map(|b| shift(b.start)..shift(b.end)),
    )
}

fn analysis_from_bundle(bundle: &serde_json::Value) -> Option<String> {
    bundle["commentaires"].as_array()?.iter().find_map(|c| {
        (c["kind"].as_str() == Some("analyse"))
            .then(|| c["body"].as_str())
            .flatten()
            .filter(|body| !body.trim().is_empty())
            .map(str::to_string)
    })
}

/// ArianeWeb bundle → its `conclusions` link line: the document itself is never
/// stored (rights reserved), its existence is, and the link line is composed
/// here, the single place to fix should the Conseil d'Etat change its URL scheme.
fn conclusions_from_bundle(bundle: &serde_json::Value) -> Vec<lj_dtos::Commentaire> {
    let (Some(date), Some(dossier)) = (bundle["date_lecture"].as_str(), bundle["dossier"].as_str())
    else {
        return Vec::new();
    };
    let present = bundle["commentaires"]
        .as_array()
        .is_some_and(|a| a.iter().any(|c| c["kind"].as_str() == Some("conclusions")));
    if !present {
        return Vec::new();
    }
    vec![lj_dtos::Commentaire {
        kind: "conclusions".to_string(),
        author: None,
        date: Some(date.to_string()),
        title: None,
        publisher: None,
        volume: None,
        issue: None,
        pages: None,
        url: Some(format!(
            "https://www.conseil-etat.fr/fr/arianeweb/CRP/conclusion/{date}/{dossier}"
        )),
        body: None,
    }]
}

/// Prévisualisation légère d'une décision par `public_id` : identité + solution
/// + codes de publication + résumé, sans lire le corps.
#[instrument(skip(state), fields(db.system = "postgresql", public_id = %public_id))]
pub async fn decision_preview(state: &AppState, public_id: &str) -> Result<DecisionPreview> {
    let refs = referential(state).await?;
    let conn = state.pool.get().await?;

    let row = conn
        .query_opt(
            &format!(
                "
            SELECT
              d.public_id,
              d.jurisdiction_type,
              d.jurisdiction_code,
              d.date_lecture::text,
              d.docket_numbers,
              d.solution_uid,
              d.formation_paths,
              d.publication_codes,
              {summary},
              d.seat,
              d.formation_uid,
              d.office_uid
            FROM decisions d
            WHERE d.public_id = $1 AND d.deleted_at IS NULL
            ",
                summary = lj_store::repository::pick::text("p.summary", "d")
            ),
            &[&public_id],
        )
        .await?;
    let row = row.ok_or(ApiError::NotFound)?;

    let id: String = row.get(0);
    let jur_type_raw: String = row.get(1);
    let jurisdiction_code: Option<String> = row.get(2);
    let date_lecture: Option<String> = row.get(3);
    let docket_numbers: Option<Vec<String>> = row.get(4);
    let solution_uid: Option<String> = row.get(5);
    let formation_paths: Vec<String> = row.get::<_, Option<Vec<String>>>(6).unwrap_or_default();
    let publication_codes: Vec<String> = row.get(7);
    let summary = row
        .get::<_, Option<&str>>(8)
        .map(lj_summary::summary_teaser);
    let seat_stored: Option<lj_core::chamber::Seat> = row
        .get::<_, Option<tokio_postgres::types::Json<lj_core::chamber::Seat>>>(9)
        .map(|j| j.0);
    let formation_uid: Option<String> = row.get(10);
    let office_uid: Option<String> = row.get(11);

    let jurisdiction_name = jurisdiction_code
        .as_deref()
        .and_then(|c| refs.jurisdiction(c))
        .map(|j| j.label.clone());
    let docket_numbers = docket_numbers.filter(|d| !d.is_empty());
    let jur_display = crate::titles::decision_jurisdiction(
        refs.jurisdiction_type_label(&jur_type_raw)
            .unwrap_or(&jur_type_raw),
        jurisdiction_name.as_deref(),
    );
    let chamber_label = seat_stored.as_ref().and_then(lj_core::chamber::Seat::label);
    let seat = crate::titles::decision_seat(
        &jur_display,
        chamber_label.as_deref(),
        formation_uid.as_deref(),
        office_uid.as_deref(),
    );
    let title = lj_core::titles::decision_title(
        &jur_display,
        seat.as_deref(),
        date_lecture.as_deref(),
        None,
        docket_numbers
            .as_deref()
            .and_then(lj_core::titles::docket_display)
            .as_deref(),
    );

    Ok(DecisionPreview {
        id,
        title,
        solution: opt_tag(&solution_uid, &refs),
        formation: crate::titles::formation_tags(&formation_paths),
        publication_codes,
        summary,
    })
}

/// Décisions similaires en deux volets pour un `public_id` :
///
/// - **principe** (référence, similaires sur le fond) : décisions de portée
///   notable par trois signaux fusionnés — citations sortantes résolues
///   (`case_citation`), articles rares partagés pondérés IDF, KNN pré-filtré
///   portée notable.
/// - **espèce** (similaires en fait) : ANN top-k sur les embeddings des chunks
///   bord, en excluant les hits principe et la chronologie de l'affaire
///   (`decision_links`, déjà affichée sur la page).
///
/// Les trois jambes partent en parallèle sur des connexions distinctes du pool ;
/// `limit` s'applique à chaque volet.
#[instrument(skip(state), fields(db.system = "postgresql", public_id = %public_id))]
pub async fn similar_decisions(
    state: &AppState,
    public_id: &str,
    limit: u32,
) -> Result<SimilarDecisionsResponse> {
    let limit = limit as usize;

    let (source_id, meta) = similar_source_meta(state, public_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let legs = similar_legs(state, source_id, limit).await?;

    let abstained = similar_principe_gated(&meta);
    let principe_ids = if abstained {
        Vec::new()
    } else {
        assemble_principe(&legs.direct, &legs.idf, &legs.principe_knn, limit)
    };
    let excluded: HashSet<i64> = principe_ids
        .iter()
        .chain(legs.chronology.iter())
        .copied()
        .collect();
    let espece_ids: Vec<i64> = if similar_espece_gated(&meta) {
        Vec::new()
    } else {
        legs.espece_knn
            .into_iter()
            .filter(|id| !excluded.contains(id))
            .take(limit)
            .collect()
    };

    let mut by_id = hydrate_similar_hits(
        state,
        &principe_ids
            .iter()
            .chain(espece_ids.iter())
            .copied()
            .collect::<Vec<i64>>(),
    )
    .await?;
    let collect = |ids: &[i64], by_id: &mut HashMap<i64, SimilarDecisionHit>| {
        ids.iter().filter_map(|id| by_id.remove(id)).collect()
    };
    Ok(SimilarDecisionsResponse {
        decision_id: public_id.to_string(),
        principe: collect(&principe_ids, &mut by_id),
        espece: collect(&espece_ids, &mut by_id),
        abstained,
    })
}

/// Au-delà de cette taille, un rejet CC citant le 1014 est un arrêt mixte motivé
/// (moyens réels + un moyen 1014), pas un rejet non spécialement motivé.
const GATE_1014_MAX_CHARS: i32 = 3_500;
/// Terme d'inversion `lj_cit_terms` de l'art. 1014 du code de procédure
/// civile (rejet non spécialement motivé).
const CPC_1014_TERM: &str = "LEGITEXT000006070716|1014";

/// Métadonnées de la décision-source pilotant l'abstention des volets
/// similaires.
pub struct SimilarSourceMeta {
    pub jurisdiction_type: String,
    pub solution_uid: Option<String>,
    pub text_chars: i32,
    pub cites_1014: bool,
}

/// Volet principe muet : la source ne tranche aucune question de droit —
/// désistement, non-lieu à statuer, ou rejet CC non spécialement motivé
/// (art. 1014 CPC, texte court).
pub fn similar_principe_gated(meta: &SimilarSourceMeta) -> bool {
    match meta.solution_uid.as_deref() {
        Some("solution:DESISTEMENT") | Some("solution:NON_LIEU_A_STATUER") => true,
        Some("solution:REJET") => {
            meta.jurisdiction_type == "CC"
                && meta.cites_1014
                && meta.text_chars < GATE_1014_MAX_CHARS
        }
        _ => false,
    }
}

/// Volet espèce muet : les voisines d'un désistement sont des donné-acte en
/// série sans valeur d'usage. Les non-lieux gardent leur volet.
pub fn similar_espece_gated(meta: &SimilarSourceMeta) -> bool {
    meta.solution_uid.as_deref() == Some("solution:DESISTEMENT")
}

/// Résout une décision par `public_id` : id interne + métadonnées du gate.
pub async fn similar_source_meta(
    state: &AppState,
    public_id: &str,
) -> Result<Option<(i64, SimilarSourceMeta)>> {
    let conn = state.pool.get().await?;
    let row = conn
        .query_opt(
            &format!(
                "
            SELECT d.id, d.jurisdiction_type, d.solution_uid,
                   coalesce(length({text}), 0),
                   EXISTS (
                     SELECT 1 FROM legal_citation lc
                     WHERE lc.decision_id = d.id
                       AND public.lj_cit_terms(lc.spans) @> ARRAY[$2::text]
                   )
            FROM decisions d WHERE d.public_id = $1 AND d.deleted_at IS NULL
            ",
                text = lj_store::repository::pick::text("p.full_text", "d")
            ),
            &[&public_id, &CPC_1014_TERM],
        )
        .await?;
    Ok(row.map(|r| {
        (
            r.get(0),
            SimilarSourceMeta {
                jurisdiction_type: r.get(1),
                solution_uid: r.get(2),
                text_chars: r.get(3),
                cites_1014: r.get(4),
            },
        )
    }))
}

/// Jambes brutes des deux volets, avant assemblage et hydratation.
pub struct SimilarLegs {
    /// Cibles résolues des citations sortantes, portée notable.
    pub direct: Vec<i64>,
    /// Décisions notables citant les mêmes articles rares, rang IDF.
    pub idf: Vec<i64>,
    /// Chronologie `decision_links` de l'affaire (exclue du volet espèce).
    pub chronology: Vec<i64>,
    /// KNN pré-filtré portée notable.
    pub principe_knn: Vec<i64>,
    /// KNN générique sur-fetché (volet espèce, avant exclusions).
    pub espece_knn: Vec<i64>,
}

/// Calcule les cinq jambes en parallèle sur des connexions distinctes du pool.
pub async fn similar_legs(state: &AppState, source_id: i64, limit: usize) -> Result<SimilarLegs> {
    let (espece_knn, principe_knn, graph) = tokio::try_join!(
        espece_knn_leg(state, source_id, limit),
        principe_knn_leg(state, source_id, limit),
        graph_legs(state, source_id, limit),
    )?;
    Ok(SimilarLegs {
        direct: graph.direct,
        idf: graph.idf,
        chronology: graph.chronology,
        principe_knn,
        espece_knn,
    })
}

/// Jambe espèce : ANN top-k générique sur les embeddings des chunks bord
/// (premier + dernier, max-pool par décision), sur-fetchée pour absorber les
/// exclusions.
async fn espece_knn_leg(state: &AppState, source_id: i64, limit: usize) -> Result<Vec<i64>> {
    let outer_limit = (limit as i64) * ESPECE_OVERFETCH_FACTOR;
    let inner_limit = (limit as i64) * SIMILAR_INNER_LIMIT_FACTOR;
    let query = format!(
        "
        WITH source AS (
          SELECT embedding
          FROM (
            SELECT
              c.embedding,
              row_number() OVER (ORDER BY c.chunk_index) AS rn_first,
              row_number() OVER (ORDER BY c.chunk_index DESC) AS rn_last
            FROM decision_chunks c
            WHERE c.decision_id = $1
              AND c.embedding IS NOT NULL
          ) ordered
          WHERE rn_first = 1 OR rn_last = 1
        ),
        neighbors AS MATERIALIZED (
          SELECT
            cand.decision_id,
            1 - cand.distance AS score
          FROM source src
          JOIN LATERAL (
            SELECT
              c.decision_id,
              c.embedding <=> src.embedding AS distance
            FROM decision_chunks c
            WHERE c.embedding IS NOT NULL
              AND c.decision_id <> $1
            ORDER BY distance
            LIMIT {inner_limit}
          ) cand ON TRUE
        )
        SELECT n.decision_id
        FROM neighbors n
        GROUP BY n.decision_id
        ORDER BY MAX(n.score) DESC, n.decision_id
        LIMIT {outer_limit}
        ",
    );
    knn_query(state, &query, source_id, state.settings.vchord_probes).await
}

/// Jambe principe KNN : même ANN, pré-filtrée portée notable *dans* le scan —
/// le filtre join force vchordrq à continuer en ordre de distance jusqu'à
/// remplir le LIMIT.
async fn principe_knn_leg(state: &AppState, source_id: i64, limit: usize) -> Result<Vec<i64>> {
    let limit = limit as i64;
    let query = format!(
        "
        WITH source AS (
          SELECT embedding
          FROM (
            SELECT
              c.embedding,
              row_number() OVER (ORDER BY c.chunk_index) AS rn_first,
              row_number() OVER (ORDER BY c.chunk_index DESC) AS rn_last
            FROM decision_chunks c
            WHERE c.decision_id = $1
              AND c.embedding IS NOT NULL
          ) ordered
          WHERE rn_first = 1 OR rn_last = 1
        ),
        neighbors AS MATERIALIZED (
          SELECT
            cand.decision_id,
            1 - cand.distance AS score
          FROM source src
          JOIN LATERAL (
            SELECT
              c.decision_id,
              c.embedding <=> src.embedding AS distance
            FROM decision_chunks c
            JOIN decisions dn ON dn.id = c.decision_id
            WHERE c.embedding IS NOT NULL
              AND c.decision_id <> $1
              AND dn.publication_codes && '{{{notable}}}'::text[]
            ORDER BY distance
            LIMIT {limit}
          ) cand ON TRUE
        )
        SELECT n.decision_id
        FROM neighbors n
        GROUP BY n.decision_id
        ORDER BY MAX(n.score) DESC, n.decision_id
        LIMIT {limit}
        ",
        notable = NOTABLE_CODES.join(","),
    );
    knn_query(state, &query, source_id, PRINCIPE_KNN_PROBES).await
}

/// Exécute une requête ANN (`$1` = decision_id source, une colonne
/// `decision_id`) sous `SET LOCAL vchordrq.probes`.
async fn knn_query(state: &AppState, query: &str, source_id: i64, probes: u32) -> Result<Vec<i64>> {
    let mut conn = state.pool.get().await?;
    let tx = conn.transaction().await?;
    tx.batch_execute(&format!("SET LOCAL vchordrq.probes = {probes}"))
        .await?;
    let rows = tx.query(query, &[&source_id]).await?;
    tx.commit().await?;
    Ok(rows.iter().map(|r| r.get(0)).collect())
}

/// Signaux du graphe de citations pour le volet principe, plus la chronologie
/// de l'affaire à exclure du volet espèce.
struct GraphLegs {
    /// Cibles résolues des citations sortantes, portée notable — majeures
    /// d'abord, puis les plus récentes.
    direct: Vec<i64>,
    /// Décisions notables citant les mêmes articles rares, rang IDF.
    idf: Vec<i64>,
    /// Décisions de la chronologie `decision_links`.
    chronology: Vec<i64>,
}

async fn graph_legs(state: &AppState, source_id: i64, limit: usize) -> Result<GraphLegs> {
    let limit = limit as i64;
    let notable: Vec<&str> = NOTABLE_CODES.to_vec();
    let majeure = vec!["r", "A"];
    let conn = state.pool.get().await?;

    let direct_rows = conn
        .query(
            "
            SELECT t.id
            FROM case_citation cc
            JOIN decisions t ON t.id = cc.target_decision_id
            WHERE cc.decision_id = $1
              AND t.publication_codes && $2
            GROUP BY t.id
            ORDER BY bool_or(t.publication_codes && $3) DESC,
                     MAX(t.date_lecture) DESC NULLS LAST, t.id
            LIMIT $4
            ",
            &[&source_id, &notable, &majeure, &limit],
        )
        .await?;

    // Shared rare articles: the source's article terms (`text_uid|num_key`), kept
    // under the rarity cap, inverted by the GIN `legal_citation_terms_idx` onto the
    // notable decisions, scored Σ ln(N/df) with N the planner's estimate.
    // Codes inlined, never bound: `idx_decisions_notable` is partial, and from a
    // parameter the planner cannot prove the implication on a generic plan.
    let notable_sql = NOTABLE_CODES.map(|code| format!("'{code}'")).join(", ");
    let idf_query = format!(
        "
        WITH terms AS (
          SELECT DISTINCT t
          FROM legal_citation lc, unnest(public.lj_cit_terms(lc.spans)) AS t
          WHERE lc.decision_id = $1 AND position('|' in t) > 0
        ),
        rare AS (
          SELECT t.t, s.decision_count
          FROM terms t
          JOIN citing_decision_counts s ON s.cited_term = t.t
          WHERE s.decision_count < {IDF_RARE_DF_CAP}
          ORDER BY s.decision_count ASC
          LIMIT {IDF_MAX_RARE_TERMS}
        )
        SELECT lc2.decision_id
        FROM rare r
        JOIN legal_citation lc2 ON public.lj_cit_terms(lc2.spans) @> ARRAY[r.t]
        JOIN decisions d2 ON d2.id = lc2.decision_id
        WHERE d2.publication_codes && ARRAY[{notable_sql}]::text[]
          AND lc2.decision_id <> $1
        GROUP BY lc2.decision_id
        ORDER BY sum(ln(
                     (SELECT greatest(reltuples, 1)::float8 FROM pg_class
                      WHERE oid = 'decisions'::regclass)
                     / r.decision_count::float8
                 )) DESC,
                 lc2.decision_id
        LIMIT $2
        ",
    );
    let idf_rows = conn.query(&idf_query, &[&source_id, &limit]).await?;

    let chrono_rows = conn
        .query(
            "
            WITH RECURSIVE chain(id, depth) AS (
                SELECT $1::bigint, 0
                UNION
                SELECT CASE WHEN dl.decision_id = c.id THEN dl.target_decision_id
                            ELSE dl.decision_id END,
                       c.depth + 1
                FROM chain c
                JOIN decision_links dl
                  ON dl.target_decision_id IS NOT NULL
                 AND (dl.decision_id = c.id OR dl.target_decision_id = c.id)
                WHERE c.depth < 8
            )
            SELECT DISTINCT id FROM chain WHERE id <> $1
            ",
            &[&source_id],
        )
        .await?;

    Ok(GraphLegs {
        direct: direct_rows.iter().map(|r| r.get(0)).collect(),
        idf: idf_rows.iter().map(|r| r.get(0)).collect(),
        chronology: chrono_rows.iter().map(|r| r.get(0)).collect(),
    })
}

/// Assemble le volet principe : citations directes d'abord, puis les hits en
/// accord IDF ∩ KNN, puis entrelacement alterné IDF/KNN.
pub fn assemble_principe(direct: &[i64], idf: &[i64], knn: &[i64], limit: usize) -> Vec<i64> {
    let knn_set: HashSet<i64> = knn.iter().copied().collect();
    let accord = idf.iter().copied().filter(|id| knn_set.contains(id));

    let mut interleaved = Vec::with_capacity(idf.len() + knn.len());
    let (mut it_idf, mut it_knn) = (idf.iter().copied(), knn.iter().copied());
    loop {
        let (a, b) = (it_idf.next(), it_knn.next());
        if a.is_none() && b.is_none() {
            break;
        }
        interleaved.extend(a);
        interleaved.extend(b);
    }

    let mut seen = HashSet::new();
    direct
        .iter()
        .copied()
        .chain(accord)
        .chain(interleaved)
        .filter(|id| seen.insert(*id))
        .take(limit)
        .collect()
}

/// Hydrate les cartes des deux volets en une requête, indexées par id interne ;
/// l'appelant rétablit l'ordre d'assemblage.
async fn hydrate_similar_hits(
    state: &AppState,
    ids: &[i64],
) -> Result<HashMap<i64, SimilarDecisionHit>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let conn = state.pool.get().await?;
    let rows = conn
        .query(
            &format!(
                "
            SELECT
              d.id,
              d.public_id,
              d.jurisdiction_type,
              d.jurisdiction_code,
              d.date_lecture::text,
              d.docket_numbers,
              d.solution_uid,
              d.formation_paths,
              d.publication_codes,
              {summary}
            FROM decisions d
            WHERE d.id = ANY($1)
            ",
                summary = lj_store::repository::pick::text("p.summary", "d")
            ),
            &[&ids],
        )
        .await?;
    drop(conn);

    let refs = referential(state).await?;
    let mut hits = HashMap::with_capacity(rows.len());
    for row in &rows {
        let internal_id: i64 = row.get(0);
        let id: String = row.get(1);
        let jur_type_raw: String = row.get(2);
        let jurisdiction_code: Option<String> = row.get(3);
        let date_lecture: Option<String> = row.get(4);
        let docket_numbers: Option<Vec<String>> = row.get(5);
        let docket_numbers = docket_numbers.filter(|d| !d.is_empty());
        let solution_uid: Option<String> = row.get(6);
        let formation_paths: Vec<String> = row.get::<_, Option<Vec<String>>>(7).unwrap_or_default();
        let publication_codes: Option<Vec<String>> = row.get(8);
        let summary = row
            .get::<_, Option<&str>>(9)
            .map(lj_summary::summary_teaser);

        hits.insert(
            internal_id,
            SimilarDecisionHit {
                id,
                jurisdiction_type: parse_enum_str(&jur_type_raw).unwrap_or(JurisdictionType::Ta),
                jurisdiction_name: jurisdiction_code
                    .as_deref()
                    .and_then(|c| refs.jurisdiction(c))
                    .map(|j| j.label.clone()),
                jurisdiction_code,
                date_lecture,
                docket_numbers,
                solution: opt_tag(&solution_uid, &refs),
                formation: crate::titles::formation_tags(&formation_paths),
                publication_codes: publication_codes.unwrap_or_default(),
                summary,
            },
        );
    }
    Ok(hits)
}

/// Désérialise une valeur d'enum sérialisée (SCREAMING_SNAKE) en variante Rust.
fn parse_enum_str<T: for<'de> serde::Deserialize<'de>>(code: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(code.to_string())).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assemble_principe_direct_puis_accord_puis_entrelacement() {
        let out = assemble_principe(&[1], &[2, 3, 4], &[3, 5, 6], 6);
        assert_eq!(out, vec![1, 3, 2, 5, 4, 6]);
    }

    #[test]
    fn assemble_principe_cap_et_dedup_direct() {
        let out = assemble_principe(&[2], &[2, 3], &[4, 5], 3);
        assert_eq!(out, vec![2, 4, 3]);
    }

    fn gate_meta(
        jt: &str,
        solution: Option<&str>,
        chars: i32,
        cites_1014: bool,
    ) -> SimilarSourceMeta {
        SimilarSourceMeta {
            jurisdiction_type: jt.to_string(),
            solution_uid: solution.map(str::to_string),
            text_chars: chars,
            cites_1014,
        }
    }

    #[test]
    fn gate_desistement_coupe_les_deux_volets() {
        let m = gate_meta("TA", Some("solution:DESISTEMENT"), 2_500, false);
        assert!(similar_principe_gated(&m));
        assert!(similar_espece_gated(&m));
    }

    #[test]
    fn gate_non_lieu_coupe_le_principe_seul() {
        let m = gate_meta("TA", Some("solution:NON_LIEU_A_STATUER"), 2_500, false);
        assert!(similar_principe_gated(&m));
        assert!(!similar_espece_gated(&m));
    }

    #[test]
    fn gate_rejet_1014_court_cc_seulement() {
        let m = gate_meta("CC", Some("solution:REJET"), 2_400, true);
        assert!(similar_principe_gated(&m));
        assert!(!similar_principe_gated(&gate_meta(
            "CC",
            Some("solution:REJET"),
            8_000,
            true
        )));
        assert!(!similar_principe_gated(&gate_meta(
            "TA",
            Some("solution:REJET"),
            2_000,
            false
        )));
        assert!(!similar_principe_gated(&gate_meta(
            "CE", None, 2_000, false
        )));
    }

    #[test]
    fn assemble_principe_jambes_vides() {
        assert_eq!(assemble_principe(&[], &[], &[7, 8], 4), vec![7, 8]);
        assert!(assemble_principe(&[], &[], &[], 4).is_empty());
    }

    fn raw_ref(instrument: &str, slug: Option<&str>, articles: &[&str]) -> RawLegalRef {
        (
            instrument.to_string(),
            slug.map(str::to_string),
            "FR".to_string(),
            articles
                .iter()
                .map(|a| (a.to_string(), Some(a.to_string())))
                .collect(),
        )
    }

    #[test]
    fn parse_legal_refs_keeps_instrument_without_articles() {
        let raw = vec![raw_ref("Code du travail", Some("code-du-travail"), &[])];
        let refs = parse_legal_refs(&raw).expect("ref conservée");
        assert_eq!(refs[0].instrument, "Code du travail");
        assert!(refs[0].articles.is_empty());
    }

    #[test]
    fn parse_legal_refs_empty_is_none() {
        assert!(parse_legal_refs(&[]).is_none());
    }

    #[test]
    fn parse_legal_refs_drops_unresolved_free_text() {
        let raw = vec![
            raw_ref("Règlement intérieur", None, &["12"]),
            raw_ref("Code civil", Some("code-civil"), &["1240"]),
        ];
        let refs = parse_legal_refs(&raw).expect("la ref résolue survit");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].instrument, "Code civil");
        let only_unresolved = vec![raw_ref("Règlement intérieur", None, &["12"])];
        assert!(parse_legal_refs(&only_unresolved).is_none());
    }

    #[test]
    fn paragraph_ranges_are_codepoints_and_never_rewind() {
        let clean = "Considérant café au lait\nRejet\nÀ nouveau é\nRejet";
        let decision = mk_decision(clean, Vec::new());
        let paragraphs = decision_paragraphs(&decision);
        let ranges = paragraph_global_ranges(&decision.texte_integral_clean, &paragraphs);
        assert_eq!(ranges.len(), 4);
        for (paragraph, &(start, end)) in paragraphs.iter().zip(&ranges) {
            let cut: String = clean.chars().skip(start).take(end - start).collect();
            assert_eq!(&cut, paragraph);
        }
        assert_ne!(ranges[1], ranges[3], "le second Rejet a sa propre plage");
    }

    #[test]
    fn section_labels_canonicalize_kinds() {
        assert_eq!(section_label("dispositif"), Some("Dispositif"));
        assert_eq!(section_label("visa"), Some("Visa"));
        assert_eq!(section_label("inconnu"), None);
    }

    // ── Spans de citation cliquables ──────────────────────────────────────────

    use lj_core::decision::DecisionSection as CoreSection;

    #[test]
    fn a_visa_placed_nowhere_in_the_text_takes_no_paragraph() {
        let clean = "Attendu que la demande est fondée.\nPAR CES MOTIFS rejette.";
        let visa = CoreSection {
            label: "Visa".to_string(),
            kind: "visa".to_string(),
            start_char: lj_parse::SECTION_NO_OFFSET,
            end_char: lj_parse::SECTION_NO_OFFSET,
            text: "Article 1240 du code civil".to_string(),
        };
        let decision = mk_decision(clean, vec![visa]);
        let paragraphs = decision_paragraphs(&decision);
        let ranges = paragraph_global_ranges(&decision.texte_integral_clean, &paragraphs);
        assert!(sections_from_decision(&decision, &paragraphs, &ranges, &[]).is_none());
    }

    fn mk_decision(clean: &str, sections: Vec<CoreSection>) -> Decision {
        Decision {
            source_uid: String::new(),
            member_name: String::new(),
            ecli: None,
            jurisdiction_source_code: None,
            chamber: None,
            nac: None,
            jurisdiction_name: None,
            jurisdiction_type: None,
            jurisdiction_location: None,
            numero_dossier: None,
            numero_dossiers: None,
            numero_role: None,
            date_lecture: None,
            date_audience: None,
            date_mise_jour: None,
            formation: None,
            type_decision: None,
            type_recours: None,
            solution: None,
            publication_codes: Vec::new(),
            avocat_requerant: None,
            listed_counsel: Vec::new(),
            texte_integral_raw: clean.to_string(),
            texte_integral_clean: clean.to_string(),
            sections,
            metadata_header: String::new(),
            themes: Vec::new(),
            attacked: None,
            parse_warnings: Vec::new(),
        }
    }

    fn cite(start: usize, end: usize, href: Option<&str>) -> GlobalCitation {
        GlobalCitation {
            start,
            end,
            targets: vec![CitationTarget {
                href: href.map(str::to_string),
                label: "Code civil 1240".to_string(),
            }],
        }
    }

    #[test]
    fn a_citation_serves_the_version_and_the_territory_of_the_court() {
        let civ = "/norm/fr/code-civil";
        assert_eq!(
            citation_href(civ, Some("1240"), Some("2019-03-12"), Some("geo:PF")),
            "/norm/fr/code-civil/1240/2019-03-12?place=PF"
        );
        assert_eq!(
            citation_href(civ, Some("1240"), Some("2019-03-12"), None),
            "/norm/fr/code-civil/1240/2019-03-12"
        );
        // A text with no article is read whole: no date, the place stays.
        assert_eq!(
            citation_href(civ, None, Some("2019-03-12"), Some("geo:NC")),
            "/norm/fr/code-civil?place=NC"
        );
        assert_eq!(citation_href(civ, None, None, None), civ);
    }

    fn raw(start: usize, end: usize, text_uid: &str, num_key: Option<&str>) -> RawCite {
        RawCite {
            start,
            end,
            text_uid: text_uid.to_string(),
            path: "/norm/fr/code-de-la-route".to_string(),
            num_key: num_key.map(str::to_string),
            href: "/norm/fr/code-de-la-route".to_string(),
            label: "Code de la route".to_string(),
            suivants: false,
        }
    }

    #[test]
    fn range_detection_bounds_du_meme_texte_separees_de_a() {
        let text: Vec<char> = "les articles L. 225-1 à L. 225-9 du code".chars().collect();
        assert_eq!(text[13..21].iter().collect::<String>(), "L. 225-1");
        assert_eq!(text[24..32].iter().collect::<String>(), "L. 225-9");
        let a = raw(13, 21, "LEGITEXT000006074228", Some("L. 225-1"));
        let b = raw(24, 32, "LEGITEXT000006074228", Some("L. 225-9"));
        assert!(is_article_range(&a, &b, &text));
        let autre = raw(24, 32, "LEGITEXT000006070933", Some("L. 225-9"));
        assert!(!is_article_range(&a, &autre, &text));
        let nue = raw(24, 32, "LEGITEXT000006074228", None);
        assert!(!is_article_range(&a, &nue, &text));
        let text_et: Vec<char> = "les articles L. 225-1 et L. 225-9 du code"
            .chars()
            .collect();
        assert!(!is_article_range(
            &a,
            &raw(25, 33, "LEGITEXT000006074228", Some("L. 225-9")),
            &text_et
        ));
    }

    #[test]
    fn range_stem_enumerable_ou_non() {
        assert_eq!(
            range_stem("L. 225-1", "L. 225-9"),
            Some(("L. 225".to_string(), 1, 9))
        );
        assert_eq!(range_stem("102", "108"), Some((String::new(), 102, 108)));
        assert_eq!(range_stem("L. 225-1", "L. 226-3"), None);
        assert_eq!(range_stem("50 sexies B", "H"), None);
        assert_eq!(range_stem("L. 225-9", "L. 225-1"), None);
        assert_eq!(range_stem("1", "300"), None);
        let (exact, like) = range_candidate_keys("L. 225", 1, 4);
        assert_eq!(exact, vec!["L. 225-2", "L. 225-3"]);
        assert_eq!(like, vec!["L. 225-1-%", "L. 225-2-%", "L. 225-3-%"]);
    }

    #[test]
    fn global_offset_maps_to_correct_paragraph_local_offset_with_multibyte() {
        let clean = "Considérant l'article 1240\nDécision rendue, voir 1241.";
        let p0_start = clean.chars().position(|c| c == 'C').unwrap();
        assert_eq!(p0_start, 0);
        let n_p0 = "Considérant l'article 1240".chars().count();
        let g_start = n_p0 - 4;
        let g_end = n_p0;

        let decision = mk_decision(clean, Vec::new());
        let ranges = paragraph_global_ranges(
            &decision.texte_integral_clean,
            &decision_paragraphs(&decision),
        );
        assert_eq!(ranges.len(), 2);
        let (p0s, p0e) = ranges[0];
        assert_eq!((p0s, p0e), (0, n_p0));
        let spans0 = spans_for_range(
            &[cite(g_start, g_end, Some("/norm/code-civil/1240"))],
            p0s,
            p0e,
        );
        assert_eq!(spans0.len(), 1);
        assert_eq!((spans0[0].start, spans0[0].end), (n_p0 - 4, n_p0));
        assert_eq!(
            clean.chars().skip(g_start).take(4).collect::<String>(),
            "1240"
        );
        let (p1s, p1e) = ranges[1];
        let spans1 = spans_for_range(&[cite(g_start, g_end, None)], p1s, p1e);
        assert!(spans1.is_empty());
    }

    #[test]
    fn overlapping_spans_merge_and_union_targets() {
        let short = cite(12, 20, Some("/norm/x/court"));
        let long = cite(10, 30, Some("/norm/x/long"));
        let spans = spans_for_range(&[short, long], 0, 40);
        assert_eq!(spans.len(), 1);
        assert_eq!((spans[0].start, spans[0].end), (10, 30));
        let hrefs: Vec<_> = spans[0].targets.iter().map(|t| t.href.as_deref()).collect();
        assert_eq!(hrefs, vec![Some("/norm/x/long"), Some("/norm/x/court")]);
    }

    #[test]
    fn coextensive_multi_article_yields_one_span_multi_target() {
        let mk = |href: &str, label: &str| GlobalCitation {
            start: 5,
            end: 40,
            targets: vec![CitationTarget {
                href: Some(href.to_string()),
                label: label.to_string(),
            }],
        };
        let cites = [
            mk("/norm/code-civil/1382", "1382"),
            mk("/norm/code-civil/1383", "1383"),
            mk("/norm/code-civil/1384", "1384"),
            mk("/norm/code-civil/1382", "1382"),
        ];
        let spans = spans_for_range(&cites, 0, 60);
        assert_eq!(spans.len(), 1);
        assert_eq!((spans[0].start, spans[0].end), (5, 40));
        assert_eq!(
            spans[0].targets.len(),
            3,
            "cibles distinctes, doublon fusionné"
        );
    }

    #[test]
    fn disjoint_spans_kept_and_sorted_by_start() {
        let a = cite(20, 24, Some("/norm/x/2"));
        let b = cite(5, 9, None);
        let spans = spans_for_range(&[a, b], 0, 40);
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].start, spans[0].end), (5, 9));
        assert_eq!(spans[0].targets.len(), 1);
        assert!(spans[0].targets[0].href.is_none());
        assert_eq!((spans[1].start, spans[1].end), (20, 24));
        assert_eq!(spans[1].targets[0].href.as_deref(), Some("/norm/x/2"));
    }

    #[test]
    fn multi_occurrence_yields_one_span_per_mention() {
        let m1 = cite(3, 7, Some("/norm/x/9"));
        let m2 = cite(15, 19, Some("/norm/x/9"));
        let spans = spans_for_range(&[m1, m2], 0, 30);
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].start, spans[0].end), (3, 7));
        assert_eq!((spans[1].start, spans[1].end), (15, 19));
    }

    #[test]
    fn span_outside_paragraph_is_ignored() {
        let spans = spans_for_range(&[cite(100, 110, Some("/norm/x/1"))], 0, 40);
        assert!(spans.is_empty());
        let spans = spans_for_range(&[cite(38, 45, Some("/norm/x/1"))], 0, 40);
        assert!(spans.is_empty());
    }
}
