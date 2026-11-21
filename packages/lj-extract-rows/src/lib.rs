//! Pont ingest → store : exécute la pile d'extraction lourde (`lj-extract`) et
//! produit les bundles que `lj-store` persiste. Toute l'extraction (champs,
//! `canonical_ref`, normalisation des citations) vit ici, côté ingest.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use lj_core::decision::Decision;
use lj_extract::link::{LinkSnapshot, LinkTarget};
use lj_store::repository::{
    CitationOccurrenceRow, ExtractedFields, NamedCaseRow, TextCaseCitationRow, TextLegalCitationRow,
};
use lj_vocab::DocumentKind;

/// `canonical_ref` (citation légale) d'une décision déjà parsée, via son
/// extracteur routé. `None` si la juridiction n'est pas routée ou si l'identité
/// est incomplète : on ne pose pas de clé bancale.
pub fn canonical_ref(decision: &Decision) -> Option<String> {
    lj_extract::extract::routed(decision).ok()?;
    lj_extract::identity::decision_canonical_ref(decision)
}

/// The body-citation detector's names, from the named decisions as the base gives them.
pub fn case_name_snapshot(rows: Vec<NamedCaseRow>) -> lj_extract::case_link::CaseNameSnapshot {
    lj_extract::case_link::CaseNameSnapshot::new(rows.into_iter().map(|r| {
        let address = r
            .jurisdiction_code
            .as_deref()
            .zip(r.docket_numbers.first())
            .map(|(code, docket)| {
                lj_core::address::CaseAddress::new(
                    code,
                    docket,
                    r.date_lecture.as_deref().unwrap_or_default(),
                )
            });
        lj_extract::case_link::NamedCase {
            keys: r.keys,
            address,
            citations: u32::try_from(r.citations).expect("a count fits u32"),
        }
    }))
}

/// What the extraction reads of the corpus AT THE MOMENT OF THE PASS, and of
/// nothing else: hydrated once per process, immutable afterwards.
pub struct Snapshots<'a> {
    pub link: &'a LinkSnapshot,
    pub vocab: &'a lj_extract::compiled::CompiledVocab,
    pub chrono: &'a lj_extract::chrono::ChronoSnapshot,
    /// Source code to canonical code.
    pub jur_codes: &'a HashMap<String, String>,
    /// See [`court_sources`].
    pub jur_sources: &'a HashMap<String, String>,
    /// Closed court to the court that succeeded it, where the seat files it.
    pub successors: &'a HashMap<String, String>,
    pub case_names: &'a lj_extract::case_link::CaseNameSnapshot,
    pub seats: &'a lj_extract::seat_link::SeatVocabulary,
    /// Source code to the court's place in the `geo:` vocabulary.
    pub places: &'a HashMap<String, String>,
}

/// The forum a decision cites from: a court of another legal order from its place, the
/// CJUE and the CEDH from their own texts.
pub fn forum(
    decision: &Decision,
    places: &HashMap<String, String>,
) -> Option<lj_extract::link::Forum> {
    let place = decision
        .jurisdiction_type
        .as_deref()
        .filter(|t| lj_vocab::courts::is_foreign(t))
        .and_then(|_| places.get(decision.jurisdiction_source_code.as_deref()?));
    lj_extract::link::Forum::of(
        decision.jurisdiction_type.as_deref(),
        decision.ecli.as_deref(),
        place.map(String::as_str),
        decision.date_lecture.as_deref(),
    )
}

