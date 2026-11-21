//! Extraction unifiée des champs structurés : fonctions plates `extract_*` sur
//! `Decision`, un document = un scan de marqueurs (`crate::scan`) + un automate
//! citations (`crate::compiled`). Le gabarit se détecte dans le flux de tokens ;
//! la garde [`routed`] ne sert qu'aux call-sites qui skippent les fonds hors
//! nomenclature. Les helpers partagés vivent dans `common`.
//!
//! Le crate `regex` (v1) ne supporte pas les lookaround (`(?=`, `(?!`, `(?<`) :
//! les bornes de capture passent donc soit par une borne consommée explicite,
//! soit par un post-filtrage manuel sur les positions de match.

/// Helpers partagés (normalisation articles/instruments/dates/avocats).
pub(crate) mod common;

/// Normaliseurs canoniques d'instrument / article (`normalize_instrument`,
/// `normalize_article`), exposés pour que le banc d'éval apparie les paires
/// `legal_references` exactement comme le scoring.
pub use common::{is_unresolvable_instrument, normalize_article, normalize_instrument};

/// Signaux structurés d'une clé de citation, consommés par l'écrivain
/// `citation_key` et le résolveur.
pub use common::key_signals;

use lj_core::decision::Decision;
use lj_core::error::Result;

/// Version du pipeline d'extraction de champs, stockée par décision
/// (`decisions.extract_version`). À incrémenter à chaque changement de
/// comportement des extracteurs : un `reextract-fields` ne re-parse alors que les
/// décisions dont la version stockée diffère, reprise après interruption
/// incluse. NULL en base = extrait avant le versionnage.
///
/// La constante elle-même vit dans `lj-core`, ancêtre commun de `lj-extract` qui
/// la produit et de `lj-store` qui en gate le SQL ; re-exportée ici pour le chemin
/// stable `extract::EXTRACT_VERSION`.
pub use lj_versions::EXTRACT_VERSION;

mod cjue_formation;
mod dockets;
mod formation_label;
mod jurisdiction_names;

#[cfg(test)]
pub(crate) mod tests;

/// Uids `procedure:*` / `office:*` / `legal_domain:*` détectés par la
/// décomposition procédurale au scanner : chaque détection route directement vers
/// son axe référentiel. Chaque champ est un uid complet de `facet_value` (FK) ou
/// `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcedureUids {
    pub procedure_uid: Option<String>,
    pub office_uid: Option<String>,
    pub legal_domain_uid: Option<String>,
    /// Raffinement domaine lu dans le TEXTE (vocabulaire d'en-tête admin),
    /// à passer au vote citations ([`crate::domain::DomainContext::hint`])
    /// quand `legal_domain_uid` est `None` : il ne s'applique que si aucun
    /// code substantiel ne vote.
    pub domain_hint: Option<&'static str>,
}

/// Ordre administratif opendata (TA/CAA/CE) ?
fn is_admin(d: &Decision) -> bool {
    matches!(
        d.jurisdiction_type.as_deref(),
        Some("TA") | Some("CAA") | Some("CE")
    )
}

/// Ordre judiciaire (fonds Judilibre CC/CA/TJ/TCOM) ?
pub fn is_judiciaire(d: &Decision) -> bool {
    matches!(
        d.jurisdiction_type.as_deref(),
        Some("CC" | "CA" | "TJ" | "TCOM" | "CPH" | "ASSISES" | "CCD")
    )
}

/// Garde de routage : erreur `UnknownJuridiction` si la juridiction n'est pas
/// l'un des ordres FR connus. Consommée par les call-sites qui SKIPPENT les fonds
/// hors nomenclature ; les fonctions d'extraction elles-mêmes traitent l'inconnu
/// en générique (champs métadonnées + citations texte), jamais en erreur dure.
pub fn routed(decision: &Decision) -> Result<()> {
    match decision.jurisdiction_type.as_deref() {
        Some(
            "TA" | "CAA" | "CE" | "CC" | "CA" | "TJ" | "TCOM" | "CPH" | "ASSISES" | "CCD"
            | "CONSTIT" | "TC" | "CEDH" | "CJUE" | "CNDA" | "CNIL" | "DDD" | "ADLC" | "AMF"
            | "COMEU",
        ) => Ok(()),
        _ => Err(lj_core::error::CoreError::UnknownJuridiction(
            decision.jurisdiction_type.clone(),
        )),
    }
}

pub fn extract_docket_numbers(d: &Decision) -> Option<Vec<String>> {
    docket_numbers_scanned(d, scan_doc(d).as_ref())
}

/// Numéros de dossier : métadonnée d'abord (liste source, sinon numéro unique
/// éclaté sur les séparateurs de greffe), complétée par les jonctions lues dans
/// le texte via les gabarits auto-détectés du scan — pivot « joint les pourvois »
/// et clause « sous le(s) n° » de requête, chacun ne s'exprimant que si son token
/// est présent.
pub fn docket_numbers_scanned(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<Vec<String>> {
    // La métadonnée greffe (liste ou numéro unique) peut coller plusieurs
    // numéros dans une même valeur : chaque entrée s'éclate sur les
    // séparateurs.
    static RE_SEP: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re_sep = RE_SEP.get_or_init(|| regex::Regex::new(r"[,;\s]+").unwrap());
    let raw: Vec<&str> = match &d.numero_dossiers {
        Some(v) if !v.is_empty() => v.iter().map(String::as_str).collect(),
        _ => d
            .numero_dossier
            .as_deref()
            .filter(|s| !s.is_empty())
            .into_iter()
            .collect(),
    };
    // A Bulletin reference (`Bull. 1965, I, n° 611`) is one number.
    let mut base: Vec<Option<String>> = raw
        .iter()
        .flat_map(|nd| {
            let nd = nd.trim();
            if nd.starts_with("Bull. ") {
                vec![Some(nd.to_string())]
            } else {
                re_sep.split(nd).map(|s| Some(s.to_string())).collect()
            }
        })
        .collect();
    base.extend(dockets::joined_pourvois(scan).into_iter().map(Some));
    if let Some(joined) = dockets::joined_docket_numbers(d, scan) {
        base.extend(joined.into_iter().map(Some));
    }
    common::clean_docket_numbers(Some(&base))
}

pub fn extract_date_lecture(d: &Decision) -> Option<String> {
    common::clean_date_iso(d.date_lecture.as_deref())
}

pub fn extract_date_audience(d: &Decision) -> Option<String> {
    date_audience_scanned(d, scan_doc(d).as_ref())
}

/// Date d'audience : métadonnée d'abord, sinon la date textuelle lue dans les
/// fenêtres positionnées du scan.
pub fn date_audience_scanned(d: &Decision, scan: Option<&crate::scan::DocScan>) -> Option<String> {
    let v = d
        .date_audience
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| common::extract_textual_audience_date(d, scan));
    common::clean_date_iso(v.as_deref())
}

/// Variante plate de [`formation_axes_scanned`] : re-scanne et renvoie les axes
/// structurés.
pub fn extract_formation_axes(d: &Decision) -> crate::formation::FormationAxes {
    formation_axes_scanned(d, scan_doc(d).as_ref())
}

