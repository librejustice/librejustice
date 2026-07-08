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
    DecisionSourceXml, FacetTag, JuridictionType, LegalRefArticle, LegalReference,
    SimilarDecisionHit,
};

use tracing::instrument;

use crate::error::{ApiError, Result};
use crate::referential::{referential, Referential};
use crate::state::AppState;
use crate::titles::decision_title;

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
        nom_juridiction: decision.juridiction_nom.clone(),
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

/// Articles purement procéduraux à masquer en sortie API (port de
/// `_PROCEDURAL_ARTICLE_DENYLIST` + `is_procedural_article`, ADR 0058).
fn is_procedural_article(instrument: &str, article: &str) -> bool {
    let denylist: &[&str] = match instrument {
        "Code de procédure civile" => &[
            // frais et dépens
            "695", "696", "699", "700", // forme et prononcé du jugement
            "450", "451", "452", "453", "454", "455", "456", "457", "458", "459", "462", "463",
            "464", "465", "466", // mise en état
            "446-1", "446-2", "446-3", "446-4", "763", "776", "778", "779", "780", "785", "786",
            "787", "788", "789", "790", "799", "800", "802", "803", "804", "805", "807", "808",
            // exécution provisoire
            "514", "515", "517", "521", "524",
            // circuits d'appel et forme des conclusions
            "905", "905-1", "905-2", "906", "907", "908", "909", "910", "911", "912", "913", "914",
            "916", "954", "960", "961", "963", // désistement / péremption
            "384", "385", "394", "395", "399", // procédure de cassation
            "627", "974", "978", "979", "982", "1009-1", "1010", "1011", "1014", "1015", "1018",
            "1022", "1026", "1031-1",
        ],
        "Code de procédure pénale" => &[
            // forme de l'arrêt et procédure du pourvoi
            "567", "567-1-1", "568", "584", "585", "585-1", "586", "590", "591", "592", "593",
            "594", "598", "609-1", "612", "614", "615", "802",
        ],
        "Code de l'organisation judiciaire" => &[
            "L. 131-6",
            "L. 131-6-1",
            "L. 431-3",
            "L. 431-4",
            "L. 432-1",
            "R. 431-5",
        ],
        // frais (équivalent administratif de l'article 700 CPC)
        "Code de justice administrative" => &["L. 761-1"],
        // aide juridictionnelle
        "Loi du 10 juillet 1991" => &["20", "24", "37", "75"],
        _ => &[],
    };
    denylist.contains(&article)
}

/// Une référence brute lue de la DB : `(instrument, slug résolu, [(num affiché,
/// ref_num_key résolu)])`. Le `slug`/`num_key` portent la FK de citation résolue à
/// l'ingest (ADR 0123 §2) ; `None` = non ancré au catalogue (pas de lien).
type RawLegalRef = (String, Option<String>, Vec<(String, Option<String>)>);

/// Construit les `LegalReference` exposées, articles procéduraux masqués (port
/// de `parse_legal_refs`).
///
/// Un instrument réduit à de la pure procédure après filtrage disparaît ; un
/// instrument cité sans article précis est conservé tel quel. Le `slug` et le
/// `numKey` résolus (ADR 0123 §2) sont propagés au DTO pour bâtir les liens
/// `/loi/{slug}/{numKey}` sans re-slugifier côté front. `None` si la liste
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
            .filter(|(num, _)| !is_procedural_article(instrument, num))
            .filter(|(num, _)| seen.insert(num.clone()))
            .map(|(num, num_key)| LegalRefArticle {
                num: num.clone(),
                num_key: num_key.clone().unwrap_or_default(),
            })
            .collect();
        if !original.is_empty() && articles.is_empty() {
            continue;
        }
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

