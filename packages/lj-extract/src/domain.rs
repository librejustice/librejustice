//! Facette domaine juridique dérivée du PROFIL DES CODES CITÉS (ADR 0156 :
//! sur-exploitation du flux de citations liées). Vote pondéré : chaque
//! citation résolue vers un texte SUBSTANTIEL vote pour un domaine — les
//! codes procéduraux/génériques (CPC, CJA, COJ, CRPA) et les textes
//! transverses (CEDH, Constitution, loi aide juridique) ne votent pas. Les
//! codes multi-domaines (code civil, code de commerce, CPI) votent par plage
//! d'articles.
//!
//! Vocabulaire de sortie : suffixes d'uids `legal_domain:*` du référentiel
//! `facet_value` (taxonomie gold, ADR 0146/0148).

use std::collections::HashMap;

/// Contexte de classement : ordre + coloration de la formation (chambre
/// sociale / prud'hommes, chambre commerciale / tribunal de commerce,
/// chambre criminelle) — les mêmes articles du code civil colorent
/// différemment selon la chambre.
#[derive(Debug, Clone, Copy, Default)]
pub struct DomainContext {
    pub admin: bool,
    pub social: bool,
    pub commercial: bool,
    pub criminal: bool,
    /// Chambre civile CC (`civ1`/`civ2`/`civ3`/`ordo`/`mi`) : plancher CIVIL
    /// quand rien ne vote — les désistements/stubs CC n'ont aucune matière
    /// au texte, la chambre est le seul signal (gold : +37/0).
    pub civil_chamber: bool,
    /// Raffinement TEXTE pour l'ordre admin ([`crate::extract::ProcedureUids::
    /// domain_hint`]) : ne s'applique que si aucun code substantiel ne vote —
    /// les contentieux FP/aide sociale/urbanisme ne citent souvent que le CJA.
    pub hint: Option<&'static str>,
}

/// Contexte dérivé de la décision : ordre + coloration de chambre, lue sur
/// le code Judilibre ([`Decision::chamber`] — `soc`, `comm`, `cr` à la CC),
/// les champs greffe bruts et les axes de formation structurés (ADR 0170 —
/// la chambre bandeau y entre via `chamber_uid`/`chamber_position`). Un
/// tribunal de commerce colore commercial par nature (gold : 103/112 TCOM
/// en parent COMMERCIAL).
///
/// [`Decision::chamber`]: lj_core::decision::Decision::chamber
pub fn context_for(
    d: &lj_core::decision::Decision,
    axes: &crate::formation::FormationAxes,
    hint: Option<&'static str>,
) -> DomainContext {
    let blob = crate::compiled::fold_stable(&format!(
        "{} {} {} {}",
        d.formation.as_deref().unwrap_or(""),
        d.chamber.as_deref().unwrap_or(""),
        axes.chamber_uid.unwrap_or(""),
        axes.chamber_position.as_deref().unwrap_or("")
    ));
    let code = d.chamber.as_deref().map(str::trim).unwrap_or("");
    let judiciaire = crate::extract::is_judiciaire(d);
    DomainContext {
        admin: !judiciaire,
        social: code == "soc" || blob.contains("social") || blob.contains("prud"),
        commercial: code == "comm"
            || d.jurisdiction_type.as_deref() == Some("TCOM")
            || blob.contains("commer"),
        // Coloration JUDICIAIRE seulement : CRIMINEL n'existe pas dans
        // l'ordre admin (extraditions CE : « correctionnel » au texte,
        // gold étrangers/PUBLIC — 25 erreurs, 0 correcte).
        criminal: judiciaire
            && (code == "cr" || blob.contains("criminel") || blob.contains("correctionnel")),
        civil_chamber: matches!(code, "civ1" | "civ2" | "civ3" | "ordo" | "mi"),
        hint,
    }
}

/// Codes procéduraux/génériques : présence = contentieux devant l'ordre,
/// pas un domaine — mais leur seule présence autorise le fallback PUBLIC.
const GENERIC_ADMIN: &[&str] = &["LEGITEXT000006070933", "LEGITEXT000031366350"];

