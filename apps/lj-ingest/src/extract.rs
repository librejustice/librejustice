//! Pont ingest → store : exécute la pile d'extraction lourde (`lj-extract`) et
//! produit les bundles que `lj-store` persiste. Depuis ADR 0123 §3, `lj-store`
//! ne tire plus `lj-extract` — toute l'extraction (champs, `canonical_ref`,
//! normalisation des citations) vit ici, côté ingest.

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_core::decision::Decision;
use lj_extract::link::{LinkSnapshot, LinkTarget};
use lj_store::repository::{CitationOccurrenceRow, ExtractedFields};

/// `canonical_ref` (citation légale, ADR 0100) d'une décision déjà parsée, via
/// son extracteur routé. `None` si la juridiction n'est pas routée ou si
/// l'identité est incomplète (discriminants fiables manquants) — on ne pose pas
/// de clé bancale (#12). Ex-`compute_canonical_ref` de `lj-store`.
pub fn canonical_ref(decision: &Decision) -> Option<String> {
    lj_extract::extract::routed(decision).ok()?;
    lj_extract::identity::decision_canonical_ref(decision)
}

/// Champs structurés extraits d'une décision, prêts à persister (ex-
/// `ExtractedFields::from_decision` de `lj-store`, ADR 0085 / 0123 §3).
///
/// UN [`lj_extract::compiled::doc_extract`] par décision (ADR 0156/0158) : une
/// seule passe automate partagée entre les composeurs de champs (`DocScan`) et
/// les citations compilées — les fonctions plates champ-par-champ re-scannent
/// (~3× le coût, cf. étape 7 du plan).
pub fn extracted_fields(
    decision: &Decision,
    link: &LinkSnapshot,
    vocab: &lj_extract::compiled::CompiledVocab,
    chrono: &lj_extract::chrono::ChronoSnapshot,
) -> Result<ExtractedFields> {
    use lj_extract::extract as ex;
    ex::routed(decision).map_err(|e| anyhow!("extract: {e}"))?;

    // `date_cls.fromisoformat` toléré : un parse invalide → None (cf. Python).
    fn parse_date(s: Option<String>) -> Option<NaiveDate> {
        let s = s?;
        if s.is_empty() {
            return None;
        }
        NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()
    }

    let docx =
        lj_extract::compiled::doc_extract(&decision.texte_integral_clean, vocab, link, chrono);
    let scan = Some(&docx.scan);

    let formation = ex::formation_or_chamber_scanned(decision, scan);
    let formation_or_chamber = match formation {
        Some(f) if !f.is_empty() && f != "INCONNU" => Some(f),
        _ => None,
    };
    // Formation structurée (ADR 0170) : mêmes sources, décomposées en axes.
    let axes = ex::formation_axes_scanned(decision, scan);

    let citation_occurrences = Some(
        docx.citations
            .iter()
            .map(|c| CitationOccurrenceRow {
                char_start: c.char_start as i32,
                char_end: c.char_end as i32,
                ref_text_uid: c.target.ref_text_uid.clone(),
                ref_num_key: c.target.ref_num_key.clone(),
            })
            .collect(),
    );

    let docket_numbers = ex::docket_numbers_scanned(decision, scan).unwrap_or_default();
    let case_citations = Some(merge_attacked_spans(
        case_citation_rows(&docx.cases, &docket_numbers),
        lj_extract::chrono::attacked_text_spans(decision, chrono),
    ));

    let mut procedure = ex::procedure_scanned(decision, scan);
    // Vote domaine par le profil des codes cités (`lj_extract::domain`,
    // ADR 0156) — même fallback que le banc : seulement quand le chemin
    // signaux n'a rien rendu.
    if procedure.legal_domain_uid.is_none() {
        let chamber = format!(
            "{} {}",
            decision.formation.as_deref().unwrap_or(""),
            formation_or_chamber.as_deref().unwrap_or("")
        )
        .to_lowercase();
        let ctx = lj_extract::domain::DomainContext {
            admin: !ex::is_judiciaire(decision),
            social: chamber.contains("social") || chamber.contains("prud"),
            commercial: chamber.contains("commer"),
            hint: procedure.domain_hint,
        };
        let cites = docx.citations.iter().filter_map(|c| {
            Some((
                c.target.ref_text_uid.as_deref()?,
                c.target.ref_num_key.as_deref(),
            ))
        });
        procedure.legal_domain_uid = lj_extract::domain::legal_domain_uid(cites, ctx);
    }
    let fields = ExtractedFields {
        jurisdiction_name: ex::jurisdiction_name_scanned(decision, scan),
        date_lecture: parse_date(ex::extract_date_lecture(decision)),
        date_audience: parse_date(ex::date_audience_scanned(decision, scan)),
        docket_numbers,
        formation_or_chamber,
        chamber_position: axes.chamber_position,
        chambre_uid: axes.chambre_uid.map(str::to_string),
        formation_uid: axes.formation_uid.map(str::to_string),
        publication_codes: decision.publication_codes.clone(),
        solution_uid: ex::solution_scanned(decision, scan),
        // Le rôle lu dans la formation greffe (JLD, JCP, juge des référés…)
        // complète les scanners texte, qui priment quand ils ont parlé.
        voie_uid: procedure.voie_uid.or(axes.voie_uid.map(str::to_string)),
        office_uid: procedure.office_uid.or(axes.office_uid.map(str::to_string)),
        legal_domain_uid: procedure.legal_domain_uid,
        applicant_counsel_names: ex::applicant_counsel_names_scanned(decision, scan)
            .unwrap_or_default(),
        applicant_law_firms: ex::applicant_law_firms_scanned(decision, scan).unwrap_or_default(),
        applicant_companies: ex::applicant_companies_scanned(scan).unwrap_or_default(),
        defendant_counsel_names: ex::defendant_counsel_names_scanned(scan).unwrap_or_default(),
        defendant_law_firms: ex::defendant_law_firms_scanned(scan).unwrap_or_default(),
        defendant_companies: ex::defendant_companies_scanned(scan).unwrap_or_default(),
        themes: decision.themes.clone(),
        citation_occurrences,
        decision_links: Some(
            lj_extract::chrono::prior_decision_refs(decision, chrono)
                .into_iter()
                .map(|r| lj_store::repository::DecisionLinkRow {
                    link_type: r.link_type.as_str().to_string(),
                    target_ref: r.target_ref,
                })
                .collect(),
        ),
        case_citations,
        ..Default::default()
    };
    Ok(facet_enrich(
        fields,
        decision.juridiction_type.as_deref(),
        decision.juridiction_location.as_deref(),
    ))
}