/// Formation structurée : code chambre CC, chambre de bandeau Judilibre,
/// formation greffe — décomposés en axes.
pub fn formation_axes_scanned(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> crate::formation::FormationAxes {
    let jt = d.jurisdiction_type.as_deref();
    let mut axes = if is_admin(d) {
        crate::formation::parse_formation(jt, None, None, None, d.formation.as_deref())
    } else if jt == Some("CJUE") {
        return cjue_formation::cjue_formation(&d.texte_integral_clean, &d.source_uid);
    } else if !is_judiciaire(d) {
        return crate::formation::FormationAxes::default();
    } else if matches!(jt, Some("ASSISES" | "CCD")) {
        crate::formation::FormationAxes {
            formation_uid: crate::formation::criminal_formation(jt, d.formation.as_deref()),
            ..Default::default()
        }
    } else if jt == Some("CC") {
        let mut axes = crate::formation::parse_formation(
            jt,
            d.chamber.as_deref(),
            None,
            None,
            d.formation.as_deref(),
        );
        crate::formation::name_on_date(&mut axes, extract_date_lecture(d).as_deref());
        if axes.formation_uid.is_none() {
            axes.formation_uid = scan.and_then(|s| s.cc_formation());
        }
        axes
    } else {
        // Le champ source `chamber` prime ; le bandeau scanné complète les axes
        // (spécialisation quand le greffe ne donne que la position).
        let chamber = d.chamber.as_deref().filter(|c| !c.is_empty());
        let bandeau = scan
            .map(|s| s.bandeau_text())
            .as_deref()
            .and_then(formation_label::chamber_from_body);
        crate::formation::parse_formation(
            jt,
            None,
            chamber,
            bandeau.as_deref(),
            d.formation.as_deref(),
        )
    };
    // Composition lue au TEXTE quand ni greffe ni chambre ne l'ont posée. En
    // PREMIÈRE instance la mention est auto-référentielle (« Vice-présidente,
    // statuant en juge unique » au bloc de composition ; « demande au juge des
    // référés » = requête adressée au juge de CE document ; « magistrat désigné »
    // signé en pied). En appel et en cassation le récit d'en-tête décrit la
    // juridiction ATTAQUÉE : seule l'adresse à SON juge des référés compte (« au
    // juge des référés du Conseil d'État », « juge des référés de la cour »).
    let first_instance = matches!(jt, Some("TA") | Some("TJ") | Some("TCOM") | Some("CPH"));
    if axes.formation_uid.is_none() && axes.chamber.solemn != Some(lj_core::chamber::Solemn::Avis) {
        if let Some(s) = scan {
            let sig = s.procedure_signals();
            let member = d.source_uid.rsplit('/').next().unwrap_or("").to_uppercase();
            let ordo_refere_ta =
                jt == Some("TA") && member.starts_with("ORTA_") && s.refere_article_header();
            let ju = if first_instance {
                s.juge_unique_header()
                    || ordo_refere_ta
                    || sig.jref_demande
                    || (jt == Some("TA") && sig.magdes_tail)
            } else {
                match jt {
                    Some("CE") => sig.jref_conseil,
                    Some("CAA") => sig.refere_cour,
                    _ => false,
                }
            };
            // An administrative order is signed by the one judge who made it.
            if ju || (is_admin(d) && s.ordonnance()) {
                axes.formation_uid = Some("formation:JUGE_UNIQUE");
            }
            // A tribunal's or a cour's judgment names its bench when it closes.
            if axes.formation_uid.is_none() && matches!(jt, Some("TA") | Some("CAA")) {
                axes.formation_uid = match s.siege_judges() {
                    Some(3) => Some("formation:A_TROIS"),
                    Some(5) => Some("formation:A_CINQ"),
                    _ => None,
                };
            }
        }
    }
    axes
}

pub fn extract_jurisdiction_name(d: &Decision) -> Option<String> {
    jurisdiction_name_scanned(d, scan_doc(d).as_ref())
}

/// Variante plate de [`jurisdiction_scanned`] : re-scanne et renvoie le code
/// référentiel seul.
pub fn extract_jurisdiction_code(d: &Decision) -> Option<String> {
    let scan = scan_doc(d);
    let dockets = docket_numbers_scanned(d, scan.as_ref()).unwrap_or_default();
    jurisdiction_scanned(d, scan.as_ref(), &dockets).map(|j| j.code)
}

/// Juridiction catégorielle : la ligne référentielle directe (code + label
/// canonique + ville), seule sortie d'extraction — le nom scanné n'est qu'un
/// détail interne de la composition. Entrées : type et localisation source, nom
/// scanné ; une CAA sans ville dans le nom est identifiée par le code cour du
/// numéro de requête (« 12BX02667 » → Bordeaux). `None` = pas de code fiable.
pub fn jurisdiction_scanned(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
    docket_numbers: &[String],
) -> Option<crate::facets::JurisdictionRef> {
    let jt = d.jurisdiction_type.as_deref()?;
    let name = jurisdiction_name_scanned(d, scan);
    let location = d.jurisdiction_location.as_deref();
    crate::facets::jurisdiction_ref(jt, location, name.as_deref()).or_else(|| {
        if jt != "CAA" {
            return None;
        }
        let label = docket_numbers
            .first()
            .and_then(|n| crate::facets::caa_label_from_docket(n))?;
        crate::facets::jurisdiction_ref(jt, location, Some(label))
    })
}

/// Court name: the canonical rewrite of the administrative registry's name, the Judilibre
/// `location` table on the judicial side (with the TAE renaming read in the scanned header),
/// the parser's own for the authorities whose source names them, `None` otherwise.
pub fn jurisdiction_name_scanned(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<String> {
    if is_admin(d) {
        return jurisdiction_names::admin_name(d);
    }
    if matches!(
        d.jurisdiction_type.as_deref(),
        Some("DDD" | "ADLC" | "AMF" | "COMEU")
    ) {
        return d.jurisdiction_name.clone();
    }
    if !is_judiciaire(d) {
        return None;
    }
    let header = scan.map(|s| s.header_text());
    jurisdiction_names::from_location(d, header.as_deref())
}

/// Label solution du greffe → clé solution-17. Vocabulaire FERMÉ (codes
/// Judilibre, libellés opendata, libellés TJ/TCOM), comparaisons en minuscules
/// pliées, aucune regex.
fn solution_from_label(low: &str) -> Option<&'static str> {
    // codes Judilibre exacts
    if let Some(k) = match low {
        "rejet" => Some("REJET"),
        "cassation" | "annulation" => Some("SATISFACTION_TOTALE"),
        "cassation_partielle" => Some("SATISFACTION_PARTIELLE"),
        "irrecevabilite" | "irrecevabilité" => Some("IRRECEVABILITE"),
        "non-admission" | "non admission" => Some("NON_ADMISSION"),
        "décheance" | "decheance" => Some("DECHEANCE"),
        "non-lieu" | "nonlieu" => Some("NON_LIEU_A_STATUER"),
        "qpc_non-lieu" => Some("NON_LIEU_A_STATUER"),
        "désistement" | "desistement" => Some("DESISTEMENT"),
        "designation" | "qpc_renvoi" | "qpc" | "renvoi" => Some("AUTRE"),
        _ => None,
    } {
        return Some(k);
    }
    // libellés TJ/TCOM (substrings, ordre = priorité)
    const TJ_LABELS: &[(&str, &str)] = &[
        (
            "fait droit à l'ensemble des demandes",
            "SATISFACTION_TOTALE",
        ),
        (
            "fait droit à une partie des demandes",
            "SATISFACTION_PARTIELLE",
        ),
        ("déboute le ou les demandeurs de l'ensemble", "REJET"),
        // expulsion conditionnelle ou en référé : délais ou provisoire → PARTIELLE ;
        // « ferme au fond » reste TOTALE
        ("expulsion \"conditionnelle\"", "SATISFACTION_PARTIELLE"),
        (
            "expulsion \"ferme\" ordonnée en référé",
            "SATISFACTION_PARTIELLE",
        ),
        ("expulsion", "SATISFACTION_TOTALE"),
        // divorce prononcé : les demandes accessoires (prestation compensatoire,
        // jouissance…) ne sont jamais toutes accordées
        ("prononce le divorce", "SATISFACTION_PARTIELLE"),
        ("renvoi avec ordonnance de clôture", "AUTRE"),
        ("renvoi à la mise en état", "AUTRE"),
        ("statue sur un incident", "AUTRE"),
        ("mee :", "AUTRE"),
        ("ne dessaisissant pas", "AUTRE"),
        ("se dessaisit", "AUTRE"),
        ("dessaisi au profit", "AUTRE"),
        // demande de maintien accueillie (JLD rétention/soins) = le demandeur obtient la
        // mesure
        ("maintien de la mesure", "SATISFACTION_TOTALE"),
    ];
    for (pat, key) in TJ_LABELS {
        if low.contains(pat) {
            return Some(key);
        }
    }
    // routage de greffe (« TA Marseille », question préjudicielle) : neutre
    for j in ["ta ", "caa ", "ce ", "cnda "] {
        if low.starts_with(j) {
            return Some("AUTRE");
        }
    }
    if low.contains("question préjudicielle") {
        return Some("AUTRE");
    }
    if low.contains("manifestement infondé")
        || low.contains("manifestement infonde")
        || low.contains("rejet moyen")
    {
        return Some("REJET");
    }
    if low.contains("irrecevab") {
        return Some("IRRECEVABILITE");
    }
    // « Renvoi après cassation », « R. 122-12-6 Renvoi cassation série » (CE) :
    // gagné = cassé, la conversion cassation rend CASSATION.
    if low.contains("renvoi") && low.contains("cassation") {
        return Some("SATISFACTION_TOTALE");
    }
    if low.contains("incompétence") || low.contains("incompetence") {
        // « Rejet - incompétence » : le dispositif rejette (verbatim) ; le déclinatoire
        // pur reste neutre.
        if low.contains("rejet") {
            return Some("REJET");
        }
        return Some("AUTRE");
    }
    for neutral in [
        "radiation",
        "expertise",
        "médiation",
        "mediation",
        "avis article",
        "qpc",
    ] {
        if low.contains(neutral) {
            return Some("AUTRE");
        }
    }
    // libellés opendata (préfixes, ordre = priorité)
    const PREFIXES: &[(&str, &str)] = &[
        ("désistement", "DESISTEMENT"),
        ("non-lieu", "NON_LIEU_A_STATUER"),
        ("non lieu", "NON_LIEU_A_STATUER"),
        ("renvoi", "AUTRE"),
        ("rejet partiel", "SATISFACTION_PARTIELLE"),
        ("satisfaction partielle", "SATISFACTION_PARTIELLE"),
        ("annulation partielle", "SATISFACTION_PARTIELLE"),
        ("admission partielle", "SATISFACTION_PARTIELLE"),
        ("cassation partielle", "SATISFACTION_PARTIELLE"),
        ("cassation", "SATISFACTION_TOTALE"),
        ("annulation", "SATISFACTION_TOTALE"),
        ("satisfaction", "SATISFACTION_TOTALE"),
        ("rejet", "REJET"),
    ];
    for (prefix, key) in PREFIXES {
        if low.starts_with(prefix) {
            return Some(key);
        }
    }
    if low.contains("désistement") || low.contains("desistement") {
        return Some("DESISTEMENT");
    }
    if low.contains("non-lieu") || low.contains("non lieu") {
        return Some("NON_LIEU_A_STATUER");
    }
    if low.contains("renvoi") {
        return Some("AUTRE");
    }
    if low.contains("rejet") {
        return Some("REJET");
    }
    if low.contains("annul") || low.contains("astreinte") {
        return Some("SATISFACTION_TOTALE");
    }
    None
}

// DILA normalises the Conseil constitutionnel's dispositif into this label, which
// alone decides.
fn constit_solution(low: &str) -> &'static str {
    const PREFIXES: &[(&str, &str)] = &[
        ("conformité", "CONFORMITE"),
        ("non conformité", "NON_CONFORMITE"),
        ("inéligibilité", "INELIGIBILITE"),
        ("annulation", "ANNULATION"),
        ("réformation", "REFORMATION"),
        ("rejet", "REJET"),
        ("irrecevabilité", "IRRECEVABILITE"),
        ("non lieu", "NON_LIEU_A_STATUER"),
    ];
    PREFIXES
        .iter()
        .find(|(prefix, _)| low.starts_with(prefix))
        .map_or("AUTRE", |(_, key)| key)
}