/// Raffine par les votes de TERMES du scan ([`crate::scan::DocScan::
/// domain_term_votes`]) un domaine ABSENT ou parent nu (PUBLIC / CIVIL /
/// SOCIAL / COMMERCIAL) : gagnant à ≥ 2 occurrences ET ≥ 2× le second, même
/// parent que l'existant. Cas particulier : CIVIL_DROIT_RESPONSABILITE_
/// CONTRATS — la plage « obligations » par défaut du code civil — se
/// sur-classe à seuil RENFORCÉ (≥ 5 occurrences, ≥ 3× le second, parent
/// CIVIL ou COMMERCIAL) : un prêt qui cite 1103/1343 reste du bancaire.
/// Les autres sous-domaines posés par les codes cités sont intouchables.
/// Simulations gold 2026-07-08 : +75/−4 (parents nus), +24/−8 (RC_CONTRATS).
pub fn refine_with_terms(current: Option<String>, votes: &[(&'static str, u32)]) -> Option<String> {
    const PARENTS: &[&str] = &["PUBLIC", "CIVIL", "SOCIAL", "COMMERCIAL"];
    const SOFT_DEFAULT: &str = "CIVIL_DROIT_RESPONSABILITE_CONTRATS";
    let key = current
        .as_deref()
        .map(|u| u.strip_prefix("legal_domain:").unwrap_or(u));
    let soft = key == Some(SOFT_DEFAULT);
    if key.is_some_and(|k| !soft && !PARENTS.contains(&k)) {
        return current;
    }
    let mut sorted: Vec<_> = votes.to_vec();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let Some(&(winner, n)) = sorted.first() else {
        return current;
    };
    let second = sorted.get(1).map_or(0, |x| x.1);
    let (min_n, ratio) = if soft { (5, 3) } else { (2, 2) };
    if n < min_n || n < ratio * second {
        return current;
    }
    let parent_ok = match key {
        None => true,
        Some(SOFT_DEFAULT) => {
            winner != SOFT_DEFAULT
                && (winner.starts_with("CIVIL") || winner.starts_with("COMMERCIAL"))
        }
        Some(k) => winner.starts_with(k),
    };
    if !parent_ok {
        return current;
    }
    Some(format!("legal_domain:{winner}"))
}

/// Domaine porté par le code NAC (nomenclature des affaires civiles, posée
/// par le greffe à l'enregistrement — [`Decision::nac`]). Table curée depuis
/// la nomenclature officielle DACS (data.gouv.fr, familles 2 chars +
/// exceptions 3 chars), validée contre le gold : 73 % exact, 89 % parent.
/// L'école gold guide les familles mixtes : bail commercial (30) → LOCATIF,
/// JLD rétention des étrangers (14G/H/N/Q/R) → ETRANGERS, saisie immobilière
/// (78A/B/E) hors PCE, recouvrement de cotisations (88B) → SOCIAL nu.
/// `None` : famille inconnue (codes historiques hors 51) ou hors nomenclature.
///
/// [`Decision::nac`]: lj_core::decision::Decision::nac
pub fn nac_domain(nac: &str) -> Option<&'static str> {
    let code = nac.trim().to_ascii_uppercase();
    let by_code = match code.get(..3)? {
        // JLD étrangers : rétention / zone d'attente (14G historique).
        "14G" | "14H" | "14N" | "14Q" | "14R" => Some("PUBLIC_DROIT_ETRANGERS_NATIONALITE"),
        // Liquidation du régime matrimonial : famille, pas divorce.
        "22G" => Some("CIVIL_DROIT_PERSONNES_FAMILLE"),
        // Vente immobilière forcée et incidents de saisie immobilière.
        "78A" | "78B" | "78E" => Some("CIVIL_DROIT_SAISIE_IMMOBILIERE"),
        // Recouvrement de cotisations (mise en demeure, contrainte) : ni
        // prestation ni travail — parent nu.
        "88B" => Some("SOCIAL"),
        // Faute inexcusable de l'employeur : contentieux du travail.
        "89B" => Some("SOCIAL_DROIT_TRAVAIL"),
        // Responsabilité d'un établissement de crédit envers son client.
        "38E" => Some("CIVIL_DROIT_BANCAIRE_BOURSIER"),
        _ => None,
    };
    if by_code.is_some() {
        return by_code;
    }
    Some(match code.get(..2)? {
        "10" => "PUBLIC_DROIT_ETRANGERS_NATIONALITE",
        "11" | "12" | "13" | "14" | "15" | "16" | "17" | "18" | "23" | "24" | "26" | "27"
        | "2A" => "CIVIL_DROIT_PERSONNES_FAMILLE",
        "20" | "21" | "22" => "CIVIL_DIVORCE_SEPARATION_CORPS",
        "28" | "29" => "CIVIL_DROIT_SUCCESSIONS",
        // Bail commercial : école gold LOCATIF (9/9). 51 = baux d'habitation
        // de la nomenclature historique, encore massif dans les stocks.
        "30" | "51" | "5A" | "5B" => "CIVIL_DROIT_LOCATIF",
        "31" | "32" | "33" => "COMMERCIAL_DROIT_CONTRATS",
        "34" | "35" | "36" => "COMMERCIAL_DROIT_SOCIETES",
        "38" => "COMMERCIAL_DROIT_BANCAIRE_BOURSIER",
        "39" => "COMMERCIAL_DROIT_CONCURRENCE",
        "3A" | "3B" | "3C" | "3D" | "3E" => "PROPRIETE_INTELLECTUELLE_INDUSTRIELLE",
        "48" => "COMMERCIAL_DROIT_CONSOMMATION",
        "4A" | "4B" | "4C" | "4D" | "4E" | "4F" | "4G" | "4H" | "4I" | "4J" => {
            "COMMERCIAL_DROIT_ENTREPRISES_DIFFICULTE"
        }
        "50" | "56" | "57" | "59" => "CIVIL_DROIT_RESPONSABILITE_CONTRATS",
        "52" => "CIVIL_DROIT_RURAL",
        "53" | "76" => "CIVIL_DROIT_BANCAIRE_BOURSIER",
        "54" => "CIVIL_DROIT_IMMOBILIER_CONSTRUCTION",
        "55" => "COMMERCIAL_DROIT_TRANSPORT",
        "58" => "CIVIL_DROIT_ASSURANCES",
        "60" | "61" | "62" | "63" | "64" | "65" | "66" => "CIVIL_DROIT_RESPONSABILITE",
        "70" | "71" | "72" | "73" | "74" | "75" => "CIVIL_DROIT_COPROPRIETE_PROPRIETE_IMMOBILIERE",
        "77" => "CIVIL",
        "78" => "CIVIL_PROCEDURES_CIVILES_EXECUTION",
        "79" => "PROPRIETE_INTELLECTUELLE_LITTERAIRE_ARTISTIQUE",
        "80" | "81" | "82" | "83" | "84" | "85" | "86" | "87" => "SOCIAL_DROIT_TRAVAIL",
        "88" | "89" => "SOCIAL_DROIT_AIDE_ACTION_SOCIALE",
        "90" | "91" | "92" | "93" => "FISCAL",
        "94" | "95" | "96" | "97" => "PUBLIC",
        _ => return None,
    })
}

/// Parent de la taxonomie (`CIVIL_DROIT_LOCATIF` → `CIVIL`).
fn parent(d: &str) -> &str {
    d.split('_').next().unwrap_or(d)
}

/// Raffine par le code NAC : il COMBLE un domaine absent ou parent nu, et
/// TRANCHE un désaccord de parent (le greffe qualifie l'affaire mieux que des
/// citations trompeuses) — mais ne conteste jamais un sous-domaine du même
/// parent posé par les codes cités, plus fins que la famille NAC (plages
/// d'articles ; l'axe civil↔commercial des contrats dépend des parties, que
/// la NAC ne voit pas). Simulation gold 2026-07-08 : +42/−8 ; l'override
/// total faisait +61/−22.
pub fn refine_with_nac(current: Option<String>, nac: Option<&str>) -> Option<String> {
    let Some(dom) = nac.and_then(nac_domain) else {
        return current;
    };
    let apply = match current
        .as_deref()
        .map(|u| u.strip_prefix("legal_domain:").unwrap_or(u))
    {
        None => true,
        Some(k) => k == parent(k) || parent(k) != parent(dom),
    };
    if apply {
        Some(format!("legal_domain:{dom}"))
    } else {
        current
    }
}

/// Domaine porté par les thèmes Judilibre ([`Decision::themes`] — titrage de
/// la Cour de cassation, libellés de nomenclature côté CA/TJ). La MATIÈRE
/// (premier thème, préfixe de moyen « (sur le 2e moyen) » / « 1) » raboté)
/// se mappe par table curée validée contre le gold ; les intitulés
/// procéduraux (cassation, procédure civile, appel civil, conflit de lois…)
/// ne votent pas. Les mots-clés rétention / zone d'attente / nationalité,
/// où qu'ils soient dans les thèmes, signent le contentieux JLD des
/// étrangers (matière CC « droit des personnes »).
///
/// [`Decision::themes`]: lj_core::decision::Decision::themes
pub fn theme_domain(themes: &[String]) -> Option<&'static str> {
    let first = themes.first()?;
    let blob = crate::compiled::fold_stable(&themes.join(" | "));
    // JLD étrangers : rétention / zone d'attente / nationalité, à n'importe
    // quelle profondeur du titrage.
    if (blob.contains("retention") && blob.contains("etranger")) || blob.contains("zone d'attente")
    {
        return Some("PUBLIC_DROIT_ETRANGERS_NATIONALITE");
    }
    // matière = premier thème, préfixe de moyen raboté.
    let folded = crate::compiled::fold_stable(first);
    let mut m = folded.trim();
    if m.starts_with('(') {
        if let Some(p) = m.find(')') {
            m = m[p + 1..].trim_start();
        }
    }
    if m.len() >= 2 && m.starts_with(|c: char| c.is_ascii_digit()) && m[1..].starts_with(')') {
        m = m[2..].trim_start();
    }
    let by_matiere = match m {
        // pénal (titrage chambre criminelle)
        "instruction"
        | "chambre de l'instruction"
        | "chambre d'accusation"
        | "juridictions correctionnelles"
        | "appel correctionnel ou de police"
        | "cour d'assises"
        | "detention provisoire"
        | "action civile"
        | "presse"
        | "extradition"
        | "mandat d'arret europeen"
        | "contrainte par corps"
        | "banqueroute"
        | "crimes et delits flagrants"
        | "crimes et delits commis par des magistrats et certains fonctionnaires"
        | "enquete preliminaire"
        | "controle judiciaire"
        | "crime contre l'humanite"
        | "restitution"
        | "chasse"
        | "jeux de hasard"
        | "ingerence de fonctionnaires" => Some("CRIMINEL"),
        // travail
        "prud'hommes"
        | "conventions collectives"
        | "statut collectif du travail"
        | "travail reglementation"
        | "representation des salaries"
        | "elections professionnelles"
        | "statuts professionnels particuliers"
        | "relations du travail et protection sociale"
        | "securite sociale, accident du travail" => Some("SOCIAL_DROIT_TRAVAIL"),
        // sécurité sociale : prestations → aide, recouvrement/contentieux
        // technique → SOCIAL nu (école gold)
        "securite sociale, prestations familiales"
        | "securite sociale, assurances sociales"
        | "securite sociale, allocations diverses" => Some("SOCIAL_DROIT_AIDE_ACTION_SOCIALE"),
        "securite sociale, contentieux"
        | "securite sociale, allocation vieillesse pour personnes non salariees"
        | "demande d'annulation d'une mise en demeure ou d'une contrainte" => Some("SOCIAL"),
        // famille / divorce / successions
        "mariage" | "communaute entre epoux" | "nom" => Some("CIVIL_DROIT_PERSONNES_FAMILLE"),
        "divorce" | "divorce, separation de corps" | "divorce separation de corps" => {
            Some("CIVIL_DIVORCE_SEPARATION_CORPS")
        }
        "succession"
        | "successions"
        | "demande en partage, ou contestations relatives au partage" => {
            Some("CIVIL_DROIT_SUCCESSIONS")
        }
        "droit de la famille" => Some(if blob.contains("divorce") {
            "CIVIL_DIVORCE_SEPARATION_CORPS"
        } else if blob.contains("succession")
            || blob.contains("partage")
            || blob.contains("testament")
        {
            "CIVIL_DROIT_SUCCESSIONS"
        } else {
            "CIVIL_DROIT_PERSONNES_FAMILLE"
        }),
        "droit des personnes" => Some(if blob.contains("nationalite") {
            "PUBLIC_DROIT_ETRANGERS_NATIONALITE"
        } else {
            "CIVIL_DROIT_PERSONNES_FAMILLE"
        }),
        // locatif
        "bail commercial" | "autres demandes en matiere de baux commerciaux" => {
            Some("CIVIL_DROIT_LOCATIF")
        }
        // responsabilité
        "accident de la circulation"
        | "responsabilite et quasi-contrats"
        | "enrichissement sans cause"
        | "animaux"
        | "diffamation et injures"
        | "protection des droits de la personne" => Some("CIVIL_DROIT_RESPONSABILITE"),
        // contrats civils
        "contrats et obligations"
        | "responsabilite contractuelle"
        | "contrat d'entreprise"
        | "tourisme" => Some("CIVIL_DROIT_RESPONSABILITE_CONTRATS"),
        // copropriété / propriété
        "servitude" | "propriete" | "copropriete" | "demande relative a un droit de passage" => {
            Some("CIVIL_DROIT_COPROPRIETE_PROPRIETE_IMMOBILIERE")
        }
        // saisie immobilière / exécution
        "adjudication"
        | "autres demandes relatives a la procedure de saisie immobiliere"
        | "demande tendant a la vente immobiliere et a la distribution du prix" => {
            Some("CIVIL_DROIT_SAISIE_IMMOBILIERE")
        }
        "procedures civiles d'execution" | "juge de l'execution" => {
            Some("CIVIL_PROCEDURES_CIVILES_EXECUTION")
        }
        "expropriation pour cause d'utilite publique" => {
            Some("CIVIL_DROIT_EXPROPRIATION_PREEMPTION")
        }
        // fiscal
        "douanes"
        | "impots et taxes"
        | "demande relative a d'autres droits d'enregistrement ou assimiles" => Some("FISCAL"),
        "contrefacon" => Some("PROPRIETE_INTELLECTUELLE"),
        // commercial
        "arbitrage" => Some("COMMERCIAL_DROIT_ARBITRAGE"),
        "concurrence deloyale ou illicite" => Some("COMMERCIAL_DROIT_CONCURRENCE"),
        "protection des consommateurs" => Some("COMMERCIAL_DROIT_CONSOMMATION"),
        "construction immobiliere" | "architecte entrepreneur" => {
            Some("CIVIL_DROIT_IMMOBILIER_CONSTRUCTION")
        }
        "elections" => Some("PUBLIC"),
        _ => None,
    };
    if by_matiere.is_some() {
        return by_matiere;
    }
    const PREFIX: &[(&str, &str)] = &[
        ("contrat de travail", "SOCIAL_DROIT_TRAVAIL"),
        (
            "demande d'indemnites liees a la rupture du contrat de travail",
            "SOCIAL_DROIT_TRAVAIL",
        ),
        ("a.t.m.p.", "SOCIAL_DROIT_AIDE_ACTION_SOCIALE"),
        (
            "entreprise en difficulte",
            "COMMERCIAL_DROIT_ENTREPRISES_DIFFICULTE",
        ),
        (
            "reglement judiciaire, liquidation des biens",
            "COMMERCIAL_DROIT_ENTREPRISES_DIFFICULTE",
        ),
        ("bail (", "CIVIL_DROIT_LOCATIF"),
        ("bail a loyer", "CIVIL_DROIT_LOCATIF"),
        ("demande en paiement des loyers", "CIVIL_DROIT_LOCATIF"),
        ("assurance", "CIVIL_DROIT_ASSURANCES"),
        ("responsabilite delictuelle", "CIVIL_DROIT_RESPONSABILITE"),
        ("transports", "COMMERCIAL_DROIT_TRANSPORT"),
    ];
    PREFIX
        .iter()
        .find(|(p, _)| m.starts_with(p))
        .map(|&(_, d)| d)
}