/// Champs structurés extraits d'une décision, prêts à persister.
///
/// Un [`lj_extract::compiled::doc_extract`] par décision : une seule passe
/// automate partagée entre les composeurs de champs (`DocScan`) et les citations
/// compilées.
pub fn extracted_fields(decision: &Decision, snap: &Snapshots<'_>) -> Result<ExtractedFields> {
    let Snapshots {
        link,
        vocab,
        chrono,
        jur_codes,
        jur_sources,
        successors,
        case_names,
        seats,
        places,
    } = *snap;
    use lj_extract::extract as ex;
    // `routed` knows the French and EU courts; a foreign court's own types
    // (`COUR_SUPREME`, `CCJA`…) go through the generic extractors.
    if ![
        "juricaf/",
        "entscheidsuche/",
        "legimonaco/",
        "juportal/",
        "rechtspraak/",
        "justice-lu/",
        "scc-csc/",
        "cce/",
        "cc-bj/",
        "cc-ci/",
        "cc-dj/",
        "cc-ma/",
        "hcc-mg/",
        "jurisprudence-mg/",
        "african-court/",
        "ccja/",
        "cassation-ci/",
        "cndj-ci/",
        "journalofficiel-dj/",
        "opencaselaw/",
        "a2aj/",
    ]
    .iter()
    .any(|p| decision.source_uid.starts_with(p))
    {
        ex::routed(decision).map_err(|e| anyhow!("extract: {e}"))?;
    }

    fn parse_date(s: Option<String>) -> Option<NaiveDate> {
        let s = s?;
        if s.is_empty() {
            return None;
        }
        NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()
    }

    let forum = forum(decision, places);
    // Pas de texte porteur : une décision cite, elle ne revendique pas ses
    // propres numéraux nus.
    let docx = lj_extract::compiled::doc_extract(
        &decision.texte_integral_clean,
        vocab,
        link,
        chrono,
        forum,
        None,
    );
    let scan = Some(&docx.scan);

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
    // Juridiction catégorielle : la ligne référentielle directe.
    let jurisdiction = ex::jurisdiction_scanned(decision, scan, &docket_numbers).map(|j| {
        lj_store::repository::JurisdictionRow {
            source_code: court_source(&j.code, jur_codes, jur_sources),
            code: j.code,
            jurisdiction_type: j.jurisdiction_type,
            city: j.city,
            label: j.label,
            place: None,
        }
    });
    // Closed TA/CAA nomenclatures: a registry name outside the list never
    // fabricates a ghost code. The decision is ingested without a juridiction
    // facet, and that is reported.
    if jurisdiction.is_none()
        && matches!(
            decision.jurisdiction_type.as_deref(),
            Some("TA") | Some("CAA")
        )
    {
        tracing::warn!(
            source_uid = %decision.source_uid,
            nom = decision.jurisdiction_name.as_deref().unwrap_or(""),
            "admin juridiction outside the nomenclature: decision without a juridiction facet"
        );
    }
    let own_court = jurisdiction
        .as_ref()
        .map(|j| canonical_code(j, jur_codes))
        .unwrap_or_default();
    let own_date = ex::extract_date_lecture(decision).unwrap_or_default();
    let soi: Vec<lj_core::address::CaseAddress> = docket_numbers
        .iter()
        .map(|n| lj_core::address::CaseAddress::new(&own_court, n, &own_date))
        .collect();
    let numerotees = merge_bridged_spans(
        case_citation_rows(&docx.cases, &docket_numbers),
        lj_extract::chrono::attacked_text_spans(decision, chrono),
    );
    let nommees = drop_named_next_to_ref(
        lj_extract::case_link::cited_case_name_spans(
            &decision.texte_integral_clean,
            case_names,
            &soi,
            lj_extract::case_link::Citing {
                european: match decision.jurisdiction_type.as_deref() {
                    Some("CJUE") => Some("cjue"),
                    Some("CEDH") => Some("cedh"),
                    _ => None,
                },
                date: decision.date_lecture.as_deref(),
            },
        ),
        &numerotees,
        &decision.texte_integral_clean,
    );
    let case_citations = Some(merge_bridged_spans(numerotees, nommees));

    let mut procedure = ex::procedure_scanned(decision, scan);
    // Vote domaine par le profil des codes cités (`lj_extract::domain`),
    // seulement quand le chemin signaux n'a rien rendu.
    if procedure.legal_domain_uid.is_none() {
        let ctx = lj_extract::domain::DomainContext {
            admin: !lj_extract::extract::is_judiciaire(decision),
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
    // Raffinement par votes de termes du scan : ne touche qu'un domaine nul ou
    // parent nu — les sous-domaines posés par les codes cités priment.
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
    let seat = seat_of(
        &axes,
        decision,
        jurisdiction.as_ref(),
        jur_codes,
        successors,
    );
    let kind = document_kind(decision);
    let mut fields = ExtractedFields {
        jurisdiction,
        date_lecture: parse_date(ex::extract_date_lecture(decision)),
        date_audience: parse_date(ex::date_audience_scanned(decision, scan)),
        docket_numbers,
        seat_paths: seat
            .as_ref()
            .map(|s| {
                seats
                    .resolve(&lj_extract::seat_link::SeatRow {
                        seat: s.clone(),
                        chamber: decision.chamber.clone(),
                        formation: decision.formation.clone(),
                        span: None,
                    })
                    .paths()
            })
            .unwrap_or_default(),
        seat,
        formation_uid: axes.formation_uid.map(str::to_string),
        publication_codes: decision.publication_codes.clone(),
        solution_uid: (kind == DocumentKind::Decision)
            .then(|| ex::solution_scanned(decision, scan))
            .flatten(),
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
            lj_extract::chrono::prior_decision_refs(decision, chrono, &docx.scan)
                .into_iter()
                .map(|r| lj_store::repository::DecisionLinkRow {
                    link_type: r.link_type.as_str().to_string(),
                    address: r.address,
                })
                .collect(),
        ),
        case_citations,
        // The CELLAR title, read by `prebuilt_from_source_fields`, comes first;
        // the EUR-Lex title line names the parties of the texts it lacks.
        case_name: decision
            .source_uid
            .starts_with("cjue/")
            .then(|| lj_parse::cjue_case_name(&decision.texte_integral_clean))
            .flatten(),
        kind,
        ..Default::default()
    };
    // The actors, derived from the final flat cells (mentions, nature,
    // resolve_key). The `intervenor` quality emits no actor.
    use lj_extract::parties::Cell;
    // Les parties personnes physiques ne sont pas des colonnes de `decisions` :
    // elles n'existent que dans la relation, où la nature les distingue.
    let app_indiv = ex::applicant_individuals_scanned(scan).unwrap_or_default();
    let def_indiv = ex::defendant_individuals_scanned(scan).unwrap_or_default();
    let (listed_names, listed_firms) = ex::listed_counsel_scanned(
        decision,
        scan,
        &[
            fields.applicant_counsel_names.as_slice(),
            &fields.defendant_counsel_names,
        ]
        .concat(),
        &[
            fields.applicant_law_firms.as_slice(),
            &fields.defendant_law_firms,
        ]
        .concat(),
    );
    let cells: [Cell<'_>; 10] = [
        Cell::new("party", Some("applicant"), &fields.applicant_companies),
        Cell::new("party", Some("defendant"), &fields.defendant_companies),
        Cell::physique("party", Some("applicant"), &app_indiv),
        Cell::physique("party", Some("defendant"), &def_indiv),
        Cell::new("law_firm", Some("applicant"), &fields.applicant_law_firms),
        Cell::new("law_firm", Some("defendant"), &fields.defendant_law_firms),
        Cell::new(
            "counsel_name",
            Some("applicant"),
            &fields.applicant_counsel_names,
        ),
        Cell::new(
            "counsel_name",
            Some("defendant"),
            &fields.defendant_counsel_names,
        ),
        Cell::new("law_firm", None, &listed_firms),
        Cell::new("counsel_name", None, &listed_names),
    ];
    let parties = lj_extract::parties::actor_rows_scanned(&docx.scan, &cells)
        .into_iter()
        .map(|r| lj_store::repository::DecisionPartyRow {
            quality: r.quality.to_string(),
            side: r.side.map(str::to_string),
            value: r.value,
            resolve_key: r.resolve_key,
            nature: r.nature.map(|n| n.as_str().to_string()),
            barreau: r.barreau,
            role: r.role.map(str::to_string),
            firm_ord: r.firm_ord,
            party_ord: r.party_ord,
            sub_ord: r.sub_ord,
            mentions: r.mentions,
        })
        .collect();
    fields.parties = Some(parties);
    Ok(fields)
}

/// The seat the decision sat in, positioned on its court. The court code is the
/// CANONICAL one, the resolution `ensure_jurisdictions` runs at write time: the
/// paths are derived here, before it. A closed court sits under its successor.
fn seat_of(
    axes: &lj_extract::formation::FormationAxes,
    decision: &Decision,
    jurisdiction: Option<&lj_store::repository::JurisdictionRow>,
    jur_codes: &HashMap<String, String>,
    successors: &HashMap<String, String>,
) -> Option<lj_core::chamber::Seat> {
    let jurisdiction_type = decision.jurisdiction_type.clone()?;
    let jurisdiction_code = jurisdiction.map(|j| {
        let code = canonical_code(j, jur_codes);
        successors.get(&code).cloned().unwrap_or(code)
    });
    // A foreign court no scanner names comes with its seat prebuilt by its parser.
    if jurisdiction_code.is_none() && lj_vocab::courts::is_foreign(&jurisdiction_type) {
        return None;
    }
    let mut seat = axes.chamber.clone();
    seat.jurisdiction_type = jurisdiction_type;
    seat.jurisdiction_code = jurisdiction_code;
    Some(seat)
}

/// A court's source code by its code, and by the code its place gives when the
/// referential keeps it under a source number: `cph101132`, at Strasbourg, also
/// answers to `cph_strasbourg`, the code a parser builds from its name.
pub fn court_sources(rows: &[lj_store::repository::JurisdictionRow]) -> HashMap<String, String> {
    let mut sources: HashMap<String, String> = rows
        .iter()
        .map(|j| (j.code.clone(), j.source_code.clone()))
        .collect();
    for j in rows {
        let numbered = j.code == j.source_code
            && j.code
                .trim_start_matches(|c: char| c.is_ascii_lowercase())
                .parse::<u64>()
                .is_ok();
        if let (true, Some(city)) = (numbered, j.city.as_deref()) {
            sources
                .entry(lj_store::repository::canonical_jurisdiction_code(
                    &j.jurisdiction_type,
                    Some(city),
                    &j.code,
                ))
                .or_insert_with(|| j.code.clone());
        }
    }
    sources
}

/// The source code a scanned court is written under: its own when the
/// referential holds it, else the one [`court_sources`] gives its code.
fn court_source(
    code: &str,
    jur_codes: &HashMap<String, String>,
    jur_sources: &HashMap<String, String>,
) -> String {
    match jur_sources.get(code) {
        Some(source) if !jur_codes.contains_key(code) => source.clone(),
        _ => code.to_string(),
    }
}

/// The canonical code of the court a parser names, the one [`extracted_fields`]
/// writes for it.
pub fn court_code(
    j: &lj_extract::facets::JurisdictionRef,
    jur_codes: &HashMap<String, String>,
    jur_sources: &HashMap<String, String>,
) -> String {
    let source = court_source(&j.code, jur_codes, jur_sources);
    jur_codes.get(&source).cloned().unwrap_or_else(|| {
        lj_store::repository::canonical_jurisdiction_code(
            &j.jurisdiction_type,
            j.city.as_deref(),
            &source,
        )
    })
}

/// The canonical code of a scanned court, the one `ensure_jurisdictions` will
/// write and the one every address joins on.
fn canonical_code(
    j: &lj_store::repository::JurisdictionRow,
    jur_codes: &HashMap<String, String>,
) -> String {
    jur_codes.get(&j.source_code).cloned().unwrap_or_else(|| {
        lj_store::repository::canonical_jurisdiction_code(
            &j.jurisdiction_type,
            j.city.as_deref(),
            &j.source_code,
        )
    })
}

/// Persistent canonical title (`search_title`), composed through `lj_core::titles`,
/// the same recomposition the display runs. Jurisdiction is the categorial label,
/// healed by the referential one (`jur_labels`, code to label with city) when bare,
/// otherwise the type label. The seat is NOT in it: it reads from the stored seat,
/// and the indexed title equals the displayed one.
///
/// `case_name` is taken rather than read off `e`: the extraction only knows the name
/// its source carries, while the corpus also holds names elected downstream, and
/// composing without them silently drops the name from the indexed title.
///
/// Call it through [`with_search_title`], the writer of the field.
pub fn compose_search_title(
    e: &ExtractedFields,
    jurisdiction_type: Option<&str>,
    jur_labels: &std::collections::HashMap<String, String>,
    case_name: Option<&str>,
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
    let date = e.date_lecture.map(|d| d.to_string());
    if e.kind == DocumentKind::Conclusions {
        return Some(t::conclusions_title(
            e.author.as_deref(),
            date.as_deref(),
            case_name,
            t::docket_display(&e.docket_numbers).as_deref(),
        ));
    }
    Some(t::decision_title(
        &jurisdiction,
        None,
        date.as_deref(),
        case_name,
        t::docket_display(&e.docket_numbers).as_deref(),
    ))
}

/// Variante sans `Decision` (fonds scrapés à `ExtractedFields` préconstruits,
/// ex. CNDA) : ligne `jurisdiction` par override du type et titre, pas de code
/// de localisation ni de nom scanné.
pub fn with_facet_uids(mut e: ExtractedFields, jurisdiction_type: Option<&str>) -> ExtractedFields {
    e.jurisdiction = jurisdiction_type
        .and_then(|jt| lj_extract::facets::jurisdiction_ref(jt, None, None))
        .map(|j| lj_store::repository::JurisdictionRow {
            source_code: j.code.clone(),
            code: j.code,
            jurisdiction_type: j.jurisdiction_type,
            city: j.city,
            label: j.label,
            place: None,
        });
    // Label toujours porté par l'override du type (CNDA…) : rien à guérir.
    with_search_title(
        e,
        jurisdiction_type,
        &std::collections::HashMap::new(),
        None,
    )
}

/// Writes `search_title` from the final state of `e`, and the only place that
/// does: the title is a function of jurisdiction, date, name and dockets, so
/// composing it before the prebuilt layer has spoken leaves the CEDH column
/// naming nobody, `merge_prebuilt` then keeping the two out of step. `elected`
/// overrides the extracted name, the corpus holding names no source carries.
pub fn with_search_title(
    mut e: ExtractedFields,
    jurisdiction_type: Option<&str>,
    jur_labels: &std::collections::HashMap<String, String>,
    elected: Option<&str>,
) -> ExtractedFields {
    let name = elected.map(str::to_string).or_else(|| e.case_name.clone());
    e.search_title = compose_search_title(&e, jurisdiction_type, jur_labels, name.as_deref());
    e
}

/// What `decision` is: an Advocate General's opinion is conclusions, which decide nothing.
fn document_kind(decision: &Decision) -> DocumentKind {
    if decision.jurisdiction_type.as_deref() == Some("CJUE")
        && lj_parse::celex_is_opinion(&decision.member_name)
    {
        DocumentKind::Conclusions
    } else {
        DocumentKind::Decision
    }
}

/// A provenance's fields read again from `(full_text, source_fields)`, as its ingest wrote them.
pub fn reread_fields(
    decision: &Decision,
    source_fields: &serde_json::Value,
    snap: &Snapshots<'_>,
) -> Result<ExtractedFields> {
    let mut fields = extracted_fields(decision, snap)?;
    if let Some(pre) = prebuilt_from_source_fields(&decision.source_uid, source_fields) {
        fields = merge_prebuilt(pre, fields);
    }
    if let Some(court) = foreign_court(decision)? {
        fields = merge_prebuilt(court, fields);
    }
    Ok(fields)
}

/// The court a foreign parser named, seated with no chamber, as its ingest writes it.
fn foreign_court(decision: &Decision) -> Result<Option<ExtractedFields>> {
    let Some(jurisdiction_type) = decision
        .jurisdiction_type
        .as_deref()
        .filter(|t| lj_vocab::courts::is_foreign(t))
    else {
        return Ok(None);
    };
    let (Some(code), Some(label)) = (
        &decision.jurisdiction_source_code,
        &decision.jurisdiction_name,
    ) else {
        anyhow::bail!(
            "{}: a foreign court its parser did not name",
            decision.source_uid
        );
    };
    let seat = lj_core::chamber::Seat {
        jurisdiction_type: jurisdiction_type.to_string(),
        jurisdiction_code: Some(code.clone()),
        ..Default::default()
    };
    Ok(Some(ExtractedFields {
        seat_paths: seat.paths(),
        seat: Some(seat),
        jurisdiction: Some(lj_store::repository::JurisdictionRow {
            code: code.clone(),
            source_code: code.clone(),
            jurisdiction_type: jurisdiction_type.to_string(),
            city: None,
            label: label.clone(),
            place: None,
        }),
        ..Default::default()
    }))
}

/// Couche préconstruite reconstruite depuis `(source_uid, source_fields)` — le
/// pendant, en ré-extraction, de ce que le parser source pose à l'ingest via
/// `prebuilt_extracted`.
///
/// La règle : un champ préconstruit survit à la décomposition `source_payload
/// ⟷ (full_text, source_fields)` s'il tient dans `Decision` ou dans
/// `source_fields`. ArianeWeb passe par le second (il écrit un payload
/// Judilibre minimal, relu par `from_source_fields_json`). CNDA a un champ qui
/// ne tenait dans ni l'un ni l'autre — `solution_uid`, lu du dispositif par une
/// connaissance que l'extracteur générique n'a pas ; il vit en `source_fields`,
/// et se relit ici. La CEDH est dans le même cas : le nom de l'affaire est dans
/// le `docname` HUDOC, que le texte ne porte pas.
/// So is the CJUE's, in the CELLAR title its listing carries.
pub fn prebuilt_from_source_fields(
    source_uid: &str,
    source_fields: &serde_json::Value,
) -> Option<ExtractedFields> {
    if source_uid.starts_with("cedh/") {
        let docname = source_fields.get("docname").and_then(|v| v.as_str())?;
        return Some(ExtractedFields {
            case_name: Some(lj_parse::cedh_case_name(docname)?),
            ..Default::default()
        });
    }
    if source_uid.starts_with("cjue/") {
        let title = source_fields
            .get("expression_title")
            .and_then(|v| v.as_str())?;
        return Some(ExtractedFields {
            case_name: lj_parse::cjue_title_case_name(title),
            author: lj_parse::cjue_advocate_general(title)
                .map(|(role, name)| format!("{role} {name}")),
            ..Default::default()
        });
    }
    if source_uid.starts_with("comeu/") {
        let title = lj_parse::comeu_first(source_fields, "caseTitle")?.trim();
        return Some(ExtractedFields {
            case_name: Some(title.to_string()).filter(|t| !t.is_empty()),
            ..Default::default()
        });
    }
    if source_uid.starts_with("scc-csc/") {
        let title = source_fields.get("title").and_then(|v| v.as_str())?.trim();
        return Some(ExtractedFields {
            case_name: Some(title.to_string()).filter(|t| !t.is_empty()),
            ..Default::default()
        });
    }
    let solution_uid = source_uid
        .starts_with("cnda/")
        .then(|| source_fields.get("solution_uid").and_then(|v| v.as_str()))
        .flatten()?;
    Some(ExtractedFields {
        solution_uid: Some(solution_uid.to_string()),
        ..Default::default()
    })
}

/// Fusionne les champs préconstruits par un parser source (CNDA, ArianeWeb :
/// connaissance de grain source — solution lue du dispositif, dates ISO de la
/// fiche) sur une extraction complète : le préconstruit prime là où il a parlé
/// (`Option` posée, `Vec` non vide), l'extraction fournit tout le reste
/// (citations, domaine, instruments, parties…).
pub fn merge_prebuilt(pre: ExtractedFields, full: ExtractedFields) -> ExtractedFields {
    fn vec_or(pre: Vec<String>, full: Vec<String>) -> Vec<String> {
        if pre.is_empty() {
            full
        } else {
            pre
        }
    }
    ExtractedFields {
        date_lecture: pre.date_lecture.or(full.date_lecture),
        date_audience: pre.date_audience.or(full.date_audience),
        docket_numbers: vec_or(pre.docket_numbers, full.docket_numbers),
        seat: pre.seat.or(full.seat),
        seat_paths: vec_or(pre.seat_paths, full.seat_paths),
        formation_uid: pre.formation_uid.or(full.formation_uid),
        case_name: pre.case_name.or(full.case_name),
        // Derived from `case_name` above, so merged independently it goes out of
        // step with it. `with_search_title` recomposes it from the result.
        search_title: None,
        publication_codes: vec_or(pre.publication_codes, full.publication_codes),
        solution_uid: pre.solution_uid.or(full.solution_uid),
        procedure_uid: pre.procedure_uid.or(full.procedure_uid),
        office_uid: pre.office_uid.or(full.office_uid),
        legal_domain_uid: pre.legal_domain_uid.or(full.legal_domain_uid),
        applicant_counsel_names: vec_or(pre.applicant_counsel_names, full.applicant_counsel_names),
        applicant_law_firms: vec_or(pre.applicant_law_firms, full.applicant_law_firms),
        applicant_companies: vec_or(pre.applicant_companies, full.applicant_companies),
        defendant_counsel_names: vec_or(pre.defendant_counsel_names, full.defendant_counsel_names),
        defendant_law_firms: vec_or(pre.defendant_law_firms, full.defendant_law_firms),
        defendant_companies: vec_or(pre.defendant_companies, full.defendant_companies),
        intervenors: vec_or(pre.intervenors, full.intervenors),
        themes: vec_or(pre.themes, full.themes),
        jurisdiction: pre.jurisdiction.or(full.jurisdiction),
        citation_occurrences: pre.citation_occurrences.or(full.citation_occurrences),
        decision_links: pre.decision_links.or(full.decision_links),
        case_citations: pre.case_citations.or(full.case_citations),
        parties: pre.parties.or(full.parties),
        kind: full.kind,
        author: pre.author.or(full.author),
    }
}

/// One decision's fields from all its active provenances, the authority's
/// first: a single value is the authority's, else the next provenance's that
/// carries one; a list is the union of all, duplicates folded. The layers read
/// in the served text stay the authority's.
pub fn fuse_provenances(
    authority: ExtractedFields,
    others: Vec<ExtractedFields>,
) -> ExtractedFields {
    others.into_iter().fold(authority, |a, b| ExtractedFields {
        date_lecture: a.date_lecture.or(b.date_lecture),
        date_audience: a.date_audience.or(b.date_audience),
        docket_numbers: union(a.docket_numbers, b.docket_numbers, |n| {
            lj_core::docket_key::docket_key(n)
        }),
        formation_uid: a.formation_uid.or(b.formation_uid),
        seat: a.seat.or(b.seat),
        seat_paths: first_filled(a.seat_paths, b.seat_paths),
        case_name: a.case_name.or(b.case_name),
        search_title: None,
        publication_codes: first_filled(a.publication_codes, b.publication_codes),
        solution_uid: a.solution_uid.or(b.solution_uid),
        procedure_uid: a.procedure_uid.or(b.procedure_uid),
        office_uid: a.office_uid.or(b.office_uid),
        legal_domain_uid: a.legal_domain_uid.or(b.legal_domain_uid),
        applicant_counsel_names: union(
            a.applicant_counsel_names,
            b.applicant_counsel_names,
            fold_key,
        ),
        applicant_law_firms: union(a.applicant_law_firms, b.applicant_law_firms, fold_key),
        applicant_companies: union(a.applicant_companies, b.applicant_companies, fold_key),
        defendant_counsel_names: union(
            a.defendant_counsel_names,
            b.defendant_counsel_names,
            fold_key,
        ),
        defendant_law_firms: union(a.defendant_law_firms, b.defendant_law_firms, fold_key),
        defendant_companies: union(a.defendant_companies, b.defendant_companies, fold_key),
        intervenors: union(a.intervenors, b.intervenors, fold_key),
        themes: union(a.themes, b.themes, fold_key),
        jurisdiction: a.jurisdiction.or(b.jurisdiction),
        citation_occurrences: a.citation_occurrences.or(b.citation_occurrences),
        decision_links: match (a.decision_links, b.decision_links) {
            (Some(mut links), Some(more)) => {
                for l in more {
                    if !links
                        .iter()
                        .any(|k| k.link_type == l.link_type && k.address.same_target_as(&l.address))
                    {
                        links.push(l);
                    }
                }
                Some(links)
            }
            (a, b) => a.or(b),
        },
        case_citations: a.case_citations.or(b.case_citations),
        parties: match (a.parties, b.parties) {
            (Some(rows), Some(more)) => Some(union_parties(rows, more)),
            (a, b) => a.or(b),
        },
        kind: a.kind,
        author: a.author.or(b.author),
    })
}

fn fold_key(s: &str) -> String {
    lj_fold::slug(s, ' ')
}

fn first_filled(a: Vec<String>, b: Vec<String>) -> Vec<String> {
    if a.is_empty() {
        b
    } else {
        a
    }
}

fn union(mut a: Vec<String>, b: Vec<String>, key: impl Fn(&str) -> String) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = a.iter().map(|v| key(v)).collect();
    for v in b {
        if seen.insert(key(&v)) {
            a.push(v);
        }
    }
    a
}

