//! Trade of a market, folded from the BOAMP descriptors, PURE.
//!
//! They classify who answers where the CPV classifies what is bought, and do
//! not nest under it: `Prestations de services` spans many divisions.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use lj_fold::fold;

const TABLE_TSV: &str = include_str!("../data/metiers_fr.tsv");
const LEXIQUE_TSV: &str = include_str!("../data/metiers_lexique_fr.tsv");
const CODES_TSV: &str = include_str!("../data/metiers_cpv_fr.tsv");

/// Lengths training kept, as in `cpv`.
const LONGUEUR: std::ops::RangeInclusive<usize> = 4..=24;

struct Table {
    /// Folded descriptor, its trade code.
    replis: HashMap<&'static str, &'static str>,
    /// Trade codes in editorial order, with their label.
    metiers: Vec<(&'static str, &'static str)>,
}

static TABLE: LazyLock<Table> = LazyLock::new(|| {
    let mut replis = HashMap::new();
    let mut metiers = Vec::new();
    let mut plies: HashMap<String, &'static str> = HashMap::new();
    for ligne in TABLE_TSV.lines() {
        if ligne.starts_with('#') || ligne.is_empty() {
            continue;
        }
        let mut champs = ligne.split('\t');
        match champs.next().unwrap_or_default() {
            "@" => {
                let code = champs.next().expect("bloc @ : un code");
                metiers.push((code, champs.next().expect("bloc @ : un libellé")));
            }
            descripteur => {
                let code = champs.next().expect("corps : un code de métier");
                plies.insert(fold(descripteur), code);
            }
        }
    }
    for (plie, code) in plies {
        replis.insert(Box::leak(plie.into_boxed_str()) as &'static str, code);
    }
    Table { replis, metiers }
});

/// CPV prefix, the trade it names, `None` for a prefix that names none.
static CODES: LazyLock<HashMap<&'static str, Option<&'static str>>> = LazyLock::new(|| {
    CODES_TSV
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let (prefixe, code) = l.split_once('\t').expect("ligne : un préfixe, un métier");
            (prefixe, (code != "-").then_some(code))
        })
        .collect()
});

/// The trade of a lot's CPV code, read on its longest prefix in the table.
fn metier_du_code(cpv: &str) -> Option<&'static str> {
    let chiffres = cpv.trim();
    let n = chiffres
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count()
        .min(8);
    (2..=n)
        .rev()
        .find_map(|k| CODES.get(&chiffres[..k]))
        .copied()
        .flatten()
}

fn dans_l_ordre(vus: &[&str]) -> Vec<String> {
    TABLE
        .metiers
        .iter()
        .filter(|(code, _)| vus.contains(code))
        .map(|(code, _)| format!("metier:{code}"))
        .collect()
}

/// The trades a market's descriptors name, `metier:<code>`, deduplicated and in
/// the table's order. Empty when no descriptor names a trade: `Prestations de
/// services`, the BOAMP's own catch-all, says nothing of who answers.
pub fn metiers_probables<S: AsRef<str>>(descripteurs: &[S]) -> Vec<String> {
    let vus: Vec<&str> = descripteurs
        .iter()
        .filter_map(|d| TABLE.replis.get(fold(d.as_ref()).as_str()).copied())
        .collect();
    dans_l_ordre(&vus)
}

/// Appends the trade of each works lot's CPV code, a lot being the unit a trade bids on.
fn avec_les_lots<C: AsRef<str>>(mut metiers: Vec<String>, codes_des_lots: &[C]) -> Vec<String> {
    let vus: Vec<&str> = codes_des_lots
        .iter()
        .filter_map(|c| metier_du_code(c.as_ref()))
        .collect();
    for m in dans_l_ordre(&vus) {
        if !metiers.contains(&m) {
            metiers.push(m);
        }
    }
    metiers
}

/// What the market publishes of who answers: its descriptors' fold, then its works lots.
pub fn metiers_publies<S: AsRef<str>, C: AsRef<str>>(
    descripteurs: &[S],
    codes_des_lots: &[C],
) -> Vec<String> {
    avec_les_lots(metiers_probables(descripteurs), codes_des_lots)
}

/// Label of a `metier:<code>`, the uid itself when the table does not know it.
pub fn libelle(uid: &str) -> &str {
    let code = uid.strip_prefix("metier:").unwrap_or(uid);
    TABLE
        .metiers
        .iter()
        .find(|(c, _)| *c == code)
        .map_or(uid, |(_, l)| l)
}