/// The CEDH's ruling, read from HUDOC's `conclusion`: one `;`-separated finding
/// per item, in French or in English.
fn cedh_solution(low: &str) -> &'static str {
    let items: Vec<&str> = low
        .split(';')
        .map(|item| {
            let item = item.trim();
            ["partiellement ", "partly ", "remainder "]
                .iter()
                .find_map(|p| item.strip_prefix(p))
                .unwrap_or(item)
        })
        .collect();
    let starts = |heads: &[&str]| items.iter().any(|i| heads.iter().any(|h| i.starts_with(h)));
    let contains = |words: &[&str]| items.iter().any(|i| words.iter().any(|w| i.contains(w)));
    if starts(&["violation"]) {
        "VIOLATION"
    } else if starts(&["non-violation", "no violation"]) {
        "NON_VIOLATION"
    } else if starts(&["recevable", "admissible"]) {
        "RECEVABLE"
    } else if starts(&[
        "irrecevable",
        "inadmissible",
        "inadmisible",
        "exception préliminaire retenue",
        "exceptions préliminaires retenues",
        "preliminary objection allowed",
        "preliminary objections allowed",
        "incompétence",
    ]) {
        "IRRECEVABILITE"
    } else if contains(&["règlement amiable", "friendly settlement"]) {
        "REGLEMENT_AMIABLE"
    } else if contains(&["radiation", "struck out"]) {
        "RADIATION"
    } else {
        "AUTRE"
    }
}

fn solution_uid(key: &str) -> String {
    format!("solution:{key}")
}

/// Solution (uid `solution:*`) : le label du greffe (métadonnée) fait foi via un
/// vocabulaire fermé ; le texte n'est que fallback (composeur
/// [`crate::scan::DocScan::outcome`] sur la ZONE dispositif), sauf détections qui
/// PRIMENT (désistement constaté en cours d'instance, pourvoi non admis).
/// Vocabulaire de sortie par gabarit : pourvoi en cassation → gagner = casser.
pub fn extract_solution(d: &Decision) -> Option<String> {
    solution_scanned(d, scan_doc(d).as_ref())
}