/// Raffine par les thèmes : COMBLE un domaine absent ou parent nu et TRANCHE
/// un désaccord de parent, comme le NAC (ADR 0177) — sans contester un
/// sous-domaine du même parent, ni JAMAIS le CRIMINEL (la chambre criminelle
/// prime tout, fraude fiscale titrée « impots et taxes » comprise).
/// Simulation gold 2026-07-12 : +25/−1 ; sans la garde CRIMINEL, +25/−5.
pub fn refine_with_themes(current: Option<String>, themes: &[String]) -> Option<String> {
    let Some(dom) = theme_domain(themes) else {
        return current;
    };
    let apply = match current
        .as_deref()
        .map(|u| u.strip_prefix("legal_domain:").unwrap_or(u))
    {
        None => true,
        Some("CRIMINEL") => false,
        Some(k) => k == parent(k) || parent(k) != parent(dom),
    };
    if apply {
        Some(format!("legal_domain:{dom}"))
    } else {
        current
    }
}

/// Recolore le défaut « obligations » (CIVIL_DROIT_RESPONSABILITE_CONTRATS)
/// en COMMERCIAL_DROIT_CONTRATS quand le litige oppose deux SOCIÉTÉS (champs
/// companies extraits des deux côtés) : le code civil cité ne dit pas la
/// qualité des parties, l'acte entre commerçants est commercial (C. com.
/// L. 110-1). Ne touche que ce défaut — jamais un sous-domaine spécifique ni
/// un CIVIL nu (gold : +10/−2 sur le défaut, −5 de plus sur CIVIL nu).
pub fn recolor_commercial_parties(
    current: Option<String>,
    both_sides_companies: bool,
) -> Option<String> {
    if both_sides_companies
        && current.as_deref() == Some("legal_domain:CIVIL_DROIT_RESPONSABILITE_CONTRATS")
    {
        return Some("legal_domain:COMMERCIAL_DROIT_CONTRATS".to_string());
    }
    current
}

