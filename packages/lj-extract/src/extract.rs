//! Extraction unifiée des champs structurés (ADR 0157) : fonctions plates
//! `extract_*` sur `Decision`, un document = un scan de marqueurs
//! (`crate::scan`) + un automate citations (`crate::compiled`). Plus de trait
//! par source : le gabarit se détecte dans le flux de tokens, la garde
//! [`routed`] ne sert qu'aux call-sites qui skippent les fonds hors
//! nomenclature. Les helpers partagés vivent dans `common`.
//!
//! Note d'implémentation : le crate `regex` (v1) ne supporte pas les
//! lookaround (`(?=`, `(?!`, `(?<`). Toutes les bornes de capture qui s'en
//! servent en Python sont réécrites en Rust soit par une borne consommée
//! explicite, soit par un post-filtrage manuel sur les positions de match.

/// Helpers partagés (normalisation articles/instruments/dates/avocats),
/// port de `extract/common.py`. `pub(crate)` — consommés par les
/// implémentations par famille (`opendata` / `judilibre`), transitoires.
pub(crate) mod common;

/// Normaliseurs canoniques d'instrument / article (`_normalize_instrument`,
/// `_normalize_article`), exposés pour que le banc d'éval apparie les paires
/// `legal_references` exactement comme le scoring Python (`eval/metrics.py`),
/// au lieu d'une réduction allégée divergente.
pub use common::{is_unresolvable_instrument, normalize_article, normalize_instrument};

/// Signaux structurés d'une clé de citation (ADR 0144) — consommés par
/// l'écrivain `citation_key` (lj-ingest → lj-store) et le résolveur.
pub use common::key_signals;

use lj_core::decision::Decision;
use lj_core::error::Result;

/// Version du pipeline d'extraction de champs, stockée par décision
/// (`decisions.extract_version`, ADR 0083). À incrémenter à chaque changement
/// de comportement des extracteurs : un `reextract-fields` ne re-parse alors
/// que les décisions dont la version stockée diffère (reprise après
/// interruption incluse). NULL en base = extrait avant le versionnage.
///
/// La constante elle-même vit dans `lj-core` (ancêtre commun de `lj-extract`,
/// qui la produit, et `lj-store`, qui en gate le SQL sans tirer `lj-extract` —
/// ADR 0123 §3) ; re-exportée ici pour le chemin stable `extract::EXTRACT_VERSION`.
///
/// v1 : display d'instrument brut fidèle, clé interne normalisée (ADR 0079).
/// v2 : gardes labels (run-ons, anaphores, tiret-préfixe) + fenêtre
///      anaphorique qui replie les blocs cités.
/// v3 : repli du numéro daté après strip d'article (normalisation idempotente
///      sur « la loi n° X du <date> »).
/// v4 : extracteurs Judilibre clean-first (ADR 0085) — `main_outcome` (scans CA/
///      CC/TJ/TCOM) et `joined_pourvois` (docket CC) lisent `texte_integral_clean`
///      au lieu de `_raw`, pour une extraction stable après le drop de
///      `source_payload` (reconstruction depuis `(full_text, source_fields)`).
/// v5 : `visa → legal_refs` étendu à toutes les juridictions Judilibre (ADR
///      0091) — le gating CC-only est levé, les CA/TJ/TCOM portant une section
///      `visa` voient ses refs fusionnées aux refs texte (sur-ensemble).
pub use lj_core::EXTRACT_VERSION;

mod judilibre;
mod opendata;

/// Uids `voie:*` / `office:*` / `domaine:*` détectés par la décomposition
/// procédurale AU scanner (ADR 0148 : l'ex-`special_procedure` n'existe plus,
/// chaque détection route directement vers son axe référentiel). Chaque champ
/// est un uid complet de `facet_value` (FK) ou `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcedureUids {
    pub voie_uid: Option<String>,
    pub office_uid: Option<String>,
    pub legal_domain_uid: Option<String>,
    /// Raffinement domaine lu dans le TEXTE (vocabulaire d'en-tête admin),
    /// à passer au vote citations ([`crate::domain::DomainContext::hint`])
    /// quand `legal_domain_uid` est `None` : il ne s'applique que si aucun
    /// code substantiel ne vote.
    pub domain_hint: Option<&'static str>,
}