/// The axis itself, `(metier:<code>, libellé)` in editorial order. Seeds the
/// referential and the facet.
pub fn axe() -> impl Iterator<Item = (String, &'static str)> {
    TABLE
        .metiers
        .iter()
        .map(|(code, libelle)| (format!("metier:{code}"), *libelle))
}

struct Lexique {
    vides: HashSet<&'static str>,
    /// Log-prior and default log-weight of a trade.
    metiers: Vec<(&'static str, f32, f32)>,
    /// Per token, its gap to the default of each trade it was seen in.
    poids: HashMap<&'static str, Vec<(usize, f32)>>,
}

static LEXIQUE: LazyLock<Lexique> = LazyLock::new(|| {
    let mut vides = HashSet::new();
    let mut metiers: Vec<(&str, f32, f32)> = Vec::new();
    let mut rang: HashMap<&str, usize> = HashMap::new();
    let mut poids: HashMap<&str, Vec<(usize, f32)>> = HashMap::new();
    for ligne in LEXIQUE_TSV.lines() {
        if ligne.starts_with('#') || ligne.is_empty() {
            continue;
        }
        let mut champs = ligne.split('\t');
        match champs.next().unwrap_or_default() {
            "!" => vides.extend(champs.next().unwrap_or_default().split_whitespace()),
            "@" => {
                let code = champs.next().expect("métier");
                let prior: f32 = champs.next().expect("prior").parse().expect("prior");
                let defaut: f32 = champs.next().expect("défaut").parse().expect("défaut");
                rang.insert(code, metiers.len());
                metiers.push((code, prior / 100.0, defaut / 100.0));
            }
            jeton => {
                let ecarts = champs
                    .next()
                    .unwrap_or_default()
                    .split_whitespace()
                    .filter_map(|p| {
                        let (m, v) = p.split_once(':')?;
                        Some((*rang.get(m)?, v.parse::<f32>().ok()? / 100.0))
                    })
                    .collect();
                poids.insert(jeton, ecarts);
            }
        }
    }
    Lexique {
        vides,
        metiers,
        poids,
    }
});

/// Words of the title, plus the bigram of two adjacent ones: `espaces verts`,
/// `gros oeuvre` and `courants faibles` are trades in two words whose halves
/// each point elsewhere.
fn jetons(titre: &str) -> HashSet<String> {
    let plie = fold(titre);
    let mots: Vec<&str> = plie
        .split(|c: char| !c.is_alphabetic())
        .filter(|m| LONGUEUR.contains(&m.chars().count()) && !LEXIQUE.vides.contains(m))
        .collect();
    mots.iter()
        .map(|m| (*m).to_string())
        .chain(mots.windows(2).map(|p| format!("{}_{}", p[0], p[1])))
        .collect()
}

/// Log-score gap under which a runner-up is posted beside the winner, capped at
/// `RETENUS`: the axis then names a trade the market really carries more often
/// than the winner alone does.
const MARGE: f32 = 4.0;
const RETENUS: usize = 3;

/// The `metier:<code>` a title reads as, for the engines that publish no
/// descriptor. Empty only when no token of the title is in the table.
pub fn metiers_du_titre(titre: &str) -> Vec<String> {
    let l = &*LEXIQUE;
    let mut scores: Vec<f32> = l.metiers.iter().map(|(_, prior, _)| *prior).collect();
    let mut vu = false;
    for mot in jetons(titre) {
        let Some(ecarts) = l.poids.get(mot.as_str()) else {
            continue;
        };
        vu = true;
        for (i, (_, _, defaut)) in l.metiers.iter().enumerate() {
            scores[i] += defaut;
        }
        for (i, ecart) in ecarts {
            scores[*i] += ecart;
        }
    }
    if !vu {
        return Vec::new();
    }
    let mut rangs: Vec<usize> = (0..scores.len()).collect();
    rangs.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]));
    let seuil = scores[rangs[0]] - MARGE;
    rangs.truncate(RETENUS);
    rangs
        .into_iter()
        .filter(|i| scores[*i] >= seuil)
        .map(|i| format!("metier:{}", l.metiers[i].0))
        .collect()
}

/// The fold of the descriptors, else the reading of the title, then the works lots.
pub fn metiers_du_marche<S: AsRef<str>, C: AsRef<str>>(
    descripteurs: &[S],
    codes_des_lots: &[C],
    titre: &str,
) -> Vec<String> {
    let repli = metiers_probables(descripteurs);
    let lus = match repli.is_empty() {
        true => metiers_du_titre(titre),
        false => repli,
    };
    avec_les_lots(lus, codes_des_lots)
}

#[cfg(test)]
mod tests {
    use lj_vocab::DIVISION_TRAVAUX;

    use super::*;

