//! Facette domaine juridique dérivée du PROFIL DES CODES CITÉS (ADR 0156 :
//! sur-exploitation du flux de citations liées). Vote pondéré : chaque
//! citation résolue vers un texte SUBSTANTIEL vote pour un domaine — les
//! codes procéduraux/génériques (CPC, CJA, COJ, CRPA) et les textes
//! transverses (CEDH, Constitution, loi aide juridique) ne votent pas. Les
//! codes multi-domaines (code civil, code de commerce, CPI) votent par plage
//! d'articles.
//!
//! Vocabulaire de sortie : suffixes d'uids `domaine:*` du référentiel
//! `facet_value` (taxonomie gold, ADR 0146/0148).

use std::collections::HashMap;

/// Contexte de classement : ordre + coloration de la formation (chambre
/// sociale / prud'hommes, chambre commerciale / tribunal de commerce) — les
/// mêmes articles du code civil colorent différemment selon la chambre.
#[derive(Debug, Clone, Copy, Default)]
pub struct DomainContext {
    pub admin: bool,
    pub social: bool,
    pub commercial: bool,
    /// Raffinement TEXTE pour l'ordre admin ([`crate::extract::ProcedureUids::
    /// domain_hint`]) : ne s'applique que si aucun code substantiel ne vote —
    /// les contentieux FP/aide sociale/urbanisme ne citent souvent que le CJA.
    pub hint: Option<&'static str>,
}

/// Codes procéduraux/génériques : présence = contentieux devant l'ordre,
/// pas un domaine — mais leur seule présence autorise le fallback PUBLIC.
const GENERIC_ADMIN: &[&str] = &["LEGITEXT000006070933", "LEGITEXT000031366350"];

/// Domaine (`domaine:<KEY>`) déduit des citations liées d'une décision —
/// paires `(ref_text_uid, num_key)` — et du contexte de juridiction.
/// `None` quand aucun texte substantiel ne vote : on ne devine pas.
pub fn legal_domain_uid<'a>(
    cites: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    ctx: DomainContext,
) -> Option<String> {
    let mut votes: HashMap<&'static str, f64> = HashMap::new();
    let mut generic_admin = false;
    for (uid, num_key) in cites {
        generic_admin |= GENERIC_ADMIN.contains(&uid);
        if let Some((domain, weight)) = domain_vote(uid, num_key, ctx.admin) {
            *votes.entry(domain).or_default() += weight;
        }
    }
    let winner = votes
        .into_iter()
        // départage déterministe : poids, puis ordre lexical inverse (stable).
        .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(domain, _)| domain);
    // La chambre colore les votes génériques du code civil : les obligations
    // devant la chambre commerciale sont du contentieux commercial, devant
    // la chambre sociale du contrat de travail.
    let winner = match winner {
        Some("CIVIL_DROIT_RESPONSABILITE_CONTRATS" | "CIVIL") if ctx.commercial => {
            Some("COMMERCIAL_DROIT_CONTRATS")
        }
        Some("CIVIL_DROIT_RESPONSABILITE_CONTRATS" | "CIVIL") if ctx.social => {
            Some("SOCIAL_DROIT_TRAVAIL")
        }
        None if ctx.social => Some("SOCIAL_DROIT_TRAVAIL"),
        None if ctx.commercial => Some("COMMERCIAL"),
        // Vote muet ou générique en admin : le vocabulaire du texte raffine.
        None | Some("PUBLIC") if ctx.admin && ctx.hint.is_some() => ctx.hint,
        // Contentieux administratif sans texte substantiel : générique.
        None if ctx.admin && generic_admin => Some("PUBLIC"),
        w => w,
    };
    winner.map(|domain| format!("domaine:{domain}"))
}