/// Extraction unifiée (ADR 0157) : fonctions plates par champ, plus de trait
/// ni de dispatch par source. `None` = champ absent/inconnu ; les champs de
/// facettes sortent en **uids complets des référentiels** (ADR 0148, v12) :
/// [`extract_solution`] → `solution:*`-17, [`extract_procedure`] →
/// voie/office/domaine. Aucun vocabulaire intermédiaire.
///
/// Le routage `Family` ci-dessous est TRANSITOIRE (étapes 4-5/8 du plan ADR
/// 0157) : les champs pas encore migrés au scan compilé (`crate::scan`)
/// appellent leur fallback texte legacy par famille de source ; il meurt
/// champ par champ. Les champs déjà unifiés (companies) n'y passent plus.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    /// Ordre administratif opendata (TA/CAA/CE).
    Admin,
    /// Ordre judiciaire Judilibre (CC/CA/TJ/TCOM).
    Judiciaire,
    /// Familles sans pipeline texte dédié (CONSTIT/TC/CEDH/CJUE/CNDA) :
    /// champs métadonnées + citations texte.
    Generic,
}

fn family(d: &Decision) -> Option<Family> {
    match d.juridiction_type.as_deref() {
        Some("TA") | Some("CAA") | Some("CE") => Some(Family::Admin),
        Some("CC") | Some("CA") | Some("TJ") | Some("TCOM") => Some(Family::Judiciaire),
        Some("CONSTIT") | Some("TC") | Some("CEDH") | Some("CJUE") | Some("CNDA") => {
            Some(Family::Generic)
        }
        _ => None,
    }
}

/// Hors nomenclature = extraction générique (métadonnées) : un fond scrapé
/// inconnu dégrade en champs `Decision`, jamais en erreur dure.
fn fam(d: &Decision) -> Family {
    family(d).unwrap_or(Family::Generic)
}

/// Ordre judiciaire (fonds Judilibre) ? Consommé par le vote domaine
/// ([`crate::domain::DomainContext::admin`] = tout le reste).
pub fn is_judiciaire(d: &Decision) -> bool {
    fam(d) == Family::Judiciaire
}

/// Garde de routage (ex-`get_extractors`) : erreur `UnknownJuridiction` si la
/// juridiction n'est pas l'un des ordres FR connus. Consommée par les
/// call-sites qui SKIPPENT les fonds hors nomenclature (canonical_ref,
/// backfill/resplit, banc) ; les fonctions d'extraction elles-mêmes traitent
/// l'inconnu comme [`Family::Generic`].
pub fn routed(decision: &Decision) -> Result<()> {
    family(decision).map(|_| ()).ok_or_else(|| {
        lj_core::error::CoreError::UnknownJuridiction(decision.juridiction_type.clone())
    })
}

pub fn extract_docket_numbers(d: &Decision) -> Option<Vec<String>> {
    docket_numbers_scanned(d, scan_doc(d).as_ref())
}