    #[test]
    fn deux_descripteurs_du_meme_corps_d_etat_ne_font_qu_un_metier() {
        assert_eq!(
            metiers_probables(&["Charpente", "Couverture", "Zinguerie"]),
            ["metier:charpente-couverture"]
        );
    }

    #[test]
    fn un_marche_traverse_les_metiers_qu_il_nomme_dans_l_ordre_de_la_table() {
        assert_eq!(
            metiers_probables(&["Voirie", "Espaces verts", "Assainissement"]),
            ["metier:eau", "metier:espaces-verts", "metier:vrd"]
        );
    }

    /// The BOAMP's own catch-all names no profession, and a market carrying it
    /// alone stays off the axis rather than landing in a bucket at random.
    #[test]
    fn le_fourre_tout_du_boamp_ne_rend_aucun_metier() {
        for d in ["Prestations de services", "Délégation de service public"] {
            assert!(metiers_probables(&[d]).is_empty(), "descripteur : {d}");
        }
    }

    /// The fund lost accents on some rows and the source is not stable on case.
    #[test]
    fn un_descripteur_deforme_retrouve_son_metier() {
        assert_eq!(
            metiers_probables(&["ESPACES VERTS"]),
            metiers_probables(&["Espaces verts"])
        );
    }

    #[test]
    fn un_intitule_de_metier_se_lit_sans_descripteur() {
        for (titre, attendu) in [
            (
                "Réfection de la couverture et de la charpente de l'église",
                "metier:charpente-couverture",
            ),
            (
                "Entretien des espaces verts et élagage des arbres communaux",
                "metier:espaces-verts",
            ),
            (
                "Fourniture de repas en liaison froide pour le restaurant scolaire",
                "metier:restauration",
            ),
        ] {
            assert_eq!(
                metiers_du_titre(titre).first().map(String::as_str),
                Some(attendu),
                "{titre}"
            );
        }
    }

    #[test]
    fn un_titre_vague_pose_plusieurs_metiers_et_un_titre_precis_un_seul() {
        let vague = metiers_du_titre("Réhabilitation de la salle polyvalente, lot unique");
        assert!(vague.len() > 1, "un seul métier posé : {vague:?}");
        assert!(vague.len() <= RETENUS, "{vague:?}");
        assert_eq!(
            metiers_du_titre("Fourniture et pose de menuiseries extérieures aluminium"),
            ["metier:menuiserie"]
        );
    }

    #[test]
    fn a_works_lot_names_its_trade_by_its_code() {
        let codes = [
            "45421000", "45261000", "45262660", "45223220", "45000000", "45262700",
        ];
        assert_eq!(
            metiers_publies(&["Gros oeuvre"], &codes),
            [
                "metier:gros-oeuvre",
                "metier:amiante",
                "metier:batiment-tce",
                "metier:charpente-couverture",
                "metier:menuiserie",
            ]
        );
        assert_eq!(
            metiers_du_marche(&[] as &[&str], &["45421000-4"], "Zzz"),
            ["metier:menuiserie"]
        );
        for code in ["45262000", "45452000", "71000000", "", "45"] {
            assert!(metier_du_code(code).is_none(), "{code}");
        }
    }

    #[test]
    fn un_titre_sans_aucun_jeton_connu_ne_rend_aucun_metier() {
        assert!(metiers_du_titre("Zzz").is_empty());
    }

    /// A code in one table and not the other serves a facet value the
    /// referential cannot label.
    #[test]
    fn les_deux_tables_portent_les_memes_metiers() {
        let fold: Vec<&str> = TABLE.metiers.iter().map(|(c, _)| *c).collect();
        let lex: Vec<&str> = LEXIQUE.metiers.iter().map(|(c, _, _)| *c).collect();
        for code in &fold {
            assert!(lex.contains(code), "absent du lexique : {code}");
        }
        for code in &lex {
            assert!(fold.contains(code), "absent du repli : {code}");
        }
    }

    #[test]
    fn chaque_metier_de_la_table_est_atteignable() {
        let codes: Vec<&str> = TABLE.metiers.iter().map(|(c, _)| *c).collect();
        for code in &codes {
            assert!(
                TABLE.replis.values().any(|c| c == code),
                "métier sans descripteur : {code}"
            );
        }
        for code in TABLE.replis.values() {
            assert!(codes.contains(code), "descripteur sans métier : {code}");
        }
        for (prefixe, code) in CODES.iter() {
            assert!(prefixe.starts_with(DIVISION_TRAVAUX), "{prefixe}");
            assert!(
                code.is_none_or(|c| codes.contains(&c)),
                "{prefixe} : {code:?}"
            );
        }
    }
}