/// Complète un [`ExtractedFields`] avec les référentiels dérivés hors-scanner :
/// `publication:*` (agrégat des codes de publication) et la ligne
/// `jurisdiction` (code + création à la volée). Les autres uids (`solution:*`,
/// `voie:*`, `office:*`, `domaine:*`) sont émis par les scanners (v12).
pub fn facet_enrich(
    mut e: ExtractedFields,
    juridiction_type: Option<&str>,
    location: Option<&str>,
) -> ExtractedFields {
    e.publication_uid = Some(lj_extract::facets::publication_uid(&e.publication_codes));
    e.jurisdiction = juridiction_type
        .and_then(|jt| {
            lj_extract::facets::jurisdiction_ref(
                jt,
                location,
                e.jurisdiction_name.as_deref(),
                e.formation_or_chamber.as_deref(),
            )
        })
        .map(|j| lj_store::repository::JurisdictionRow {
            code: j.code,
            juridiction_type: j.juridiction_type,
            city: j.city,
            label: j.label,
        });
    e
}

/// Variante sans `Decision` (fonds scrapés à `ExtractedFields` préconstruits,
/// ex. CNDA) : pas de code de localisation.
pub fn with_facet_uids(e: ExtractedFields, juridiction_type: Option<&str>) -> ExtractedFields {
    facet_enrich(e, juridiction_type, None)
}