/// The rows of `more` the first set lacks, appended, their `ord` references
/// carried over to where the rows they point at now stand.
fn union_parties(
    mut rows: Vec<lj_store::repository::DecisionPartyRow>,
    more: Vec<lj_store::repository::DecisionPartyRow>,
) -> Vec<lj_store::repository::DecisionPartyRow> {
    let index: HashMap<(String, String), usize> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| ((r.quality.clone(), r.resolve_key.clone()), i))
        .collect();
    let mut moved: Vec<usize> = Vec::with_capacity(more.len());
    let mut appended = Vec::new();
    for r in more {
        match index.get(&(r.quality.clone(), r.resolve_key.clone())) {
            Some(&i) => moved.push(i),
            None => {
                moved.push(rows.len() + appended.len());
                appended.push(r);
            }
        }
    }
    let remap = |ord: Option<i32>| ord.and_then(|o| moved.get(o as usize)).map(|&i| i as i32);
    for mut r in appended {
        r.firm_ord = remap(r.firm_ord);
        r.party_ord = remap(r.party_ord);
        r.sub_ord = remap(r.sub_ord);
        // Their mentions point into another provenance's text.
        r.mentions.clear();
        rows.push(r);
    }
    rows
}

/// Une occurrence de citation enrichie de ses clés canoniques et de la cible
/// posée par le linker in-pass — l'artefact partagé ingest ↔ banc : l'ingest le
/// projette en [`CitationOccurrenceRow`] (span + cible), le banc le score tel
/// quel. Même normalisation, même linker.
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