pub fn solution_scanned(d: &Decision, scan: Option<&crate::scan::DocScan>) -> Option<String> {
    let raw = d.solution.as_deref().unwrap_or("").trim().to_lowercase();
    if d.jurisdiction_type.as_deref() == Some("CONSTIT") && !raw.is_empty() {
        return Some(solution_uid(constit_solution(&raw)));
    }
    // The generic scan reads Strasbourg's operative part as a French one: HUDOC's label alone decides.
    if d.jurisdiction_type.as_deref() == Some("CEDH") {
        return (!raw.is_empty()).then(|| solution_uid(cedh_solution(&raw)));
    }
    let text = scan.and_then(|s| s.outcome());
    // vocabulaire cassation : la juridiction (métadonnée) fait foi, le gabarit texte
    // rattrape les fonds sans métadonnée ; le pourvoi CE (gabarit Admin sans pivot
    // judiciaire) se signale par « pourvoi » en en-tête
    let cc = d.jurisdiction_type.as_deref() == Some("CC")
        || scan.is_some_and(|s| s.gabarit_cc())
        || (d.jurisdiction_type.as_deref() == Some("CE")
            && scan.is_some_and(|s| s.header_has_pourvoi()));
    let admin = !cc && is_admin(d);
    // détection texte qui prime le label — sauf label de satisfaction
    if let Some((k, true)) = text {
        if !(raw.starts_with("satisfaction")
            || raw.starts_with("annulation")
            || raw.starts_with("cassation"))
        {
            return Some(solution_uid(k));
        }
    }
    if !raw.is_empty() {
        // filtrage cassation admin (PAPC) : l'issue vit dans le dispositif
        if raw.contains("papc") || raw.contains("822-5") {
            return Some(solution_uid(match text {
                Some(("DESISTEMENT", _)) => "DESISTEMENT",
                Some(("NON_LIEU_A_STATUER", _)) => "NON_LIEU_A_STATUER",
                Some(("SATISFACTION_TOTALE", _)) => "SATISFACTION_TOTALE",
                Some(("REJET", _)) => "REJET",
                _ => "NON_ADMISSION",
            }));
        }
        if let Some(key) = solution_from_label(&raw) {
            let key = if cc {
                match key {
                    "SATISFACTION_TOTALE" => {
                        if scan.as_ref().is_some_and(|s| s.cassation_partial()) {
                            "CASSATION_PARTIELLE"
                        } else {
                            "CASSATION"
                        }
                    }
                    // Un label opendata « satisfaction partielle » décrit le FOND
                    // (suspension bornée, décharge partielle), pas la cassation :
                    // la partialité de la cassation se lit au dispositif. Le label
                    // greffe « cassation partielle » reste souverain.
                    "SATISFACTION_PARTIELLE" => {
                        if raw.contains("cassation")
                            || scan.as_ref().is_some_and(|s| s.cassation_partial())
                        {
                            "CASSATION_PARTIELLE"
                        } else {
                            "CASSATION"
                        }
                    }
                    // label de routage (« qpc »…) : le dispositif « dit n'y
                    // avoir lieu de renvoyer » lit un non-lieu réel
                    "AUTRE" => match text {
                        Some(("NON_LIEU_A_STATUER", _)) => "NON_LIEU_A_STATUER",
                        _ => "AUTRE",
                    },
                    other => other,
                }
            } else if admin {
                // Administrative order: an annulment label gives ANNULATION, or
                // ANNULATION_PARTIELLE when partial, unless the operative part reforms;
                // an ambiguous label is settled by the operative part.
                match key {
                    "SATISFACTION_TOTALE" | "SATISFACTION_PARTIELLE" if raw.contains("annul") => {
                        match text {
                            Some(("REFORMATION", _)) => "REFORMATION",
                            _ if raw.contains("annulation partielle") => "ANNULATION_PARTIELLE",
                            _ => "ANNULATION",
                        }
                    }
                    "SATISFACTION_TOTALE" | "SATISFACTION_PARTIELLE" => match text {
                        Some(("ANNULATION", _)) => "ANNULATION",
                        Some(("REFORMATION", _)) => "REFORMATION",
                        Some(("SATISFACTION_PARTIELLE", _)) if raw.contains("ou partielle") => {
                            "SATISFACTION_PARTIELLE"
                        }
                        _ => key,
                    },
                    "IRRECEVABILITE" => match text {
                        Some(("REJET", _)) => "REJET",
                        _ => key,
                    },
                    // Label « Rejet » = littéral du dispositif ; on code la
                    // catégorie la plus spécifique : irrecevabilité PRONONCÉE au
                    // texte (« rejetée comme manifestement irrecevable »,
                    // R. 222-1) → IRRECEVABILITE. Évocation en appel : « le
                    // jugement est annulé ; la demande de première instance est
                    // rejetée » — le greffe clique « Rejet » (sort de la demande
                    // initiale), le dispositif se lit VERBATIM : jugement renversé
                    // → ANNULATION (« réformé » → REFORMATION).
                    "REJET" => match text {
                        Some(("IRRECEVABILITE", _)) => "IRRECEVABILITE",
                        Some(("ANNULATION", _)) => "ANNULATION",
                        Some(("REFORMATION", _)) => "REFORMATION",
                        _ => key,
                    },
                    other => other,
                }
            } else {
                // Ordre judiciaire : le dispositif VERBATIM prime le label
                // cliqué du greffe — « Fait droit à l'ensemble des demandes »
                // avec des têtes rejetées ou déboutées au dispositif =
                // PARTIELLE ; label « irrecevabilité » sur un dispositif qui
                // rejette au fond = REJET (le fond absorbe).
                match (key, &text) {
                    ("SATISFACTION_TOTALE", Some(("SATISFACTION_PARTIELLE", _))) => {
                        "SATISFACTION_PARTIELLE"
                    }
                    ("IRRECEVABILITE", Some(("REJET", _))) => "REJET",
                    // Labels de routage (« renvoi à la mise en état », « ne
                    // dessaisissant pas », « statue sur un incident », qpc…) : on
                    // code le sort RÉEL de l'incident quand le dispositif en lit
                    // un — AUTRE n'est que le silence du dispositif.
                    ("AUTRE", Some((t, _))) if *t != "AUTRE" => t,
                    _ => key,
                }
            };
            return Some(solution_uid(key));
        }
    }
    if let Some((k, _)) = text {
        // Pourvoi (CC/CE) : annuler = casser — le gabarit Admin d'un pourvoi CE rend
        // ANNULATION, le vocabulaire de sortie est la cassation.
        let k = if cc && k == "ANNULATION" {
            if scan.as_ref().is_some_and(|s| s.cassation_partial()) {
                "CASSATION_PARTIELLE"
            } else {
                "CASSATION"
            }
        } else {
            k
        };
        return Some(solution_uid(k));
    }
    if raw.is_empty() {
        None
    } else {
        Some(solution_uid("AUTRE"))
    }
}

fn office_uid(key: &str) -> ProcedureUids {
    ProcedureUids {
        office_uid: Some(format!("office:{key}")),
        ..Default::default()
    }
}

fn domaine_uid(key: &str) -> ProcedureUids {
    ProcedureUids {
        legal_domain_uid: Some(format!("legal_domain:{key}")),
        ..Default::default()
    }
}

/// Le mot `w` apparaît-il en MOT ENTIER dans `blob` (déjà plié) ?
fn blob_word(blob: &str, w: &str) -> bool {
    blob.split(|c: char| !c.is_alphanumeric()).any(|x| x == w)
}

