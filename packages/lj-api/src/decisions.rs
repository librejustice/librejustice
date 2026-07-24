//! Lecture des décisions (détail, sections, similaires) — port de
//! `decisions.py`.
//!
//! Hydration du détail (méta DB + `Decision` reconstruite depuis
//! `(full_text, source_fields)` — ADR 0085, sans payload brut — + sections
//! repliées depuis la structure autoritative du parser), voisins sémantiques
//! (ANN top-k sur les embeddings des chunks bord).

use std::collections::HashMap;

use lj_core::decision::Decision;
use lj_core::truecase;
use lj_dtos::{
    CitationSpan, CitationTarget, DecisionDetail, DecisionPreview, DecisionSection,
    DecisionSourceXml, FacetTag, JurisdictionType, LegalRefArticle, LegalReference,
    SimilarDecisionHit,
};

use tracing::instrument;

use crate::error::{ApiError, Result};
use crate::referential::{referential, Referential};
use crate::state::AppState;

/// Facteur d'élargissement de la fenêtre ANN interne (parité avec
/// `_SIMILAR_INNER_LIMIT_FACTOR`).
const SIMILAR_INNER_LIMIT_FACTOR: i64 = 24;

/// Titres de sommaire canoniques par `kind` — uniformise l'affichage quel que
/// soit le marqueur source (ADR 0046). Port de `_SECTION_LABELS`.
fn section_label(kind: &str) -> Option<&'static str> {
    match kind {
        "preamble" => Some("Introduction"),
        "procedure" => Some("Procédure"),
        "visa" => Some("Visa"),
        "expose" => Some("Exposé du litige"),
        "moyens" => Some("Moyens"),
        "motivations" => Some("Motivations"),
        "dispositif" => Some("Dispositif"),
        _ => None,
    }
}