/// Projette les citations de jurisprudence en lignes à persister, propre
/// en-tête exclu : un span dont le numéro (chiffres seuls) est un docket de la
/// décision est sa propre référence (« RG n° 21/04532 » du bandeau CA,
/// « Pourvoi n° … » Cassation), pas une citation. Partagé par le bridge ingest
/// et le banc.
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
                .address
                .docket
                .as_deref()
                .unwrap_or_default()
                .chars()
                .filter(char::is_ascii_digit)
                .collect();
            !own_digits.contains(&digits)
        })
        .map(|c| lj_store::repository::CaseCitationRow {
            char_start: c.char_start as i32,
            char_end: c.char_end as i32,
            address: c.address.clone(),
        })
        .collect()
}

/// Fusionne les spans qu'un pont apporte là où le texte ne porte pas de numéro :
/// la décision attaquée citée inline (« l'arrêt attaqué (Paris, 22 août 2024) »)
/// et le nom d'usage (« la jurisprudence Danthony »). Les spans déjà présents
/// priment en cas de chevauchement.
/// Codepoints between the end of a name and the number that follows it: « arrêt
/// Pellegrin c. France [GC], n° 28541/95 » is one citation, not two. It clears
/// « décision Czabaj (Assemblée du contentieux, 13 juillet 2016, no 387763) ».
const NAMED_REF_WINDOW: i32 = 60;