/// Voie/office/domaine depuis la FORMATION et le code juridiction (métadonnées de
/// greffe, vocabulaire fermé plié — aucune regex).
fn procedure_from_chamber(d: &Decision, sig: &crate::scan::ProcSignals) -> Option<ProcedureUids> {
    let blob = lj_fold::fold_chars(&format!(
        "{} {}",
        d.chamber.as_deref().unwrap_or(""),
        d.formation.as_deref().unwrap_or("")
    ));
    if blob.trim().is_empty() {
        return None;
    }
    if blob.contains("retention") || blob_word(&blob, "etranger") || blob_word(&blob, "etrangers") {
        return Some(domaine_uid("PUBLIC_DROIT_ETRANGERS_NATIONALITE"));
    }
    if blob.contains("hospitalisation")
        || blob_word(&blob, "hsc")
        || blob.contains("h.o.")
        || blob.contains("soins psychiatriques")
        || blob.contains("soin psychiatrique")
    {
        return Some(domaine_uid("CIVIL_DROIT_PERSONNES_FAMILLE"));
    }
    if blob.contains("securite sociale")
        || blob.contains("sec soc")
        || blob.contains("protection sociale")
        || blob.contains("pole social")
        || blob_word(&blob, "secu")
        || blob.contains("secu.")
        || blob.contains("secu-")
        || blob_word(&blob, "fiva")
        || blob_word(&blob, "cdas")
        || blob_word(&blob, "tass")
        || blob.contains("( ps )")
        || blob.contains("(ps)")
        || blob.contains("affaires de securite")
        || blob.contains("affaire de securite")
    {
        return Some(domaine_uid("SOCIAL_DROIT_AIDE_ACTION_SOCIALE"));
    }
    if blob_word(&blob, "jcp")
        || blob_word(&blob, "pcp")
        || blob.contains("contentieux de la protection")
    {
        return Some(office_uid("JCP"));
    }
    if blob_word(&blob, "jaf") || blob.contains("affaires familiales") {
        return Some(office_uid("JAF"));
    }
    if blob.contains("surendettement") {
        return Some(domaine_uid("COMMERCIAL_DROIT_CONSOMMATION"));
    }
    if blob_word(&blob, "jld")
        || blob.contains("j.l.d")
        || blob.contains("libertes et detention")
        || blob.contains("liberte et detention")
        || blob.contains("libertes et de la detention")
        || blob.contains("liberte et de la detention")
    {
        // le JLD statue surtout en rétention (étrangers) et en soins sans
        // consentement : le TEXTE tranche le domaine, sinon l'office
        if sig.retention_anywhere {
            return Some(domaine_uid("PUBLIC_DROIT_ETRANGERS_NATIONALITE"));
        }
        if sig.hospi_anywhere {
            return Some(domaine_uid("CIVIL_DROIT_PERSONNES_FAMILLE"));
        }
        return Some(office_uid("JLD"));
    }
    None
}

/// La voie de recours (`procedure:*`) — cascade métadonnées puis signaux texte.
fn procedure_key(
    d: &Decision,
    sig: &crate::scan::ProcSignals,
    chamber: &str,
) -> Option<&'static str> {
    let sol = lj_fold::fold_chars(d.solution.as_deref().unwrap_or(""));
    let tr = lj_fold::fold_chars(d.type_recours.as_deref().unwrap_or(""));
    let jt = d.jurisdiction_type.as_deref();
    if sol.contains("qpc") || tr.trim() == "qpc" {
        return Some("QPC");
    }
    if sol.contains("non-admission") || sol.contains("non admission") {
        return Some("PAPC");
    }
    // filtrage R222-1 affiché au label (ordonnances TA/CAA)
    if !sol.contains("incompetence")
        && (sol.contains("222-1")
            || sol.contains("appel manifestement infonde")
            || sol.contains("irrecevabilite manifeste")
            || sol.contains("serie identique"))
    {
        return Some("FILTRAGE_R222_1");
    }
    // « R.822-5 Désistement PAPC » : quand le greffe qualifie la voie, elle
    // tient même sur un désistement en phase d'admission
    if sol.contains("papc") {
        return Some("PAPC");
    }
    // désistement : R. 822-5 cité pour la procédure d'interruption, pas une
    // vraie voie PAPC
    if !sol.contains("desist")
        && (sol.contains("822-5") || (sol.contains("admission") && sol.contains("cassation")))
    {
        return Some("PAPC");
    }
    if sol.contains("expertise") || sol.contains("mediation") {
        return None;
    }
    if sol.contains("revision") {
        return Some("RECOURS_REVISION");
    }
    if sig.qpc {
        return Some("QPC");
    }
    // filtrage cassation : seules les cours de cassation (CC/CE) en ont un ; pas sur
    // un désistement (le pourvoi s'interrompt, la voie n'est pas la procédure
    // d'admission) ; pas en chambre criminelle, la non-admission art. 567-1-1 CPP
    // n'étant pas la PAPC civile
    if sig.papc
        && matches!(jt, Some("CC") | Some("CE"))
        && !sol.contains("desist")
        && !blob_word(chamber, "cr")
        && !chamber.contains("crim")
    {
        return Some("PAPC");
    }
    // ordonnances de filtrage TA/CAA sans label : R222-1 qualifié aux motifs
    // (le préfixe ORTA_/ORCA_ vit sur le NOM DE MEMBRE, après le zip)
    let member = d.source_uid.rsplit('/').next().unwrap_or("").to_uppercase();
    if sig.filtrage && (member.starts_with("ORTA_") || member.starts_with("ORCA_")) {
        return Some("FILTRAGE_R222_1");
    }
    if sig.rectification {
        return Some("RECTIFICATION_INTERPRETATION");
    }
    // référés administratifs (articles CJA dans l'en-tête)
    if sig.refere_cour && sig.refere_utiles {
        // référé d'appel L521-3 du juge de la cour : pas de voie référencée
        return None;
    }
    if sig.refere_liberte {
        return Some("REFERE_LIBERTE");
    }
    if sig.refere_utiles {
        return Some("REFERE_MESURES_UTILES");
    }
    if sig.refere_precontractuel {
        return Some("REFERE_PRECONTRACTUEL");
    }
    if sig.refere_provision {
        return Some("REFERE_PROVISION");
    }
    if sig.refere_suspension {
        return Some("REFERE_SUSPENSION");
    }
    // référé judiciaire : formation de référé en métadonnée (fonds judiciaires), ou
    // dit par le texte pour les juridictions du fond — pas en cassation, où le récit
    // « ordonnance de référé » raconte l'instance d'origine et où les vieux arrêts
    // sans bandeau ouvrent directement sur les motifs
    if chamber.contains("refere") && !matches!(jt, Some("TA") | Some("CAA") | Some("CE")) {
        return Some("REFERE_CIVIL");
    }
    // pas sur un désistement : l'instance s'interrompt, comme pour la PAPC
    if sig.refere_civil
        && matches!(jt, Some("CA") | Some("TJ") | Some("TCOM") | Some("CPH"))
        && !sol.contains("desist")
        && !sig.desist_bandeau
    {
        return Some("REFERE_CIVIL");
    }
    None
}

/// Décomposition procédurale voie/office/domaine : les trois axes se composent
/// INDÉPENDAMMENT (une ordonnance de référé OQTF porte à la fois
/// `procedure:REFERE_SUSPENSION` et `legal_domain:…_ETRANGERS_…`). Métadonnées
/// d'abord (label solution, type de recours, formation), puis signaux textuels du
/// scan ([`crate::scan::DocScan::procedure_signals`] — articles CJA compilés en
/// marqueurs, zones par tokens).
pub fn extract_procedure(d: &Decision) -> ProcedureUids {
    procedure_scanned(d, scan_doc(d).as_ref())
}

