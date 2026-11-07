//! Éponymes : le nom d'usage d'un arrêt ou d'une loi vers sa référence.
//!
//! Le nom est absent du texte de sa propre décision, la pseudonymisation le
//! retirant, et c'est pourtant par lui que le praticien entre. Deux sources le
//! portent : les renvois du Conseil d'État et la locution « dite loi » des
//! corps de décision. Ces parsers émettent des candidats et n'apparient rien :
//! le vote et l'activation vivent en aval.

use std::sync::LazyLock;

use lj_core::case_names::case_name_key;
use lj_core::dates::parse_date_fr;
use lj_core::text::normalize_spaces;
use lj_fold::fold;
use regex::Regex;

/// Citation portée par un renvoi. `dockets` est vide quand le renvoi ne donne que
/// la page du Lebon, courant avant les années 1990 ; `nom` est vide quand la
/// source a caviardé la désignation des parties, ce que le Conseil d'État fait
/// dans ses propres renvois (celui vers Danthony se lit « et autres »). La
/// citation vaut sans le nom, le nom ne vaut pas sans la cible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenvoiCandidat {
    pub juridiction: String,
    pub nom: Option<String>,
    pub date_iso: String,
    pub dockets: Vec<String>,
}

/// Candidat éponyme de loi, non résolu : le voisinage se trompe souvent d'une
/// loi. `date_iso` est le second recours, le juge citant souvent la loi par sa
/// seule date (« la loi 27 juillet 2023 [...] dite loi Kasbarian », 648
/// décisions) ; le catalogue tranche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiteLoiCandidat {
    pub surnom: String,
    pub numero: Option<String>,
    pub date_iso: Option<String>,
}

/// Les préfixes institutionnels (« Commune de », « Société », « Département
/// de ») ne sont pas ici : ils font partie du nom d'usage.
const CIVILITES: &[&str] = &[
    "m.",
    "m",
    "mme",
    "mmes",
    "mlle",
    "mlles",
    "mm.",
    "sieur",
    "sieurs",
    "dame",
    "dames",
    "demoiselle",
    "demoiselles",
    "veuve",
    "vve",
    "epoux",
    "epouse",
    "epouses",
    "consorts",
];

const SUFFIXES: &[&str] = &["et autres", "et autre", "et al.", "et a."];

/// Désignations qui ne nomment personne : partie anonymisée ou générique.
const GENERIQUES: &[&str] = &["x", "y", "z", "societe x", "bull", "autres"];

/// Le renvoi qui ne nomme pas laisse sa référence là où le nom se lit.
const REFERENCES: &[&str] = &["n", "no", "nos", "p", "pp", "req"];

/// Un nom qui commence par là désigne une fonction, jamais un arrêt.
const FONCTIONS: &[&str] = &[
    "ministre",
    "ministere",
    "min.",
    "garde des sceaux",
    "secretaire d",
    "premier ministre",
    "president de la republique",
    "prefet",
    "directeur general",
];

/// Premiers mots qui ne peuvent pas ouvrir un surnom de loi.
const SURNOM_INTERDIT: &[&str] = &[
    "organique",
    "relative",
    "relatif",
    "portant",
    "nationale",
    "cette",
    "la",
    "le",
    "les",
    "du",
    "de",
    "des",
    "et",
    "ou",
    "sur",
    "par",
    "loi",
    "susvisee",
    "precitee",
];

/// Au-delà, le numéro attrapé en amont est celui d'une autre loi de la phrase.
const FENETRE_NUMERO: usize = 160;

static DATE_RENVOI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b\d{1,2}(?:er)?\s+[a-zàâäéèêëîïôöùûüç]+\s+\d{4}\b").unwrap()
});

static NUMEROS_RENVOI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)n[°o]s?\s*((?:\d[\d\s.\-]*)+)").unwrap());

static DITE_LOI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i:dite\s+loi|loi\s+dite)\s+[«\x22]?\s*([A-ZÉÈÀÂÎÔÇ][\p{L}'\-]+(?:\s+[A-ZÉÈÀÂÎÔÇ][\p{L}'\-]*){0,2})",
    )
    .unwrap()
});