/// Paragraphes du corps : lignes non vides du texte nettoyé (port de
/// `_decision_paragraphs`).
///
/// `pub` : le banc de parité re-extract (#18) compare la sortie de rendu
/// reconstruite vs payload, pas le `Decision.sections` brut (trop strict —
/// insensible au tri/±1 char que ce rendu absorbe).
pub fn decision_paragraphs(decision: &Decision) -> Vec<String> {
    decision
        .texte_integral_clean
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Plage globale `[start, end)` (codepoints sur `texte_integral_clean` =
/// `decisions.full_text`, ADR 0125 §2) de chaque paragraphe rendu, alignée
/// index-à-index sur [`decision_paragraphs`].
///
/// Réplique l'avance par curseur de [`sections_from_decision`] : chaque paragraphe
/// (ligne *trimmée*) est localisé via [`find_char`] depuis le curseur, sa plage
/// couvre `[pos, pos + nb_codepoints)`. Introuvable ⇒ plage dégénérée `(cursor,
/// cursor)` (aucun span ne s'y rattache). C'est l'ancrage qui mappe les spans de
/// citation globaux vers les offsets locaux au paragraphe.
fn paragraph_global_ranges(decision: &Decision) -> Vec<(usize, usize)> {
    let text = &decision.texte_integral_clean;
    let mut cursor = 0usize;
    let mut ranges = Vec::new();
    for paragraph in decision_paragraphs(decision) {
        let plen = paragraph.chars().count();
        match find_char(text, &paragraph, cursor) {
            Some(idx) => {
                cursor = idx + plen;
                ranges.push((idx, idx + plen));
            }
            None => ranges.push((cursor, cursor)),
        }
    }
    ranges
}

/// Méta source brute pour `DecisionSourceXml` (port de `_decision_source_meta`).
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
/// paragraphes pour le sommaire/corps front (ADR 0046 / 0048). Port fidèle de
/// `sections_from_decision`.
///
/// Chaque paragraphe est rattaché — par ses offsets char — à la section
/// canonique qui le couvre, en partition stricte. Les paragraphes sont émis
/// dans l'ordre du texte ; on ne fusionne que les runs consécutifs de même
/// `kind`. `None` quand aucune section autoritative (`start_char >= 0`)
/// n'existe.
///
/// `pub` : exposé au banc de parité re-extract (#18) comme oracle de rendu (les
/// bornes sont triées par `start_char` et l'affectation se fait par position →
/// ce rendu absorbe les réordonnancements et décalages ±1 char des bornes brutes).
pub fn sections_from_decision(
    decision: &Decision,
    citations: &[GlobalCitation],
) -> Option<Vec<DecisionSection>> {
    // (start_char, end_char, kind, label), trié par start_char.
    let mut bounds: Vec<(usize, usize, &str, &str)> = decision
        .sections
        .iter()
        // En Python `start_char >= 0` exclut les sections synthétiques hors
        // corps (visa Judilibre marqué start_char < 0). Côté Rust `start_char`
        // est `usize` : ces sections synthétiques sont représentées
        // différemment dans le port lj-core (cf. parsing). On garde donc toutes
        // les sections — le filtre `>= 0` est trivialement vrai pour un usize.
        .map(|s| (s.start_char, s.end_char, s.kind.as_str(), s.label.as_str()))
        .collect();
    bounds.sort_by_key(|b| b.0);

    if bounds.is_empty() {
        return None;
    }

    // section_for(pos) : dernier bound dont start_char <= pos (bounds trié).
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
    // Plages globales alignées sur `out[i].paragraphs[j]` (regroupées comme les
    // sections), pour mapper ensuite les spans de citation globaux → locaux.
    let mut out_ranges: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut seen_kinds: HashMap<String, usize> = HashMap::new();

    let ranges = paragraph_global_ranges(decision);
    for (paragraph, range) in decision_paragraphs(decision).into_iter().zip(ranges) {
        let pos = range.0;
        let (kind, label) = section_for(pos);

        if let Some(last) = out.last_mut() {
            if last.kind == kind {
                last.paragraphs.push(paragraph);
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
            paragraphs: vec![paragraph],
            paragraph_spans: Vec::new(),
        });
        out_ranges.push(vec![range]);
    }

    if out.is_empty() {
        return None;
    }

    // Attache les spans de citation cliquables, paragraphe par paragraphe, depuis
    // les plages globales calculées ci-dessus. Vide quand aucune citation ne
    // tombe dans le paragraphe (sérialisé omis si tous vides).
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
/// codepoints sur `texte_integral_clean` (= `decisions.full_text`, ADR 0125 §2)
/// et ses cibles résolues (une seule pour une mention simple ; toutes les
/// bornes + intermédiaires pour une plage fusionnée).
#[derive(Debug, Clone)]
pub struct GlobalCitation {
    pub start: usize,
    pub end: usize,
    pub targets: Vec<CitationTarget>,
}

/// Une mention liée brute (une ligne `legal_citation` résolue) : matière du
/// rendu (`href`) et de la détection de plage (`text_uid` + `num_key` +
/// adjacence dans le texte).
struct RawCite {
    start: usize,
    end: usize,
    text_uid: String,
    slug: String,
    num_key: Option<String>,
    href: String,
    label: String,
    /// Span « N et suivants » (ADR 0226) : la cible est la famille TOC de
    /// l'ancre — résolue à l'assemblage, menu comme les plages.
    suivants: bool,
}

/// Deux mentions adjacentes forment-elles une plage d'articles
/// (« L. 225-1 à L. 225-9 ») ? Bornes du MÊME texte, toutes deux à article,
/// séparées du seul mot « à » (ADR 0143 §6 : la capture pose une ligne par
/// borne écrite ; la plage est reconstituée ici, à la résolution).
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
/// bornes. Cap à 50 : une plage géante n'est pas un menu.
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
/// pas (`like`, « L. 225-3-1 » ; le pas de la borne basse inclus — inséré
/// APRÈS elle, donc dans la plage — pas celui de la borne haute).
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
/// retenue que si elle est **entièrement contenue** dans le paragraphe (les
/// mentions sont des slices d'une ligne — jamais à cheval sur deux paragraphes).
///
/// Chevauchement entre mentions → **fusion** en leur enveloppe avec **union des
/// cibles** (dédup par `(href, label)`), jamais de drop : une citation
/// multi-articles partage un seul span mais vise N articles → le front rend un
/// lien simple pour 1 cible, un menu déroulant pour ≥2 (ADR 0125). Les spans
/// rendus sont triés par début, disjoints.
fn spans_for_range(citations: &[GlobalCitation], pstart: usize, pend: usize) -> Vec<CitationSpan> {
    // Candidats locaux : contenus dans le paragraphe, non vides.
    let mut cands: Vec<(usize, usize, &GlobalCitation)> = citations
        .iter()
        .filter(|c| c.start >= pstart && c.end <= pend && c.end > c.start)
        .map(|c| (c.start - pstart, c.end - pstart, c))
        .collect();
    // Tri par début (puis fin) → fusion des chevauchements en un seul balayage.
    cands.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    // Tout candidat qui chevauche le cluster courant (trié par début, donc seul le
    // dernier cluster peut chevaucher) étend l'enveloppe et **ajoute ses cibles** —
    // aucune n'est perdue. Deux mentions disjointes restent deux spans.
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
/// sections autoritatives). Renvoie `Vec` vide si aucune citation ne tombe dans
/// aucun paragraphe (le DTO omet alors le champ).
fn flat_paragraph_spans(
    decision: &Decision,
    citations: &[GlobalCitation],
) -> Vec<Vec<CitationSpan>> {
    let spans: Vec<Vec<CitationSpan>> = paragraph_global_ranges(decision)
        .into_iter()
        .map(|(pstart, pend)| spans_for_range(citations, pstart, pend))
        .collect();
    if spans.iter().any(|s| !s.is_empty()) {
        spans
    } else {
        Vec::new()
    }
}

/// Indice (en codepoints) de la première occurrence de `needle` dans `haystack`
/// à partir de `from_char` — réplique `str.find(needle, start)` de Python (qui
/// indexe en codepoints, pas en octets).
fn find_char(haystack: &str, needle: &str, from_char: usize) -> Option<usize> {
    // Octet de départ correspondant au codepoint `from_char`.
    let byte_start = haystack
        .char_indices()
        .nth(from_char)
        .map(|(b, _)| b)
        .unwrap_or(haystack.len());
    let slice = &haystack[byte_start..];
    let byte_off = slice.find(needle)?;
    // Convertit l'offset octet (dans `slice`) en offset codepoint absolu.
    let char_off = slice[..byte_off].chars().count();
    Some(from_char + char_off)
}

/// Une référence brute lue de la DB : `(instrument, slug résolu, [(num affiché,
/// ref_num_key résolu)])`. Le `slug`/`num_key` portent la FK de citation résolue à
/// l'ingest (ADR 0123 §2) ; `None` = non ancré au catalogue (pas de lien).
type RawLegalRef = (String, Option<String>, Vec<(String, Option<String>)>);

/// Construit les `LegalReference` exposées. Les citations procédurales ne sont
/// plus dans le stock (ADR 0211) — rien à masquer en sortie.
///
/// Un instrument cité sans article précis est conservé tel quel. Le `slug` et le
/// `numKey` résolus (ADR 0123 §2) sont propagés au DTO pour bâtir les liens
/// `/texte/{slug}/{numKey}` sans re-slugifier côté front. `None` si la liste
/// résultante est vide.
fn parse_legal_refs(raw: &[RawLegalRef]) -> Option<Vec<LegalReference>> {
    if raw.is_empty() {
        return None;
    }
    let mut refs: Vec<LegalReference> = Vec::new();
    for (instrument, slug, original) in raw {
        // Citations non résolues (pas de `slug` catalogue : free text « règlement
        // intérieur », codes non catalogués) → exclues de la liste « Textes cités ».
        // On n'expose que les références ancrées à un texte enregistré (décision
        // opérateur 2026-06-30). Capture inchangée (le farming de recall s'en sert).
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
            articles,
        });
    }
    if refs.is_empty() {
        None
    } else {
        Some(refs)
    }
}

/// Valide le code `jurisdiction_type` issu de la DB et le convertit en
/// [`JurisdictionType`] (parité avec le garde Python `if jur_type not in (...)`).
fn parse_jur_type(raw: &str) -> Result<JurisdictionType> {
    match raw {
        "TA" => Ok(JurisdictionType::Ta),
        "CAA" => Ok(JurisdictionType::Caa),
        "CE" => Ok(JurisdictionType::Ce),
        "CONSTIT" => Ok(JurisdictionType::Constit),
        "TC" => Ok(JurisdictionType::Tc),
        "CC" => Ok(JurisdictionType::Cc),
        "CA" => Ok(JurisdictionType::Ca),
        "TJ" => Ok(JurisdictionType::Tj),
        "TCOM" => Ok(JurisdictionType::Tcom),
        "CEDH" => Ok(JurisdictionType::Cedh),
        "CJUE" => Ok(JurisdictionType::Cjue),
        "CNDA" => Ok(JurisdictionType::Cnda),
        "CNIL" => Ok(JurisdictionType::Cnil),
        other => Err(ApiError::Internal(format!(
            "jurisdiction_type DB invalide : {other:?}"
        ))),
    }
}

/// Tag référentiel d'un uid optionnel (`solution:REJET` → `{REJET, Rejet}`).
fn opt_tag(uid: &Option<String>, refs: &Referential) -> Option<FacetTag> {
    uid.as_deref().map(|u| refs.tag(u))
}

/// Projette `themes` depuis `source_fields` : casefold + déduplication
/// ordre-stable, entrées vides retirées (ADR 0090). Scalaire au lieu de liste,
/// ou clé absente ⇒ `Vec` vide.
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
        // Casefold ⇒ clé de dédup ; on garde la première forme rencontrée.
        if seen.insert(theme.to_lowercase()) {
            out.push(theme.to_string());
        }
    }
    out
}