pub fn docket_numbers_scanned(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<Vec<String>> {
    match fam(d) {
        Family::Admin => opendata::extract_docket_numbers(d, scan),
        Family::Judiciaire => judilibre::extract_docket_numbers(d, scan),
        Family::Generic => d.numero_dossiers.clone(),
    }
}

pub fn extract_date_lecture(d: &Decision) -> Option<String> {
    match fam(d) {
        Family::Admin => opendata::extract_date_lecture(d),
        Family::Judiciaire => judilibre::extract_date_lecture(d),
        // Verbatim : pas de re-parse FR fragile (#12).
        Family::Generic => d.date_lecture.clone(),
    }
}

pub fn extract_date_audience(d: &Decision) -> Option<String> {
    date_audience_scanned(d, scan_doc(d).as_ref())
}

pub fn date_audience_scanned(d: &Decision, scan: Option<&crate::scan::DocScan>) -> Option<String> {
    match fam(d) {
        Family::Admin => opendata::extract_date_audience(d, scan),
        Family::Judiciaire => judilibre::extract_date_audience(d, scan),
        Family::Generic => None,
    }
}

pub fn extract_formation_or_chamber(d: &Decision) -> Option<String> {
    formation_or_chamber_scanned(d, scan_doc(d).as_ref())
}

pub fn formation_or_chamber_scanned(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<String> {
    match fam(d) {
        Family::Admin => opendata::extract_formation_or_chamber(d),
        Family::Judiciaire => {
            let bandeau = scan.map(|s| s.bandeau_text());
            judilibre::extract_formation_or_chamber(d, bandeau.as_deref())
        }
        Family::Generic => None,
    }
}

/// Formation structurée (ADR 0170) : mêmes entrées source que
/// [`formation_or_chamber_scanned`] — code chambre CC, chambre de bandeau
/// Judilibre, formation greffe — décomposées en axes au lieu d'être aplaties.
pub fn formation_axes_scanned(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> crate::formation::FormationAxes {
    let jt = d.juridiction_type.as_deref();
    match fam(d) {
        Family::Admin => {
            crate::formation::parse_formation(jt, None, None, None, d.formation.as_deref())
        }
        Family::Judiciaire => {
            if jt == Some("CC") {
                return crate::formation::parse_formation(
                    jt,
                    d.juridiction_code.as_deref(),
                    None,
                    None,
                    d.formation.as_deref(),
                );
            }
            // Le champ source `chamber` (texte libre CA/TJ/TCOM, porté par
            // juridiction_code) prime ; le bandeau scanné complète les axes
            // (spécialisation quand le greffe ne donne que la position).
            let chamber = d.juridiction_code.as_deref().filter(|c| !c.is_empty());
            let bandeau = scan
                .map(|s| s.bandeau_text())
                .as_deref()
                .and_then(judilibre::chamber_from_body);
            crate::formation::parse_formation(
                jt,
                None,
                chamber,
                bandeau.as_deref(),
                d.formation.as_deref(),
            )
        }
        Family::Generic => crate::formation::FormationAxes::default(),
    }
}

pub fn extract_jurisdiction_name(d: &Decision) -> Option<String> {
    jurisdiction_name_scanned(d, scan_doc(d).as_ref())
}

pub fn jurisdiction_name_scanned(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<String> {
    match fam(d) {
        Family::Admin => opendata::extract_jurisdiction_name(d),
        Family::Judiciaire => {
            let header = scan.map(|s| s.header_text());
            judilibre::extract_jurisdiction_name(d, header.as_deref())
        }
        Family::Generic => None,
    }
}

/// Label solution du greffe → clé solution-17. Vocabulaire FERMÉ (codes
/// Judilibre + libellés opendata + libellés TJ/TCOM), comparaisons en
/// minuscules pliées — aucune regex (ADR 0157 §4/§6).
fn solution_from_label(low: &str) -> Option<&'static str> {
    // codes Judilibre exacts
    if let Some(k) = match low {
        "rejet" => Some("REJET"),
        "cassation" | "annulation" => Some("SATISFACTION_TOTALE"),
        "cassation_partielle" => Some("SATISFACTION_PARTIELLE"),
        "irrecevabilite" | "irrecevabilité" => Some("IRRECEVABILITE"),
        "non-admission" | "non admission" => Some("IRRECEVABILITE"),
        "non-lieu" | "nonlieu" => Some("NON_LIEU_A_STATUER"),
        "qpc_non-lieu" => Some("NON_LIEU_A_STATUER"),
        "désistement" | "desistement" => Some("DESISTEMENT"),
        "designation" | "décheance" | "decheance" | "qpc_renvoi" | "qpc" | "renvoi" => {
            Some("AUTRE")
        }
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
        ("expulsion", "SATISFACTION_TOTALE"),
        ("prononce le divorce", "SATISFACTION_TOTALE"),
        ("renvoi avec ordonnance de clôture", "AUTRE"),
        ("renvoi à la mise en état", "AUTRE"),
        ("statue sur un incident", "AUTRE"),
        ("mee :", "AUTRE"),
        ("ne dessaisissant pas", "AUTRE"),
        ("se dessaisit", "AUTRE"),
        ("dessaisi au profit", "AUTRE"),
        // demande de maintien accueillie (JLD rétention/soins) = le
        // demandeur obtient la mesure — école gold : SATISFACTION_TOTALE
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
    // gagné = cassé — la conversion cassation (cc) rend CASSATION.
    if low.contains("renvoi") && low.contains("cassation") {
        return Some("SATISFACTION_TOTALE");
    }
    if low.contains("incompétence") || low.contains("incompetence") {
        // « Rejet - incompétence » : le dispositif rejette (verbatim) — le
        // déclinatoire pur reste neutre.
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

fn solution_uid(key: &str) -> String {
    format!("solution:{key}")
}

/// Solution (uid `solution:*`-17) — UNIFIÉ (ADR 0157) : le label du greffe
/// (métadonnée) fait foi via un vocabulaire fermé ; le texte n'est que
/// fallback (composeur [`crate::scan::DocScan::outcome`] sur la ZONE
/// dispositif), sauf détections qui PRIMENT (désistement constaté en cours
/// d'instance, pourvoi non admis). Vocabulaire de sortie par gabarit :
/// pourvoi en cassation → gagner = casser.
pub fn extract_solution(d: &Decision) -> Option<String> {
    solution_scanned(d, scan_doc(d).as_ref())
}

pub fn solution_scanned(d: &Decision, scan: Option<&crate::scan::DocScan>) -> Option<String> {
    let text = scan.and_then(|s| s.outcome());
    // vocabulaire cassation : la juridiction (métadonnée) fait foi, le
    // gabarit texte rattrape les fonds sans métadonnée ; le pourvoi CE
    // (gabarit Admin sans pivot judiciaire) se signale par « pourvoi »
    // en en-tête
    let cc = d.juridiction_type.as_deref() == Some("CC")
        || scan.is_some_and(|s| s.gabarit_cc())
        || (d.juridiction_type.as_deref() == Some("CE")
            && scan.is_some_and(|s| s.header_has_pourvoi()));
    let admin = !cc && fam(d) == Family::Admin;
    let raw = d.solution.as_deref().unwrap_or("").trim().to_lowercase();
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
                _ => "IRRECEVABILITE",
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
                    "SATISFACTION_PARTIELLE" => "CASSATION_PARTIELLE",
                    other => other,
                }
            } else if admin {
                // École gold, ordre administratif : le dispositif se lit
                // VERBATIM — un label d'annulation (totale OU partielle) =
                // ANNULATION (sauf dispositif « réformé ») ; le label greffe
                // ambigu (« satisfaction totale ou partielle »,
                // « irrecevabilité » sur dispositif « rejetée ») s'arbitre
                // au dispositif.
                match key {
                    "SATISFACTION_TOTALE" | "SATISFACTION_PARTIELLE" if raw.contains("annul") => {
                        match text {
                            Some(("REFORMATION", _)) => "REFORMATION",
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
                    other => other,
                }
            } else {
                key
            };
            return Some(solution_uid(key));
        }
    }
    if let Some((k, _)) = text {
        // Pourvoi (CC/CE) : annuler = casser — le gabarit Admin d'un pourvoi
        // CE rend ANNULATION, le vocabulaire de sortie est la cassation.
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
        legal_domain_uid: Some(format!("domaine:{key}")),
        ..Default::default()
    }
}

/// Le mot `w` apparaît-il en MOT ENTIER dans `blob` (déjà plié) ?
fn blob_word(blob: &str, w: &str) -> bool {
    blob.split(|c: char| !c.is_alphanumeric()).any(|x| x == w)
}

/// Voie/office/domaine depuis la FORMATION et le code juridiction
/// (métadonnées de greffe, vocabulaire fermé plié — aucune regex).
fn procedure_from_chamber(d: &Decision, sig: &crate::scan::ProcSignals) -> Option<ProcedureUids> {
    let blob = crate::compiled::fold_stable(&format!(
        "{} {}",
        d.juridiction_code.as_deref().unwrap_or(""),
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

/// La voie de recours (`voie:*`) — cascade métadonnées puis signaux texte.
fn voie_key(d: &Decision, sig: &crate::scan::ProcSignals, chamber: &str) -> Option<&'static str> {
    let sol = crate::compiled::fold_stable(d.solution.as_deref().unwrap_or(""));
    let tr = crate::compiled::fold_stable(d.type_recours.as_deref().unwrap_or(""));
    let jt = d.juridiction_type.as_deref();
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
    // filtrage cassation : seules les cours de cassation (CC/CE) en ont un ;
    // pas sur un désistement (le pourvoi s'interrompt, la voie n'est pas
    // la procédure d'admission) ; pas en chambre criminelle (la
    // non-admission art. 567-1-1 CPP n'est pas la PAPC civile — gold la
    // laisse sans voie)
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
    // formation de référé judiciaire (jamais pour l'ordre administratif)
    if chamber.contains("refere") && !matches!(jt, Some("TA") | Some("CAA") | Some("CE")) {
        return Some("REFERE_CIVIL");
    }
    None
}

/// Décomposition procédurale voie/office/domaine — UNIFIÉ (ADR 0157) : les
/// trois axes se composent INDÉPENDAMMENT (une ordonnance de référé OQTF
/// porte à la fois `voie:REFERE_SUSPENSION` et `domaine:…_ETRANGERS_…`).
/// Métadonnées d'abord (label solution, type de recours, formation), puis
/// signaux textuels du scan ([`crate::scan::DocScan::procedure_signals`] —
/// articles CJA compilés en marqueurs, zones par tokens).
pub fn extract_procedure(d: &Decision) -> ProcedureUids {
    procedure_scanned(d, scan_doc(d).as_ref())
}

pub fn procedure_scanned(d: &Decision, scan: Option<&crate::scan::DocScan>) -> ProcedureUids {
    // texte vide (rare) : signaux à défaut, les fallbacks chambre restent
    let sig = scan.map(|s| s.procedure_signals()).unwrap_or_default();
    let chamber = crate::compiled::fold_stable(&format!(
        "{} {}",
        d.juridiction_code.as_deref().unwrap_or(""),
        d.formation.as_deref().unwrap_or("")
    ));
    // office/domaine : formation d'abord, texte ensuite — les offices
    // texte (premier président, JEX) sont judiciaires par construction
    let judiciaire = matches!(
        d.juridiction_type.as_deref(),
        Some("CC") | Some("CA") | Some("TJ") | Some("TCOM")
    );
    let mut out = procedure_from_chamber(d, &sig).unwrap_or_default();
    if out.office_uid.is_none() && out.legal_domain_uid.is_none() {
        if sig.hospi {
            out.legal_domain_uid = domaine_uid("CIVIL_DROIT_PERSONNES_FAMILLE").legal_domain_uid;
        } else if sig.retention {
            out.legal_domain_uid =
                domaine_uid("PUBLIC_DROIT_ETRANGERS_NATIONALITE").legal_domain_uid;
        } else if d.juridiction_type.as_deref() == Some("TCOM") && sig.proc_collective {
            out.legal_domain_uid =
                domaine_uid("COMMERCIAL_DROIT_ENTREPRISES_DIFFICULTE").legal_domain_uid;
        } else if sig.premier_president && judiciaire {
            out.office_uid = office_uid("PREMIER_PRESIDENT").office_uid;
        } else if sig.jex
            && d.juridiction_type.as_deref() == Some("TJ")
            && (sig.jex_saisie_immo || !chamber.contains("refere"))
        {
            // TJ seulement : en appel comme en cassation, « juge de
            // l'exécution » désigne le juge du jugement ATTAQUÉ (gold CA :
            // 25 sans office pour 3 JEX)
            out.office_uid = office_uid("JEX").office_uid;
        }
    }
    let voie = voie_key(d, &sig, &chamber);
    // Axe office INDÉPENDANT du domaine (ADR 0157) : un OQTF juge unique
    // porte domaine étrangers ET office magistrat désigné — hors de la
    // chaîne ci-dessus, qui s'arrête au premier axe posé.
    //
    // TA SEULEMENT : l'école gold réserve MAGISTRAT_DESIGNE au juge unique
    // de première instance — « magistrat désigné » OQTF comme juge des
    // référés (GT : TA+référé 48/6 magdes, CE+référé 0/41). En appel et en
    // cassation la surface désigne le juge du jugement ATTAQUÉ (CAA 43
    // spurious / 9 gold), et gold laisse l'office vide.
    if out.office_uid.is_none()
        && d.juridiction_type.as_deref() == Some("TA")
        && (sig.magdes || voie.is_some_and(|k| k.starts_with("REFERE_")))
    {
        out.office_uid = office_uid("MAGISTRAT_DESIGNE").office_uid;
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
        } else if sig.dom_etr {
            Some("PUBLIC_DROIT_ETRANGERS_NATIONALITE")
        } else if sig.dom_fp {
            Some("PUBLIC_DROIT_TRAVAIL")
        } else {
            None
        };
    }
    out.voie_uid = voie.map(|k| format!("voie:{k}"));
    out
}

/// Scan de marqueurs du document (ADR 0156 volet 2 / ADR 0157) : texte
/// clean tel quel — `Norm` mappe les blancs 1:1, les offsets du scan sont
/// ceux du texte (même espace de chars que les citations compilées).
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

/// Conseils (avocats + cabinets) — UNIFIÉ : mêmes gabarits structurels que
/// [`companies`], sortie verbatim (plus de titlecase ni de recasse).
fn counsel(scan: Option<&crate::scan::DocScan>) -> crate::scan::CounselOut {
    scan.map(|s| s.counsel()).unwrap_or_default()
}

/// Métadonnée `avocat_requerant` (XML opendata) : fait foi côté requérant
/// (ADR 0157 §4, le texte n'est que fallback). Cabinet quand elle ouvre sur
/// une structure d'avocats ou mentionne « avocat »/« cabinet », personne
/// sinon — vocabulaire fermé, comparaison pliée.
fn avocat_requerant_meta(d: &Decision) -> Option<(String, bool)> {
    let compact = common::normalize_spaces(d.avocat_requerant.as_deref()?);
    if compact.is_empty() {
        return None;
    }
    let f = crate::compiled::fold_stable(&compact);
    let first = f.split_whitespace().next().unwrap_or("");
    let is_firm = matches!(
        first,
        "scp"
            | "selarl"
            | "selarlu"
            | "seleurl"
            | "selas"
            | "selafa"
            | "selca"
            | "scm"
            | "aarpi"
            | "sarl"
            | "sas"
            | "sasu"
            | "sa"
            | "societe"
    ) || f.contains("avocat")
        || f.contains("cabinet");
    Some((compact, is_firm))
}

/// Fusion métadonnée-d'abord : la valeur structurée ouvre la liste ; une
/// tranche texte dont tous les mots pliés figurent déjà dans la métadonnée
/// (« Tachon » face à « SCP WABLE TRUNECEK TACHON AUBRON ») est redondante.
fn merge_meta_first(meta: Option<String>, text: Vec<String>) -> Vec<String> {
    let Some(meta) = meta else { return text };
    let mw: Vec<String> = crate::compiled::fold_stable(&meta)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let mut out = vec![meta];
    for n in text {
        let redundant = crate::compiled::fold_stable(&n)
            .split_whitespace()
            .all(|w| mw.iter().any(|m| m == w));
        if !redundant {
            out.push(n);
        }
    }
    out
}

pub fn extract_applicant_counsel_names(d: &Decision) -> Option<Vec<String>> {
    applicant_counsel_names_scanned(d, scan_doc(d).as_ref())
}

pub fn applicant_counsel_names_scanned(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<Vec<String>> {
    let meta = avocat_requerant_meta(d)
        .filter(|(_, firm)| !firm)
        .map(|(v, _)| v);
    common::unique_nonempty(&merge_meta_first(meta, counsel(scan).applicant_names))
}

pub fn extract_applicant_law_firms(d: &Decision) -> Option<Vec<String>> {
    applicant_law_firms_scanned(d, scan_doc(d).as_ref())
}

pub fn applicant_law_firms_scanned(
    d: &Decision,
    scan: Option<&crate::scan::DocScan>,
) -> Option<Vec<String>> {
    let meta = avocat_requerant_meta(d)
        .filter(|(_, firm)| *firm)
        .map(|(v, _)| v);
    common::unique_nonempty(&merge_meta_first(meta, counsel(scan).applicant_firms))
}

pub fn extract_defendant_counsel_names(d: &Decision) -> Option<Vec<String>> {
    defendant_counsel_names_scanned(scan_doc(d).as_ref())
}

pub fn defendant_counsel_names_scanned(scan: Option<&crate::scan::DocScan>) -> Option<Vec<String>> {
    common::unique_nonempty(&counsel(scan).defendant_names)
}

pub fn extract_defendant_law_firms(d: &Decision) -> Option<Vec<String>> {
    defendant_law_firms_scanned(scan_doc(d).as_ref())
}

pub fn defendant_law_firms_scanned(scan: Option<&crate::scan::DocScan>) -> Option<Vec<String>> {
    common::unique_nonempty(&counsel(scan).defendant_firms)
}

/// Personnes morales parties — UNIFIÉ : gabarit (pivot CC / blocs / requête
/// admin) détecté dans le flux de tokens. Même chemin pour toutes les sources.
fn companies(scan: Option<&crate::scan::DocScan>) -> (Vec<String>, Vec<String>) {
    scan.map(|s| s.companies()).unwrap_or_default()
}

pub fn extract_applicant_companies(d: &Decision) -> Option<Vec<String>> {
    applicant_companies_scanned(scan_doc(d).as_ref())
}

pub fn applicant_companies_scanned(scan: Option<&crate::scan::DocScan>) -> Option<Vec<String>> {
    common::unique_nonempty(&common::dedupe_prefix_variants(companies(scan).0))
}

pub fn extract_defendant_companies(d: &Decision) -> Option<Vec<String>> {
    defendant_companies_scanned(scan_doc(d).as_ref())
}

pub fn defendant_companies_scanned(scan: Option<&crate::scan::DocScan>) -> Option<Vec<String>> {
    common::unique_nonempty(&common::dedupe_prefix_variants(companies(scan).1))
}

pub fn extract_publication_code(d: &Decision) -> Option<String> {
    match fam(d) {
        Family::Admin => opendata::extract_publication_code(d),
        Family::Judiciaire => judilibre::extract_publication_code(d),
        Family::Generic => d.publication_codes.first().cloned(),
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
            juridiction_code: None,
            juridiction_nom: None,
            juridiction_type: jt.map(str::to_string),
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
            publication_codes: vec![],
            avocat_requerant: None,
            texte_integral_raw: String::new(),
            texte_integral_clean: String::new(),
            sections: vec![],
            metadata_header: String::new(),
            visa_trim: String::new(),
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
        // Verbatim : pas de re-parse de la date.
        assert_eq!(extract_date_lecture(&d).as_deref(), Some("2020-01-15"));
        assert_eq!(extract_publication_code(&d).as_deref(), Some("B"));
        // Champs non portés → None (pas de crash, extraction minimale).
        assert_eq!(extract_solution(&d), None);
        assert_eq!(extract_jurisdiction_name(&d), None);
    }

    // Spec #35 / ADR 0102 §B : une citation « code … <gentilé étranger> » (droit du
    // pays d'origine, fréquent en asile CNDA) ne doit PAS se replier sur le code FR
    // homonyme — sinon faux lien Legifrance. Le nom reste distinct → non résolu vers
    // LEGI (extraction libre jusqu'à un référentiel étranger). Les codes FR et les
    // conventions internationales légitimes restent intacts.
    #[test]
    fn foreign_code_does_not_collapse_to_french_homonym() {
        use crate::extract::normalize_instrument;
        // Étranger (mesuré en prod CNDA) : NE doit PAS valoir le titre FR canonique.
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
        // Régression : codes FR et conventions intl légitimes inchangés.
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