static NUMERO_LOI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)loi\s+n[°o]\s*(\d{2,4}-\d+)").unwrap());

static TITRE_LOI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*loi(?:\s+(?:organique|constitutionnelle|de\s+programme))?\s+n[°o]\s*(\d{2,4}-\d+)\s+du\s+")
        .unwrap()
});

fn est_civilite(mot: &str) -> bool {
    CIVILITES.contains(&fold(mot).as_str())
}

/// Les analyses du Conseil d'État renvoient aussi hors de son propre ordre. Une
/// formation reconnue (Section, Assemblée, chambres) ou rien du tout vaut `ce`.
fn juridiction_du_prefixe(avant: &str) -> &'static str {
    let p = fold(avant);
    if p.contains("tribunal des conflits") || p.contains("tc,") || p.contains(" tc ") {
        "tc"
    } else if p.contains("cedh") || p.contains("cour edh") {
        "cedh"
    } else if p.contains("cjue") || p.contains("cjce") {
        "cjue"
    } else if p.contains("cons. const") || p.contains("conseil constitutionnel") {
        "constit"
    } else if p.contains("cass.") || p.contains("cour de cassation") {
        "cc"
    } else {
        "ce"
    }
}

/// Nom d'usage tiré d'une désignation de partie, `None` quand elle ne nomme
/// personne. « M. Danthony et autres » et « Danthony et autres » rendent le même
/// nom.
pub fn normalise_nom(brut: &str) -> Option<String> {
    let net = normalize_spaces(brut);
    let mut mots: Vec<&str> = net
        .trim()
        .trim_matches(|c: char| c == '.' || c == ',' || c == ';' || c.is_whitespace())
        .split_whitespace()
        .collect();

    while mots.len() >= 3 {
        let queue = fold(&mots[mots.len() - 2..].join(" "));
        if SUFFIXES.contains(&queue.as_str()) {
            mots.truncate(mots.len() - 2);
        } else {
            break;
        }
    }
    // « et Société Leguem » : la partie de gauche est tombée au découpage, la
    // conjonction reste et ouvre le nom.
    while mots
        .first()
        .is_some_and(|tete| est_civilite(tete) || fold(tete) == "et")
    {
        mots.remove(0);
    }

    let mut nom = mots.join(" ");
    if let Some(coupe) = nom.rfind(" et ") {
        if FONCTIONS
            .iter()
            .any(|f| fold(&nom[coupe + 4..]).starts_with(f))
        {
            nom.truncate(coupe);
        }
    }
    let plie = fold(&nom);
    let cle = case_name_key(&nom);
    let premier = cle.split_whitespace().next().unwrap_or_default();
    let initiales = cle.split_whitespace().all(|t| t.chars().count() == 1);
    if nom.chars().count() < 3
        || initiales
        || plie.starts_with("c/ ")
        || plie.starts_with("c\\ ")
        || GENERIQUES.contains(&plie.as_str())
        || SUFFIXES.contains(&plie.as_str())
        || REFERENCES.contains(&premier)
        || FONCTIONS.iter().any(|f| plie.starts_with(f))
        || !nom.chars().any(char::is_alphabetic)
    {
        return None;
    }
    Some(nom)
}