/// Une occurrence de citation enrichie de ses clés canoniques et de la cible
/// posée par le linker in-pass — l'artefact partagé ingest ↔ banc (ADR 0145) :
/// l'ingest le projette en [`CitationOccurrenceRow`] (span + cible), le banc
/// le score tel quel. Même normalisation, même linker.
pub struct LinkedOccurrence {
    pub char_start: usize,
    pub char_end: usize,
    /// Forme de surface de l'instrument (capture brute).
    pub instrument: String,
    /// Forme de surface de l'article, si la ligne en porte un.
    pub article: Option<String>,
    /// `normalize_instrument(instrument)` — même vocabulaire que
    /// `legal_text.title_key`.
    pub text_key: String,
    pub article_key: Option<String>,
    pub target: LinkTarget,
}

/// Projette les citations de jurisprudence (ADR 0165) en lignes à persister,
/// propre en-tête exclu : un span dont le numéro (chiffres seuls) est un
/// docket de la décision est sa propre référence (« RG n° 21/04532 » du
/// bandeau CA, « Pourvoi n° … » Cassation), pas une citation. Partagé par le
/// bridge ingest et le banc (qui score exactement l'artefact persisté).
pub fn case_citation_rows(
    cases: &[lj_extract::compiled::CompiledCase],
    docket_numbers: &[String],
) -> Vec<lj_store::repository::CaseCitationRow> {
    let own_digits: std::collections::HashSet<String> = docket_numbers
        .iter()
        .map(|d| d.chars().filter(char::is_ascii_digit).collect())
        .collect();
    cases
        .iter()
        .filter(|c| {
            let digits: String = c
                .target_ref
                .rsplit('|')
                .next()
                .unwrap_or("")
                .chars()
                .filter(char::is_ascii_digit)
                .collect();
            !own_digits.contains(&digits)
        })
        .map(|c| lj_store::repository::CaseCitationRow {
            char_start: c.char_start as i32,
            char_end: c.char_end as i32,
            target_ref: c.target_ref.clone(),
        })
        .collect()
}

/// Fusionne les spans pontés métadonnée (ADR 0161 ∩ 0165) dans les lignes du
/// lexer : la décision attaquée citée inline sans docket (« l'arrêt attaqué
/// (Paris, 22 août 2024) ») porte le `target_ref` du lien de chronologie,
/// résolu par le pont SQL depuis `decision_links`. Les spans du lexer priment
/// en cas de chevauchement. Pont prod complet — rejoué tel quel par le banc.
pub fn merge_attacked_spans(
    mut case_rows: Vec<lj_store::repository::CaseCitationRow>,
    attacked: Vec<(usize, usize, String)>,
) -> Vec<lj_store::repository::CaseCitationRow> {
    for (s, e, target_ref) in attacked {
        let (s, e) = (s as i32, e as i32);
        if case_rows.iter().any(|c| c.char_start < e && s < c.char_end) {
            continue;
        }
        case_rows.push(lj_store::repository::CaseCitationRow {
            char_start: s,
            char_end: e,
            target_ref,
        });
    }
    case_rows.sort_by_key(|c| c.char_start);
    case_rows
}

/// Occurrences liées du moteur compilé (ADR 0156/0158/0160) : scan automate
/// unique + composition par-document, lien par le snapshot catalogue du run.
pub fn linked_occurrences_compiled(
    text: &str,
    vocab: &lj_extract::compiled::CompiledVocab,
    link: &LinkSnapshot,
) -> Vec<LinkedOccurrence> {
    lj_extract::compiled::extract_citations(text, vocab, link)
        .into_iter()
        .map(|c| LinkedOccurrence {
            char_start: c.char_start,
            char_end: c.char_end,
            instrument: c.instrument,
            article: c.article,
            text_key: c.text_key,
            article_key: c.article_key,
            target: c.target,
        })
        .collect()
}