/// A usage name followed by its reference yields a single line, carried by the
/// number, the only identifying token. Sentence-ending punctuation in between
/// separates two citations.
pub fn drop_named_next_to_ref(
    named: Vec<(usize, usize, lj_core::address::CaseAddress)>,
    case_rows: &[lj_store::repository::CaseCitationRow],
    text: &str,
) -> Vec<(usize, usize, lj_core::address::CaseAddress)> {
    named
        .into_iter()
        .filter(|(_, fin, _)| {
            let fin = *fin as i32;
            !case_rows.iter().any(|c| {
                c.char_start >= fin
                    && c.char_start - fin <= NAMED_REF_WINDOW
                    && !text
                        .chars()
                        .skip(fin as usize)
                        .take((c.char_start - fin) as usize)
                        .any(|ch| ch == '.' || ch == ';')
            })
        })
        .collect()
}

pub fn merge_bridged_spans(
    mut case_rows: Vec<lj_store::repository::CaseCitationRow>,
    attacked: Vec<(usize, usize, lj_core::address::CaseAddress)>,
) -> Vec<lj_store::repository::CaseCitationRow> {
    for (s, e, address) in attacked {
        let (s, e) = (s as i32, e as i32);
        if case_rows.iter().any(|c| c.char_start < e && s < c.char_end) {
            continue;
        }
        case_rows.push(lj_store::repository::CaseCitationRow {
            char_start: s,
            char_end: e,
            address,
        });
    }
    case_rows.sort_by_key(|c| c.char_start);
    case_rows
}

/// Occurrences liées du moteur compilé : scan automate unique + composition
/// par-document, lien par le snapshot catalogue du run.
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

/// Spans extraits d'UN corps émetteur, prêts à écrire.
pub struct OwnerRows {
    pub cases: Vec<TextCaseCitationRow>,
    pub cites: Vec<TextLegalCitationRow>,
}

/// Oracle DILA : `(propriétaire, article, numéro cité en clé publique)` →
/// texte déclaré cible. `None` = le même numéro est déclaré vers plusieurs textes
/// depuis cet article, l'oracle s'abstient.
pub type Declared = HashMap<(String, String, String), Option<String>>;

/// Plie les arêtes déclarées en oracle. Les propriétaires au grain texte
/// (corps monolithique) portent `owner_num_key = ''`, comme `legal_link`.
pub fn build_declared(rows: Vec<(String, String, String, String)>) -> Declared {
    let mut oracle: Declared = HashMap::with_capacity(rows.len());
    for (owner_text_uid, owner_num_key, target_num_key, target_text_uid) in rows {
        let key = (
            owner_text_uid,
            owner_num_key,
            lj_norm::article_key::article_key(&target_num_key),
        );
        match oracle.entry(key) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(Some(target_text_uid));
            }
            std::collections::hash_map::Entry::Occupied(mut e) => {
                if e.get().as_deref() != Some(target_text_uid.as_str()) {
                    e.insert(None);
                }
            }
        }
    }
    oracle
}

/// Texte que la source DÉCLARE cible pour ce numéro cité depuis cet article.
/// `None` si elle ne déclare rien, ou plusieurs textes pour le même numéro.
fn declared_target<'a>(
    oracle: &'a Declared,
    owner_text_uid: &str,
    owner_num_key: Option<&str>,
    article_key: &str,
) -> Option<&'a str> {
    oracle
        .get(&(
            owner_text_uid.to_string(),
            owner_num_key.unwrap_or_default().to_string(),
            article_key.to_string(),
        ))?
        .as_deref()
}