/// Rend lisible la formation source. Deux familles de bruit :
/// - le marqueur cryptique « (JU) » — variantes « (J.U) », « (ju) »,
///   « (J.U.) » — signifie *juge unique* (par opposition à « formation à 3 »)
///   et est développé en clair ;
/// - les codes de tri interne des TA (`formationJugement` DILA :
///   « - etrangers - 15 jours », « - 96h - eloignement », « - 10 000€ »…),
///   qui désignent un circuit d'urgence à juge unique, mappés vers un libellé
///   de formation propre. Un label vide ou réduit au tiret disparaît.
fn formation_display(raw: String) -> Option<String> {
    let stripped = raw
        .trim()
        .trim_start_matches(['-', '–', '—'])
        .trim()
        .to_lowercase();
    let mapped = match stripped.as_str() {
        "" => return None,
        "etrangers - 15 jours" => Some("Juge unique — étrangers (15 jours)"),
        "asile - 15 jours" => Some("Juge unique — asile (15 jours)"),
        "96h - eloignement" => Some("Juge unique — éloignement (96 h)"),
        "ju refere etr 15 jours" | "ju refere etrangers 15 jours" => {
            Some("Juge unique — référé étrangers (15 jours)")
        }
        "10 000€" => Some("Juge unique — litiges de moins de 10 000 €"),
        "48h - gens du voyage" => Some("Juge unique — gens du voyage (48 h)"),
        "référé suspension" | "référés suspension" => Some("Référé suspension"),
        "référé \"mesures utiles\"" | "référés \"mesures utiles\"" => {
            Some("Référé mesures utiles")
        }
        _ => None,
    };
    if let Some(label) = mapped {
        return Some(label.to_string());
    }
    static RE_JU: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    Some(
        RE_JU
            .get_or_init(|| regex::Regex::new(r"(?i)\(\s*j\.?\s*u\.?\s*\)").unwrap())
            .replace_all(&raw, "(juge unique)")
            .into_owned(),
    )
}