/// Renvoi officiel du Conseil d'État vers ses candidats, au format
/// « Cf. Section, 18 décembre 2002, Mme Duvignères, n° 233618 ». Un renvoi porte
/// plusieurs citations séparées par des points-virgules.
pub fn parse_renvoi(renvoi: &str) -> Vec<RenvoiCandidat> {
    let mut out: Vec<RenvoiCandidat> = Vec::new();
    for frag in renvoi.split(';') {
        let Some(m) = DATE_RENVOI.find(frag) else {
            continue;
        };
        let Some(date) = parse_date_fr(m.as_str()) else {
            continue;
        };
        let reste = &frag[m.end()..];
        let nom_brut = reste
            .trim_start_matches(|c: char| c == ',' || c.is_whitespace())
            .split(',')
            .next()
            .unwrap_or_default();
        let nom = normalise_nom(nom_brut);
        let dockets = NUMEROS_RENVOI
            .captures(reste)
            .map(|c| {
                c[1].split(|ch: char| !ch.is_ascii_digit())
                    .filter(|n| n.len() >= 3)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if nom.is_none() && dockets.is_empty() {
            continue;
        }
        let cand = RenvoiCandidat {
            juridiction: juridiction_du_prefixe(&frag[..m.start()]).to_string(),
            nom,
            date_iso: date.to_string(),
            dockets,
        };
        if !out.contains(&cand) {
            out.push(cand);
        }
    }
    out
}

/// Surnoms de loi d'un corps de décision, au format « la loi n° 85-677 du
/// 5 juillet 1985, dite loi Badinter » : le surnom suit la locution, le numéro
/// officiel la précède.
pub fn parse_dite_loi(texte: &str) -> Vec<DiteLoiCandidat> {
    let mut out: Vec<DiteLoiCandidat> = Vec::new();
    for m in DITE_LOI.captures_iter(texte) {
        let brut = m.get(1).map(|g| g.as_str()).unwrap_or_default();
        let surnom = fold(&normalize_spaces(brut));
        let premier = surnom.split_whitespace().next().unwrap_or_default();
        if surnom.chars().count() < 3 || SURNOM_INTERDIT.contains(&premier) {
            continue;
        }
        let locution = m.get(0).map(|g| g.start()).unwrap_or_default();
        let mut debut = locution.saturating_sub(FENETRE_NUMERO);
        while debut < locution && !texte.is_char_boundary(debut) {
            debut += 1;
        }
        let fenetre = &texte[debut..locution];
        let numero = NUMERO_LOI
            .captures_iter(fenetre)
            .last()
            .map(|c| c[1].to_string());
        let date_iso = DATE_RENVOI
            .find_iter(fenetre)
            .last()
            .and_then(|m| parse_date_fr(m.as_str()))
            .map(|d| d.to_string());
        let cand = DiteLoiCandidat {
            surnom,
            numero,
            date_iso,
        };
        if !out.contains(&cand) {
            out.push(cand);
        }
    }
    out
}

/// Number and ISO date of a catalogue law, read in its title: `date_texte` is often
/// missing, the title always carries both.
pub fn parse_titre_loi(titre: &str) -> Option<(String, String)> {
    let numero = TITRE_LOI.captures(titre)?;
    let apres = &titre[numero.get(0)?.end()..];
    let date = parse_date_fr(DATE_RENVOI.find(apres)?.as_str())?;
    Some((numero[1].to_string(), date.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_renvoi_rend_nom_date_et_numero() {
        let r = parse_renvoi(
            "(1) Cf. Section, 18 décembre 2002, Mme Duvignères, n° 233618, à publier.",
        );
        assert_eq!(
            r,
            vec![RenvoiCandidat {
                juridiction: "ce".into(),
                nom: Some("Duvignères".into()),
                date_iso: "2002-12-18".into(),
                dockets: vec!["233618".into()],
            }]
        );
    }

    #[test]
    fn le_renvoi_sort_de_l_ordre_administratif() {
        let r = parse_renvoi("Cf. TC, 8 février 1873, Blanco, p. 61.");
        assert_eq!(r[0].juridiction, "tc");
        assert_eq!(r[0].nom.as_deref(), Some("Blanco"));
        let r = parse_renvoi("Cf. Cons. const., 3 septembre 1986, Loi de finances, n° 86-215.");
        assert_eq!(r[0].juridiction, "constit");
        let r = parse_renvoi("Cf. CEDH, 19 avril 2007, Vilho Eskelinen, n° 63235/00.");
        assert_eq!(r[0].juridiction, "cedh");
    }

    #[test]
    fn le_renvoi_ancien_ne_donne_que_la_page_du_lebon() {
        let r = parse_renvoi("Section, 11 décembre 1970, Crédit foncier de France, p. 750.");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].nom.as_deref(), Some("Crédit foncier de France"));
        assert_eq!(r[0].date_iso, "1970-12-11");
        assert!(r[0].dockets.is_empty());
    }

    #[test]
    fn un_renvoi_porte_plusieurs_citations() {
        let r = parse_renvoi(
            "Cf. Section, 11 décembre 1970, Crédit foncier de France, p. 750 ; \
             Section, 18 décembre 2002, Mme Duvignères, p. 463.",
        );
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].nom.as_deref(), Some("Duvignères"));
    }

    #[test]
    fn la_civilite_et_le_et_autres_ne_font_pas_deux_eponymes() {
        let a = parse_renvoi("Assemblée, 23 décembre 2011, M. Danthony et autres, n° 335033.");
        let b = parse_renvoi("Assemblée, 23 décembre 2011, Danthony et autres, n° 335033.");
        assert_eq!(a[0].nom.as_deref(), Some("Danthony"));
        assert_eq!(a[0].nom, b[0].nom);
        assert_eq!(a[0].dockets, vec!["335033".to_string()]);
    }

    #[test]
    fn le_decoupage_qui_perd_la_partie_de_gauche_ne_nomme_personne() {
        assert_eq!(
            normalise_nom("et Société Leguem").as_deref(),
            Some("Société Leguem")
        );
        assert_eq!(normalise_nom("et et Mme"), None);
        assert_eq!(normalise_nom("et a"), None);
        assert_eq!(normalise_nom("épouse"), None);
        assert_eq!(normalise_nom("c/ ministre de la justice"), None);
        assert_eq!(
            normalise_nom("de Lasteyrie du Saillant").as_deref(),
            Some("de Lasteyrie du Saillant")
        );
    }

    #[test]
    fn m_et_mme_tombent_ensemble() {
        let r = parse_renvoi("Cf. 12 février 2020, M. et Mme Sarran, n° 421444.");
        assert_eq!(r[0].nom.as_deref(), Some("Sarran"));
        let r = parse_renvoi("Cf. 2 mars 1973, Demoiselle Arbousset, n° 84740.");
        assert_eq!(r[0].nom.as_deref(), Some("Arbousset"));
    }

    #[test]
    fn plusieurs_numeros_dans_un_seul_renvoi() {
        let r = parse_renvoi("Section, 14 février 2014, Sarran, n°s 375081 375090.");
        assert_eq!(
            r[0].dockets,
            vec!["375081".to_string(), "375090".to_string()]
        );
    }

    #[test]
    fn la_partie_anonymisee_et_la_fonction_ne_nomment_rien() {
        for frag in [
            "Cf. Section, 13 décembre 2006, Mme, n° 287845.",
            "Cf. Section, 14 février 2014, Mme et autres, n° 375081.",
            "Cf. 27 juillet 2005, Ministre de l'économie, p. 1.",
            "Cf. 28 juin 2002, Garde des sceaux, p. 2.",
            "Cf. 6 novembre 1974, Société X, p. 3.",
            "Cf. 18 décembre 2003, Bull, p. 4.",
            "Cf. CE, Assemblée, 23 décembre 2011, et autres, n° 335033, p. 649.",
            "Cf. CE, Section, 13 juillet 2016, n° 387763, p. 340.",
            "Cf. CE, Section, 28 juillet 2000, E.A, n° 151068.",
        ] {
            for c in parse_renvoi(frag) {
                assert!(c.nom.is_none(), "{frag}");
            }
        }
    }

    #[test]
    fn le_renvoi_caviarde_garde_sa_cible() {
        let r = parse_renvoi("Cf. CE, Assemblée, 23 décembre 2011, et autres, n° 335033, p. 649.");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].nom, None);
        assert_eq!(r[0].dockets, vec!["335033".to_string()]);
        assert!(parse_renvoi("Cf. Section, 6 novembre 1974, Société X, p. 3.").is_empty());
    }

    #[test]
    fn la_cle_replie_la_ponctuation_et_les_accents() {
        assert_eq!(
            case_name_key("Association AC !"),
            case_name_key("Association AC!")
        );
        assert_eq!(case_name_key("Duvignères"), "duvigneres");
        assert_eq!(
            case_name_key("Département de Tarn-et-Garonne"),
            "departement de tarn et garonne"
        );
    }

    #[test]
    fn la_fonction_accolee_par_et_tombe() {
        let r = parse_renvoi(
            "Section, 11 juillet 2011, Société d'équipement du département \
             de Maine-et-Loire Sodemel et ministre de l'intérieur, n°s 320735 320854.",
        );
        assert_eq!(
            r[0].nom.as_deref(),
            Some("Société d'équipement du département de Maine-et-Loire Sodemel")
        );
    }

    #[test]
    fn les_personnes_morales_gardent_leur_prefixe() {
        let r = parse_renvoi("Assemblée, 4 avril 2014, Département de Tarn-et-Garonne, n° 358994.");
        assert_eq!(r[0].nom.as_deref(), Some("Département de Tarn-et-Garonne"));
        let r = parse_renvoi(
            "Assemblée, 16 juillet 2007, Société Tropic Travaux Signalisation, n° 291545.",
        );
        assert_eq!(
            r[0].nom.as_deref(),
            Some("Société Tropic Travaux Signalisation")
        );
    }

    #[test]
    fn dite_loi_prend_le_surnom_apres_et_le_numero_avant() {
        let r = parse_dite_loi(
            "il résulte de la loi n° 85-677 du 5 juillet 1985, dite loi Badinter, que la victime",
        );
        assert_eq!(
            r,
            vec![DiteLoiCandidat {
                surnom: "badinter".into(),
                numero: Some("85-677".into()),
                date_iso: Some("1985-07-05".into()),
            }]
        );
    }

    #[test]
    fn loi_dite_marche_dans_les_deux_ordres_et_replie_la_casse() {
        let a = parse_dite_loi("la loi dite ALUR du 24 mars 2014");
        let b = parse_dite_loi("la loi n° 2014-366 du 24 mars 2014, dite loi Alur");
        assert_eq!(a[0].surnom, "alur");
        assert_eq!(b[0].surnom, "alur");
        assert_eq!(a[0].numero, None);
        assert_eq!(b[0].numero, Some("2014-366".into()));
    }

    #[test]
    fn un_surnom_de_deux_mots_tient() {
        let r = parse_dite_loi("la loi n° 2016-87 du 2 février 2016, dite loi Claeys Leonetti");
        assert_eq!(r[0].surnom, "claeys leonetti");
    }

    #[test]
    fn le_mot_generique_apres_la_locution_ne_fait_pas_un_surnom() {
        assert!(parse_dite_loi("la loi dite Relative aux droits des malades").is_empty());
        assert!(parse_dite_loi("la loi dite Organique du 1er août 2001").is_empty());
    }

    #[test]
    fn le_numero_hors_fenetre_n_est_pas_attrape() {
        let loin = format!(
            "la loi n° 58-677 du 3 juin 1958{}, dite loi Badinter",
            " lorem ipsum".repeat(30)
        );
        let c = &parse_dite_loi(&loin)[0];
        assert_eq!(c.numero, None);
        assert_eq!(c.date_iso, None);
    }

    #[test]
    fn la_loi_citee_par_sa_seule_date_garde_sa_date() {
        let r = parse_dite_loi(
            "le bail ayant été conclu avant l'entrée en vigueur de la loi 27 juillet 2023 \
             visant à protéger les logements contre l'occupation illicite dite loi Kasbarian, \
             toute clause",
        );
        assert_eq!(r[0].surnom, "kasbarian");
        assert_eq!(r[0].numero, None);
        assert_eq!(r[0].date_iso.as_deref(), Some("2023-07-27"));
    }

    #[test]
    fn le_titre_du_catalogue_rend_numero_et_date() {
        assert_eq!(
            parse_titre_loi(
                "LOI n° 2023-668 du 27 juillet 2023 visant à protéger les logements (1)"
            ),
            Some(("2023-668".into(), "2023-07-27".into()))
        );
        assert_eq!(
            parse_titre_loi("LOI organique n° 2001-692 du 1er août 2001 relative aux lois"),
            Some(("2001-692".into(), "2001-08-01".into()))
        );
        assert_eq!(
            parse_titre_loi("Décret n° 2023-668 du 19 juillet 2023"),
            None
        );
    }
}