/// Projette `nac` depuis `source_fields` : le littéral chaîne `"null"` et la
/// chaîne vide → `None` ; code brut sinon (ADR 0090, pas de table code→libellé).
fn project_nac(source_fields: &serde_json::Value) -> Option<String> {
    let s = source_fields.get("nac")?.as_str()?.trim();
    if s.is_empty() || s == "null" {
        return None;
    }
    Some(s.to_string())
}

/// Triplet hydraté depuis la `Decision` reconstruite : paragraphes, sections,
/// méta XML. Aliasé pour le retour du `spawn_blocking` de parse (clippy
/// `type_complexity`).
type ParsedPayload = (
    Vec<String>,
    Vec<Vec<CitationSpan>>,
    Option<Vec<DecisionSection>>,
    Option<DecisionSourceXml>,
);

/// Détail complet d'une décision par `public_id` (paragraphes, sections, refs).
///
/// Port de `fetch_decision` : une requête méta + `(full_text, source_fields)`,
/// une requête de ré-agrégation des références légales par instrument, puis
/// hydration du corps depuis la `Decision` reconstruite (ADR 0085 — plus de
/// payload brut dégzippé).
///
/// Span DB nommé (`db.system="postgresql"`) pour le drill-down `/decision` du
/// cockpit Tempo ; `state` (pool non-`Debug`) est exclu de l'enregistrement.
#[instrument(skip(state), fields(db.system = "postgresql", public_id = %public_id))]
pub async fn get_decision(state: &AppState, public_id: &str) -> Result<DecisionDetail> {
    let refs = referential(state).await?;
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;

    let row = conn
        .query_opt(
            "
            SELECT
              d.id,
              d.public_id,
              d.jurisdiction_type,
              d.full_text,
              ds.source_fields,
              d.solution_uid,
              d.procedure_uid,
              d.office_uid,
              d.legal_domain_uid,
              d.publication_codes,
              d.date_lecture::text,
              d.date_audience::text,
              d.jurisdiction_code,
              d.docket_numbers,
              d.chamber_position,
              d.summary,
              d.ecli,
              ds.source,
              d.chamber_uid,
              d.formation_uid,
              ariane.source_fields,
              d.publication_uid,
              notes.files,
              web.notes
            FROM decisions d
            LEFT JOIN LATERAL (
                SELECT source_fields, source
                FROM decision_sources
                WHERE decision_id = d.id AND deleted_at IS NULL
                ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, id ASC
                LIMIT 1
            ) ds ON true
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
            WHERE d.public_id = $1
            ",
            &[&public_id],
        )
        .await?;

    let row = row.ok_or(ApiError::NotFound)?;

    let decision_id: i64 = row.get(0);
    let id: String = row.get(1);
    let jur_type_raw: String = row.get(2);
    let full_text: Option<String> = row.get(3);
    let source_fields: Option<serde_json::Value> = row.get(4);
    let solution_uid: Option<String> = row.get(5);
    let procedure_uid: Option<String> = row.get(6);
    let office_uid: Option<String> = row.get(7);
    let legal_domain_uid: Option<String> = row.get(8);
    let publication_codes: Vec<String> = row.get(9);
    let date_lecture: Option<String> = row.get(10);
    let date_audience: Option<String> = row.get(11);
    let jurisdiction_code: Option<String> = row.get(12);
    let docket_numbers_raw: Option<Vec<String>> = row.get(13);
    let chamber_position: Option<String> = row.get(14);
    let summary: Option<String> = row.get(15);
    let ecli: Option<String> = row.get(16);
    let source: Option<String> = row.get(17);
    let chamber_uid: Option<String> = row.get(18);
    let formation_uid: Option<String> = row.get(19);
    let ariane_bundle: Option<serde_json::Value> = row.get(20);
    let publication_uid: Option<String> = row.get(21);
    let judilibre_files: Option<serde_json::Value> = row.get(22);
    let web_notes: Option<serde_json::Value> = row.get(23);

    let jur_type = parse_jur_type(&jur_type_raw)?;
    // Nom de juridiction résolu depuis le référentiel `jurisdiction` (ADR 0146) —
    // le label vit en donnée, plus dans une colonne texte libre.
    let jurisdiction_name = jurisdiction_code
        .as_deref()
        .and_then(|c| refs.jurisdiction(c))
        .map(|j| j.label.clone());

    // Références légales par instrument depuis la relation du domaine
    // (`legal_citation`, ADR 0145 M4). Seules les citations liées apparaissent
    // (ref_text_uid NOT NULL — même doctrine que l'overlay : on ne liste pas ce
    // qui ne mène nulle part). Libellé = titre catalogue ; articles = les
    // `ref_num_key` distincts (clé canonique du lien `/texte/{slug}/{numKey}`,
    // qui sert aussi de libellé).
    let ref_rows = conn
        .query(
            "
            SELECT lt.title AS instrument,
                   lt.slug,
                   lt.text_uid,
                   COALESCE(array_agg(DISTINCT el->>3 ORDER BY el->>3)
                            FILTER (WHERE el->>3 IS NOT NULL),
                            ARRAY[]::text[]) AS num_keys
            FROM legal_citation lc
            CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
            JOIN legal_text lt ON lt.text_uid = el->>2
            WHERE lc.decision_id = $1
            GROUP BY lt.text_uid, lt.title, lt.slug
            ORDER BY 1
            ",
            &[&decision_id],
        )
        .await?;

    // Spans cliquables (ADR 0125 / 0145 / 0247) : un élément du blob `spans` =
    // une mention (codepoints sur `decisions.full_text`, convention 0143). PK
    // scan, une table ; le JOIN `legal_text` fournit slug (href) et titre
    // (libellé).
    //
    // **INNER JOIN sur `legal_text` (ref_text_uid lié)** : on n'overlaye QUE
    // les citations ancrées à un texte enregistré. Les références non liées
    // (`ref_text_uid` NULL) ne sont PAS surlignées dans le corps — on ne
    // souligne pas ce qui ne mène nulle part (décision opérateur 2026-06-30).
    let span_rows = conn
        .query(
            "
            SELECT (el->>0)::int AS char_start,
                   (el->>1)::int AS char_end,
                   lt.text_uid,
                   lt.slug,
                   el->>3 AS ref_num_key,
                   lt.title AS label,
                   EXISTS (SELECT 1 FROM legal_article a
                           WHERE a.text_uid = lt.text_uid) AS has_articles,
                   (el->>4)::bool AS suivants
            FROM legal_citation lc
            CROSS JOIN LATERAL jsonb_array_elements(lc.spans) AS el
            JOIN legal_text lt ON lt.text_uid = el->>2
            WHERE lc.decision_id = $1
            ",
            &[&decision_id],
        )
        .await?;

    // Citations de jurisprudence (ADR 0165) : une ligne `case_citation`
    // RÉSOLUE = un span cliquable vers la décision citée. Même doctrine que
    // l'INNER JOIN ci-dessus : une clé pendante (`target_decision_id` NULL)
    // n'est pas décorée du tout.
    let case_rows = conn
        .query(
            "
            SELECT cc.char_start,
                   cc.char_end,
                   t.public_id,
                   t.jurisdiction_code,
                   t.date_lecture::text,
                   t.docket_numbers
            FROM case_citation cc
            JOIN decisions t ON t.id = cc.target_decision_id
            WHERE cc.decision_id = $1 AND t.deleted_at IS NULL
            ",
            &[&decision_id],
        )
        .await?;

    let mut raw: Vec<RawCite> = span_rows
        .iter()
        .filter_map(|r| {
            let start: i32 = r.get(0);
            let end: i32 = r.get(1);
            let text_uid: String = r.get(2);
            let slug: Option<String> = r.get(3);
            let num_key: Option<String> = r.get(4);
            let label: String = r.get(5);
            let has_articles: bool = r.get(6);
            let suivants: bool = r.get(7);
            let slug = slug?;
            let num_key = num_key.filter(|k| !k.is_empty());
            // Article ciblé → /texte/{slug}/{numKey}. Mention nue → /texte/{slug}
            // seulement si le texte a ≥ 1 article en base (ADR 0162 §4). Pas de
            // lien = pas de rendu du tout (mort du pointillé, décision
            // opérateur 2026-07-05) : on ne décore pas ce qui ne mène nulle part.
            let href = match num_key.as_deref() {
                Some(k) => format!("/texte/{slug}/{k}"),
                None if has_articles => format!("/texte/{slug}"),
                None => return None,
            };
            Some(RawCite {
                start: start.max(0) as usize,
                end: end.max(0) as usize,
                text_uid,
                slug,
                num_key,
                href,
                label,
                suivants,
            })
        })
        .collect();
    raw.sort_by_key(|c| c.start);

    let text_chars: Vec<char> = full_text.as_deref().unwrap_or_default().chars().collect();

    // Plages d'articles (« L. 225-1 à L. 225-9 ») : la capture pose une ligne
    // par borne écrite (ADR 0143 §6) ; ici on reconstitue LA plage — un span de
    // borne à borne — et on résout les articles intermédiaires contre le
    // catalogue.
    let single = |c: &RawCite| CitationTarget {
        href: Some(c.href.clone()),
        label: c.label.clone(),
    };
    let mut citations: Vec<GlobalCitation> = Vec::with_capacity(raw.len());
    // Membres des plages recollées (bornes + intermédiaires), par `text_uid` :
    // ils rejoignent la liste « Références juridiques » du texte cité, pour que
    // l'en-tête montre la même chose que le corps.
    let mut range_members: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut it = raw.into_iter().peekable();
    while let Some(a) = it.next() {
        let ranged = it
            .peek()
            .is_some_and(|b| is_article_range(&a, b, &text_chars));
        if !ranged {
            // « N et suivants » (ADR 0226) : la famille TOC de l'ancre en
            // menu, comme une plage — `_suivants_family_keys` porte la
            // sémantique (section unique, VIGUEUR, cap 20 ; NULL sinon,
            // l'ancre reste alors un lien simple). Les clés publiques de la
            // famille repassent en numéro d'affichage (`legal_article.num`)
            // pour parler la même forme que les plages.
            let family: Vec<String> = match (a.suivants, a.num_key.as_deref()) {
                (true, Some(nk)) => conn
                    .query(
                        "
                        SELECT coalesce(
                                 (SELECT la.num FROM legal_article la
                                  WHERE la.text_uid = $1 AND la.num_key = t.k
                                  ORDER BY la.date_debut DESC LIMIT 1), t.k)
                        FROM unnest(_suivants_family_keys($1, $2))
                             WITH ORDINALITY AS t(k, ord)
                        ORDER BY t.ord
                        ",
                        &[&a.text_uid, &lj_core::article_key::article_key(nk)],
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
                        // Libellé en numéro d'affichage, lien dans l'alphabet
                        // public (ADR 0209) — la route ne connaît que lui.
                        href: Some(format!(
                            "/texte/{}/{}",
                            a.slug,
                            lj_core::article_key::article_key(nk)
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
        // catalogue (sous-articles insérés compris, « L. 225-3-1 »). Bornes non
        // énumérables (« 50 sexies B à H ») → la plage garde ses deux bornes.
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
            // Ordre de lecture : pas numérique, puis la clé (les sous-articles
            // d'un pas suivent leur article).
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
        // Dans le menu d'une plage, chaque ligne s'identifie par son numéro
        // en forme d'affichage (le titre du texte est déjà porté par la
        // phrase autour du span).
        let bound = |c: &RawCite| CitationTarget {
            href: Some(c.href.clone()),
            label: format!(
                "{} — {}",
                lj_core::article_key::display(
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
            href: Some(format!("/texte/{}/{nk}", a.slug)),
            label: format!("{} — {}", lj_core::article_key::display(&nk), a.label),
        }));
        targets.push(bound(&b));
        citations.push(GlobalCitation {
            start: a.start,
            end: b.end,
            targets,
        });
    }

    // Spans jurisprudence : libellé = identité de la décision citée
    // (juridiction du référentiel, date, n°), href = sa page.
    for r in &case_rows {
        let start: i32 = r.get(0);
        let end: i32 = r.get(1);
        let target_pid: String = r.get(2);
        let code: Option<String> = r.get(3);
        let date: Option<String> = r.get(4);
        let dockets: Option<Vec<String>> = r.get(5);
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
                href: Some(format!("/decision/{target_pid}")),
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
            let text_uid: String = r.get(2);
            let mut num_keys: Vec<String> = r.get(3);
            if let Some(members) = range_members.get(&text_uid) {
                num_keys.extend(members.iter().cloned());
                num_keys.sort();
                num_keys.dedup();
            }
            // Libellé en forme d'affichage, lien en alphabet public (ADR 0209).
            let articles: Vec<(String, Option<String>)> = num_keys
                .into_iter()
                .map(|k| {
                    let key = lj_core::article_key::article_key(&k);
                    (lj_core::article_key::display(&key), Some(key))
                })
                .collect();
            (instrument, slug, articles)
        })
        .collect();

    // Chronologie de l'affaire (ADR 0161/0169) : décisions atteignables par les
    // liens appel/pourvoi/renvoi RÉSOLUS, traversés dans les deux sens (la
    // marche non orientée remonte les renvois après cassation : CE → CAA →
    // CE → CAA). Les liens pendants (cible hors base) ne produisent pas
    // d'étape — on ne liste pas ce qui ne mène nulle part. Profondeur bornée
    // à 8 (les chaînes réelles font ≤ 4-5 étapes ; l'UNION coupe les cycles).
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

    // Arêtes de la chaîne : la nature du lien (appel / pourvoi / renvoi) entre
    // chaque paire de décisions —  ne montre pas cette information.
    let chrono_edges = if chrono_rows.len() >= 2 {
        let ids: Vec<i64> = chrono_rows.iter().map(|r| r.get::<_, i64>(5)).collect();
        conn.query(
            "
            SELECT s.public_id, t.public_id, dl.link_type
            FROM decision_links dl
            JOIN decisions s ON s.id = dl.decision_id
            JOIN decisions t ON t.id = dl.target_decision_id
            WHERE dl.decision_id = ANY($1) AND dl.target_decision_id = ANY($1)
            ",
            &[&ids],
        )
        .await?
    } else {
        Vec::new()
    };

    // Les requêtes sont faites : on rend la connexion au pool AVANT le parse
    // offloadé (qui ne touche plus la DB). Sinon la connexion resterait checkout
    // pendant tout le `.await` du `spawn_blocking` (~10-60 ms pour une grosse
    // décision), gaspillant une place du pool sous charge concurrente.
    drop(conn);

    // Une chaîne se montre à partir de deux décisions : seule dans sa
    // « chaîne », la décision courante n'a pas de chronologie.
    let chronology: Vec<lj_dtos::ChronologyEntry> = if chrono_rows.len() >= 2 {
        let edges: std::collections::HashMap<(String, String), String> = chrono_edges
            .iter()
            .map(|r| ((r.get(0), r.get(1)), r.get(2)))
            .collect();
        let mut entries: Vec<lj_dtos::ChronologyEntry> = chrono_rows
            .iter()
            .map(|r| {
                let entry_id: String = r.get(0);
                let code: Option<String> = r.get(1);
                let jt: String = r.get(2);
                let date: Option<String> = r.get(3);
                let current: bool = r.get(4);
                let solution_uid: Option<String> = r.get(6);
                let label = code
                    .as_deref()
                    .and_then(|c| refs.jurisdiction(c))
                    .map(|j| j.label.clone())
                    .unwrap_or_else(|| {
                        refs.jurisdiction_type_label(&jt).unwrap_or(&jt).to_string()
                    });
                lj_dtos::ChronologyEntry {
                    id: entry_id,
                    label,
                    date,
                    current,
                    solution: solution_uid.as_deref().map(|u| refs.tag(u).key),
                    docket_numbers: r.get(7),
                    link: None,
                }
            })
            .collect();
        // Le lien porte du plus récent vers la décision qu'il attaque : posé
        // sur l'étape du haut quand la paire adjacente est directement liée.
        for i in 0..entries.len() - 1 {
            entries[i].link = edges
                .get(&(entries[i].id.clone(), entries[i + 1].id.clone()))
                .cloned();
        }
        entries
    } else {
        Vec::new()
    };

    // Projection méta (ADR 0090) : lecture pure depuis `source_fields`, unique
    // frontière de validation de ces champs (#12). `source_fields` est un JSONB
    // hétérogène par source : forme inattendue ⇒ champ vide, jamais d'erreur.
    let (themes, nac) = match source_fields.as_ref() {
        Some(sf) => (project_themes(sf), project_nac(sf)),
        None => (Vec::new(), None),
    };

    // Reconstruction `(full_text, source_fields)` → `Decision` (ADR 0085) puis
    // dérivation paragraphes/sections = CPU-bloquant. Offload sur
    // `spawn_blocking` (même convention que `highlight` dans search.rs) : le
    // runtime `current_thread` (contrainte SSR Leptos, ADR 0061) sérialiserait
    // sinon les requêtes en vol pendant le parse sur l'unique thread async. La
    // `Decision` reconstruite est byte-identique au parse du payload brut (banc
    // de parité #20, gate banc #18).
    let (paragraphs, paragraph_spans, sections, source_xml) =
        if let (Some(full_text), Some(source_fields)) = (full_text, source_fields) {
            let pid = public_id.to_string();
            tokio::task::spawn_blocking(move || -> ParsedPayload {
                let decision = Decision::from_source_fields(&full_text, &source_fields, &pid);
                let mut paragraphs = decision_paragraphs(&decision);
                let mut sections = sections_from_decision(&decision, &citations);
                // Spans alignés sur le corps plat (repli front sans sections). Les
                // offsets sont en codepoints sur le texte original ; truecase
                // (length-preserving en codepoints) ne les invalide pas.
                let flat_spans = flat_paragraph_spans(&decision, &citations);
                // Vieilles décisions intégralement en MAJUSCULES (surtout Cassation) :
                // recasse déterministe pour l'affichage uniquement. Appliquée APRÈS
                // l'assignation des sections (qui matche les paragraphes par offsets
                // sur le texte source original). N'altère ni le stocké ni l'index BM25.
                if truecase::is_caps_lock(&decision.texte_integral_clean) {
                    for p in &mut paragraphs {
                        *p = truecase::truecase(p);
                    }
                    if let Some(secs) = sections.as_mut() {
                        for section in secs.iter_mut() {
                            for p in &mut section.paragraphs {
                                *p = truecase::truecase(p);
                            }
                        }
                    }
                }
                // Python : `source_xml=DecisionSourceXml(**payload_meta) if
                // payload_meta else None`. `payload_meta` n'est falsy que s'il est
                // vide ; dès qu'un payload est parsé le dict est construit (truthy
                // même tout-None) → on émet la méta systématiquement.
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

    // Titre canonique (ADR 0170) : siège recomposé depuis les axes structurés,
    // même composition que `search_title` à l'ingest.
    let jur_display = crate::titles::decision_jurisdiction(
        refs.jurisdiction_type_label(&jur_type_raw)
            .unwrap_or(&jur_type_raw),
        jurisdiction_name.as_deref(),
    );
    let seat = crate::titles::decision_seat(
        &jur_display,
        chamber_position.as_deref(),
        formation_uid.as_deref(),
        office_uid.as_deref(),
    );
    let title = lj_core::titles::decision_title(
        &jur_display,
        seat.as_deref(),
        date_lecture.as_deref(),
        docket_numbers
            .as_deref()
            .and_then(|d| d.first())
            .map(String::as_str),
    );

    Ok(DecisionDetail {
        id,
        jurisdiction_type: jur_type,
        title,
        paragraphs,
        paragraph_spans,
        sections,
        summary,
        jurisdiction_code,
        jurisdiction_name,
        date_lecture,
        solution: opt_tag(&solution_uid, &refs),
        procedure: opt_tag(&procedure_uid, &refs),
        office: opt_tag(&office_uid, &refs),
        legal_domain: opt_tag(&legal_domain_uid, &refs),
        publication: opt_tag(&publication_uid, &refs),
        publication_codes,
        date_audience,
        docket_numbers,
        seat,
        chamber: opt_tag(&chamber_uid, &refs),
        formation: opt_tag(&formation_uid, &refs),
        legal_references,
        source_xml,
        themes,
        nac,
        ecli,
        source,
        chronology,
        commentaires: {
            let mut c = ariane_bundle
                .as_ref()
                .map(commentaires_from_bundle)
                .unwrap_or_default();
            if let Some(files) = judilibre_files.as_ref() {
                c.extend(notes_from_judilibre_files(files));
            }
            if let Some(notes) = web_notes.as_ref() {
                c.extend(notes_from_web_bundle(notes));
            }
            c
        },
    })
}

/// Commentaires doctrine web (`source_fields.commentaires[]` des fournisseurs
/// autres qu'ArianeWeb : ADDE, plus tard GISTI…) → DTO. Entrées `note`
/// auto-suffisantes (le lien est stocké tel quel — pas de composition).
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
                    body: None,
                    title: c["title"].as_str().map(str::to_string),
                    publisher: c["publisher"].as_str().map(str::to_string),
                    access: c["access"].as_str().map(str::to_string),
                    rubriques: Vec::new(),
                    renvois: Vec::new(),
                    url: c["url"].as_str().map(str::to_string),
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

/// Documents liés Judilibre (`files[]`, Licence Ouverte 2.0) → commentaires
/// `note` : rapports, avis, communiqués, notes explicatives de la Cour de
/// cassation. Lien direct vers le PDF public (`rawUrl`). Les graphiques et la
/// décision annotée (`datt_*`) sont écartés — ce ne sont pas des commentaires.
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
                        body: None,
                        title: Some(label.to_string()),
                        publisher: Some("Cour de cassation".to_string()),
                        access: None,
                        rubriques: Vec::new(),
                        renvois: Vec::new(),
                        url: Some(url),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Bundle `commentaires[]` ArianeWeb (ADR 0204) → DTO. Les analyses se
/// déplient sur place ; l'entrée `conclusions` (existence seule en base)
/// devient une ligne-lien composée ici — **seul endroit** à corriger si le
/// Conseil d'État change son schéma d'URL.
fn commentaires_from_bundle(bundle: &serde_json::Value) -> Vec<lj_dtos::Commentaire> {
    let dossier = bundle["dossier"].as_str();
    let date = bundle["date_lecture"].as_str();
    let str_vec = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    bundle["commentaires"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|c| match c["kind"].as_str()? {
                    "analyse" => Some(lj_dtos::Commentaire {
                        kind: "analyse".to_string(),
                        author: c["author"].as_str().map(str::to_string),
                        date: c["date"].as_str().map(str::to_string),
                        body: c["body"].as_str().map(str::to_string),
                        title: None,
                        publisher: None,
                        access: None,
                        rubriques: str_vec(&c["meta"]["rubriques"]),
                        renvois: str_vec(&c["meta"]["renvois"]),
                        url: None,
                    }),
                    "conclusions" => Some(lj_dtos::Commentaire {
                        kind: "conclusions".to_string(),
                        author: None,
                        date: date.map(str::to_string),
                        body: None,
                        title: None,
                        publisher: None,
                        access: None,
                        rubriques: Vec::new(),
                        renvois: Vec::new(),
                        url: Some(format!(
                            "https://www.conseil-etat.fr/fr/arianeweb/CRP/conclusion/{}/{}",
                            date?, dossier?
                        )),
                    }),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Prévisualisation légère d'une décision par `public_id` (hover card des
/// liens de jurisprudence, ADR 0168) : identité + solution + codes de
/// publication + résumé — le corps n'est jamais lu.
#[instrument(skip(state), fields(db.system = "postgresql", public_id = %public_id))]
pub async fn decision_preview(state: &AppState, public_id: &str) -> Result<DecisionPreview> {
    let refs = referential(state).await?;
    let conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;

    let row = conn
        .query_opt(
            "
            SELECT
              d.public_id,
              d.jurisdiction_type,
              d.jurisdiction_code,
              d.date_lecture::text,
              d.docket_numbers,
              d.solution_uid,
              d.procedure_uid,
              d.publication_codes,
              d.summary,
              d.chamber_position,
              d.formation_uid,
              d.office_uid
            FROM decisions d
            WHERE d.public_id = $1
            ",
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
    let procedure_uid: Option<String> = row.get(6);
    let publication_codes: Vec<String> = row.get(7);
    let summary: Option<String> = row.get(8);
    let chamber_position: Option<String> = row.get(9);
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
    let seat = crate::titles::decision_seat(
        &jur_display,
        chamber_position.as_deref(),
        formation_uid.as_deref(),
        office_uid.as_deref(),
    );
    let title = lj_core::titles::decision_title(
        &jur_display,
        seat.as_deref(),
        date_lecture.as_deref(),
        docket_numbers
            .as_deref()
            .and_then(|d| d.first())
            .map(String::as_str),
    );

    Ok(DecisionPreview {
        id,
        title,
        solution: opt_tag(&solution_uid, &refs),
        procedure: opt_tag(&procedure_uid, &refs),
        publication_codes,
        summary,
    })
}

/// Décisions similaires (ANN top-k sur l'embedding des chunks bord) pour un
/// `public_id`. Port de `fetch_similar_decisions`.
///
/// Le `title_html` du DTO Rust est construit via [`decision_title`] (le DTO
/// Python expose les champs bruts et le front calcule le titre ; ici le contrat
/// `SimilarDecisionHit` ne porte qu'`id`/`title_html`/`score`).
///
/// `probes` provient de `settings.vchord_probes` (paramètre `SET LOCAL
/// vchordrq.probes`).
///
/// Span DB nommé (`db.system="postgresql"`) pour le drill-down Tempo.
#[instrument(skip(state), fields(db.system = "postgresql", public_id = %public_id))]
pub async fn similar_decisions(
    state: &AppState,
    public_id: &str,
    limit: u32,
) -> Result<Vec<SimilarDecisionHit>> {
    let limit_i64 = limit as i64;
    let inner_limit = (limit_i64 * SIMILAR_INNER_LIMIT_FACTOR).max(limit_i64 + 1);
    let probes = state.settings.vchord_probes;

    // `inner_limit`/`outer_limit` injectés en littéraux (parité avec
    // `sql.Literal`) ; `probes` idem pour `SET LOCAL`.
    let query = format!(
        "
        WITH source AS (
          SELECT decision_id, embedding
          FROM (
            SELECT
              c.decision_id,
              c.embedding,
              row_number() OVER (ORDER BY c.chunk_index) AS rn_first,
              row_number() OVER (ORDER BY c.chunk_index DESC) AS rn_last
            FROM decisions d
            JOIN decision_chunks c ON c.decision_id = d.id
            WHERE d.public_id = $1
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
              AND c.decision_id <> src.decision_id
            ORDER BY distance
            LIMIT {inner_limit}
          ) cand ON TRUE
        ),
        ranked AS (
          SELECT n.decision_id, MAX(n.score) AS score
          FROM neighbors n
          GROUP BY n.decision_id
          ORDER BY score DESC
          LIMIT {outer_limit}
        )
        SELECT
          d.public_id,
          d.jurisdiction_type,
          d.jurisdiction_code,
          r.score,
          d.date_lecture::text,
          d.docket_numbers,
          d.solution_uid,
          d.procedure_uid,
          d.office_uid,
          d.publication_codes,
          d.summary
        FROM ranked r
        JOIN decisions d ON d.id = r.decision_id
        ORDER BY r.score DESC, d.date_lecture DESC NULLS LAST, d.public_id
        ",
        inner_limit = inner_limit,
        outer_limit = limit_i64,
    );

    let mut conn = state
        .pool
        .get()
        .await
        .map_err(|e| ApiError::Internal(format!("checkout connexion: {e}")))?;

    let rows = {
        let tx = conn.transaction().await?;
        tx.batch_execute(&format!("SET LOCAL vchordrq.probes = {probes}"))
            .await?;
        let rows = tx.query(&query, &[&public_id]).await?;
        // Lecture seule : commit (parité avec le `with conn.transaction()`).
        tx.commit().await?;
        rows
    };

    if rows.is_empty() {
        // Distinguer 404 (décision absente) de « pas de voisins ».
        let exists = conn
            .query_opt(
                "SELECT 1 FROM decisions WHERE public_id = $1",
                &[&public_id],
            )
            .await?;
        if exists.is_none() {
            return Err(ApiError::NotFound);
        }
        return Ok(Vec::new());
    }

    let refs = referential(state).await?;
    let mut hits = Vec::with_capacity(rows.len());
    for row in &rows {
        let id: String = row.get(0);
        let jur_type_raw: String = row.get(1);
        let jurisdiction_code: Option<String> = row.get(2);
        let score: f64 = row.get(3);
        let date_lecture: Option<String> = row.get(4);
        let docket_numbers: Option<Vec<String>> = row.get(5);
        let docket_numbers = docket_numbers.filter(|d| !d.is_empty());
        let solution_uid: Option<String> = row.get(6);
        let procedure_uid: Option<String> = row.get(7);
        let office_uid: Option<String> = row.get(8);
        let publication_codes: Option<Vec<String>> = row.get(9);
        let summary: Option<String> = row.get(10);

        hits.push(SimilarDecisionHit {
            id,
            jurisdiction_type: parse_enum_str(&jur_type_raw).unwrap_or(JurisdictionType::Ta),
            jurisdiction_name: jurisdiction_code
                .as_deref()
                .and_then(|c| refs.jurisdiction(c))
                .map(|j| j.label.clone()),
            score,
            date_lecture,
            docket_numbers,
            solution: opt_tag(&solution_uid, &refs),
            procedure: opt_tag(&procedure_uid, &refs),
            office: opt_tag(&office_uid, &refs),
            publication_codes: publication_codes.unwrap_or_default(),
            summary,
        });
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

    /// Raccourci de construction d'une `RawLegalRef` pour les tests : `(num,
    /// num_key)` posés à l'identique (forme déjà canonique).
    fn raw_ref(instrument: &str, slug: Option<&str>, articles: &[&str]) -> RawLegalRef {
        (
            instrument.to_string(),
            slug.map(str::to_string),
            articles
                .iter()
                .map(|a| (a.to_string(), Some(a.to_string())))
                .collect(),
        )
    }

    #[test]
    fn parse_legal_refs_keeps_instrument_without_articles() {
        // Instrument cité sans article précis : conservé tel quel.
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
        // Citation non résolue (slug None : free text « règlement intérieur ») →
        // exclue ; seule la référence ancrée au catalogue survit.
        let raw = vec![
            raw_ref("Règlement intérieur", None, &["12"]),
            raw_ref("Code civil", Some("code-civil"), &["1240"]),
        ];
        let refs = parse_legal_refs(&raw).expect("la ref résolue survit");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].instrument, "Code civil");
        // Que des non-résolues → liste vide → None.
        let only_unresolved = vec![raw_ref("Règlement intérieur", None, &["12"])];
        assert!(parse_legal_refs(&only_unresolved).is_none());
    }

    #[test]
    fn find_char_indexes_in_codepoints() {
        // Texte avec un caractère multi-octets avant la cible.
        let text = "café au lait, motivations: rejet";
        // « motivations » commence après « café au lait, » (codepoints).
        let pos = find_char(text, "motivations", 0).unwrap();
        assert_eq!(
            text.chars().skip(pos).take(11).collect::<String>(),
            "motivations"
        );
        // Recherche à partir d'un curseur au-delà : introuvable.
        assert!(find_char(text, "café", 5).is_none());
    }

    #[test]
    fn section_labels_canonicalize_kinds() {
        assert_eq!(section_label("dispositif"), Some("Dispositif"));
        assert_eq!(section_label("visa"), Some("Visa"));
        assert_eq!(section_label("inconnu"), None);
    }

    // ── Spans de citation cliquables (ADR 0125 / 0134) ───────────────────────

    use lj_core::decision::DecisionSection as CoreSection;

    /// `Decision` minimale pour les tests de mapping : seuls `texte_integral_clean`
    /// et `sections` (offsets codepoints) portent l'information utile.
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
            texte_integral_raw: clean.to_string(),
            texte_integral_clean: clean.to_string(),
            sections,
            metadata_header: String::new(),
            visa_trim: String::new(),
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

    fn raw(start: usize, end: usize, text_uid: &str, num_key: Option<&str>) -> RawCite {
        RawCite {
            start,
            end,
            text_uid: text_uid.to_string(),
            slug: "code-de-la-route".to_string(),
            num_key: num_key.map(str::to_string),
            href: "/texte/code-de-la-route".to_string(),
            label: "Code de la route".to_string(),
            suivants: false,
        }
    }

    #[test]
    fn range_detection_bounds_du_meme_texte_separees_de_a() {
        // « les articles L. 225-1 à L. 225-9 du code » : deux bornes du même
        // texte, à article, séparées du seul mot « à » → plage (le handler les
        // fusionne en un span pleine-plage et résout les intermédiaires).
        let text: Vec<char> = "les articles L. 225-1 à L. 225-9 du code".chars().collect();
        assert_eq!(text[13..21].iter().collect::<String>(), "L. 225-1");
        assert_eq!(text[24..32].iter().collect::<String>(), "L. 225-9");
        let a = raw(13, 21, "LEGITEXT000006074228", Some("L. 225-1"));
        let b = raw(24, 32, "LEGITEXT000006074228", Some("L. 225-9"));
        assert!(is_article_range(&a, &b, &text));
        // Textes différents → pas une plage.
        let autre = raw(24, 32, "LEGITEXT000006070933", Some("L. 225-9"));
        assert!(!is_article_range(&a, &autre, &text));
        // Mention nue (sans numéro d'article) → pas une plage.
        let nue = raw(24, 32, "LEGITEXT000006074228", None);
        assert!(!is_article_range(&a, &nue, &text));
        // Interstice ≠ « à » (« et ») → pas une plage.
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
        // Bornes énumérables : même radical, pas entiers.
        assert_eq!(
            range_stem("L. 225-1", "L. 225-9"),
            Some(("L. 225".to_string(), 1, 9))
        );
        assert_eq!(range_stem("102", "108"), Some((String::new(), 102, 108)));
        // Radicaux différents, suffixe lettré, ordre inversé, plage géante → None.
        assert_eq!(range_stem("L. 225-1", "L. 226-3"), None);
        assert_eq!(range_stem("50 sexies B", "H"), None);
        assert_eq!(range_stem("L. 225-9", "L. 225-1"), None);
        assert_eq!(range_stem("1", "300"), None);
        // Clés candidates : entiers stricts + sous-articles du pas bas inclus.
        let (exact, like) = range_candidate_keys("L. 225", 1, 4);
        assert_eq!(exact, vec!["L. 225-2", "L. 225-3"]);
        assert_eq!(like, vec!["L. 225-1-%", "L. 225-2-%", "L. 225-3-%"]);
    }

    #[test]
    fn global_offset_maps_to_correct_paragraph_local_offset_with_multibyte() {
        // Deux paragraphes ; « é » (2 octets, 1 codepoint) avant et dans la cible.
        // P0 = "Considérant l'article 1240" (0..26 codepoints)
        // séparateur "\n" en codepoint 26
        // P1 = "Décision rendue, voir 1241." (27..54)
        let clean = "Considérant l'article 1240\nDécision rendue, voir 1241.";
        // Vérifie les offsets codepoints qu'on vise.
        let p0_start = clean.chars().position(|c| c == 'C').unwrap();
        assert_eq!(p0_start, 0);
        let n_p0 = "Considérant l'article 1240".chars().count();
        // « 1240 » global = derniers 4 codepoints de P0.
        let g_start = n_p0 - 4;
        let g_end = n_p0;

        let decision = mk_decision(clean, Vec::new());
        // Spans renvoyés section par section : une seule section (aucune fournie ⇒
        // None). On teste directement les plages globales + spans_for_range.
        let ranges = paragraph_global_ranges(&decision);
        assert_eq!(ranges.len(), 2);
        // P0 occupe [0, n_p0) ; le span global tombe dedans → local [n_p0-4, n_p0).
        let (p0s, p0e) = ranges[0];
        assert_eq!((p0s, p0e), (0, n_p0));
        let spans0 = spans_for_range(
            &[cite(g_start, g_end, Some("/texte/code-civil/1240"))],
            p0s,
            p0e,
        );
        assert_eq!(spans0.len(), 1);
        assert_eq!((spans0[0].start, spans0[0].end), (n_p0 - 4, n_p0));
        assert_eq!(
            clean.chars().skip(g_start).take(4).collect::<String>(),
            "1240"
        );
        // P1 n'inclut PAS ce span (hors de sa plage).
        let (p1s, p1e) = ranges[1];
        let spans1 = spans_for_range(&[cite(g_start, g_end, None)], p1s, p1e);
        assert!(spans1.is_empty());
    }

    #[test]
    fn overlapping_spans_merge_and_union_targets() {
        // Paragraphe [0, 40). Deux citations qui se chevauchent → UNE région
        // (enveloppe 10..30) portant les DEUX cibles (menu déroulant côté front) :
        // aucune n'est droppée, contrairement à l'ancien longest-win.
        let short = cite(12, 20, Some("/texte/x/court"));
        let long = cite(10, 30, Some("/texte/x/long"));
        let spans = spans_for_range(&[short, long], 0, 40);
        assert_eq!(spans.len(), 1);
        assert_eq!((spans[0].start, spans[0].end), (10, 30));
        // Ordre = apparition (tri par début) : long (10) puis court (12).
        let hrefs: Vec<_> = spans[0].targets.iter().map(|t| t.href.as_deref()).collect();
        assert_eq!(hrefs, vec![Some("/texte/x/long"), Some("/texte/x/court")]);
    }

    #[test]
    fn coextensive_multi_article_yields_one_span_multi_target() {
        // « articles 1382, 1383 et 1384 du code civil » : N lignes legal_citation au
        // MÊME span → une région, N cibles (menu). C'est le cas que l'ancien
        // longest-win réduisait à un seul lien.
        let mk = |href: &str, label: &str| GlobalCitation {
            start: 5,
            end: 40,
            targets: vec![CitationTarget {
                href: Some(href.to_string()),
                label: label.to_string(),
            }],
        };
        let cites = [
            mk("/texte/code-civil/1382", "1382"),
            mk("/texte/code-civil/1383", "1383"),
            mk("/texte/code-civil/1384", "1384"),
            mk("/texte/code-civil/1382", "1382"), // doublon exact → dédupliqué
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
        // Deux citations disjointes (résolue + non résolue) → deux spans, triés.
        let a = cite(20, 24, Some("/texte/x/2"));
        let b = cite(5, 9, None);
        let spans = spans_for_range(&[a, b], 0, 40);
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].start, spans[0].end), (5, 9));
        assert_eq!(spans[0].targets.len(), 1);
        assert!(spans[0].targets[0].href.is_none()); // la projection est agnostique au href
        assert_eq!((spans[1].start, spans[1].end), (20, 24));
        assert_eq!(spans[1].targets[0].href.as_deref(), Some("/texte/x/2"));
    }

    #[test]
    fn multi_occurrence_yields_one_span_per_mention() {
        // Une même citation mentionnée deux fois dans le paragraphe → deux spans.
        let m1 = cite(3, 7, Some("/texte/x/9"));
        let m2 = cite(15, 19, Some("/texte/x/9"));
        let spans = spans_for_range(&[m1, m2], 0, 30);
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].start, spans[0].end), (3, 7));
        assert_eq!((spans[1].start, spans[1].end), (15, 19));
    }

    #[test]
    fn span_outside_paragraph_is_ignored() {
        // Span entièrement hors de la plage du paragraphe → aucun span.
        let spans = spans_for_range(&[cite(100, 110, Some("/texte/x/1"))], 0, 40);
        assert!(spans.is_empty());
        // Span à cheval sur la borne (non entièrement contenu) → ignoré.
        let spans = spans_for_range(&[cite(38, 45, Some("/texte/x/1"))], 0, 40);
        assert!(spans.is_empty());
    }
}