pub fn procedure_scanned(d: &Decision, scan: Option<&crate::scan::DocScan>) -> ProcedureUids {
    // texte vide (rare) : signaux à défaut, les fallbacks chambre restent
    let sig = scan.map(|s| s.procedure_signals()).unwrap_or_default();
    let chamber = lj_fold::fold_chars(&format!(
        "{} {}",
        d.chamber.as_deref().unwrap_or(""),
        d.formation.as_deref().unwrap_or("")
    ));
    // office/domaine : formation d'abord, texte ensuite — les offices
    // texte (premier président, JEX) sont judiciaires par construction
    let judiciaire = is_judiciaire(d);
    let mut out = procedure_from_chamber(d, &sig).unwrap_or_default();
    if out.office_uid.is_none() && out.legal_domain_uid.is_none() {
        if sig.hospi {
            out.legal_domain_uid = domaine_uid("CIVIL_DROIT_PERSONNES_FAMILLE").legal_domain_uid;
        } else if sig.retention {
            out.legal_domain_uid =
                domaine_uid("PUBLIC_DROIT_ETRANGERS_NATIONALITE").legal_domain_uid;
        } else if d.jurisdiction_type.as_deref() == Some("TCOM") && sig.proc_collective {
            out.legal_domain_uid =
                domaine_uid("COMMERCIAL_DROIT_ENTREPRISES_DIFFICULTE").legal_domain_uid;
        } else if sig.premier_president && judiciaire {
            out.office_uid = office_uid("PREMIER_PRESIDENT").office_uid;
        } else if sig.jex
            && d.jurisdiction_type.as_deref() == Some("TJ")
            && (sig.jex_saisie_immo || !chamber.contains("refere"))
        {
            // TJ seulement : en appel comme en cassation, « juge de
            // l'exécution » désigne le juge du jugement ATTAQUÉ
            out.office_uid = office_uid("JEX").office_uid;
        }
    }
    // A rétention or a psychiatric-care order is the JLD's, its appeal the premier
    // président's, whatever the domain the chain above has posed.
    if out.office_uid.is_none()
        && judiciaire
        && (sig.hospi || sig.retention || sig.premier_president)
    {
        out.office_uid = match d.jurisdiction_type.as_deref() {
            Some("CA") => office_uid("PREMIER_PRESIDENT").office_uid,
            Some("CC") if sig.premier_president => office_uid("PREMIER_PRESIDENT").office_uid,
            Some("TJ") if sig.hospi || sig.retention => office_uid("JLD").office_uid,
            _ => None,
        };
    }
    let voie = procedure_key(d, &sig, &chamber);
    // Axe office INDÉPENDANT du domaine : un OQTF juge unique porte domaine
    // étrangers ET office magistrat désigné — hors de la chaîne ci-dessus, qui
    // s'arrête au premier axe posé.
    //
    // TA SEULEMENT : MAGISTRAT_DESIGNE est réservé au juge unique de première
    // instance. En appel et en cassation la surface désigne le juge du jugement
    // ATTAQUÉ, et l'office reste vide.
    if out.office_uid.is_none()
        && d.jurisdiction_type.as_deref() == Some("TA")
        && (sig.magdes || sig.magdes_form_trib || voie.is_some_and(|k| k.starts_with("REFERE_")))
    {
        out.office_uid = office_uid("MAGISTRAT_DESIGNE").office_uid;
    }
    // CAA/CE : l'ordonnance signée « Le président désigné » / « Le conseiller d'État
    // désigné » est rendue par un juge unique désigné → MAGISTRAT_DESIGNE. Compte la
    // signature (`magdes_tail`), et pour la CAA la formule d'en-tête « le président
    // de la cour a désigné Mme X … » (R. 222-1 / juge des référés).
    if out.office_uid.is_none()
        && matches!(d.jurisdiction_type.as_deref(), Some("CAA") | Some("CE"))
        && (sig.magdes_tail
            || (d.jurisdiction_type.as_deref() == Some("CAA") && sig.magdes_form_cour))
    {
        out.office_uid = office_uid("MAGISTRAT_DESIGNE").office_uid;
    }
    // CA/TJ: an order signed by the judge of the mise en état is that judge's; one
    // signed « le magistrat chargé d'instruire l'affaire », a delegated magistrate's.
    // At the cour d'appel the header's « ordonnance du juge de la mise en état » is
    // the order under appeal.
    let jt = d.jurisdiction_type.as_deref();
    if out.office_uid.is_none() && matches!(jt, Some("CA") | Some("TJ")) {
        if sig.mee_signed || (jt == Some("TJ") && sig.mee_banner) {
            out.office_uid = office_uid("MISE_EN_ETAT").office_uid;
        } else if sig.magdes_tail {
            out.office_uid = office_uid("MAGISTRAT_DESIGNE").office_uid;
        }
    }
    // Raffinement domaine par le vocabulaire du texte (ordre admin) — porté
    // en HINT, pas posé : le vote citations garde la priorité (un code
    // substantiel cité bat un mot d'en-tête). Objet du litige avant statut
    // du requérant (un fonctionnaire qui attaque un refus de permis de
    // construire plaide de l'urbanisme).
    if out.legal_domain_uid.is_none() && !judiciaire {
        out.domain_hint = if sig.immig_anywhere {
            Some("PUBLIC_DROIT_ETRANGERS_NATIONALITE")
        } else if sig.dom_fisc {
            Some("FISCAL")
        } else if sig.dom_aide {
            Some("PUBLIC_DROIT_AIDE_ACTION_SOCIALE")
        } else if sig.dom_urba {
            Some("PUBLIC_DROIT_URBANISME_IMMOBILIER_PUBLIC")
        } else if sig.dom_env {
            Some("PUBLIC_DROIT_ENVIRONNEMENT")
        } else if sig.dom_penal_pub {
            Some("PUBLIC_DROIT_PENAL_PUBLIC")
        } else if sig.dom_etr {
            Some("PUBLIC_DROIT_ETRANGERS_NATIONALITE")
        } else if sig.dom_fp {
            Some("PUBLIC_DROIT_TRAVAIL")
        } else {
            None
        };
    }
    // An opinion speaks of procedures and judges without following one or being signed by one.
    if crate::formation::consultative(d) {
        out.office_uid = None;
    } else {
        out.procedure_uid = voie.map(|k| format!("procedure:{k}"));
    }
    out
}

/// Scan de marqueurs du document : texte clean tel quel — `Norm` mappe les blancs
/// 1:1, les offsets du scan sont ceux du texte (même espace de chars que les
/// citations compilées).
pub(crate) fn scan_doc(d: &Decision) -> Option<crate::scan::DocScan> {
    let raw = if !d.texte_integral_clean.is_empty() {
        d.texte_integral_clean.as_str()
    } else {
        d.texte_integral_raw.as_str()
    };
    if raw.is_empty() {
        return None;
    }
    Some(crate::scan::scan(raw))
}

/// Conseils (avocats + cabinets) : mêmes gabarits structurels que [`companies`],
/// sortie verbatim.
fn counsel(scan: Option<&crate::scan::DocScan>) -> crate::scan::CounselOut {
    scan.map(|s| s.counsel()).unwrap_or_default()
}

/// Opendata `avocat_requerant` names the applicant's side, the body being only a
/// fallback. The semicolon separates counsel and never belongs to a name.
fn avocat_requerant_meta(d: &Decision) -> Vec<(String, bool)> {
    let Some(brut) = d.avocat_requerant.as_deref() else {
        return Vec::new();
    };
    brut.split(';')
        .filter_map(|item| {
            let compact = common::normalize_spaces(item);
            (!compact.is_empty()).then(|| classe_conseil_meta(compact))
        })
        .collect()
}