/// Vote d'une citation : domaine + poids. Poids 1.0 = signal univoque ;
/// 0.5 = plage par défaut d'un code multi-domaines (cédera face à un
/// signal fort du même document).
fn domain_vote(uid: &str, num_key: Option<&str>, admin: bool) -> Option<(&'static str, f64)> {
    let strong = |d: &'static str| Some((d, 1.0));
    match uid {
        // ── codes mono-domaine ──────────────────────────────────────────
        // CESEDA : poids double — les contentieux rétention (JLD) citent
        // massivement le CPP sans cesser d'être du droit des étrangers.
        "LEGITEXT000006070158" => Some(("PUBLIC_DROIT_ETRANGERS_NATIONALITE", 2.0)),
        "LEGITEXT000006069577" | "LEGITEXT000006069583" => strong("FISCAL"), // CGI, LPF
        "LEGITEXT000006071154" | "LEGITEXT000006070719" => strong("CRIMINEL"), // CPP, CP
        "LEGITEXT000006072050" => strong(if admin {
            "PUBLIC_DROIT_TRAVAIL"
        } else {
            "SOCIAL_DROIT_TRAVAIL"
        }), // code du travail
        "LEGITEXT000044416551" => strong("PUBLIC_DROIT_TRAVAIL"),            // CGFP
        "LEGITEXT000006074075" => strong("PUBLIC_DROIT_URBANISME_IMMOBILIER_PUBLIC"), // urbanisme
        // CCH : le contentieux DALO / attribution de logement social
        // (L. 300-1, L. 441-2-3) est de l'aide sociale, pas de la construction
        "LEGITEXT000006074096" => match head_num(num_key) {
            Some(300..=302 | 441) => strong(if admin {
                "PUBLIC_DROIT_AIDE_ACTION_SOCIALE"
            } else {
                "SOCIAL_DROIT_AIDE_ACTION_SOCIALE"
            }),
            _ => strong("PUBLIC_DROIT_URBANISME_IMMOBILIER_PUBLIC"),
        },
        "LEGITEXT000006074220" => strong("PUBLIC_DROIT_ENVIRONNEMENT"),
        "LEGITEXT000006073189" => strong("SOCIAL_DROIT_AIDE_ACTION_SOCIALE"), // CSS
        "LEGITEXT000006074069" => strong(if admin {
            "PUBLIC_DROIT_AIDE_ACTION_SOCIALE"
        } else {
            "SOCIAL_DROIT_AIDE_ACTION_SOCIALE"
        }), // CASF
        "LEGITEXT000025024948" => Some(cpce_vote(num_key)),
        "LEGITEXT000006073984" => strong("CIVIL_DROIT_ASSURANCES"),
        "LEGITEXT000006069565" => strong("COMMERCIAL_DROIT_CONSOMMATION"),
        "LEGITEXT000006072026" => strong("CIVIL_DROIT_BANCAIRE_BOURSIER"), // CMF
        // CSP : ordre admin = santé publique (PUBLIC) ; ordre judiciaire =
        // soins sans consentement devant le JLD (personnes).
        "LEGITEXT000006072665" => strong(if admin {
            "PUBLIC"
        } else {
            "CIVIL_DROIT_PERSONNES_FAMILLE"
        }),
        // ── lois spéciales ──────────────────────────────────────────────
        "JORFTEXT000000509310" => strong("CIVIL_DROIT_LOCATIF"), // loi 89-462
        "JORFTEXT000000880200" => {
            strong("CIVIL_DROIT_COPROPRIETE_PROPRIETE_IMMOBILIERE") // loi 65-557
        }
        // Statuts de la fonction publique (Le Pors, FPE, FPT, FPH).
        "JORFTEXT000000504704"
        | "JORFTEXT000000501099"
        | "JORFTEXT000000320434"
        | "JORFTEXT000000512459" => strong("PUBLIC_DROIT_TRAVAIL"),
        // ── codes multi-domaines : plage d'articles ─────────────────────
        "LEGITEXT000006070721" => Some(code_civil_vote(num_key, admin)), // code civil
        "LEGITEXT000005634379" => Some(code_commerce_vote(num_key)),     // code de commerce
        "LEGITEXT000006069414" => Some(code_pi_vote(num_key)),           // CPI
        _ => None,
    }
}