/// Les deux flux d'un corps émetteur du référentiel (offsets codepoints sur le
/// texte émetteur). Renvois : seulement les cibles résolues, jamais l'auto-renvoi
/// (cible = l'émetteur lui-même).
#[allow(clippy::too_many_arguments)]
pub fn owner_rows(
    texte: &str,
    owner_text_uid: &str,
    owner_num_key: Option<String>,
    owner_date_debut: Option<NaiveDate>,
    link: &LinkSnapshot,
    vocab: &lj_extract::compiled::CompiledVocab,
    chrono: &lj_extract::chrono::ChronoSnapshot,
    declared: &Declared,
) -> OwnerRows {
    let docx =
        lj_extract::compiled::doc_extract(texte, vocab, link, chrono, None, Some(owner_text_uid));
    let cases = docx
        .cases
        .into_iter()
        .map(|c| TextCaseCitationRow {
            owner_num_key: owner_num_key.clone(),
            owner_date_debut,
            char_start: c.char_start as i32,
            char_end: c.char_end as i32,
            address: c.address,
        })
        .collect();
    let occurrences = arbitrate(
        docx.citations,
        owner_text_uid,
        owner_num_key.as_deref(),
        link,
        declared,
    );
    let cites = occurrences
        .into_iter()
        .filter_map(|o| {
            Some(TextLegalCitationRow {
                owner_num_key: owner_num_key.clone(),
                owner_date_debut,
                char_start: o.char_start as i32,
                char_end: o.char_end as i32,
                ref_text_uid: o.target.ref_text_uid?,
                ref_num_key: o.target.ref_num_key,
            })
        })
        .collect();
    OwnerRows { cases, cites }
}

/// Occurrences d'un corps émetteur du référentiel, cible arbitrée mais rien de
/// jeté : le banc a besoin de voir aussi ce qui n'a pas été lié, pour scorer le
/// rappel et l'abstention. [`owner_rows`] n'en garde ensuite que les cibles
/// résolues.
#[allow(clippy::too_many_arguments)]
pub fn owner_occurrences(
    texte: &str,
    owner_text_uid: &str,
    owner_num_key: Option<&str>,
    link: &LinkSnapshot,
    vocab: &lj_extract::compiled::CompiledVocab,
    chrono: &lj_extract::chrono::ChronoSnapshot,
    declared: &Declared,
) -> Vec<LinkedOccurrence> {
    let docx =
        lj_extract::compiled::doc_extract(texte, vocab, link, chrono, None, Some(owner_text_uid));
    arbitrate(
        docx.citations,
        owner_text_uid,
        owner_num_key,
        link,
        declared,
    )
}