/// Cabinet quand l'item ouvre ou finit sur une structure d'avocats, ou mentionne
/// « avocat »/« cabinet » ; personne sinon.
fn classe_conseil_meta(compact: String) -> (String, bool) {
    let f = lj_fold::fold_chars(&compact);
    // Structure en tête (« SCP X ») OU en queue (« DHALLUIN SCP ») : la
    // métadonnée écrit les deux ordres.
    let structural = |t: &str| {
        matches!(
            t,
            "scp"
                | "selarl"
                | "selarlu"
                | "seleurl"
                | "selas"
                | "selasu"
                | "selafa"
                | "selca"
                | "scm"
                | "aarpi"
                | "sarl"
                | "sas"
                | "sasu"
                | "sa"
                | "societe"
        )
    };
    let mut toks = f.split_whitespace();
    let first = toks.next().unwrap_or("");
    let last = toks.next_back().unwrap_or(first);
    let is_firm =
        structural(first) || structural(last) || f.contains("avocat") || f.contains("cabinet");
    (compact, is_firm)
}

/// Ébarbe une valeur counsel de sa queue d'apposition cabinet (« Jean-Claude
/// NEBOT de la SELASU NEBOT AVOCAT » → « Jean-Claude NEBOT ») et du titre nu
/// final (« Julie GALLAND … Avocat »). La structure reste captée par les
/// champs `*_law_firms` ; la valeur counsel est la personne seule.
pub(crate) fn trim_counsel_tail(v: String) -> String {
    static RE_FIRM_TAIL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static RE_TITLE_TAIL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re_firm = RE_FIRM_TAIL.get_or_init(|| {
        regex::Regex::new(
            r"(?i)\s(?:de la|de l'|du|de|-|–)\s.*?\b(?:scp|selarl|selasu|selas|selafa|seleurl|selarlu|selca|scm|aarpi|sas|sasu|sarl|avocats?|cabinet)\b",
        )
        .unwrap()
    });
    let re_title = RE_TITLE_TAIL.get_or_init(|| regex::Regex::new(r"(?i)\s+avocats?\s*$").unwrap());
    let norm = crate::compiled::Norm::new(&v);
    let cut = re_firm.find(&norm.folded).map(|m| m.start());
    let v = match cut {
        Some(at) => v.chars().take(norm.byte2char[at]).collect(),
        None => v,
    };
    let mut v = re_title.replace(&v, "").into_owned();
    // Ponctuation de bord — sans jamais mordre l'ellipse d'anonymisation
    // ancienne (« Me F... »).
    loop {
        let t = v.trim_end_matches([' ', ',', ';']);
        let t = match t.strip_suffix('.') {
            Some(rest) if !rest.ends_with('.') => rest,
            _ => t,
        };
        if t.len() == v.len() {
            break;
        }
        v.truncate(t.len());
    }
    v
}

/// Fusion métadonnée-d'abord : la valeur structurée ouvre la liste ; une
/// tranche texte dont tous les mots pliés figurent déjà dans la métadonnée
/// (« Tachon » face à « SCP WABLE TRUNECEK TACHON AUBRON ») est redondante.
/// Quand la tranche texte recouvre le MÊME nom entier (mêmes mots pliés) et
/// que la métadonnée est tout-CAPS, la tranche du corps en casse mixte prend
/// la place — les capitales ont détruit casse et accents.
fn merge_meta_first(metas: Vec<String>, text: Vec<String>) -> Vec<String> {
    if metas.is_empty() {
        return text;
    }
    let mots = |v: &str| -> Vec<String> {
        lj_fold::fold_chars(v)
            .split_whitespace()
            .map(str::to_string)
            .collect()
    };
    let mw: Vec<Vec<String>> = metas.iter().map(|m| mots(m)).collect();
    let mut out = metas;
    for n in text {
        let nw = mots(&n);
        match mw.iter().position(|m| nw.iter().all(|w| m.contains(w))) {
            Some(j) if nw.len() == mw[j].len() && common::better_cased(&out[j], &n) => out[j] = n,
            Some(_) => {}
            None => out.push(n),
        }
    }
    out
}

/// Ré-ancre chaque valeur counsel/firm à sa jumelle mieux cassée dans le texte —
/// champs PERSONNES uniquement : les companies gardent leur tranche d'origine
/// (« SA SOCIÉTÉ DES AUTOROUTES » reste tel quel), c'est le patronyme que
/// l'en-tête écrase en capitales.
fn recase_by_text(scan: Option<&crate::scan::DocScan>, values: Vec<String>) -> Vec<String> {
    let Some(s) = scan else { return values };
    values
        .into_iter()
        .map(|v| s.best_cased_twin(&v).unwrap_or(v))
        .collect()
}

pub fn extract_applicant_counsel_names(d: &Decision) -> Option<Vec<String>> {
    applicant_counsel_names_scanned(d, scan_doc(d).as_ref())
}