/// Premier segment numérique d'un `num_key` (« L. 145-41 » → 145).
fn head_num(num_key: Option<&str>) -> Option<u32> {
    let nk = num_key?;
    let digits: String = nk
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn cpce_vote(num_key: Option<&str>) -> (&'static str, f64) {
    match head_num(num_key) {
        // saisie immobilière (livre III)
        Some(311..=341) => ("CIVIL_DROIT_SAISIE_IMMOBILIERE", 1.0),
        // expulsion (livre IV)
        Some(411..=451) => ("CIVIL_DROIT_LOCATIF", 1.0),
        _ => ("CIVIL_PROCEDURES_CIVILES_EXECUTION", 1.0),
    }
}

fn code_civil_vote(num_key: Option<&str>, admin: bool) -> (&'static str, f64) {
    let Some(n) = head_num(num_key) else {
        return ("CIVIL", 0.5);
    };
    // Nationalité (17-33 bis) : devant le juge administratif, c'est le
    // contentieux de la naturalisation.
    if admin && (17..=33).contains(&n) {
        return ("PUBLIC_DROIT_ETRANGERS_NATIONALITE", 1.0);
    }
    match n {
        // mariage, filiation, autorité parentale, PACS, régimes matrimoniaux
        16..=228 | 311..=515 | 1387..=1581 => ("CIVIL_DROIT_PERSONNES_FAMILLE", 1.0),
        229..=310 => ("CIVIL_DIVORCE_SEPARATION_CORPS", 1.0),
        // biens, servitudes, mitoyenneté
        516..=710 => ("CIVIL_DROIT_COPROPRIETE_PROPRIETE_IMMOBILIERE", 1.0),
        // successions et libéralités
        711..=1099 => ("CIVIL_DROIT_SUCCESSIONS", 1.0),
        // responsabilité extracontractuelle (1240-1252 + anciens 1382-1386)
        1240..=1252 | 1382..=1386 => ("CIVIL_DROIT_RESPONSABILITE", 1.0),
        // obligations, contrats, vente, régime général
        1100..=1239 | 1253..=1381 | 1582..=1707 | 1800..=2010 => {
            ("CIVIL_DROIT_RESPONSABILITE_CONTRATS", 1.0)
        }
        // louage d'ouvrage et d'industrie : construction (décennale 1792)
        1779..=1799 => ("CIVIL_DROIT_IMMOBILIER_CONSTRUCTION", 1.0),
        // louage de choses
        1708..=1778 => ("CIVIL_DROIT_LOCATIF", 1.0),
        _ => ("CIVIL", 0.5),
    }
}

fn code_commerce_vote(num_key: Option<&str>) -> (&'static str, f64) {
    let Some(n) = head_num(num_key) else {
        return ("COMMERCIAL", 0.5);
    };
    match n {
        145 => ("CIVIL_DROIT_LOCATIF", 1.0), // bail commercial
        210..=252 => ("COMMERCIAL_DROIT_SOCIETES", 1.0),
        420..=490 => ("COMMERCIAL_DROIT_CONCURRENCE", 1.0),
        610..=696 => ("COMMERCIAL_DROIT_ENTREPRISES_DIFFICULTE", 1.0),
        _ => ("COMMERCIAL", 0.5),
    }
}

fn code_pi_vote(num_key: Option<&str>) -> (&'static str, f64) {
    match head_num(num_key) {
        Some(111..=343) => ("PROPRIETE_INTELLECTUELLE_LITTERAIRE_ARTISTIQUE", 1.0),
        Some(411..=799) => ("PROPRIETE_INTELLECTUELLE_INDUSTRIELLE", 1.0),
        _ => ("PROPRIETE_INTELLECTUELLE", 0.5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceseda_wins_over_procedural_codes() {
        // CJA/CPC ne votent pas : une seule citation CESEDA suffit.
        let cites = vec![
            ("LEGITEXT000006070933", Some("L. 761-1")),
            ("LEGITEXT000006070158", Some("L. 611-1")),
        ];
        let ctx = DomainContext {
            admin: true,
            ..Default::default()
        };
        assert_eq!(
            legal_domain_uid(cites, ctx).as_deref(),
            Some("domaine:PUBLIC_DROIT_ETRANGERS_NATIONALITE")
        );
    }

    #[test]
    fn code_travail_splits_by_order() {
        let cites = || vec![("LEGITEXT000006072050", Some("L. 1234-1"))];
        let admin = |a| DomainContext {
            admin: a,
            ..Default::default()
        };
        assert_eq!(
            legal_domain_uid(cites(), admin(false)).as_deref(),
            Some("domaine:SOCIAL_DROIT_TRAVAIL")
        );
        assert_eq!(
            legal_domain_uid(cites(), admin(true)).as_deref(),
            Some("domaine:PUBLIC_DROIT_TRAVAIL")
        );
    }

    #[test]
    fn code_civil_ranges_vote_and_strong_beats_default() {
        // 1240 (responsabilité) + deux plages par défaut : le fort gagne.
        let cites = vec![
            ("LEGITEXT000006070721", Some("2224")),
            ("LEGITEXT000006070721", Some("1240")),
        ];
        assert_eq!(
            legal_domain_uid(cites, DomainContext::default()).as_deref(),
            Some("domaine:CIVIL_DROIT_RESPONSABILITE")
        );
    }

    #[test]
    fn generic_admin_only_falls_back_to_public() {
        // CJA seul : pas de domaine substantiel — fallback PUBLIC en admin,
        // rien en judiciaire.
        let cites = || vec![("LEGITEXT000006070933", Some("R. 222-1"))];
        let ctx = DomainContext {
            admin: true,
            ..Default::default()
        };
        assert_eq!(
            legal_domain_uid(cites(), ctx).as_deref(),
            Some("domaine:PUBLIC")
        );
        assert_eq!(legal_domain_uid(cites(), DomainContext::default()), None);
    }
}
