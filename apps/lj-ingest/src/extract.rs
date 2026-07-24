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
    jur_labels: &std::collections::HashMap<String, String>,
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

    // Forum (parent implicite par juridiction, v68) : CJUE/CEDH citent leurs
    // textes propres sans les nommer — dérivé du type + ECLI (EU:C/T/F).
    let forum = lj_extract::link::Forum::of(
        decision.jurisdiction_type.as_deref(),
        decision.ecli.as_deref(),
    );
    let docx = lj_extract::compiled::doc_extract(
        &decision.texte_integral_clean,
        vocab,
        link,
        chrono,
        forum,
    );
    let scan = Some(&docx.scan);

    // Formation structurée (ADR 0170) : champs source + bandeau scanné,
    // décomposés en axes ; nourrit aussi la coloration du vote domaine.
    let axes = ex::formation_axes_scanned(decision, scan);

    let citation_occurrences = Some(
        docx.citations
            .iter()
            .map(|c| CitationOccurrenceRow {
                char_start: c.char_start as i32,
                char_end: c.char_end as i32,
                ref_text_uid: c.target.ref_text_uid.clone(),
                ref_num_key: c.target.ref_num_key.clone(),
                suivants: c.suivants,
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
        let ctx = lj_extract::domain::context_for(decision, &axes, procedure.domain_hint);
        let cites = docx.citations.iter().filter_map(|c| {
            Some((
                c.target.ref_text_uid.as_deref()?,
                c.target.ref_num_key.as_deref(),
            ))
        });
        procedure.legal_domain_uid = lj_extract::domain::legal_domain_uid(cites, ctx);
    }
    // Raffinement par votes de TERMES du scan : ne touche qu'un domaine nul
    // ou parent nu — les sous-domaines posés par les codes cités priment.
    procedure.legal_domain_uid = lj_extract::domain::refine_with_terms(
        procedure.legal_domain_uid,
        &docx
            .scan
            .domain_term_votes(!lj_extract::extract::is_judiciaire(decision)),
    );
    // Le code NAC du greffe comble les trous et tranche les désaccords de
    // parent — jamais un sous-domaine du même parent posé par les codes.
    procedure.legal_domain_uid =
        lj_extract::domain::refine_with_nac(procedure.legal_domain_uid, decision.nac.as_deref());
    // Les thèmes Judilibre (titrage CC, nomenclature CA/TJ) comblent et
    // tranchent de même — le CRIMINEL de la chambre criminelle reste
    // intouchable.
    procedure.legal_domain_uid =
        lj_extract::domain::refine_with_themes(procedure.legal_domain_uid, &decision.themes);
    // Deux sociétés au litige : le défaut « obligations » est commercial.
    let applicant_companies = ex::applicant_companies_scanned(scan).unwrap_or_default();
    let defendant_companies = ex::defendant_companies_scanned(scan).unwrap_or_default();
    procedure.legal_domain_uid = lj_extract::domain::recolor_commercial_parties(
        procedure.legal_domain_uid,
        !applicant_companies.is_empty() && !defendant_companies.is_empty(),
    );
    // Juridiction catégorielle : la ligne référentielle directe (le nom scanné
    // est un détail interne de `jurisdiction_scanned`, ADR 0146/0170 ét.7).
    let jurisdiction = ex::jurisdiction_scanned(decision, scan, &docket_numbers).map(|j| {
        lj_store::repository::JurisdictionRow {
            source_code: j.code.clone(),
            code: j.code,
            jurisdiction_type: j.jurisdiction_type,
            city: j.city,
            label: j.label,
        }
    });
    // Nomenclatures TA/CAA fermées : un nom de greffe hors liste ne fabrique
    // pas de code fantôme — la décision s'ingère sans facette juridiction et
    // on le signale, pour capitaliser (variante légitime → TA_CITY_SLUGS,
    // corruption → réécriture `admin_name`).
    if jurisdiction.is_none()
        && matches!(
            decision.jurisdiction_type.as_deref(),
            Some("TA") | Some("CAA")
        )
    {
        tracing::warn!(
            source_uid = %decision.source_uid,
            nom = decision.jurisdiction_name.as_deref().unwrap_or(""),
            "juridiction admin hors nomenclature : décision sans facette juridiction"
        );
    }
    let mut fields = ExtractedFields {
        jurisdiction,
        publication_uid: Some(lj_extract::facets::publication_uid(
            &decision.publication_codes,
        )),
        date_lecture: parse_date(ex::extract_date_lecture(decision)),
        date_audience: parse_date(ex::date_audience_scanned(decision, scan)),
        docket_numbers,
        chamber_position: axes.chamber_position,
        chamber_uid: axes.chamber_uid.map(str::to_string),
        formation_uid: axes.formation_uid.map(str::to_string),
        publication_codes: decision.publication_codes.clone(),
        solution_uid: ex::solution_scanned(decision, scan),
        // Le rôle lu dans la formation greffe (JLD, JCP, juge des référés…)
        // complète les scanners texte, qui priment quand ils ont parlé.
        procedure_uid: procedure
            .procedure_uid
            .or(axes.procedure_uid.map(str::to_string)),
        office_uid: procedure.office_uid.or(axes.office_uid.map(str::to_string)),
        legal_domain_uid: procedure.legal_domain_uid,
        applicant_counsel_names: ex::applicant_counsel_names_scanned(decision, scan)
            .unwrap_or_default(),
        applicant_law_firms: ex::applicant_law_firms_scanned(decision, scan).unwrap_or_default(),
        applicant_companies,
        defendant_counsel_names: ex::defendant_counsel_names_scanned(scan).unwrap_or_default(),
        defendant_law_firms: ex::defendant_law_firms_scanned(scan).unwrap_or_default(),
        defendant_companies,
        intervenors: ex::intervenors_scanned(scan).unwrap_or_default(),
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
    fields.search_title =
        compose_search_title(&fields, decision.jurisdiction_type.as_deref(), jur_labels);
    // Relation canonique `decision_party` (ADR 0182) : grain acteur dérivé
    // des cellules plates finales (spans-évidences par matching replié,
    // nature, resolve_key). La qualité `intervenor` est gatée en prod
    // (§7 : P < 85 % au banc) — la colonne plate reste la seule émission.
    let cells: [lj_extract::parties::Cell<'_>; 6] = [
        ("party", Some("applicant"), &fields.applicant_companies),
        ("party", Some("defendant"), &fields.defendant_companies),
        ("law_firm", Some("applicant"), &fields.applicant_law_firms),
        ("law_firm", Some("defendant"), &fields.defendant_law_firms),
        (
            "counsel_name",
            Some("applicant"),
            &fields.applicant_counsel_names,
        ),
        (
            "counsel_name",
            Some("defendant"),
            &fields.defendant_counsel_names,
        ),
    ];
    let parties = lj_extract::parties::actor_rows_folded(docx.scan.folded(), &cells)
        .into_iter()
        .map(|r| lj_store::repository::DecisionPartyRow {
            quality: r.quality.to_string(),
            side: r.side.map(str::to_string),
            value: r.value,
            resolve_key: r.resolve_key,
            nature: r.nature.map(|n| n.as_str().to_string()),
            barreau: r.barreau,
            role: r.role.map(str::to_string),
            char_starts: r.char_starts,
            char_ends: r.char_ends,
        })
        .collect();
    fields.parties = Some(parties);
    Ok(fields)
}

/// Titre canonique persistant (`search_title`, ADR 0170 ét.5) : composé à
/// l'extraction via `lj_core::titles` — la même recomposition que l'affichage.
/// Juridiction = label de la ligne catégorielle, guéri par le label
/// référentiel (`jur_labels`, code → label avec ville) quand il est nu, sinon
/// label du type ; siège recomposé depuis les axes.
pub fn compose_search_title(
    e: &ExtractedFields,
    jurisdiction_type: Option<&str>,
    jur_labels: &std::collections::HashMap<String, String>,
) -> Option<String> {
    use lj_core::titles as t;
    let jurisdiction = e
        .jurisdiction
        .as_ref()
        .map(|j| match &j.city {
            Some(_) => j.label.clone(),
            None => jur_labels.get(&j.code).cloned().unwrap_or(j.label.clone()),
        })
        .or_else(|| {
            jurisdiction_type
                .and_then(t::jurisdiction_type_label)
                .map(str::to_string)
        })?;
    let seat = t::seat_display(
        &jurisdiction,
        e.chamber_position.as_deref(),
        e.formation_uid.as_deref().and_then(t::formation_label),
        e.office_uid.as_deref().and_then(t::office_label),
    );
    let date = e.date_lecture.map(|d| d.to_string());
    Some(t::decision_title(
        &jurisdiction,
        seat.as_deref(),
        date.as_deref(),
        e.docket_numbers.first().map(String::as_str),
    ))
}

/// Variante sans `Decision` (fonds scrapés à `ExtractedFields` préconstruits,
/// ex. CNDA) : ligne `jurisdiction` par override du type, `publication:*`,
/// titre — pas de code de localisation ni de nom scanné.
pub fn with_facet_uids(mut e: ExtractedFields, jurisdiction_type: Option<&str>) -> ExtractedFields {
    e.publication_uid = Some(lj_extract::facets::publication_uid(&e.publication_codes));
    e.jurisdiction = jurisdiction_type
        .and_then(|jt| lj_extract::facets::jurisdiction_ref(jt, None, None))
        .map(|j| lj_store::repository::JurisdictionRow {
            source_code: j.code.clone(),
            code: j.code,
            jurisdiction_type: j.jurisdiction_type,
            city: j.city,
            label: j.label,
        });
    // Label toujours porté par l'override du type (CNDA…) : rien à guérir.
    e.search_title = compose_search_title(&e, jurisdiction_type, &std::collections::HashMap::new());
    e
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
    forum: Option<lj_extract::link::Forum>,
) -> Vec<LinkedOccurrence> {
    lj_extract::compiled::extract_citations(text, vocab, link, forum)
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