/// Domaine (`legal_domain:<KEY>`) déduit des citations liées d'une décision —
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
    // la chambre sociale du contrat de travail ; l'action civile devant la
    // chambre criminelle reste du contentieux pénal (gold : `cr` → CRIMINEL
    // 98 %, intérêts civils compris).
    let winner = match winner {
        // La chambre criminelle prime TOUT vote (gold : `cr` → CRIMINEL
        // 98 % — action civile, fraude fiscale au pénal compris).
        Some(_) if ctx.criminal => Some("CRIMINEL"),
        Some("CIVIL_DROIT_RESPONSABILITE_CONTRATS" | "CIVIL") if ctx.commercial => {
            Some("COMMERCIAL_DROIT_CONTRATS")
        }
        Some("CIVIL_DROIT_RESPONSABILITE_CONTRATS" | "CIVIL") if ctx.social => {
            Some("SOCIAL_DROIT_TRAVAIL")
        }
        None if ctx.criminal => Some("CRIMINEL"),
        None if ctx.social => Some("SOCIAL_DROIT_TRAVAIL"),
        None if ctx.commercial => Some("COMMERCIAL"),
        None if ctx.civil_chamber => Some("CIVIL"),
        // Vote muet ou générique en admin : le vocabulaire du texte raffine.
        None | Some("PUBLIC") if ctx.admin && ctx.hint.is_some() => ctx.hint,
        // Contentieux administratif sans texte substantiel : générique.
        None if ctx.admin && generic_admin => Some("PUBLIC"),
        w => w,
    };
    winner.map(|domain| format!("legal_domain:{domain}"))
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
        // CPP, CP : CRIMINEL n'existe pas dans l'ordre admin — les
        // extraditions CE citent massivement le CPP en restant du droit des
        // étrangers (gold : 25 sorties CRIMINEL admin, 0 correcte).
        "LEGITEXT000006071154" | "LEGITEXT000006070719" => (!admin).then_some(("CRIMINEL", 1.0)),
        "LEGITEXT000006072050" => strong(if admin {
            "PUBLIC_DROIT_TRAVAIL"
        } else {
            "SOCIAL_DROIT_TRAVAIL"
        }), // code du travail
        "LEGITEXT000044416551" => strong("PUBLIC_DROIT_TRAVAIL"), // CGFP
        "LEGITEXT000006074075" => strong("PUBLIC_DROIT_URBANISME_IMMOBILIER_PUBLIC"), // urbanisme
        // CG3P (domaine public), voirie routière : l'immobilier/patrimoine
        // public au sens de l'école gold.
        "LEGITEXT000006070299" | "LEGITEXT000006070667" => {
            admin.then_some(("PUBLIC_DROIT_URBANISME_IMMOBILIER_PUBLIC", 1.0))
        }
        // Code de l'expropriation : juge judiciaire de l'expropriation
        // (indemnités) vs légalité de la DUP côté admin.
        "LEGITEXT000006074224" => strong(if admin {
            "PUBLIC_DROIT_URBANISME_IMMOBILIER_PUBLIC"
        } else {
            "CIVIL_DROIT_EXPROPRIATION_PREEMPTION"
        }),
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
        // CSS par plage : livre 4 entier (AT/MP — déclaration L. 441, rentes
        // L. 434, maladies professionnelles L. 461, faute inexcusable
        // L. 452) et prévoyance collective d'entreprise (L. 911-914) =
        // contentieux du travail ; cotisations & recouvrement URSSAF
        // (L. 133/136, livre 2, régimes des indépendants 6xx) = SOCIAL nu
        // (école gold) ; le reste (prestations, livre 8) = aide/action
        // sociale — SOCIAL même devant l'ordre admin (école gold : le
        // contentieux technique de la sécu reste SOCIAL, contrairement au
        // CASF).
        "LEGITEXT000006073189" => match head_num(num_key) {
            Some(411..=482 | 911..=914) => strong(if admin {
                "PUBLIC_DROIT_TRAVAIL"
            } else {
                "SOCIAL_DROIT_TRAVAIL"
            }),
            Some(
                114
                | 124
                | 131..=145
                | 161
                | 165
                | 213..=285
                | 311
                | 315
                | 633
                | 651
                | 652
                | 661
                | 662,
            ) if !admin => strong("SOCIAL"),
            // Mention nue (« du code de la sécurité sociale » sans article
            // lié) : signal parent, pas une prestation.
            None if !admin => Some(("SOCIAL", 0.5)),
            _ => strong("SOCIAL_DROIT_AIDE_ACTION_SOCIALE"),
        },
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
        // et quasi-contrats anciens (1371-1381 : gestion d'affaires,
        // répétition de l'indu — école thèmes « resp. et quasi-contrats »)
        1240..=1252 | 1371..=1386 => ("CIVIL_DROIT_RESPONSABILITE", 1.0),
        // preuve (anciens 1315-1316) : transverse, ne signe pas la matière
        1315 | 1316 => ("CIVIL", 0.5),
        // obligations, contrats, vente, régime général
        1100..=1239 | 1253..=1370 | 1582..=1707 | 1800..=2010 => {
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
    fn termes_raffinent_parent_nu_jamais_sous_domaine() {
        let votes = [
            ("CIVIL_DROIT_LOCATIF", 5u32),
            ("CIVIL_DROIT_SUCCESSIONS", 1),
        ];
        // parent nu cohérent : raffiné
        assert_eq!(
            refine_with_terms(Some("legal_domain:CIVIL".into()), &votes).as_deref(),
            Some("legal_domain:CIVIL_DROIT_LOCATIF")
        );
        // domaine absent : posé
        assert_eq!(
            refine_with_terms(None, &votes).as_deref(),
            Some("legal_domain:CIVIL_DROIT_LOCATIF")
        );
        // sous-domaine posé par les codes : intouchable
        assert_eq!(
            refine_with_terms(Some("legal_domain:CIVIL_DROIT_ASSURANCES".into()), &votes)
                .as_deref(),
            Some("legal_domain:CIVIL_DROIT_ASSURANCES")
        );
        // parent incohérent : refus
        assert_eq!(
            refine_with_terms(Some("legal_domain:COMMERCIAL".into()), &votes).as_deref(),
            Some("legal_domain:COMMERCIAL")
        );
        // sous les seuils (1 occurrence, ou gagnant < 2× le second) : refus
        assert_eq!(refine_with_terms(None, &[("CIVIL_DROIT_LOCATIF", 1)]), None);
        assert_eq!(
            refine_with_terms(
                None,
                &[("CIVIL_DROIT_LOCATIF", 3), ("CIVIL_DROIT_SUCCESSIONS", 2)]
            ),
            None
        );
    }

    #[test]
    fn chamber_criminelle_recolore_l_action_civile() {
        // Pourvoi `cr` ne citant que le code civil (intérêts civils) :
        // la coloration criminelle prime le vote responsabilité.
        let cites = || vec![("LEGITEXT000006070721", Some("1240"))];
        let cr = DomainContext {
            criminal: true,
            ..Default::default()
        };
        assert_eq!(
            legal_domain_uid(cites(), cr).as_deref(),
            Some("legal_domain:CRIMINEL")
        );
        assert_eq!(
            legal_domain_uid(cites(), DomainContext::default()).as_deref(),
            Some("legal_domain:CIVIL_DROIT_RESPONSABILITE")
        );
        // Vote muet : plancher CRIMINEL.
        assert_eq!(
            legal_domain_uid(vec![], cr).as_deref(),
            Some("legal_domain:CRIMINEL")
        );
    }

    #[test]
    fn tcom_colore_les_obligations_en_commercial() {
        // Obligations du code civil devant le tribunal de commerce.
        let cites = vec![("LEGITEXT000006070721", Some("1103"))];
        let ctx = DomainContext {
            commercial: true,
            ..Default::default()
        };
        assert_eq!(
            legal_domain_uid(cites, ctx).as_deref(),
            Some("legal_domain:COMMERCIAL_DROIT_CONTRATS")
        );
    }

    #[test]
    fn nac_comble_et_tranche_le_parent_sans_contester_le_sous_domaine() {
        // Comble un domaine absent ou parent nu.
        assert_eq!(
            refine_with_nac(None, Some("20A")).as_deref(),
            Some("legal_domain:CIVIL_DIVORCE_SEPARATION_CORPS")
        );
        assert_eq!(
            refine_with_nac(Some("legal_domain:CIVIL".into()), Some("5A1")).as_deref(),
            Some("legal_domain:CIVIL_DROIT_LOCATIF")
        );
        // Tranche un désaccord de PARENT (citations trompeuses).
        assert_eq!(
            refine_with_nac(
                Some("legal_domain:CIVIL_DROIT_PERSONNES_FAMILLE".into()),
                Some("14H")
            )
            .as_deref(),
            Some("legal_domain:PUBLIC_DROIT_ETRANGERS_NATIONALITE")
        );
        // Ne conteste jamais un sous-domaine du même parent (codes plus fins).
        assert_eq!(
            refine_with_nac(
                Some("legal_domain:CIVIL_DROIT_BANCAIRE_BOURSIER".into()),
                Some("50D")
            )
            .as_deref(),
            Some("legal_domain:CIVIL_DROIT_BANCAIRE_BOURSIER")
        );
        // NAC inconnue : inerte.
        assert_eq!(refine_with_nac(None, Some("99Z")), None);
    }

    #[test]
    fn themes_comblent_et_tranchent_sauf_criminel() {
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Matière pénale du titrage : comble un domaine absent.
        assert_eq!(
            refine_with_themes(None, &t(&["instruction", "perquisition"])).as_deref(),
            Some("legal_domain:CRIMINEL")
        );
        // Préfixe de moyen raboté avant la table.
        assert_eq!(
            theme_domain(&t(&["(sur le 2e moyen) conventions collectives"])),
            Some("SOCIAL_DROIT_TRAVAIL")
        );
        // Rétention d'un étranger : mots-clés à toute profondeur du titrage.
        assert_eq!(
            theme_domain(&t(&[
                "droit des personnes",
                "demande de mainlevée de la rétention formée devant le juge \
                 des libertés et de la détention par l'étranger"
            ])),
            Some("PUBLIC_DROIT_ETRANGERS_NATIONALITE")
        );
        // Tranche un désaccord de PARENT, jamais un sous-domaine du même
        // parent, jamais le CRIMINEL (fraude fiscale chambre criminelle).
        assert_eq!(
            refine_with_themes(Some("legal_domain:CIVIL".into()), &t(&["divorce"])).as_deref(),
            Some("legal_domain:CIVIL_DIVORCE_SEPARATION_CORPS")
        );
        assert_eq!(
            refine_with_themes(
                Some("legal_domain:CIVIL_DROIT_BANCAIRE_BOURSIER".into()),
                &t(&["contrats et obligations"])
            )
            .as_deref(),
            Some("legal_domain:CIVIL_DROIT_BANCAIRE_BOURSIER")
        );
        assert_eq!(
            refine_with_themes(
                Some("legal_domain:CRIMINEL".into()),
                &t(&["impots et taxes"])
            )
            .as_deref(),
            Some("legal_domain:CRIMINEL")
        );
        // Intitulé procédural : ne vote pas.
        assert_eq!(theme_domain(&t(&["cassation", "pouvoir"])), None);
    }

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
            Some("legal_domain:PUBLIC_DROIT_ETRANGERS_NATIONALITE")
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
            Some("legal_domain:SOCIAL_DROIT_TRAVAIL")
        );
        assert_eq!(
            legal_domain_uid(cites(), admin(true)).as_deref(),
            Some("legal_domain:PUBLIC_DROIT_TRAVAIL")
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
            Some("legal_domain:CIVIL_DROIT_RESPONSABILITE")
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
            Some("legal_domain:PUBLIC")
        );
        assert_eq!(legal_domain_uid(cites(), DomainContext::default()), None);
    }
}