/// Valide le code `juridiction_type` issu de la DB et le convertit en
/// [`JuridictionType`] (parité avec le garde Python `if jur_type not in (...)`).
fn parse_jur_type(raw: &str) -> Result<JuridictionType> {
    match raw {
        "TA" => Ok(JuridictionType::Ta),
        "CAA" => Ok(JuridictionType::Caa),
        "CE" => Ok(JuridictionType::Ce),
        "CONSTIT" => Ok(JuridictionType::Constit),
        "TC" => Ok(JuridictionType::Tc),
        "CC" => Ok(JuridictionType::Cc),
        "CA" => Ok(JuridictionType::Ca),
        "TJ" => Ok(JuridictionType::Tj),
        "TCOM" => Ok(JuridictionType::Tcom),
        "CEDH" => Ok(JuridictionType::Cedh),
        "CJUE" => Ok(JuridictionType::Cjue),
        "CNDA" => Ok(JuridictionType::Cnda),
        other => Err(ApiError::Internal(format!(
            "juridiction_type DB invalide : {other:?}"
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
              d.juridiction_type,
              d.full_text,
              ds.source_fields,
              d.solution_uid,
              d.voie_uid,
              d.office_uid,
              d.legal_domain_uid,
              d.publication_codes,
              d.date_lecture::text,
              d.date_audience::text,
              d.jurisdiction_code,
              d.docket_numbers,
              d.formation_or_chamber,
              d.summary,
              d.ecli,
              ds.source
            FROM decisions d
            LEFT JOIN LATERAL (
                SELECT source_fields, source
                FROM decision_sources
                WHERE decision_id = d.id AND deleted_at IS NULL
                ORDER BY source_rank DESC, (lang = 'fra') IS TRUE DESC, id ASC
                LIMIT 1
            ) ds ON true
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
    let voie_uid: Option<String> = row.get(6);
    let office_uid: Option<String> = row.get(7);
    let legal_domain_uid: Option<String> = row.get(8);
    let publication_codes: Vec<String> = row.get(9);
    let date_lecture: Option<String> = row.get(10);
    let date_audience: Option<String> = row.get(11);
    let jurisdiction_code: Option<String> = row.get(12);
    let docket_numbers_raw: Option<Vec<String>> = row.get(13);
    let formation_or_chamber: Option<String> = row.get(14);
    let summary: Option<String> = row.get(15);
    let ecli: Option<String> = row.get(16);
    let source: Option<String> = row.get(17);

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
    // `ref_num_key` distincts (clé canonique du lien `/loi/{slug}/{numKey}`,
    // qui sert aussi de libellé).
    let ref_rows = conn
        .query(
            "
            SELECT lt.title AS instrument,
                   lt.slug,
                   lt.text_uid,
                   COALESCE(array_agg(DISTINCT lc.ref_num_key ORDER BY lc.ref_num_key)
                            FILTER (WHERE lc.ref_num_key IS NOT NULL),
                            ARRAY[]::text[]) AS num_keys
            FROM legal_citation lc
            JOIN legal_text lt ON lt.text_uid = lc.ref_text_uid
            WHERE lc.decision_id = $1
            GROUP BY lt.text_uid, lt.title, lt.slug
            ORDER BY 1
            ",
            &[&decision_id],
        )
        .await?;

    // Spans cliquables (ADR 0125 / 0145) : une ligne `legal_citation` = une
    // mention (codepoints sur `decisions.full_text`, convention 0143). PK scan,
    // une table ; le JOIN `legal_text` fournit slug (href) et titre (libellé).
    //
    // **INNER JOIN sur `legal_text` (ref_text_uid lié)** : on n'overlaye QUE
    // les citations ancrées à un texte enregistré. Les références non liées
    // (`ref_text_uid` NULL) ne sont PAS surlignées dans le corps — on ne
    // souligne pas ce qui ne mène nulle part (décision opérateur 2026-06-30).
    let span_rows = conn
        .query(
            "
            SELECT lc.char_start,
                   lc.char_end,
                   lt.text_uid,
                   lt.slug,
                   lc.ref_num_key,
                   lt.title AS label,
                   EXISTS (SELECT 1 FROM legal_article a
                           WHERE a.text_uid = lt.text_uid) AS has_articles
            FROM legal_citation lc
            JOIN legal_text lt ON lt.text_uid = lc.ref_text_uid
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
            let slug = slug?;
            let num_key = num_key.filter(|k| !k.is_empty());
            // Article ciblé → /loi/{slug}/{numKey}. Mention nue → /loi/{slug}
            // seulement si le texte a ≥ 1 article en base (ADR 0162 §4). Pas de
            // lien = pas de rendu du tout (mort du pointillé, décision
            // opérateur 2026-07-05) : on ne décore pas ce qui ne mène nulle part.
            let href = match num_key.as_deref() {
                Some(k) => format!("/loi/{slug}/{k}"),
                None if has_articles => format!("/loi/{slug}"),
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
            citations.push(GlobalCitation {
                start: a.start,
                end: a.end,
                targets: vec![single(&a)],
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
        // (le titre du texte est déjà porté par la phrase autour du span).
        let bound = |c: &RawCite| CitationTarget {
            href: Some(c.href.clone()),
            label: format!(
                "{} — {}",
                c.num_key.as_deref().expect("gardé par is_article_range"),
                c.label
            ),
        };
        let members = range_members.entry(a.text_uid.clone()).or_default();
        members.push(a.num_key.clone().expect("gardé par is_article_range"));
        members.extend(inter.iter().cloned());
        members.push(b.num_key.clone().expect("gardé par is_article_range"));
        let mut targets = vec![bound(&a)];
        targets.extend(inter.into_iter().map(|nk| CitationTarget {
            href: Some(format!("/loi/{}/{nk}", a.slug)),
            label: format!("{nk} — {}", a.label),
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
            let articles: Vec<(String, Option<String>)> =
                num_keys.into_iter().map(|k| (k.clone(), Some(k))).collect();
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
                   d.juridiction_type,
                   d.date_lecture::text,
                   d.id = $1 AS is_current,
                   d.id
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
                let label = code
                    .as_deref()
                    .and_then(|c| refs.jurisdiction(c))
                    .map(|j| j.label.clone())
                    .unwrap_or_else(|| refs.juridiction_type_label(&jt).unwrap_or(&jt).to_string());
                lj_dtos::ChronologyEntry {
                    id: entry_id,
                    label,
                    date,
                    current,
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

    // Titre machine/stable servi au front (SEO) — même source que MCP/résumé.
    let formation_or_chamber = formation_or_chamber.and_then(formation_display);
    let title = decision_title(
        refs.juridiction_type_label(&jur_type_raw)
            .unwrap_or(&jur_type_raw),
        jurisdiction_name.as_deref(),
        formation_or_chamber.as_deref(),
        date_lecture.as_deref(),
        docket_numbers.as_deref(),
    );

    Ok(DecisionDetail {
        id,
        juridiction_type: jur_type,
        title,
        paragraphs,
        paragraph_spans,
        sections,
        summary,
        jurisdiction_name,
        date_lecture,
        solution: opt_tag(&solution_uid, &refs),
        voie: opt_tag(&voie_uid, &refs),
        office: opt_tag(&office_uid, &refs),
        legal_domain: opt_tag(&legal_domain_uid, &refs),
        publication_codes,
        date_audience,
        docket_numbers,
        formation_or_chamber,
        legal_references,
        source_xml,
        themes,
        nac,
        ecli,
        source,
        chronology,
    })
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
              d.juridiction_type,
              d.jurisdiction_code,
              d.date_lecture::text,
              d.docket_numbers,
              d.solution_uid,
              d.voie_uid,
              d.publication_codes,
              d.summary,
              d.formation_or_chamber
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
    let voie_uid: Option<String> = row.get(6);
    let publication_codes: Vec<String> = row.get(7);
    let summary: Option<String> = row.get(8);
    let formation: Option<String> = row.get::<_, Option<String>>(9).and_then(formation_display);

    let jurisdiction_name = jurisdiction_code
        .as_deref()
        .and_then(|c| refs.jurisdiction(c))
        .map(|j| j.label.clone());
    let docket_numbers = docket_numbers.filter(|d| !d.is_empty());
    let title = decision_title(
        refs.juridiction_type_label(&jur_type_raw)
            .unwrap_or(&jur_type_raw),
        jurisdiction_name.as_deref(),
        formation.as_deref(),
        date_lecture.as_deref(),
        docket_numbers.as_deref(),
    );

    Ok(DecisionPreview {
        id,
        title,
        solution: opt_tag(&solution_uid, &refs),
        voie: opt_tag(&voie_uid, &refs),
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
          d.juridiction_type,
          d.jurisdiction_code,
          r.score,
          d.date_lecture::text,
          d.docket_numbers,
          d.solution_uid,
          d.voie_uid,
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
        let voie_uid: Option<String> = row.get(7);
        let office_uid: Option<String> = row.get(8);
        let publication_codes: Option<Vec<String>> = row.get(9);
        let summary: Option<String> = row.get(10);

        hits.push(SimilarDecisionHit {
            id,
            juridiction_type: parse_enum_str(&jur_type_raw).unwrap_or(JuridictionType::Ta),
            jurisdiction_name: jurisdiction_code
                .as_deref()
                .and_then(|c| refs.jurisdiction(c))
                .map(|j| j.label.clone()),
            score,
            date_lecture,
            docket_numbers,
            solution: opt_tag(&solution_uid, &refs),
            voie: opt_tag(&voie_uid, &refs),
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

    #[test]
    fn formation_display_expands_juge_unique() {
        let f = |s: &str| formation_display(s.to_string()).unwrap();
        assert_eq!(f("10ème chambre (JU)"), "10ème chambre (juge unique)");
        assert_eq!(f("3ème chambre (J.U)"), "3ème chambre (juge unique)");
        assert_eq!(f("Pole social (ju)"), "Pole social (juge unique)");
        assert_eq!(f("8ème chambre (J.U.)"), "8ème chambre (juge unique)");
        // Les autres parenthèses passent telles quelles.
        assert_eq!(
            f("4ème chambre (formation à 3)"),
            "4ème chambre (formation à 3)"
        );
        assert_eq!(f("Juge unique (6)"), "Juge unique (6)");
    }

    #[test]
    fn formation_display_maps_ta_urgency_circuits() {
        let f = |s: &str| formation_display(s.to_string());
        assert_eq!(
            f("- etrangers - 15 jours").as_deref(),
            Some("Juge unique — étrangers (15 jours)")
        );
        assert_eq!(
            f("Asile - 15 jours").as_deref(),
            Some("Juge unique — asile (15 jours)")
        );
        assert_eq!(
            f("- 96h - eloignement").as_deref(),
            Some("Juge unique — éloignement (96 h)")
        );
        assert_eq!(
            f("JU refere etr 15 jours").as_deref(),
            Some("Juge unique — référé étrangers (15 jours)")
        );
        assert_eq!(
            f("- 10 000€").as_deref(),
            Some("Juge unique — litiges de moins de 10 000 €")
        );
        assert_eq!(
            f("- référés \"mesures utiles\"").as_deref(),
            Some("Référé mesures utiles")
        );
        // Tiret seul = pas de formation.
        assert_eq!(f("-"), None);
        // Un label normal passe tel quel.
        assert_eq!(f("1ère chambre").as_deref(), Some("1ère chambre"));
    }

    #[test]
    fn procedural_articles_are_masked_per_instrument() {
        // CPC 700 (frais) est procédural ; CPC 4 (principe directeur) ne l'est pas.
        assert!(is_procedural_article("Code de procédure civile", "700"));
        assert!(!is_procedural_article("Code de procédure civile", "4"));
        // CJA L. 761-1 (équivalent 700) masqué.
        assert!(is_procedural_article(
            "Code de justice administrative",
            "L. 761-1"
        ));
        // Instrument inconnu → jamais procédural.
        assert!(!is_procedural_article("Code civil", "1240"));
    }

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
    fn parse_legal_refs_drops_fully_procedural_instrument() {
        let raw = vec![
            raw_ref(
                "Code de procédure civile",
                Some("code-procedure-civile"),
                &["700", "699"],
            ),
            raw_ref("Code civil", Some("code-civil"), &["1240"]),
        ];
        let refs = parse_legal_refs(&raw).expect("au moins une ref");
        // L'instrument 100% procédural disparaît ; le Code civil reste.
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].instrument, "Code civil");
        assert_eq!(refs[0].slug.as_deref(), Some("code-civil"));
        assert_eq!(refs[0].articles.len(), 1);
        assert_eq!(refs[0].articles[0].num, "1240");
        assert_eq!(refs[0].articles[0].num_key, "1240");
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
            juridiction_code: None,
            juridiction_nom: None,
            juridiction_type: None,
            juridiction_location: None,
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
            href: "/loi/code-de-la-route".to_string(),
            label: "Code de la route".to_string(),
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
            &[cite(g_start, g_end, Some("/loi/code-civil/1240"))],
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
        let short = cite(12, 20, Some("/loi/x/court"));
        let long = cite(10, 30, Some("/loi/x/long"));
        let spans = spans_for_range(&[short, long], 0, 40);
        assert_eq!(spans.len(), 1);
        assert_eq!((spans[0].start, spans[0].end), (10, 30));
        // Ordre = apparition (tri par début) : long (10) puis court (12).
        let hrefs: Vec<_> = spans[0].targets.iter().map(|t| t.href.as_deref()).collect();
        assert_eq!(hrefs, vec![Some("/loi/x/long"), Some("/loi/x/court")]);
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
            mk("/loi/code-civil/1382", "1382"),
            mk("/loi/code-civil/1383", "1383"),
            mk("/loi/code-civil/1384", "1384"),
            mk("/loi/code-civil/1382", "1382"), // doublon exact → dédupliqué
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
        let a = cite(20, 24, Some("/loi/x/2"));
        let b = cite(5, 9, None);
        let spans = spans_for_range(&[a, b], 0, 40);
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].start, spans[0].end), (5, 9));
        assert_eq!(spans[0].targets.len(), 1);
        assert!(spans[0].targets[0].href.is_none()); // la projection est agnostique au href
        assert_eq!((spans[1].start, spans[1].end), (20, 24));
        assert_eq!(spans[1].targets[0].href.as_deref(), Some("/loi/x/2"));
    }

    #[test]
    fn multi_occurrence_yields_one_span_per_mention() {
        // Une même citation mentionnée deux fois dans le paragraphe → deux spans.
        let m1 = cite(3, 7, Some("/loi/x/9"));
        let m2 = cite(15, 19, Some("/loi/x/9"));
        let spans = spans_for_range(&[m1, m2], 0, 30);
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].start, spans[0].end), (3, 7));
        assert_eq!((spans[1].start, spans[1].end), (15, 19));
    }

    #[test]
    fn span_outside_paragraph_is_ignored() {
        // Span entièrement hors de la plage du paragraphe → aucun span.
        let spans = spans_for_range(&[cite(100, 110, Some("/loi/x/1"))], 0, 40);
        assert!(spans.is_empty());
        // Span à cheval sur la borne (non entièrement contenu) → ignoré.
        let spans = spans_for_range(&[cite(38, 45, Some("/loi/x/1"))], 0, 40);
        assert!(spans.is_empty());
    }
}