pub fn applicant_counsel_names_scanned(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<Vec<String>> {
    let metas = avocat_requerant_meta(d)
        .into_iter()
        .filter(|(_, firm)| !firm)
        .map(|(v, _)| v)
        .collect();
    let names = merge_meta_first(metas, counsel(scan).applicant_names)
        .into_iter()
        .map(trim_counsel_tail)
        .filter(|v| !common::is_anonymized_person(v))
        .collect();
    common::unique_nonempty(&recase_by_text(scan, names))
}

pub fn extract_applicant_law_firms(d: &Decision) -> Option<Vec<String>> {
    applicant_law_firms_scanned(d, scan_doc(d).as_ref())
}

pub fn applicant_law_firms_scanned(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<Vec<String>> {
    let metas = avocat_requerant_meta(d)
        .into_iter()
        .filter(|(_, firm)| *firm)
        .map(|(v, _)| v)
        .collect();
    let firms = merge_meta_first(metas, counsel(scan).applicant_firms)
        .into_iter()
        .filter(|v| !common::is_anonymized_firm(v))
        .collect();
    common::unique_nonempty(&recase_by_text(scan, firms))
}

pub fn extract_defendant_counsel_names(d: &Decision) -> Option<Vec<String>> {
    defendant_counsel_names_scanned(scan_doc(d).as_ref())
}

pub fn defendant_counsel_names_scanned(scan: Option<&crate::scan::DocScan>) -> Option<Vec<String>> {
    let names = counsel(scan)
        .defendant_names
        .into_iter()
        .map(trim_counsel_tail)
        .filter(|v| !common::is_anonymized_person(v))
        .collect();
    common::unique_nonempty(&recase_by_text(scan, names))
}

pub fn extract_defendant_law_firms(d: &Decision) -> Option<Vec<String>> {
    defendant_law_firms_scanned(scan_doc(d).as_ref())
}

pub fn defendant_law_firms_scanned(scan: Option<&crate::scan::DocScan>) -> Option<Vec<String>> {
    let firms = counsel(scan)
        .defendant_firms
        .into_iter()
        .filter(|v| !common::is_anonymized_firm(v))
        .collect();
    common::unique_nonempty(&recase_by_text(scan, firms))
}

/// The counsel the source lists without a side, `(names, firms)`, less those a
/// placed value already names: two values one of whose words all appear in the
/// other are the same counsel.
pub fn listed_counsel_scanned(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
    placed_names: &[String],
    placed_firms: &[String],
) -> (Vec<String>, Vec<String>) {
    let words = |v: &str| -> Vec<String> {
        lj_fold::slug(v, ' ')
            .split(' ')
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect()
    };
    let placed = |v: &str, among: &[String]| {
        let w = words(v);
        among.iter().map(|p| words(p)).any(|p| {
            !p.is_empty() && (p.iter().all(|x| w.contains(x)) || w.iter().all(|x| p.contains(x)))
        })
    };
    let (mut names, mut firms) = (Vec::new(), Vec::new());
    for item in &d.listed_counsel {
        match classe_conseil_meta(common::normalize_spaces(item)) {
            (firm, true) => {
                if !placed(&firm, placed_firms) && !common::is_anonymized_firm(&firm) {
                    firms.push(firm);
                }
            }
            (name, false) => {
                let name = trim_counsel_tail(name);
                if !placed(&name, placed_names) && !common::is_anonymized_person(&name) {
                    names.push(name);
                }
            }
        }
    }
    (
        common::unique_nonempty(&recase_by_text(scan, names)).unwrap_or_default(),
        common::unique_nonempty(&recase_by_text(scan, firms)).unwrap_or_default(),
    )
}

/// Personnes morales parties : gabarit (pivot CC / blocs / requête admin) détecté
/// dans le flux de tokens.
fn companies(scan: Option<&crate::scan::DocScan>) -> (Vec<String>, Vec<String>) {
    scan.map(|s| s.companies()).unwrap_or_default()
}

pub fn extract_applicant_companies(d: &Decision) -> Option<Vec<String>> {
    applicant_companies_scanned(scan_doc(d).as_ref())
}

pub fn applicant_companies_scanned(scan: Option<&crate::scan::DocScan>) -> Option<Vec<String>> {
    common::unique_nonempty(&common::dedupe_prefix_variants(companies(scan).0))
}

pub fn intervenors_scanned(scan: Option<&crate::scan::DocScan>) -> Option<Vec<String>> {
    common::unique_nonempty(&scan.map(|s| s.intervenors()).unwrap_or_default())
}

pub fn extract_defendant_companies(d: &Decision) -> Option<Vec<String>> {
    defendant_companies_scanned(scan_doc(d).as_ref())
}

pub fn defendant_companies_scanned(scan: Option<&crate::scan::DocScan>) -> Option<Vec<String>> {
    common::unique_nonempty(&common::dedupe_prefix_variants(companies(scan).1))
}

/// Parties personnes physiques : la source les pseudonymise, la valeur est la
/// tranche verbatim sans la civilité.
fn individuals(scan: Option<&crate::scan::DocScan>) -> (Vec<String>, Vec<String>) {
    use crate::registry::{Quality, Side};
    match scan {
        Some(s) => {
            let r = s.party_registry();
            (
                r.view(Some(Side::Applicant), Quality::Individual),
                r.view(Some(Side::Defendant), Quality::Individual),
            )
        }
        None => Default::default(),
    }
}

pub fn extract_applicant_individuals(d: &Decision) -> Option<Vec<String>> {
    applicant_individuals_scanned(scan_doc(d).as_ref())
}

pub fn applicant_individuals_scanned(scan: Option<&crate::scan::DocScan>) -> Option<Vec<String>> {
    common::unique_nonempty(&individuals(scan).0)
}

pub fn extract_defendant_individuals(d: &Decision) -> Option<Vec<String>> {
    defendant_individuals_scanned(scan_doc(d).as_ref())
}

pub fn defendant_individuals_scanned(scan: Option<&crate::scan::DocScan>) -> Option<Vec<String>> {
    common::unique_nonempty(&individuals(scan).1)
}

/// Codes de publication : métadonnée source jointe — un seul code = le code nu.
pub fn extract_publication_code(d: &Decision) -> Option<String> {
    if d.publication_codes.is_empty() {
        None
    } else {
        Some(d.publication_codes.join(","))
    }
}

#[cfg(test)]
mod generic_tests {
    use super::*;
    use lj_core::decision::Decision;

    fn decision(jt: Option<&str>) -> Decision {
        Decision {
            source_uid: "test".into(),
            member_name: "test".into(),
            ecli: None,
            jurisdiction_source_code: None,
            chamber: None,
            nac: None,
            jurisdiction_name: None,
            jurisdiction_type: jt.map(str::to_string),
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
            publication_codes: vec![],
            avocat_requerant: None,
            listed_counsel: Vec::new(),
            texte_integral_raw: String::new(),
            texte_integral_clean: String::new(),
            sections: vec![],
            metadata_header: String::new(),
            themes: Vec::new(),
            attacked: None,
            parse_warnings: vec![],
        }
    }

    #[test]
    fn routed_accepts_generic_juridictions() {
        for jt in ["CONSTIT", "TC", "CEDH", "CJUE", "CNDA"] {
            assert!(
                routed(&decision(Some(jt))).is_ok(),
                "{jt} doit être routé sans erreur"
            );
        }
    }

    #[test]
    fn generic_reads_already_parsed_docket_and_date_verbatim() {
        let mut d = decision(Some("CEDH"));
        d.numero_dossiers = Some(vec!["12345/06".into(), "678/07".into()]);
        d.date_lecture = Some("2020-01-15".into());
        d.publication_codes = vec!["B".into(), "P".into()];

        routed(&d).expect("CEDH routé");
        assert_eq!(
            extract_docket_numbers(&d),
            Some(vec!["12345/06".into(), "678/07".into()])
        );
        assert_eq!(extract_date_lecture(&d).as_deref(), Some("2020-01-15"));
        assert_eq!(extract_publication_code(&d).as_deref(), Some("B,P"));
        assert_eq!(extract_solution(&d), None);
        assert_eq!(extract_jurisdiction_name(&d), None);
    }

    // Une citation « code … <gentilé étranger> » (droit du pays d'origine, fréquent
    // en asile CNDA) ne doit PAS se replier sur le code FR homonyme : le nom reste
    // distinct, donc non résolu vers LEGI. Les codes FR et les conventions
    // internationales restent intacts.
    #[test]
    fn foreign_code_does_not_collapse_to_french_homonym() {
        use crate::extract::normalize_instrument;
        assert_ne!(normalize_instrument("code pénal iranien"), "Code pénal");
        assert_ne!(normalize_instrument("code  pénal  iranien"), "Code pénal");
        assert_ne!(normalize_instrument("code civil ivoirien"), "Code civil");
        assert_ne!(
            normalize_instrument("code de justice militaire congolais"),
            "Code de justice militaire"
        );
        assert_ne!(normalize_instrument("code pénal bangladais"), "Code pénal");
        assert_ne!(
            normalize_instrument("code de la famille albanais"),
            "Code de la famille"
        );
        assert_ne!(
            normalize_instrument("code de la nationalité algérienne"),
            "Code de la nationalité"
        );
        assert_eq!(normalize_instrument("code pénal"), "Code pénal");
        assert_eq!(normalize_instrument("code civil"), "Code civil");
        assert_eq!(normalize_instrument("code de commerce"), "Code de commerce");
        assert_eq!(
            normalize_instrument("code du travail maritime"),
            "Code du travail maritime"
        );
        assert_eq!(
            normalize_instrument("code de l'entrée et du séjour des étrangers et du droit d'asile"),
            "Code de l'entrée et du séjour des étrangers et du droit d'asile"
        );
        assert_eq!(
            normalize_instrument("convention de Genève"),
            "Convention de Genève"
        );
    }

    #[test]
    fn unknown_juridiction_still_errors() {
        match routed(&decision(Some("BIDON"))) {
            Err(lj_core::error::CoreError::UnknownJuridiction(Some(t))) => assert_eq!(t, "BIDON"),
            _ => panic!("type bidon doit lever UnknownJuridiction"),
        }
    }
}