/// Tranche la cible de chaque occurrence d'un corps du référentiel : oracle de
/// la source, puis cible de l'extracteur, puis repli d'auto-renvoi. Une cible
/// écartée devient une occurrence sans cible, jamais une occurrence perdue.
fn arbitrate(
    citations: Vec<lj_extract::compiled::CompiledCitation>,
    owner_text_uid: &str,
    owner_num_key: Option<&str>,
    link: &LinkSnapshot,
    declared: &Declared,
) -> Vec<LinkedOccurrence> {
    citations
        .into_iter()
        .map(|c| {
            // Oracle DILA : pour un numéro qu'elle déclare citer depuis cet article, la
            // source tranche le texte — elle sait de quel décret du même jour elle parle,
            // et si le numéro nu est un renvoi interne.
            let oracle = c
                .article_key
                .as_deref()
                .and_then(|ak| declared_target(declared, owner_text_uid, owner_num_key, ak))
                .map(|uid| {
                    let num = link.num_key_for(uid, c.article_key.as_deref());
                    (uid.to_string(), num)
                });
            let target = match (oracle, c.target.ref_text_uid.clone()) {
                (Some((uid, num)), _) => Some((uid, num)),
                (None, Some(uid)) => Some((uid, c.target.ref_num_key.clone())),
                // Article nu dans un corps du référentiel (« en application de
                // l'article 21-2 » dans le code civil) : la cible est le texte émetteur
                // lui-même quand il possède l'article — renvoi interne, la convention
                // rédactionnelle des codes. L'orphelin génitif (« du présent code », « de la
                // loi susvisée ») n'est pas nu : il reste écarté.
                (None, None) => c
                    .article_key
                    .as_deref()
                    .filter(|_| c.bare)
                    .filter(|ak| link.has_article(owner_text_uid, ak))
                    .map(|ak| {
                        (
                            owner_text_uid.to_string(),
                            link.num_key_for(owner_text_uid, Some(ak)),
                        )
                    }),
            };
            // L'auto-renvoi à soi-même (même texte, même article) n'est pas un lien : le
            // lecteur y est déjà.
            let target = target
                .filter(|(uid, num)| uid != owner_text_uid || num.as_deref() != owner_num_key);
            LinkedOccurrence {
                char_start: c.char_start,
                char_end: c.char_end,
                instrument: c.instrument,
                article: c.article,
                text_key: c.text_key,
                article_key: c.article_key,
                target: LinkTarget {
                    ref_text_uid: target.as_ref().map(|(uid, _)| uid.clone()),
                    ref_num_key: target.and_then(|(_, num)| num),
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_court_a_parser_names_is_the_one_the_referential_holds() {
        let court = |code: &str, source: &str, kind: &str, city: Option<&str>| {
            lj_store::repository::JurisdictionRow {
                code: code.into(),
                source_code: source.into(),
                jurisdiction_type: kind.into(),
                city: city.map(Into::into),
                label: String::new(),
                place: None,
            }
        };
        let rows = [
            court(
                "tj_la_roche_sur_yon",
                "tj85191",
                "TJ",
                Some("La Roche-sur-Yon"),
            ),
            court("cph101132", "cph101132", "CPH", Some("Strasbourg")),
            court("cph101213", "cph101213", "CPH", None),
            court("tj_strasbourg", "tj67482", "TJ", Some("Strasbourg")),
            court(
                "tass_strasbourg",
                "tass_strasbourg",
                "TJ",
                Some("Strasbourg"),
            ),
        ];
        let jur_codes: HashMap<String, String> = rows
            .iter()
            .map(|j| (j.source_code.clone(), j.code.clone()))
            .collect();
        let jur_sources = court_sources(&rows);
        let code = |code: &str, kind: &str, city: Option<&str>| {
            let j = lj_extract::facets::JurisdictionRef {
                code: code.into(),
                jurisdiction_type: kind.into(),
                city: city.map(Into::into),
                label: String::new(),
            };
            court_code(&j, &jur_codes, &jur_sources)
        };
        assert_eq!(
            code("tj_la_roche_sur_yon", "TJ", Some("Roche sur Yon")),
            "tj_la_roche_sur_yon"
        );
        assert_eq!(
            code("tj85191", "TJ", Some("Roche sur Yon")),
            "tj_la_roche_sur_yon"
        );
        assert_eq!(
            code("cph_strasbourg", "CPH", Some("Strasbourg")),
            "cph101132"
        );
        assert_eq!(
            code("tj_strasbourg", "TJ", Some("Strasbourg")),
            "tj_strasbourg"
        );
        assert_eq!(code("tass_strasbourg", "TJ", None), "tass_strasbourg");
        assert_eq!(code("tass_valence", "TJ", None), "tass_valence");
        assert_eq!(code("cph_bobigny", "CPH", Some("Bobigny")), "cph_bobigny");
    }

    fn row(owner: &str, art: &str, num: &str, target: &str) -> (String, String, String, String) {
        (
            owner.to_string(),
            art.to_string(),
            num.to_string(),
            target.to_string(),
        )
    }

    #[test]
    fn a_foreign_court_no_scanner_names_gets_no_extracted_seat() {
        let html = format!(
            "<html><head><meta content=\"urn:lex;sn;cour.supreme;arret;2024-11-14;57\">\
             <meta name=\"dc.language\" content=\"FR\" /></head><body>\
             <article id=\"textArret\"><p>{}</p></article><h5>Synthèse</h5>\
             Pays : <a>Sénégal</a><br>Juridiction : <a>Cour suprême</a><br/>\
             Date de la décision : <span>14/11/2024</span><hr></body></html>",
            "LA COUR SUPRÊME, statuant en matière civile. ".repeat(10)
        );
        let parsed =
            lj_parse::parse_juricaf("SENEGAL-COURSUPREME-20241114-57", html.as_bytes(), None)
                .expect("a juricaf page");
        let axes = lj_extract::formation::FormationAxes::default();
        let none = HashMap::new();
        assert!(seat_of(&axes, &parsed.decision, None, &none, &none).is_none());
    }

    #[test]
    fn a_foreign_court_read_again_is_the_one_its_ingest_wrote() {
        use lj_parse::FromSourceFields;
        let html = format!(
            "<html><head><meta content=\"urn:lex;sn;cour.supreme;arret;2019-06-21;260.ca\">\
             <meta name=\"dc.language\" content=\"FR\" /></head><body>\
             <article id=\"textArret\"><p>{}</p></article><h5>Synthèse</h5>\
             Pays : <a>Bénin</a><br>Juridiction : <a>Cour suprême</a><br/>\
             Date de la décision : <span>21/06/2019</span><hr></body></html>",
            "LA COUR SUPRÊME, statuant en matière civile. ".repeat(10)
        );
        let parsed =
            lj_parse::parse_juricaf("BENIN-COURSUPREME-20190621-260CA", html.as_bytes(), None)
                .expect("a juricaf page");
        let reread = Decision::from_source_fields(
            &parsed.decision.texte_integral_clean,
            &parsed.source_fields,
            &parsed.decision.source_uid,
        );
        let court = foreign_court(&reread)
            .expect("a court its parser named")
            .expect("a foreign court");
        let seat = court.seat.expect("its seat");
        assert_eq!(parsed.jurisdiction_code, "bj_cour_supreme");
        assert_eq!(
            court.jurisdiction.expect("its court").code,
            "bj_cour_supreme"
        );
        assert_eq!(seat.jurisdiction_code.as_deref(), Some("bj_cour_supreme"));
        assert_eq!(court.seat_paths, seat.paths());
    }

    #[test]
    fn a_juportal_page_fused_with_its_juricaf_twin_keeps_the_court() {
        use lj_parse::FromSourceFields;
        let juportal = serde_json::json!({
            "court": "GHCC",
            "ecli": "ECLI:BE:GHCC:2025:ARR.120",
            "date": "2025-09-18",
            "dockets": ["120/2025"],
            "chamber": null,
            "lang": "fra",
        });
        let juricaf = serde_json::json!({
            "urn_lex": "be;cour.constitutionnelle;arret;2025-09-18;120.2025",
            "country": "Belgique",
            "jurisdiction": "Cour constitutionnelle",
            "tribunal": null,
            "date": "2025-09-18",
            "lang": "fra",
        });
        let link = LinkSnapshot::build(Vec::new(), Vec::new());
        let vocab = lj_extract::compiled::CompiledVocab::build(&[], &link);
        let chrono =
            lj_extract::chrono::ChronoSnapshot::with_closed(std::iter::empty(), Default::default());
        let case_names = lj_extract::case_link::CaseNameSnapshot::new(std::iter::empty());
        let seats = lj_extract::seat_link::SeatVocabulary::default();
        let none = HashMap::new();
        let snap = Snapshots {
            link: &link,
            vocab: &vocab,
            chrono: &chrono,
            jur_codes: &none,
            jur_sources: &none,
            successors: &none,
            case_names: &case_names,
            seats: &seats,
            places: &none,
        };
        let text = "La Cour constitutionnelle rend l'arrêt suivant. ".repeat(10);
        let read = |uid: &str, source_fields: &serde_json::Value| {
            let decision = Decision::from_source_fields(&text, source_fields, uid);
            reread_fields(&decision, source_fields, &snap).expect("a reading")
        };
        let fused = fuse_provenances(
            read("juportal/ECLI:BE:GHCC:2025:ARR.120", &juportal),
            vec![read(
                "juricaf/BELGIQUE-COURCONSTITUTIONNELLE-20250918-1202025",
                &juricaf,
            )],
        );
        assert_eq!(
            fused.jurisdiction.map(|j| j.code).as_deref(),
            Some("be_cour_constitutionnelle")
        );
        assert_eq!(
            fused.seat.and_then(|s| s.jurisdiction_code).as_deref(),
            Some("be_cour_constitutionnelle")
        );
        assert_eq!(fused.seat_paths, ["BE", "BE.COUR_CONSTITUTIONNELLE"]);
    }

    #[test]
    fn declared_oracle_arbitrates_then_abstains() {
        let oracle = build_declared(vec![
            row("LEGI_CPC", "1056-1", "1049", "LEGI_CPC"),
            row("LEGI_CPC", "1056-1", "57", "LEGI_CIV"),
            row("LEGI_CSS", "L161-1", "5", "LEGI_RUR"),
            row("LEGI_CSS", "L161-1", "5", "LEGI_CIV"),
            row("cir_60001", "", "12", "LEGI_TRAV"),
        ]);

        assert_eq!(
            declared_target(&oracle, "LEGI_CPC", Some("1056-1"), "1049"),
            Some("LEGI_CPC")
        );
        assert_eq!(
            declared_target(&oracle, "LEGI_CPC", Some("1056-1"), "57"),
            Some("LEGI_CIV")
        );
        assert_eq!(
            declared_target(&oracle, "LEGI_CSS", Some("L161-1"), "5"),
            None
        );
        assert_eq!(
            declared_target(&oracle, "LEGI_CPC", Some("1056-1"), "1055"),
            None
        );
        assert_eq!(
            declared_target(&oracle, "LEGI_CPC", Some("1057"), "1049"),
            None
        );
        assert_eq!(
            declared_target(&oracle, "cir_60001", None, "12"),
            Some("LEGI_TRAV")
        );
    }

    #[test]
    fn declared_oracle_folds_the_cited_number() {
        let oracle = build_declared(vec![row("LEGI_CSS", "L162-1", "L. 161-1", "LEGI_RUR")]);
        let ak = lj_norm::article_key::article_key("L. 161-1");
        assert_eq!(
            declared_target(&oracle, "LEGI_CSS", Some("L162-1"), &ak),
            Some("LEGI_RUR")
        );
    }

    #[test]
    fn prebuilt_from_source_fields_restores_the_cnda_solution() {
        let sf = serde_json::json!({ "solution_uid": "solution:REJET" });
        let pre = prebuilt_from_source_fields("cnda/24001234", &sf).expect("couche CNDA");
        assert_eq!(pre.solution_uid.as_deref(), Some("solution:REJET"));

        assert!(prebuilt_from_source_fields("cnda/24001234", &serde_json::json!({})).is_none());
        assert!(prebuilt_from_source_fields("judilibre/abc", &sf).is_none());

        let full = ExtractedFields {
            solution_uid: Some("solution:AUTRE".to_string()),
            legal_domain_uid: Some("legal_domain:PUBLIC".to_string()),
            ..Default::default()
        };
        let merged = merge_prebuilt(pre, full);
        assert_eq!(merged.solution_uid.as_deref(), Some("solution:REJET"));
        assert_eq!(
            merged.legal_domain_uid.as_deref(),
            Some("legal_domain:PUBLIC")
        );
    }

    #[test]
    fn prebuilt_from_source_fields_names_the_cedh_case() {
        let sf = serde_json::json!({ "docname": "AFFAIRE TAMAKULOVA ET AUTRES c. UKRAINE" });
        let pre = prebuilt_from_source_fields("cedh/001-250438", &sf).expect("couche CEDH");
        assert_eq!(
            pre.case_name.as_deref(),
            Some("TAMAKULOVA ET AUTRES c. UKRAINE")
        );

        let anonyme = serde_json::json!({ "docname": "AFFAIRE K c. FRANCE" });
        assert!(prebuilt_from_source_fields("cedh/001-250439", &anonyme).is_none());
        assert!(prebuilt_from_source_fields("cedh/001-250440", &serde_json::json!({})).is_none());
    }

    /// The name HUDOC carries outside the text joins the fields at `merge_prebuilt`,
    /// after the extraction, and still reaches the indexed title.
    #[test]
    fn the_prebuilt_name_reaches_the_title_through_the_merge() {
        let sf = serde_json::json!({ "docname": "AFFAIRE CIULLA c. ITALIE" });
        let pre = prebuilt_from_source_fields("cedh/001-57456", &sf).expect("couche CEDH");
        let full = ExtractedFields {
            date_lecture: NaiveDate::from_ymd_opt(1989, 2, 22),
            docket_numbers: vec!["11152/84".to_string()],
            ..Default::default()
        };
        let merged = with_search_title(
            merge_prebuilt(pre, full),
            Some("CEDH"),
            &HashMap::new(),
            None,
        );
        assert_eq!(
            merged.search_title.as_deref(),
            Some(
                "Cour européenne des droits de l'homme, 22 février 1989, CIULLA c. ITALIE, 11152/84"
            )
        );
    }

    /// A name elected downstream wins over the one the source carries: the
    /// corpus holds names no parser can know.
    #[test]
    fn the_elected_name_overrides_the_one_the_source_carries() {
        let e = ExtractedFields {
            date_lecture: NaiveDate::from_ymd_opt(2014, 2, 21),
            docket_numbers: vec!["360633".to_string()],
            case_name: Some("Nom de la source".to_string()),
            ..Default::default()
        };
        let titre = with_search_title(e, Some("CE"), &HashMap::new(), Some("Danthony"))
            .search_title
            .expect("un titre");
        assert!(titre.contains("Danthony"), "titre : {titre}");
        assert!(!titre.contains("Nom de la source"), "titre : {titre}");
    }

    #[test]
    fn compose_search_title_places_the_case_name_between_date_and_docket() {
        let nomme = ExtractedFields {
            date_lecture: NaiveDate::from_ymd_opt(1992, 12, 16),
            docket_numbers: vec!["C-144/91".to_string()],
            case_name: Some("Gilbert Demoor en Zonen NV contre État belge".to_string()),
            ..Default::default()
        };
        assert_eq!(
            compose_search_title(
                &nomme,
                Some("CJUE"),
                &HashMap::new(),
                nomme.case_name.as_deref()
            )
            .as_deref(),
            Some(
                "Cour de justice de l'Union européenne, 16 décembre 1992, \
                 Gilbert Demoor en Zonen NV contre État belge, C-144/91"
            )
        );

        let anonyme = ExtractedFields {
            case_name: None,
            ..nomme
        };
        assert_eq!(
            compose_search_title(&anonyme, Some("CJUE"), &HashMap::new(), None).as_deref(),
            Some("Cour de justice de l'Union européenne, 16 décembre 1992, C-144/91")
        );

        // A French decision is named by the ones citing it, long after extraction, so
        // its name reaches the title only through the argument.
        assert_eq!(
            compose_search_title(&anonyme, Some("CJUE"), &HashMap::new(), Some("Danthony"))
                .as_deref(),
            Some("Cour de justice de l'Union européenne, 16 décembre 1992, Danthony, C-144/91")
        );
    }

    #[test]
    fn fusion_keeps_the_authority_value_and_unions_the_lists() {
        use lj_store::repository::DecisionPartyRow;
        let party = |quality: &str, key: &str, party_ord: Option<i32>| DecisionPartyRow {
            quality: quality.to_string(),
            side: None,
            value: key.to_string(),
            resolve_key: key.to_string(),
            nature: None,
            barreau: None,
            role: None,
            firm_ord: None,
            party_ord,
            sub_ord: None,
            mentions: Vec::new(),
        };
        let judilibre = ExtractedFields {
            solution_uid: Some("solution:REJET".to_string()),
            docket_numbers: vec!["25-84.957".to_string()],
            themes: vec!["Bail".to_string()],
            parties: Some(vec![party("party", "societe x", None)]),
            ..Default::default()
        };
        let dila = ExtractedFields {
            solution_uid: Some("solution:CASSATION".to_string()),
            date_audience: Some(NaiveDate::from_ymd_opt(2025, 9, 2).unwrap()),
            docket_numbers: vec!["25-84957".to_string(), "25-84.958".to_string()],
            themes: vec!["BAIL".to_string(), "Congé".to_string()],
            parties: Some(vec![
                party("counsel_name", "maitre y", Some(1)),
                party("party", "societe x", None),
            ]),
            ..Default::default()
        };
        let fused = fuse_provenances(judilibre, vec![dila]);
        assert_eq!(fused.solution_uid.as_deref(), Some("solution:REJET"));
        assert_eq!(fused.date_audience, NaiveDate::from_ymd_opt(2025, 9, 2));
        assert_eq!(fused.docket_numbers, vec!["25-84.957", "25-84.958"]);
        assert_eq!(fused.themes, vec!["Bail", "Congé"]);
        let parties = fused.parties.unwrap();
        assert_eq!(parties.len(), 2);
        assert_eq!(parties[1].resolve_key, "maitre y");
        assert_eq!(parties[1].party_ord, Some(0));
    }

    #[test]
    fn merge_prebuilt_overlays_parser_fields_and_keeps_extraction() {
        let pre = ExtractedFields {
            solution_uid: Some("solution:SATISFACTION_TOTALE".to_string()),
            docket_numbers: vec!["26002382".to_string()],
            ..Default::default()
        };
        let full = ExtractedFields {
            solution_uid: Some("solution:REJET".to_string()),
            docket_numbers: vec!["ignored".to_string()],
            legal_domain_uid: Some("legal_domain:ETRANGERS".to_string()),
            citation_occurrences: Some(vec![CitationOccurrenceRow {
                char_start: 0,
                char_end: 10,
                ref_text_uid: Some("legal_text:CESEDA".to_string()),
                ref_num_key: Some("l532-2".to_string()),
                suivants: false,
            }]),
            ..Default::default()
        };
        let merged = merge_prebuilt(pre, full);
        assert_eq!(
            merged.solution_uid.as_deref(),
            Some("solution:SATISFACTION_TOTALE")
        );
        assert_eq!(merged.docket_numbers, vec!["26002382".to_string()]);
        assert_eq!(
            merged.legal_domain_uid.as_deref(),
            Some("legal_domain:ETRANGERS")
        );
        assert_eq!(merged.citation_occurrences.as_ref().map(Vec::len), Some(1));
    }
}
