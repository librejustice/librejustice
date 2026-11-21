//! Structured parse of a formation: the SOURCE fields (CC chamber code,
//! Judilibre banner, registry formation from Judilibre/DILA) split into three
//! axes: the factual seat ([`lj_core::chamber::Seat`]), which carries the
//! matter naming the room (`matiere:*`), the kind of formation (`formation:*`)
//! and the role (`office:*` / `procedure:*`).
//!
//! Vocabulaire fermé, comparaisons pliées (`lj_fold::fold_chars`), regex
//! génériques de position et jamais par entrée. Résidu de greffe illisible
//! (jours d'audience TCOM, noms de magistrats TA…) = tous axes `None`.

use std::sync::OnceLock;

use lj_core::chamber::{ChamberId, Container, Joined, Seat, Solemn};
use regex::Regex;

use lj_fold::fold_chars;

/// Axes structurés d'une formation. Les uids sont des valeurs complètes de
/// `facet_value` (FK) ; le siège se stocke par sa clé canonique.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormationAxes {
    pub chamber: Seat,
    pub formation_uid: Option<&'static str>,
    pub office_uid: Option<&'static str>,
    pub procedure_uid: Option<&'static str>,
}

impl FormationAxes {
    pub fn is_empty(&self) -> bool {
        self.chamber.is_empty()
            && self.formation_uid.is_none()
            && self.office_uid.is_none()
            && self.procedure_uid.is_none()
    }

    /// Siège affichable, spécialisation comprise.
    pub fn chamber_label(&self) -> Option<String> {
        self.chamber.label()
    }
}

// ─────────────────────────── référentiels embarqués ─────────────────────────

/// Types de formation `formation:*` : (uid, label référentiel). Même contrat
/// de seed que `CHAMBRE_SEED`.
pub const FORMATION_SEED: &[(&str, &str)] = &[
    ("formation:A_TROIS", "Formation à trois"),
    ("formation:A_CINQ", "Formation à cinq"),
    ("formation:JUGE_UNIQUE", "Juge unique"),
    ("formation:CHAMBRE_SEULE", "Chambre jugeant seule"),
    ("formation:RESTREINTE", "Formation restreinte"),
    ("formation:SECTION", "Formation de section"),
    ("formation:PLENIERE", "Formation plénière"),
    ("formation:APPEL", "Statuant en appel"),
    ("formation:SPECIALE", "Spécialement composée"),
];

/// A special cour d'assises hearing an appeal is filed as special.
pub fn criminal_formation(
    jurisdiction_type: Option<&str>,
    formation: Option<&str>,
) -> Option<&'static str> {
    if jurisdiction_type != Some("ASSISES") {
        return None;
    }
    let folded = lj_fold::slug(formation?, ' ');
    let words: Vec<&str> = folded.split(' ').collect();
    if words.iter().any(|w| w.starts_with("special")) {
        Some("formation:SPECIALE")
    } else if words.contains(&"appel") {
        Some("formation:APPEL")
    } else {
        None
    }
}

/// Offices nouveaux portés par ce parse. Même contrat de seed.
pub const OFFICE_SEED_EXTRA: &[(&str, &str)] =
    &[("office:JUGE_EXPROPRIATION", "Juge de l'expropriation")];

// ─────────────────────────────── positions ──────────────────────────────────

/// Ordinaux textuels de greffe (« Première chambre », « Troisieme chambre »).
const WORD_ORDINALS: &[(&str, u32)] = &[
    ("premiere", 1),
    ("seconde", 2),
    ("deuxieme", 2),
    ("troisieme", 3),
    ("quatrieme", 4),
    ("cinquieme", 5),
    ("sixieme", 6),
    ("septieme", 7),
    ("huitieme", 8),
    ("neuvieme", 9),
    ("dixieme", 10),
    ("onzieme", 11),
    ("douzieme", 12),
];

struct PositionPatterns {
    ssr_liste: Regex,
    chambres_reunies_liste: Regex,
    sous_section: Regex,
    ss: Regex,
    pole: Regex,
    chamber_composee: Regex,
    chamber_ord: Regex,
    chamber_num: Regex,
    chamber_collee: Regex,
    chamber_mot: Regex,
    chamber_lettre: Regex,
    chamber_num_lettre: Regex,
    chamber_num_lettre_collee: Regex,
    chamber_ord_lettre: Regex,
    section_ord: Regex,
    section_num: Regex,
    section_courte: Regex,
    section_lettre: Regex,
    composee_lettre: Regex,
    spec_lettre: Regex,
    chamber_solo: Regex,
    nombres: Regex,
}

/// Suffixe ordinal OBLIGATOIRE (« 1ère », « 2eme », « 3e », « 2° ») : un chiffre
/// nu devant un mot-clé appartient au mot-clé précédent (« chambre 2 section 1 »
/// : « 2 section » n'est PAS la 2e section).
const ORD_SUFFIX_REQ: &str = r"(?:ere|eme|e|re|er|nde|°)";
const ORD_SUFFIX: &str = r"(?:ere|eme|e|re|er|nde|°)?";
/// Alias numérotables (« civil2 », « 2e civ ») ; les chambres LETTRÉES
/// n'acceptent que les graphies du mot chambre (« civile b » = section B
/// d'une chambre civile, pas la chambre B).
const CHAMBRE_ALIAS: &str = r"(?:chambre|chbre|ch|civile|civil|civ)";
const CHAMBRE_MOT_SEUL: &str = r"(?:chambre|chbre|ch)";

fn position_patterns() -> &'static PositionPatterns {
    static P: OnceLock<PositionPatterns> = OnceLock::new();
    P.get_or_init(|| PositionPatterns {
        // Liste de sous-sections réunies, tous séparateurs de greffe : « 2 / 6
        // ssr », « 1ère - 6ème ssr », « 8ème et 3ème sous-sections réunies »,
        // « 7 8 9 ssr », « 1 / 2 sous-sections reunies ».
        ssr_liste: Regex::new(&format!(
            r"\b(\d{{1,2}}\s*{ORD_SUFFIX}(?:\s*(?:et|[-/,]|\s)\s*\d{{1,2}}\s*{ORD_SUFFIX})*)\s*(?:ssr\b|sous[- ]sections?\s+reunies)|\bsous[- ]sections?\s+(\d{{1,2}}(?:\s*[-/,]\s*\d{{1,2}})+)\s+reunies"
        ))
        .unwrap(),
        // CE post-2016 : « 3ème - 8ème chambres réunies », « 1ère et 4ème
        // chambres réunies ».
        chambres_reunies_liste: Regex::new(&format!(
            r"\b(\d{{1,2}}\s*{ORD_SUFFIX}(?:\s*(?:et|[-/,])\s*\d{{1,2}}\s*{ORD_SUFFIX})+)\s*chambres\s+reunies|\bchambres\s+(\d{{1,2}}(?:\s*[-/,]\s*\d{{1,2}})+)\s+reunies"
        ))
        .unwrap(),
        sous_section: Regex::new(&format!(
            r"\b(\d{{1,2}})\s*{ORD_SUFFIX}\s*sous[- ]section\b"
        ))
        .unwrap(),
        // « 2 ss », « 10 ss. », « 4ème ssjs » — sous-section jugeant seule.
        ss: Regex::new(&format!(r"\b(\d{{1,2}})\s*{ORD_SUFFIX}\s*ss(?:js)?\b")).unwrap(),
        // « pole 1 - chambre 11 », « pole 5 ch 3 », « pole 2 ». La seconde
        // branche lit l'ordinal antéposé (« pole 5, 6e chambre ») : absente des
        // sources, c'est la graphie que nos propres libellés rendent.
        pole: Regex::new(&format!(
            r"\bpole[\s.]*n?°?\s*0*(\d{{1,2}})(?:\D{{1,15}}?(?:{CHAMBRE_ALIAS}[\s.]*n?°?\s*0*(\d{{1,2}})|0*(\d{{1,2}})\s*{ORD_SUFFIX_REQ}\s*{CHAMBRE_ALIAS}))?"
        ))
        .unwrap(),
        // TCOM « chambre 2-5 », CA « chambre civile 1-6 », « Ch.protection
        // sociale 4-7 » : jusqu'à deux mots interposés, points de greffe tolérés,
        // lettre de section (« chambre 4-8a ») ou suffixe matière (« Chambre 1-5DP »)
        // collés.
        chamber_composee: Regex::new(&format!(
            r"\b{CHAMBRE_MOT_SEUL}(?:[\s.]+[a-z']+){{0,2}}[\s.]*0*(\d{{1,2}})\s*-\s*0*(\d{{1,2}})(?:[a-h]|[a-z]{{2}})?\b"
        ))
        .unwrap(),
        // « 1ere chambre », « 2eme CH », « 3e chambre », « 2° chambre ».
        chamber_ord: Regex::new(&format!(
            r"\b0*(\d{{1,2}})\s*{ORD_SUFFIX_REQ}\s*{CHAMBRE_ALIAS}\b"
        ))
        .unwrap(),
        // « chambre 04 », « chambre n° 2 », « ch. 1 », « civil2 »,
        // « chambre-1 » (tiret de jonction — la composée « 2-5 » passe avant),
        // lettre de section collée tolérée (« Ch 9b » ; e = ordinal, h = heure).
        chamber_num: Regex::new(&format!(
            r"\b{CHAMBRE_ALIAS}[\s.\-]*n?°?\s*0*(\d{{1,2}})[a-dfg]?\b"
        ))
        .unwrap(),
        chamber_collee: Regex::new(r"\b0*(\d{1,2})(?:chambre|chbre|ch)\b").unwrap(),
        chamber_mot: Regex::new(&format!(
            r"\b(premiere|seconde|deuxieme|troisieme|quatrieme|cinquieme|sixieme|septieme|huitieme|neuvieme|dixieme|onzieme|douzieme)\s+{CHAMBRE_ALIAS}\b"
        ))
        .unwrap(),
        // « Chambre A », « Chambre B - Civile » ; the apostrophe is excluded,
        // « chambre d'appel » naming no letter D.
        chamber_lettre: Regex::new(&format!(r"\b{CHAMBRE_MOT_SEUL}\s+([a-h])(?:[^a-z0-9']|$)"))
            .unwrap(),
        // « Chambre 9 - B », « Pôle 4 - chambre 9 - A », « Chambre 1 A » —
        // lettre de section derrière le numéro de chambre. Séparateur REQUIS :
        // collé au chiffre, « e » est un ordinal (« chambre 2e section »).
        chamber_num_lettre: Regex::new(&format!(
            r"\b{CHAMBRE_MOT_SEUL}[\s.]*n?°?\s*0*\d{{1,2}}(?:\s*[-.]\s*|\s+)([a-h])(?:[^a-z0-9']|$)"
        ))
        .unwrap(),
        // « Ch 9b », « Chambre 5b » — lettre de section collée au numéro
        // (e = ordinal et h = heure exclus).
        chamber_num_lettre_collee: Regex::new(&format!(
            r"\b{CHAMBRE_MOT_SEUL}[\s.]*n?°?\s*0*\d{{1,2}}([a-dfg])\b"
        ))
        .unwrap(),
        // « 1re chambre B », « 8e chambre C ».
        chamber_ord_lettre: Regex::new(&format!(
            r"\b0*\d{{1,2}}\s*{ORD_SUFFIX_REQ}\s*{CHAMBRE_MOT_SEUL}(?:\s*[-.]\s*|\s+)([a-h])(?:[^a-z0-9']|$)"
        ))
        .unwrap(),
        // « 8e section », « 1ère sect - mesd » — suffixe ordinal REQUIS.
        section_ord: Regex::new(&format!(
            r"\b0*(\d{{1,2}})\s*{ORD_SUFFIX_REQ}\s*sect(?:ion)?\b"
        ))
        .unwrap(),
        // « section 1 », « sect. 2 » — mot-clé d'abord, prime sur l'ordinal.
        section_num: Regex::new(r"\bsect(?:ion)?[\s.]*n?°?\s*0*(\d{1,2})\b").unwrap(),
        // « S1 » (seulement adossé à une chambre).
        section_courte: Regex::new(r"\bs\s*0*(\d{1,2})\b").unwrap(),
        section_lettre: Regex::new(r"\bsect(?:ion)?[\s.]+([a-h])(?:[^a-z0-9']|$)").unwrap(),
        // « Chambre 4-8a » — lettre de section collée à la composée.
        composee_lettre: Regex::new(r"\b\d{1,2}\s*-\s*\d{1,2}([a-h])\b").unwrap(),
        // « Sociale A salle 1 », « Nationalité B », « filiation G » — lettre
        // de section portée par la spécialisation (l'apostrophe est exclue :
        // « baux d'habitation » ne porte pas de section D).
        spec_lettre: Regex::new(
            r"\b(?:sociale?|civile?|commerciale|correctionnelle|nationalite|famille|filiation)\s+([a-h])(?:[^a-z0-9']|$)",
        )
        .unwrap(),
        // « 2EME protection sociale » — ordinal suffixé isolé : numéro de la
        // chambre spécialisée (suffixe requis, jamais un chiffre nu).
        chamber_solo: Regex::new(&format!(r"\b0*(\d{{1,2}})\s*{ORD_SUFFIX_REQ}\b")).unwrap(),
        nombres: Regex::new(r"\d{1,2}").unwrap(),
    })
}

fn word_ordinal(w: &str) -> Option<u32> {
    WORD_ORDINALS.iter().find(|(s, _)| *s == w).map(|(_, n)| *n)
}

fn chamber_seat(c: ChamberId) -> Seat {
    Seat {
        chamber: Some(c),
        ..Default::default()
    }
}

fn joined_seat(r: Joined) -> Seat {
    Seat {
        joined: Some(r),
        ..Default::default()
    }
}

/// The administrative order, where a section contains the chambers instead of
/// dividing one.
fn admin_order(jurisdiction_type: Option<&str>) -> bool {
    matches!(jurisdiction_type, Some("TA" | "CAA" | "CE"))
}

fn parse_position(jurisdiction_type: Option<&str>, folded: &str) -> Option<Seat> {
    let p = position_patterns();
    if let Some(c) = p.ssr_liste.captures(folded) {
        let liste = c.get(1).or_else(|| c.get(2))?;
        let nums: Vec<u32> = p
            .nombres
            .find_iter(liste.as_str())
            .filter_map(|m| m.as_str().parse().ok())
            .collect();
        return Some(match nums.as_slice() {
            [seul] => joined_seat(Joined::SubSections(vec![*seul])),
            _ => joined_seat(Joined::SubSections(nums)),
        });
    }
    if let Some(c) = p.chambres_reunies_liste.captures(folded) {
        let liste = c.get(1).or_else(|| c.get(2))?;
        let nums: Vec<u32> = p
            .nombres
            .find_iter(liste.as_str())
            .filter_map(|m| m.as_str().parse().ok())
            .collect();
        if nums.len() > 1 {
            return Some(joined_seat(Joined::Chambers(nums)));
        }
    }
    if let Some(c) = p.sous_section.captures(folded) {
        if let Some(n) = c[1].parse().ok().filter(|n| *n > 0) {
            return Some(joined_seat(Joined::SubSections(vec![n])));
        }
    }
    if let Some(c) = p.ss.captures(folded) {
        if let Some(n) = c[1].parse().ok().filter(|n| *n > 0) {
            return Some(joined_seat(Joined::SubSections(vec![n])));
        }
    }
    if let Some(c) = p.pole.captures(folded) {
        if let Some(n) = c[1].parse().ok().filter(|n| *n > 0) {
            return Some(Seat {
                container: Some(Container::Pole(n)),
                chamber: c
                    .get(2)
                    .or_else(|| c.get(3))
                    .and_then(|m| m.as_str().parse().ok())
                    .filter(|n| *n > 0)
                    .map(ChamberId::Number),
                ..Default::default()
            });
        }
    }
    if let Some(c) = p.chamber_composee.captures(folded) {
        let a: u32 = c[1].parse().ok()?;
        let b: u32 = c[2].parse().ok()?;
        // Un zéro est un placeholder de greffe, pas une position.
        if a > 0 && b > 0 {
            return Some(chamber_seat(ChamberId::Composed(a, b)));
        }
    }
    let chamber: Option<u32> = p
        .chamber_ord
        .captures(folded)
        .or_else(|| p.chamber_num.captures(folded))
        .or_else(|| p.chamber_collee.captures(folded))
        .and_then(|c| c[1].parse().ok())
        .filter(|n| *n > 0)
        .or_else(|| {
            p.chamber_mot
                .captures(folded)
                .and_then(|c| word_ordinal(&c[1]))
        });
    // Mot-clé d'abord (« section 1 » : le numéro APRÈS le mot), l'ordinal en
    // repli — un chiffre nu devant « section » appartient au mot-clé
    // précédent (« chambre 2 section 1 »).
    let mut section: Option<u32> = p
        .section_num
        .captures(folded)
        .or_else(|| p.section_ord.captures(folded))
        .and_then(|c| c[1].parse().ok())
        .filter(|n| *n > 0);
    if section.is_none() && chamber.is_some() {
        // « 11ème civ. S1 » : la graphie courte ne vaut section qu'adossée
        // à une chambre, seule, « S3 » est du code de greffe.
        section = p
            .section_courte
            .captures(folded)
            .and_then(|c| c[1].parse().ok())
            .filter(|n| *n > 0);
    }
    if chamber.is_some() || section.is_some() {
        let mut seat = Seat {
            chamber: chamber.map(ChamberId::Number),
            ..Default::default()
        };
        // The section contains the chambers in the administrative order and
        // divides one in the judicial order. The order the registry wrote the
        // two in says nothing about that.
        match section {
            Some(n) if admin_order(jurisdiction_type) => {
                seat.container = Some(Container::Section(n))
            }
            n => seat.section = n,
        }
        return Some(seat);
    }
    if let Some(c) = p.chamber_lettre.captures(folded) {
        let letter = c.get(1)?;
        let rest = folded[letter.end()..].trim_start();
        // « Chambre à 3 juges » names no letter A.
        let preposition =
            letter.as_str() == "a" && rest.starts_with(|ch: char| ch.is_ascii_digit());
        if !preposition {
            return Some(chamber_seat(ChamberId::Letter(
                letter.as_str().chars().next()?.to_ascii_uppercase(),
            )));
        }
    }
    None
}

/// Whether a registry key carries anything besides the chamber's number and the
/// word for it: « Deuxième Chambre » does not, « Ch4.3 JCP » and « Chambre 04
/// JEX » do, and name a sitting of the chamber rather than the chamber.
pub fn says_more_than_the_number(key: &str) -> bool {
    static REST: OnceLock<Regex> = OnceLock::new();
    let rest = REST.get_or_init(|| {
        Regex::new(
            r"\b(?:chambre|chbre|ch|premiere|seconde|deuxieme|troisieme|quatrieme|cinquieme|sixieme|septieme|huitieme|neuvieme|dixieme|onzieme|douzieme)\b|\b0*\d{1,2}\s*(?:ere|eme|e|re|er|nde|°)?|n°|[^a-z]",
        )
        .unwrap()
    });
    rest.replace_all(&fold_chars(key), "")
        .chars()
        .any(|c| c.is_ascii_alphabetic())
}

// ──────────────────────────── lexiques pliés ────────────────────────────────

/// `phrase` (déjà pliée) présente dans `folded` avec bornes non
/// alphanumériques de part et d'autre — renvoie le span du match (pour la
/// consommation par les lexiques suivants).
fn find_phrase(folded: &str, phrase: &str) -> Option<(usize, usize)> {
    let mut start = 0;
    while let Some(idx) = folded[start..].find(phrase) {
        let at = start + idx;
        let before_ok = at == 0 || !folded[..at].chars().next_back().unwrap().is_alphanumeric();
        let end = at + phrase.len();
        let after_ok =
            end == folded.len() || !folded[end..].chars().next().unwrap().is_alphanumeric();
        if before_ok && after_ok {
            return Some((at, end));
        }
        start = at + phrase.len().max(1);
    }
    None
}

fn contains_phrase(folded: &str, phrase: &str) -> bool {
    find_phrase(folded, phrase).is_some()
}

/// Recolle les suites de tokens mono-caractère alphanumériques (graphies de
/// greffe éclatées : « r e f e r e » → « refere », « j a f » → « jaf »,
/// « r j l j » → « rjlj »). `None` si aucune suite (≥ 2) à recoller.
fn collapse_single_letters(folded: &str) -> Option<String> {
    let tokens: Vec<&str> = folded.split(' ').collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut run: Vec<&str> = Vec::new();
    let mut collapsed_any = false;
    for tok in tokens.iter().chain(std::iter::once(&"")) {
        if tok.len() == 1 && tok.chars().all(char::is_alphanumeric) {
            run.push(tok);
            continue;
        }
        match run.len() {
            0 => {}
            1 => out.push(run[0].to_string()),
            _ => {
                out.push(run.concat());
                collapsed_any = true;
            }
        }
        run.clear();
        if !tok.is_empty() {
            out.push((*tok).to_string());
        }
    }
    collapsed_any.then(|| out.join(" "))
}

/// Spécialisations : phrases pliées → uid. L'ordre fait autorité (les motifs
/// englobants — « protection sociale » avant « sociale » — passent d'abord).
const CHAMBRE_PHRASES: &[(&str, &str)] = &[
    ("protection sociale", "matiere:PROTECTION_SOCIALE"),
    ("protection soc", "matiere:PROTECTION_SOCIALE"),
    ("securite sociale", "matiere:PROTECTION_SOCIALE"),
    ("sec soc", "matiere:PROTECTION_SOCIALE"),
    ("secu", "matiere:PROTECTION_SOCIALE"),
    ("ctx technique", "matiere:PROTECTION_SOCIALE"),
    ("contentieux sociaux", "matiere:PROTECTION_SOCIALE"),
    ("tarification", "matiere:PROTECTION_SOCIALE"),
    ("tass", "matiere:PROTECTION_SOCIALE"),
    ("procedures collectives", "matiere:PROCEDURES_COLLECTIVES"),
    ("procedure collective", "matiere:PROCEDURES_COLLECTIVES"),
    ("procedure collectives", "matiere:PROCEDURES_COLLECTIVES"),
    ("procedures collect", "matiere:PROCEDURES_COLLECTIVES"),
    ("proc collectives", "matiere:PROCEDURES_COLLECTIVES"),
    ("proc coll", "matiere:PROCEDURES_COLLECTIVES"),
    ("pcl", "matiere:PROCEDURES_COLLECTIVES"),
    ("liquidation judiciaire", "matiere:PROCEDURES_COLLECTIVES"),
    ("liquidations judiciaires", "matiere:PROCEDURES_COLLECTIVES"),
    ("liquid judiciaire", "matiere:PROCEDURES_COLLECTIVES"),
    ("redressement judiciaire", "matiere:PROCEDURES_COLLECTIVES"),
    (
        "redressements judiciaires",
        "matiere:PROCEDURES_COLLECTIVES",
    ),
    ("cessation des paiements", "matiere:PROCEDURES_COLLECTIVES"),
    ("juge commissaire", "matiere:PROCEDURES_COLLECTIVES"),
    ("prud'homale", "matiere:PRUD_HOMALE"),
    ("prud'hommale", "matiere:PRUD_HOMALE"),
    ("prud'hommes", "matiere:PRUD_HOMALE"),
    ("prudhomale", "matiere:PRUD_HOMALE"),
    ("prudhommale", "matiere:PRUD_HOMALE"),
    ("prudhommes", "matiere:PRUD_HOMALE"),
    ("loyers commerciaux", "matiere:BAUX"),
    ("loyer commerciaux", "matiere:BAUX"),
    ("baux commerciaux", "matiere:BAUX"),
    ("baux ruraux", "matiere:BAUX"),
    ("baux", "matiere:BAUX"),
    ("tpbr", "matiere:BAUX"),
    ("sociale", "matiere:SOCIALE"),
    ("social", "matiere:SOCIALE"),
    ("commerciale", "matiere:COMMERCIALE"),
    ("commercial", "matiere:COMMERCIALE"),
    ("commerce", "matiere:COMMERCIALE"),
    ("economique", "matiere:COMMERCIALE"),
    ("ecocom", "matiere:COMMERCIALE"),
    ("chcom", "matiere:COMMERCIALE"),
    // Before the civil matter: « 17ème Ch. Presse-civile » names a press room.
    ("presse", "matiere:PRESSE"),
    ("civile", "matiere:CIVILE"),
    ("civiles", "matiere:CIVILE"),
    ("civil", "matiere:CIVILE"),
    ("civ", "matiere:CIVILE"),
    ("criminelle", "matiere:CRIMINELLE"),
    ("correctionnelle", "matiere:CORRECTIONNELLE"),
    ("correctionnel", "matiere:CORRECTIONNELLE"),
    ("appels correctionnels", "matiere:CORRECTIONNELLE"),
    ("chambre correct", "matiere:CORRECTIONNELLE"),
    ("instruction", "matiere:INSTRUCTION"),
    ("famille", "matiere:FAMILLE"),
    ("affaires familiales", "matiere:FAMILLE"),
    ("aff familiales", "matiere:FAMILLE"),
    ("divorces", "matiere:FAMILLE"),
    ("divorce", "matiere:FAMILLE"),
    ("filiation", "matiere:FAMILLE"),
    ("construction", "matiere:CONSTRUCTION"),
    ("etrangers", "matiere:ETRANGERS"),
    ("etranger", "matiere:ETRANGERS"),
    ("retention administrative", "matiere:ETRANGERS"),
    ("retentions", "matiere:ETRANGERS"),
    ("retention", "matiere:ETRANGERS"),
    ("reconduite a la frontiere", "matiere:ETRANGERS"),
    ("reconduites a la frontiere", "matiere:ETRANGERS"),
    ("reconduites", "matiere:ETRANGERS"),
    ("reconduite", "matiere:ETRANGERS"),
    ("eloignement", "matiere:ETRANGERS"),
    ("oqtf", "matiere:ETRANGERS"),
    ("asile", "matiere:ETRANGERS"),
    ("96h", "matiere:ETRANGERS"),
    ("96 h", "matiere:ETRANGERS"),
    ("expropriations", "matiere:EXPROPRIATION"),
    ("expropriation", "matiere:EXPROPRIATION"),
    ("expro", "matiere:EXPROPRIATION"),
    ("proximite", "matiere:PROXIMITE"),
    ("proxi", "matiere:PROXIMITE"),
    ("pprox", "matiere:PROXIMITE"),
    ("prox", "matiere:PROXIMITE"),
    ("tprox", "matiere:PROXIMITE"),
    ("tprx", "matiere:PROXIMITE"),
    ("tpx", "matiere:PROXIMITE"),
    ("surendettement", "matiere:SURENDETTEMENT"),
    ("surendettemment", "matiere:SURENDETTEMENT"),
    ("retablissement personnel", "matiere:SURENDETTEMENT"),
    ("surend", "matiere:SURENDETTEMENT"),
    ("surendet", "matiere:SURENDETTEMENT"),
    ("surdt", "matiere:SURENDETTEMENT"),
    ("copropriete", "matiere:COPROPRIETE"),
    ("dalo", "matiere:DALO"),
    ("mineurs", "matiere:MINEURS"),
    ("nationalite", "matiere:NATIONALITE"),
    ("elections pro", "matiere:SOCIALE"),
    ("urgences", "matiere:URGENCES"),
    ("urgence", "matiere:URGENCES"),
    ("chambre des terres", "matiere:TERRES"),
    ("tribunal foncier", "matiere:TERRES"),
    ("civip", "matiere:CIVI"),
    ("civi", "matiere:CIVI"),
];

/// The two matters the source joins into ONE name, in the order it wrote them:
/// « 1re chambre civile et commerciale ». The table above reads one matter per
/// part, by ITS order, and would keep whichever of the two it lists first.
fn composed_specialty(folded: &str) -> Option<[&'static str; 2]> {
    (0..folded.len())
        .filter(|at| folded.is_char_boundary(*at))
        .find_map(|at| {
            let (first, end) = specialty_at(folded, at, false)?;
            let after = joining_word(folded, end)?;
            let (second, _) = specialty_at(folded, after, true)?;
            (second != first).then_some([first, second])
        })
}

/// Longest matter starting exactly at `at`, both ends on a word boundary.
/// `abbreviated` adds « com. », a registry shorthand that only ever stands
/// between two matters.
fn specialty_at(folded: &str, at: usize, abbreviated: bool) -> Option<(&'static str, usize)> {
    if folded[..at].ends_with(char::is_alphanumeric) {
        return None;
    }
    let rest = &folded[at..];
    CHAMBRE_PHRASES
        .iter()
        .copied()
        .chain(abbreviated.then_some(("com", "matiere:COMMERCIALE")))
        .filter(|(phrase, _)| {
            rest.strip_prefix(phrase)
                .is_some_and(|r| !r.starts_with(char::is_alphanumeric))
        })
        .max_by_key(|(phrase, _)| phrase.len())
        .map(|(phrase, uid)| (uid, at + phrase.len()))
}

/// Offset of what a written joiner introduces. Side by side joins nothing:
/// « Presse-civile » names a room and its section, not one room of two matters.
fn joining_word(folded: &str, end: usize) -> Option<usize> {
    let rest = folded[end..].trim_start();
    let after = ["&", "/"]
        .into_iter()
        .find_map(|sign| rest.strip_prefix(sign))
        .or_else(|| {
            rest.strip_prefix("et")
                .filter(|r| !r.starts_with(char::is_alphanumeric))
        })?;
    Some(folded.len() - after.trim_start().len())
}

/// Types de formation : phrases pliées → uid, englobants d'abord.
const FORMATION_PHRASES: &[(&str, &str)] = &[
    ("formation a 3", "formation:A_TROIS"),
    ("formation a trois", "formation:A_TROIS"),
    ("formation de 3", "formation:A_TROIS"),
    ("formation de trois", "formation:A_TROIS"),
    ("formation a 5", "formation:A_CINQ"),
    ("formation a cinq", "formation:A_CINQ"),
    ("formation de 5", "formation:A_CINQ"),
    ("formation collegiale", "formation:A_TROIS"),
    ("formation restreinte", "formation:RESTREINTE"),
    ("formation de section", "formation:SECTION"),
    ("section du contentieux", "solemn:SECTION_CONTENTIEUX"),
    ("formation pleniere", "formation:PLENIERE"),
    ("pleniere de chambre", "formation:PLENIERE"),
    ("assemblee pleniere", "solemn:ASSEMBLEE_PLENIERE"),
    ("assemblee du contentieux", "solemn:ASSEMBLEE"),
    ("formation specialisee", "solemn:SPECIALISEE"),
    ("formation consultative", "solemn:AVIS"),
    // Two courts that do not name themselves alike do not share a variant: the
    // Cassation sits in « chambre mixte », the commercial courts in a
    // « formation mixte ».
    ("formation mixte", "solemn:FORMATION_MIXTE"),
    ("chambre mixte", "solemn:CHAMBRE_MIXTE"),
    ("chambres reunies", "solemn:CHAMBRES_REUNIES"),
    ("chambre reunies", "solemn:CHAMBRES_REUNIES"),
    ("formation elargie", "formation:PLENIERE"),
    ("formations reunies", "solemn:CHAMBRES_REUNIES"),
    ("formation a 2 chambres", "solemn:CHAMBRES_REUNIES"),
    ("jugeant seule", "formation:CHAMBRE_SEULE"),
    ("juge unique", "formation:JUGE_UNIQUE"),
    ("a juge unique", "formation:JUGE_UNIQUE"),
    ("juge seul", "formation:JUGE_UNIQUE"),
    ("magistrat statuant seul", "formation:JUGE_UNIQUE"),
    ("statuant seul", "formation:JUGE_UNIQUE"),
    // Contentieux à juge unique identifiés par leur régime : litiges ≤ 10 000 €
    // (art. R. 212-8 COJ), étrangers 15 jours / 72 h, magistrat désigné
    // R. 222-13 CJA, « JU OQTF 6 semaines ».
    ("10 000", "formation:JUGE_UNIQUE"),
    ("10000", "formation:JUGE_UNIQUE"),
    ("r222 13", "formation:JUGE_UNIQUE"),
    // Forme lexicale à séparateurs normalisés (« - R. 222-13 » → « r 222 13 »).
    ("r 222 13", "formation:JUGE_UNIQUE"),
    ("15 jours", "formation:JUGE_UNIQUE"),
    ("72 heures", "formation:JUGE_UNIQUE"),
    ("ju oqtf", "formation:JUGE_UNIQUE"),
    // Abréviation de greffe TA : « 10ème chambre (JU) ».
    ("ju", "formation:JUGE_UNIQUE"),
];

/// Rôles → office. Phrases pliées, englobants d'abord (le JLD avant « juge »).
const OFFICE_PHRASES: &[(&str, &str)] = &[
    // A room and not a role, but it is read here so that its span is consumed
    // before the section behind it names a formation.
    (
        "president de la section du contentieux",
        "solemn:PRESIDENT_SECTION_CONTENTIEUX",
    ),
    ("juge des libertes et de la detention", "office:JLD"),
    ("juge des libertes", "office:JLD"),
    ("juge libertes", "office:JLD"),
    ("juge liberte", "office:JLD"),
    ("libertes detention", "office:JLD"),
    ("libertes & detention", "office:JLD"),
    ("liberte et detention", "office:JLD"),
    ("libertes et detention", "office:JLD"),
    ("hsc", "office:JLD"),
    ("jld", "office:JLD"),
    ("recoursjld", "office:JLD"),
    ("hospitalisation", "office:JLD"),
    ("hospital", "office:JLD"),
    ("hospit", "office:JLD"),
    ("soins psychiatriques", "office:JLD"),
    ("soins psychiatriq", "office:JLD"),
    ("soins contraints", "office:JLD"),
    ("juge des contentieux de la protection", "office:JCP"),
    ("contentieux de la protection", "office:JCP"),
    ("ctx de la protection", "office:JCP"),
    ("cx protection", "office:JCP"),
    ("contentieux protecti", "office:JCP"),
    ("jugecontentieuxprotection", "office:JCP"),
    ("juge ctx protection", "office:JCP"),
    ("credit consommation", "office:JCP"),
    ("credits consommation", "office:JCP"),
    ("jcp", "office:JCP"),
    ("juge de l'execution", "office:JEX"),
    ("jex", "office:JEX"),
    ("saisies immobilieres", "office:JEX"),
    ("saisie immobiliere", "office:JEX"),
    ("ventes", "office:JEX"),
    ("adjudications", "office:JEX"),
    ("criees", "office:JEX"),
    ("juge aux affaires familiales", "office:JAF"),
    ("jaf", "office:JAF"),
    ("juge des enfants", "office:JUGE_ENFANTS"),
    (
        "ordonnance du premier president",
        "office:PREMIER_PRESIDENT",
    ),
    ("premier president", "office:PREMIER_PRESIDENT"),
    ("premiere presidence", "office:PREMIER_PRESIDENT"),
    ("attributions pp", "office:PREMIER_PRESIDENT"),
    ("magistrat designe", "office:MAGISTRAT_DESIGNE"),
    ("juge de l'expropriation", "office:JUGE_EXPROPRIATION"),
    // Two rooms and not roles, read on their FULL graphie: bare, « référés » is
    // a procedure suffix, and it would consume their span from below.
    ("service des referes", "matiere:REFERES"),
    ("chambre des referes", "matiere:REFERES"),
    ("juge des referes", "office:JUGE_REFERES"),
    ("juges des referes", "office:JUGE_REFERES"),
    ("referes", "office:JUGE_REFERES"),
    ("refere", "office:JUGE_REFERES"),
    ("refers", "office:JUGE_REFERES"),
];

/// Voies lisibles directement dans la formation greffe.
const VOIE_PHRASES: &[(&str, &str)] = &[
    ("qpc", "procedure:QPC"),
    ("chambre du conseil", "procedure:CHAMBRE_DU_CONSEIL"),
    ("chbre du conseil", "procedure:CHAMBRE_DU_CONSEIL"),
    ("ch du conseil", "procedure:CHAMBRE_DU_CONSEIL"),
];

/// Chaîne ENTIÈRE (pliée, séparateurs normalisés) → axe. Pour les mots trop
/// ambigus en simple `contains` (« Section » seul = jugement en Section du
/// contentieux CE, mais « 8e section » = position).
const WHOLE_STRING: &[(&str, &str)] = &[
    ("section", "solemn:SECTION_CONTENTIEUX"),
    ("avis section", "solemn:SECTION_CONTENTIEUX"),
    ("assemblee", "solemn:ASSEMBLEE"),
    ("pleniere", "formation:PLENIERE"),
    ("rj", "matiere:PROCEDURES_COLLECTIVES"),
    ("lj", "matiere:PROCEDURES_COLLECTIVES"),
    ("rj lj", "matiere:PROCEDURES_COLLECTIVES"),
    ("rjlj", "matiere:PROCEDURES_COLLECTIVES"),
    ("rlj", "matiere:PROCEDURES_COLLECTIVES"),
    ("sauvegarde", "matiere:PROCEDURES_COLLECTIVES"),
];

/// Acronymes en graphie éclatée (« J.E.X », « R E F E R E », « C.E.S.E.D.A. »),
/// appariés sur la forme compactée (alphanumériques seuls) des chaînes courtes.
const COMPACT_ACRONYMS: &[(&str, &str)] = &[
    ("jld", "office:JLD"),
    ("jcp", "office:JCP"),
    ("jex", "office:JEX"),
    ("jaf", "office:JAF"),
    ("refere", "office:JUGE_REFERES"),
    ("referes", "office:JUGE_REFERES"),
    ("ceseda", "matiere:ETRANGERS"),
];

// ────────────────────────────── chambres CC ─────────────────────────────────

/// Vocabulaire fermé Cassation : code Judilibre + label plié → axes. Les
/// chambres civiles sont numérotées, les autres n'ont pas de position.
struct CcChamber {
    code: &'static str,
    folded_label: &'static str,
    chamber_number: Option<u32>,
    matiere_uid: Option<&'static str>,
    solemn: Option<Solemn>,
    office_uid: Option<&'static str>,
}

const CC_CHAMBERS: &[CcChamber] = &[
    CcChamber {
        code: "civ1",
        folded_label: "premiere chambre civile",
        chamber_number: Some(1),
        matiere_uid: Some("matiere:CIVILE"),
        solemn: None,
        office_uid: None,
    },
    CcChamber {
        code: "civ2",
        folded_label: "deuxieme chambre civile",
        chamber_number: Some(2),
        matiere_uid: Some("matiere:CIVILE"),
        solemn: None,
        office_uid: None,
    },
    CcChamber {
        code: "civ3",
        folded_label: "troisieme chambre civile",
        chamber_number: Some(3),
        matiere_uid: Some("matiere:CIVILE"),
        solemn: None,
        office_uid: None,
    },
    CcChamber {
        code: "soc",
        folded_label: "chambre sociale",
        chamber_number: None,
        matiere_uid: Some("matiere:SOCIALE"),
        solemn: None,
        office_uid: None,
    },
    CcChamber {
        code: "comm",
        folded_label: "chambre commerciale",
        chamber_number: None,
        matiere_uid: Some("matiere:COMMERCIALE"),
        solemn: None,
        office_uid: None,
    },
    CcChamber {
        code: "comm",
        folded_label: "chambre commerciale financiere et economique",
        chamber_number: None,
        matiere_uid: Some("matiere:COMMERCIALE_FINANCIERE_ECONOMIQUE"),
        solemn: None,
        office_uid: None,
    },
    CcChamber {
        code: "cr",
        folded_label: "chambre criminelle",
        chamber_number: None,
        matiere_uid: Some("matiere:CRIMINELLE"),
        solemn: None,
        office_uid: None,
    },
    CcChamber {
        code: "mi",
        folded_label: "chambre mixte",
        chamber_number: None,
        matiere_uid: None,
        solemn: Some(Solemn::ChambreMixte),
        office_uid: None,
    },
    CcChamber {
        code: "pl",
        folded_label: "assemblee pleniere",
        chamber_number: None,
        matiere_uid: None,
        solemn: Some(Solemn::AssembleePleniere),
        office_uid: None,
    },
    CcChamber {
        code: "ord",
        folded_label: "ordonnance du premier president",
        chamber_number: None,
        matiere_uid: None,
        solemn: None,
        office_uid: Some("office:PREMIER_PRESIDENT"),
    },
    CcChamber {
        code: "ordo",
        folded_label: "ordonnance du premier president",
        chamber_number: None,
        matiere_uid: None,
        solemn: None,
        office_uid: Some("office:PREMIER_PRESIDENT"),
    },
    CcChamber {
        code: "creun",
        folded_label: "chambre reunies",
        chamber_number: None,
        matiere_uid: None,
        solemn: Some(Solemn::ChambresReunies),
        office_uid: None,
    },
    CcChamber {
        code: "creun",
        folded_label: "chambres reunies",
        chamber_number: None,
        matiere_uid: None,
        solemn: Some(Solemn::ChambresReunies),
        office_uid: None,
    },
    CcChamber {
        code: "allciv",
        folded_label: "toutes chambres civiles",
        chamber_number: None,
        matiere_uid: Some("matiere:CIVILE"),
        solemn: None,
        office_uid: None,
    },
];

/// Arrêts print the full name from spring 1989: none in January, all from July.
const COMMERCIAL_RENAMED_ON: &str = "1989-03-01";

/// The code `comm` names the commercial chamber at every date.
pub fn name_on_date(axes: &mut FormationAxes, date_lecture: Option<&str>) {
    const NAMES: [&str; 2] = [
        "matiere:COMMERCIALE",
        "matiere:COMMERCIALE_FINANCIERE_ECONOMIQUE",
    ];
    let specialty = &mut axes.chamber.specialty;
    let Some(date) = date_lecture else {
        return;
    };
    if !specialty.is_empty() && specialty.iter().all(|u| NAMES.contains(u)) {
        *specialty = vec![NAMES[usize::from(date >= COMMERCIAL_RENAMED_ON)]];
    }
}

fn cc_by_code(code: &str) -> Option<&'static CcChamber> {
    let lower = code.to_lowercase();
    CC_CHAMBERS.iter().find(|c| c.code == lower)
}

fn cc_by_label(folded: &str) -> Option<&'static CcChamber> {
    CC_CHAMBERS.iter().find(|c| c.folded_label == folded)
}

// ───────────────────────────────── parse ────────────────────────────────────

/// Le vocabulaire fermé décide le siège : `locked` interdit à une part
/// suivante d'y poser une position.
fn apply_cc(cc: &CcChamber, axes: &mut FormationAxes, state: &mut PositionState) {
    state.position = cc
        .chamber_number
        .map(|n| chamber_seat(ChamberId::Number(n)));
    state.solemn = cc.solemn;
    state.locked = true;
    if let Some(uid) = cc.matiere_uid {
        state.latch_specialty(uid);
    }
    axes.office_uid = axes.office_uid.or(cc.office_uid);
}

/// État positionnel accumulé sur les parts (premier écrivain), composé en
/// display par [`finish`] : position structurelle + section lettrée
/// (« Chambre sociale section B », « 2ème chambre section A »).
#[derive(Debug, Clone, Default)]
struct PositionState {
    position: Option<Seat>,
    section_lettre: Option<char>,
    /// Le vocabulaire fermé Cassation a parlé : plus aucune position ni
    /// lettre ne s'ajoute.
    locked: bool,
    /// Matters latched across the parts, in reading order.
    specialty: Vec<&'static str>,
    /// The solemn formation the parts have named, if any.
    solemn: Option<Solemn>,
}

impl PositionState {
    /// First writer per matter: one written twice by two parts stays one, a
    /// different one extends the name of the room instead of being dropped.
    fn latch_specialty(&mut self, uid: &'static str) {
        if !self.specialty.contains(&uid) {
            self.specialty.push(uid);
        }
    }

    /// First writer, as for the position: a solemn formation is a room, so a
    /// later part does not rename the one already read.
    fn latch_solemn(&mut self, solemn: Solemn) {
        if !self.locked && self.solemn.is_none() {
            self.solemn = Some(solemn);
        }
    }
}

/// Parse une part (champ chambre, bandeau ou formation greffe) et fusionne
/// ses axes dans `axes` (premier écrivain par axe) ; le positionnel sort dans
/// `state` (premier écrivain), le display n'est composé qu'en [`finish`] une
/// fois TOUTES les parts lues — l'adjectif de spécialisation peut venir d'une
/// autre part que la position (« 1ère Chambre » greffe + « chambre civile »
/// bandeau → « 1re chambre civile »). Retourne `true` si la part a produit au
/// moins un signal.
fn parse_part(
    jurisdiction_type: Option<&str>,
    part: &str,
    axes: &mut FormationAxes,
    state: &mut PositionState,
) -> bool {
    let folded_raw = fold_chars(part);
    // Forme lexicale : séparateurs de greffe normalisés en espaces (le tiret
    // aussi — « juge-commissaire » ; les positions gardent le leur via
    // `folded_raw` : « chambre 2-5 », « 2 / 6 ssr »).
    let folded = folded_raw
        .replace(['_', '/', '.', ':', ',', '(', ')', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if folded.is_empty() {
        return false;
    }
    let mut hit = false;

    // Chambre CC nommée (label institutionnel verbatim). Si une position est
    // déjà posée par une part antérieure, seul l'apport d'axes compte — le
    // display verbatim céderait la position.
    if let Some(cc) = cc_by_label(&folded) {
        if !state.locked && state.specialty.is_empty() && state.position.is_none() {
            apply_cc(cc, axes, state);
        } else {
            for uid in [cc.matiere_uid, cc.office_uid].into_iter().flatten() {
                merge_uid(axes, state, uid);
            }
            if let Some(solemn) = cc.solemn {
                state.latch_solemn(solemn);
            }
        }
        return true;
    }

    // Chaîne entière ambiguë en contains (« Section », « Assemblée »,
    // « RJ ») — aussi sur la forme recollée des graphies éclatées
    // (« R.J. L.J. » → « rjlj »).
    let collapsed_whole = collapse_single_letters(&folded);
    for (whole, uid) in WHOLE_STRING {
        if folded == *whole || collapsed_whole.as_deref() == Some(*whole) {
            merge_uid(axes, state, uid);
            return true;
        }
    }

    // Position structurelle — sur le folded BRUT (les « 2 / 6 ssr » portent
    // leur slash, les « chambre 2-5 » leur tiret).
    let part_position = parse_position(jurisdiction_type, &folded_raw);

    if let Some(composed) = composed_specialty(&folded_raw) {
        for uid in composed {
            merge_uid(axes, state, uid);
        }
        hit = true;
    }

    // Lexiques par axe, avec CONSOMMATION : la phrase matched est blanchie
    // avant les lexiques suivants (« président de la section du contentieux »
    // est un office — le mot « section » consommé ne vote plus la formation).
    // Le collapse recolle les graphies éclatées (« J.A.F. » → « jaf »).
    let mut lexical = folded;
    for phrases in [
        OFFICE_PHRASES,
        VOIE_PHRASES,
        FORMATION_PHRASES,
        CHAMBRE_PHRASES,
    ] {
        let collapsed = collapse_single_letters(&lexical);
        for (phrase, uid) in phrases {
            if let Some((a, b)) = find_phrase(&lexical, phrase) {
                merge_uid(axes, state, uid);
                lexical.replace_range(a..b, &" ".repeat(b - a));
                hit = true;
                break;
            }
            if collapsed
                .as_deref()
                .is_some_and(|c| contains_phrase(c, phrase))
            {
                merge_uid(axes, state, uid);
                hit = true;
                break;
            }
        }
    }

    // Acronymes éclatés (« J.E.X », « R E F E R E ») sur la forme compacte.
    if !hit {
        let compact: String = folded_raw.chars().filter(|c| c.is_alphanumeric()).collect();
        if compact.len() <= 12 {
            for (acr, uid) in COMPACT_ACRONYMS {
                if compact.contains(acr) {
                    merge_uid(axes, state, uid);
                    hit = true;
                    break;
                }
            }
            // « Ju1 », « JU2 », « Ju-6 semaines » — juge unique numéroté TA.
            if !hit
                && compact.starts_with("ju")
                && compact.chars().nth(2).is_some_and(|c| c.is_ascii_digit())
            {
                merge_uid(axes, state, "formation:JUGE_UNIQUE");
                hit = true;
            }
        }
    }

    if let Some(pos) = part_position {
        if !state.locked && state.position.is_none() {
            state.position = Some(pos);
        }
        hit = true;
    } else {
        // Ordinal suffixé isolé porté par une spécialisation (« 2EME
        // protection sociale ») : le numéro est celui de la chambre
        // spécialisée.
        if !state.locked && state.position.is_none() && !state.specialty.is_empty() {
            if let Some(c) = position_patterns().chamber_solo.captures(&folded_raw) {
                if let Some(n) = c[1].parse().ok().filter(|n| *n > 0) {
                    state.position = Some(chamber_seat(ChamberId::Number(n)));
                    hit = true;
                }
            }
        }
    }

    // Section lettrée, cumulable avec position et spécialisation — mot-clé
    // explicite (« section B »), lettre derrière le numéro ou le mot chambre
    // (« Chambre 9 - B », « 1re chambre B ») ou portée par la spécialisation
    // (« Sociale A salle 1 »). Pour la lettre « a » sans mot-clé, garde
    // contre la préposition : la suite doit être vide, chiffrée ou une salle.
    let garde_a = |c: &regex::Captures| {
        let m = c.get(1).unwrap();
        if m.as_str() != "a" {
            return true;
        }
        let rest = folded_raw[m.end()..].trim_start();
        rest.is_empty()
            || rest.starts_with(|ch: char| ch.is_ascii_digit())
            || rest.starts_with("salle")
            || rest.starts_with("sall")
            || rest.starts_with("cab")
    };
    let pat = position_patterns();
    let letter = pat
        .section_lettre
        .captures(&folded_raw)
        .or_else(|| pat.composee_lettre.captures(&folded_raw))
        .or_else(|| {
            pat.chamber_num_lettre
                .captures(&folded_raw)
                .filter(|c| garde_a(c))
        })
        .or_else(|| pat.chamber_num_lettre_collee.captures(&folded_raw))
        .or_else(|| {
            pat.chamber_ord_lettre
                .captures(&folded_raw)
                .filter(|c| garde_a(c))
        })
        .or_else(|| pat.spec_lettre.captures(&folded_raw).filter(|c| garde_a(c)));
    if let Some(c) = letter {
        if !state.locked && state.section_lettre.is_none() {
            state.section_lettre = c[1].chars().next().map(|l| l.to_ascii_uppercase());
        }
        hit = true;
    }

    hit
}

/// Single judge by statute: the juge des référés (L. 511-2 CJA, art. 484 CPC)
/// and the designated magistrate sit alone, so the room implies the composition
/// when nothing else has posed it. Exception at the courts of appeal, where a
/// NUMBERED chamber of référés hears the APPEAL of those orders as a bench.
fn imply_single_judge(jurisdiction_type: Option<&str>, axes: &mut FormationAxes) {
    let referes = axes.office_uid == Some("office:JUGE_REFERES")
        || axes.chamber.specialty.contains(&"matiere:REFERES");
    // The exception weighs the CHAMBER, never the whole seat: naming the room
    // « Service des référés » fills the seat without numbering a chamber, and
    // reading it as one would switch the implication off without a word.
    let appel_collegial = jurisdiction_type == Some("CA") && axes.chamber.chamber.is_some();
    if axes.formation_uid.is_none()
        && (referes || axes.office_uid == Some("office:MAGISTRAT_DESIGNE"))
        && !appel_collegial
    {
        axes.formation_uid = Some("formation:JUGE_UNIQUE");
    }
    // The référé is a procedure (`procedure:REFERE_*`) and not a role: the bare
    // registry surface implies the composition above and never leaves as an
    // office.
    if axes.office_uid == Some("office:JUGE_REFERES") {
        axes.office_uid = None;
    }
}

fn finish(axes: &mut FormationAxes, state: PositionState) {
    axes.chamber = state.position.unwrap_or_default();
    axes.chamber.letter = state.section_lettre;
    axes.chamber.specialty = state
        .specialty
        .iter()
        .filter_map(|uid| lj_core::chamber::static_specialty(uid))
        .collect();
    // A solemn formation takes the room's place: what a numbering has already
    // named, « 3e et 8e chambres réunies », keeps its own name.
    if axes.chamber.chamber.is_none() && axes.chamber.joined.is_none() {
        axes.chamber.solemn = state.solemn;
    }
}

/// Premier écrivain par axe : un uid déjà posé n'est jamais écrasé.
fn merge_uid(axes: &mut FormationAxes, state: &mut PositionState, uid: &'static str) {
    if uid.starts_with("matiere:") {
        state.latch_specialty(uid);
    } else if let Some(token) = uid.strip_prefix("solemn:") {
        // A slot of the seat and not a facet value: the solemn formations left
        // the `formation:*` referential for the seat.
        state.latch_solemn(
            Solemn::from_token(token).expect("a lexicon entry names a known solemn formation"),
        );
    } else if uid.starts_with("formation:") {
        if axes.formation_uid.is_none() {
            axes.formation_uid = Some(uid);
        }
    } else if uid.starts_with("office:") {
        if axes.office_uid.is_none() {
            axes.office_uid = Some(uid);
        }
    } else if uid.starts_with("procedure:") && axes.procedure_uid.is_none() {
        axes.procedure_uid = Some(uid);
    }
}

/// Entrée production : champs SOURCE de la décision. `jurisdiction_source_code` =
/// code chambre Judilibre (CC), `chamber` = champ `chamber` greffe (texte
/// libre CA/TJ/TCOM), `bandeau_chamber` = chambre lue dans le bandeau
/// d'en-tête, `formation` = champ formation greffe (Judilibre) /
/// `Formation_Jugement` (DILA).
///
/// Le champ greffe prime ; le bandeau COMPLÈTE les axes (spécialisation,
/// rôle — « 1ère Chambre » greffe + « chambre civile » bandeau → position 1
/// adjectivée civile) mais sa position structurelle ne compte que si le champ
/// greffe est resté muet.
pub fn parse_formation(
    jurisdiction_type: Option<&str>,
    jurisdiction_source_code: Option<&str>,
    chamber: Option<&str>,
    bandeau_chamber: Option<&str>,
    formation: Option<&str>,
) -> FormationAxes {
    let mut axes = FormationAxes::default();
    let mut state = PositionState::default();
    if jurisdiction_type == Some("CC") {
        if let Some(cc) = jurisdiction_source_code.and_then(cc_by_code) {
            apply_cc(cc, &mut axes, &mut state);
        }
    } else {
        let field_hit =
            chamber.is_some_and(|c| parse_part(jurisdiction_type, c, &mut axes, &mut state));
        if let Some(b) = bandeau_chamber {
            // Copie du positionnel greffe : le parse bandeau le VOIT (garde du
            // label CC verbatim) mais ses écritures sont jetées si le champ
            // greffe a parlé.
            let mut bandeau_state = state.clone();
            parse_part(jurisdiction_type, b, &mut axes, &mut bandeau_state);
            // The matter is not positional: the banner completes it even when
            // the registry field has spoken.
            if field_hit {
                state.specialty = bandeau_state.specialty;
            } else {
                state = bandeau_state;
            }
        }
    }
    if let Some(f) = formation {
        parse_part(jurisdiction_type, &strip_rnsm(f), &mut axes, &mut state);
    }
    finish(&mut axes, state);
    imply_single_judge(jurisdiction_type, &mut axes);
    axes
}

/// Whether `d` is an opinion of the Conseil d'État's consultative formation.
pub fn consultative(d: &lj_core::decision::Decision) -> bool {
    d.jurisdiction_type.as_deref() == Some("CE")
        && parse_formation(Some("CE"), None, None, None, d.formation.as_deref())
            .chamber
            .solemn
            == Some(Solemn::Avis)
}

/// Suffixe RNSM Cassation (rejet non spécialement motivé) : qualificatif de
/// publication, pas un axe de formation.
fn strip_rnsm(part: &str) -> String {
    fold_chars(part)
        .replace("hors rnsm", "")
        .replace("rnsm", "")
}

/// Statut de couverture d'un parse pour la sonde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseStatus {
    /// Toutes les parts ont produit au moins un signal.
    Full,
    /// Au moins une part a parlé, une autre est restée muette.
    Partial,
    /// Aucun axe — résidu de greffe assumé (NULL).
    Residue,
}

/// Entrée sonde / conversion GT : la colonne COMPOSÉE historique
/// (`formation_or_chamber`), re-séparée sur le tiret cadratin de jointure.
pub fn parse_composed(jurisdiction_type: &str, composed: &str) -> (FormationAxes, ParseStatus) {
    let parts: Vec<&str> = if composed.contains(" — ") {
        composed.split(" — ").collect()
    } else if jurisdiction_type == "CC" && composed.contains(" - ") {
        composed.split(" - ").collect()
    } else {
        vec![composed]
    };
    let mut axes = FormationAxes::default();
    let mut state = PositionState::default();
    let mut hits = 0usize;
    for part in &parts {
        if parse_part(
            Some(jurisdiction_type),
            &strip_rnsm(part),
            &mut axes,
            &mut state,
        ) {
            hits += 1;
        }
    }
    finish(&mut axes, state);
    imply_single_judge(Some(jurisdiction_type), &mut axes);
    let status = if axes.is_empty() {
        ParseStatus::Residue
    } else if hits == parts.len() {
        ParseStatus::Full
    } else {
        ParseStatus::Partial
    };
    (axes, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composed(t: &str, s: &str) -> FormationAxes {
        parse_composed(t, s).0
    }

    #[test]
    fn cc_code_and_formation() {
        let axes = parse_formation(
            Some("CC"),
            Some("civ2"),
            None,
            None,
            Some("Formation restreinte hors RNSM"),
        );
        assert_eq!(axes.chamber_label().as_deref(), Some("2e chambre civile"));
        assert_eq!(axes.formation_uid, Some("formation:RESTREINTE"));
    }

    #[test]
    fn cc_ordonnance_premier_president() {
        let axes = parse_formation(Some("CC"), Some("ordo"), None, None, None);
        assert_eq!(axes.chamber_label(), None);
        assert_eq!(axes.office_uid, Some("office:PREMIER_PRESIDENT"));
    }

    #[test]
    fn the_commercial_chamber_is_named_for_the_date_of_the_arret() {
        let named = |date| {
            let mut axes = parse_formation(Some("CC"), Some("comm"), None, None, None);
            name_on_date(&mut axes, Some(date));
            axes.chamber_label()
        };
        assert_eq!(named("1989-01-03").as_deref(), Some("Chambre commerciale"));
        assert_eq!(
            named("1989-03-07").as_deref(),
            Some("Chambre commerciale, financière et économique")
        );
    }

    #[test]
    fn juge_des_referes_implique_juge_unique() {
        let axes = parse_formation(Some("CAA"), None, None, None, Some("Juge des référés"));
        assert_eq!(axes.office_uid, None);
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
    }

    #[test]
    fn ca_chamber_numerotee_de_referes_reste_collegiale() {
        let axes = parse_formation(Some("CA"), None, Some("Chambre 1-11 référés"), None, None);
        assert_eq!(axes.office_uid, None);
        assert_eq!(axes.formation_uid, None);
        let axes2 = parse_formation(Some("CA"), None, Some("Service des Référés"), None, None);
        assert_eq!(axes2.formation_uid, Some("formation:JUGE_UNIQUE"));
        let axes3 = parse_formation(Some("TJ"), None, Some("REFERES 2ème Section"), None, None);
        assert_eq!(axes3.formation_uid, Some("formation:JUGE_UNIQUE"));
    }

    #[test]
    fn magistrat_designe_implique_juge_unique() {
        let axes = parse_formation(
            Some("TA"),
            None,
            None,
            None,
            Some("Président, magistrat désigné R.778-3"),
        );
        assert_eq!(axes.office_uid, Some("office:MAGISTRAT_DESIGNE"));
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
    }

    #[test]
    fn suffixe_r222_13_vaut_juge_unique() {
        let axes = parse_formation(
            Some("TA"),
            None,
            None,
            None,
            Some("Magistrat : Mme Baufume - R. 222-13"),
        );
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
    }

    #[test]
    fn bandeau_adjective_le_champ_greffe() {
        let axes = parse_formation(
            Some("CA"),
            None,
            Some("1ère Chambre"),
            Some("Chambre civile"),
            None,
        );
        assert_eq!(axes.chamber_label().as_deref(), Some("1re chambre civile"));
    }

    #[test]
    fn bandeau_ne_deloge_pas_la_position_du_greffe() {
        let axes = parse_formation(
            Some("CA"),
            None,
            Some("2ème Chambre"),
            Some("chambre sociale"),
            None,
        );
        assert_eq!(axes.chamber_label().as_deref(), Some("2e chambre sociale"));
    }

    #[test]
    fn chamber_et_section_numerotees_sans_confusion() {
        let axes = composed("CA", "Chambre 2 section 1");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("2e chambre, 1re section")
        );
        let axes = composed("TA", "4e section - 1re chambre");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("4e section, 1re chambre")
        );
        let axes = composed("TJ", "18° chambre 1ère section");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("18e chambre, 1re section")
        );
        let axes = composed("CA", "Chambre sociale-2ème sect");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Chambre sociale, 2e section")
        );
    }

    #[test]
    fn ssr_et_chambres_reunies_generiques() {
        let axes = composed("CE", "1ère - 6ème ssr");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Sous-sections 1/6 réunies")
        );
        let axes = composed("CE", "10 / 7 sous-sections réunies");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Sous-sections 7/10 réunies")
        );
        let axes = composed("CE", "3ème - 8ème - 9ème - 10ème ssr");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Sous-sections 3/8/9/10 réunies")
        );
        let axes = composed("CE", "3ème - 8ème chambres réunies");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Chambres 3/8 réunies")
        );
        // The numbering names the room; the solemn formation only takes over
        // when the source gives no number to sit in.
        assert_eq!(axes.formation_uid, None);
        assert_eq!(axes.chamber.solemn, None);
        let axes = composed("CE", "1ère et 4ème chambres réunies");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Chambres 1/4 réunies")
        );
        let orpheline = composed("CAA", "Chambres réunies");
        assert_eq!(orpheline.chamber.solemn, Some(Solemn::ChambresReunies));
        assert_eq!(
            orpheline.chamber_label().as_deref(),
            Some("Chambres réunies")
        );
        assert_eq!(orpheline.formation_uid, None);
    }

    #[test]
    fn office_consomme_son_span() {
        let axes = composed("CE", "President de la section du contentieux");
        assert_eq!(
            axes.chamber.solemn,
            Some(Solemn::PresidentSectionContentieux)
        );
        assert_eq!(axes.office_uid, None);
        assert_eq!(axes.formation_uid, None);
    }

    #[test]
    fn graphies_eclatees_recollees() {
        let axes = composed("TCOM", "R E F E R E et procédure accélérée au fond");
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
        let axes = composed("TJ", "Chambre J.A.F. cab 5");
        assert_eq!(axes.office_uid, Some("office:JAF"));
        let axes = composed("TJ", "R.J. L.J.");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Chambre des procédures collectives")
        );
    }

    #[test]
    fn lexique_gate_titres() {
        let axes = composed("TJ", "Ch. 3 cab. 5");
        assert_eq!(axes.chamber_label().as_deref(), Some("3e chambre"));
        let axes = composed("CA", "2° chambre");
        assert_eq!(axes.chamber_label().as_deref(), Some("2e chambre"));
        let axes = composed("TJ", "TJ - civil2");
        assert_eq!(axes.chamber_label().as_deref(), Some("2e chambre"));
        let axes = composed("CA", "Chambre civile 1-6");
        assert_eq!(axes.chamber_label().as_deref(), Some("Chambre 1-6"));
        let axes = composed("TCOM", "Juge-commissaire");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Chambre des procédures collectives")
        );
        let axes = composed("TJ", "CTX gal inf/= 10 000€");
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
        let axes = composed("TA", "R222-13 (JU 2)");
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
        let axes = composed("CAA", "Pleniere");
        assert_eq!(axes.formation_uid, Some("formation:PLENIERE"));
    }

    #[test]
    fn ju_marker_glued_section_letter_and_abbreviated_matters() {
        let axes = composed("TA", "10ème chambre (JU)");
        assert_eq!(axes.chamber_label().as_deref(), Some("10e chambre"));
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
        let axes = composed("TCOM", "Chambre 4-8a");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Chambre 4-8, section A")
        );
        let axes = composed("CA", "Chambre-1 civile et com.");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("1re chambre civile et commerciale")
        );
        let axes = composed("TJ", "2EME protection sociale");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("2e chambre, protection sociale")
        );
        let axes = composed("TJ", "3ème chambre famille");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("3e chambre de la famille")
        );
        let axes = composed("TJ", "Chamb. référés(sup 10000)");
        assert_eq!(axes.office_uid, None);
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
        assert_eq!(axes.chamber_label(), None);
    }

    #[test]
    fn zero_chambers_lettered_sections_and_trailing_matters() {
        let axes = composed("TJ", "Chambre 0 referes");
        assert_eq!(axes.chamber_label(), None);
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
        assert_eq!(composed("TCOM", "Chambre 00").chamber_label(), None);
        let axes = composed("CA", "Ch civ. 1-4 copropriété");
        assert_eq!(axes.chamber_label().as_deref(), Some("Chambre 1-4"));
        assert_eq!(axes.chamber.chamber, Some(ChamberId::Composed(1, 4)));
        assert_eq!(axes.chamber.specialty, ["matiere:CIVILE"]);
        let axes = composed("CA", "Ch.protection sociale 4-7");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Chambre 4-7, protection sociale")
        );
        let axes = composed("TJ", "13CH JCP civil");
        assert_eq!(axes.chamber_label().as_deref(), Some("13e chambre civile"));
        assert_eq!(axes.office_uid, Some("office:JCP"));
        let axes = composed("CA", "Pôle 4 - chambre 9 - B");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Pôle 4, 9e chambre, section B")
        );
        let axes = composed("CA", "Chambre 1 A");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("1re chambre, section A")
        );
        let axes = composed("CA", "1re chambre B");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("1re chambre, section B")
        );
        let axes = composed("CA", "Chambre B");
        assert_eq!(axes.chamber_label().as_deref(), Some("Chambre B"));
        let axes = composed("TJ", "Ch. de la filiation G");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Chambre de la famille, section G")
        );
        let axes = composed("TJ", "Jcp-baux d'habitation");
        assert_eq!(axes.chamber_label().as_deref(), Some("Chambre des baux"));
        let axes = composed("CA", "1re chambre 2e section");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("1re chambre, 2e section")
        );
        let axes = composed("TA", "Ch 9b magistrat statuant seul");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("9e chambre, section B")
        );
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
        let axes = composed("TA", "Chambre 5b");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("5e chambre, section B")
        );
        let axes = composed("CA", "Chambre 1-5DP");
        assert_eq!(axes.chamber_label().as_deref(), Some("Chambre 1-5"));
    }

    #[test]
    fn spec_lettree_sans_mot_cle() {
        let axes = composed("CA", "Sociale A salle 1");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Chambre sociale, section A")
        );
        let axes = composed("TJ", "1/2/2 nationalité B");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Chambre de la nationalité, section B")
        );
    }

    #[test]
    fn section_lettree() {
        let axes = composed("CA", "CHAMBRE SOCIALE SECTION B");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Chambre sociale, section B")
        );

        let axes = composed("CA", "2ème chambre section A");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("2e chambre, section A")
        );

        let axes = composed("CA", "3ème Ch.section E");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("3e chambre, section E")
        );

        let axes = composed("TJ", "1ERE SECTION D");
        assert_eq!(axes.chamber_label().as_deref(), Some("1re section D"));
    }

    #[test]
    fn bandeau_position_seulement_si_greffe_muet() {
        let axes = parse_formation(
            Some("CA"),
            None,
            Some("Affaire courante"),
            Some("Pôle 1 - Chambre 11"),
            None,
        );
        assert_eq!(axes.chamber_label().as_deref(), Some("Pôle 1, 11e chambre"));
        let axes = parse_formation(
            Some("TJ"),
            None,
            Some("CTX PROTECTION SOCIALE"),
            Some("3ème chambre"),
            None,
        );
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Chambre de la protection sociale")
        );
    }

    #[test]
    fn admin_ordinal_chamber() {
        let axes = composed("TA", "2ème chambre");
        assert_eq!(axes.chamber_label().as_deref(), Some("2e chambre"));
    }

    #[test]
    fn admin_chamber_with_formation() {
        let axes = composed("CAA", "1ère chambre - formation à 3");
        assert_eq!(axes.chamber_label().as_deref(), Some("1re chambre"));
        assert_eq!(axes.formation_uid, Some("formation:A_TROIS"));
    }

    #[test]
    fn ce_ssr() {
        let axes = composed("CE", "2 / 6 ssr");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Sous-sections 2/6 réunies")
        );
        // The seat says they sat together, so no formation says it a second
        // time.
        assert_eq!(axes.formation_uid, None);
    }

    /// L'ordre d'énumération de la source ne fait pas deux formations : le
    /// même couple sort sur la même clé quel que soit son sens de lecture.
    #[test]
    fn ce_ssr_textuel() {
        let axes = composed("CE", "8ème et 3ème sous-sections réunies");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Sous-sections 3/8 réunies")
        );
        assert_eq!(
            composed("CE", "3ème et 8ème sous-sections réunies").chamber_label(),
            axes.chamber_label()
        );
        assert_eq!(axes.formation_uid, None);
    }

    #[test]
    fn lassemblee_pleniere_nest_pas_la_pleniere_de_chambre() {
        let axes = parse_formation(Some("CC"), Some("pl"), None, None, None);
        assert_eq!(axes.chamber.solemn, Some(Solemn::AssembleePleniere));
        assert_eq!(axes.chamber_label().as_deref(), Some("Assemblée plénière"));
        assert_eq!(axes.formation_uid, None);
        assert_eq!(axes.chamber.chamber, None);
        let axes = parse_formation(
            Some("CC"),
            Some("soc"),
            None,
            None,
            Some("Formation plénière"),
        );
        assert_eq!(axes.formation_uid, Some("formation:PLENIERE"));
        assert_eq!(axes.chamber.specialty, ["matiere:SOCIALE"]);
    }

    /// The litigation Section does not add itself to a room: it IS the room,
    /// and the Cassation chamber sitting in section stays an axis of its own.
    #[test]
    fn ce_section_seule_est_la_formation_de_jugement() {
        for source in ["Section", "Section du contentieux"] {
            let axes = composed("CE", source);
            assert_eq!(axes.formation_uid, None, "« {source} »");
            assert_eq!(
                axes.chamber_label().as_deref(),
                Some("Section du contentieux"),
                "« {source} »"
            );
        }
        let axes = composed("CE", "Assemblee");
        assert_eq!(axes.formation_uid, None);
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Assemblée du contentieux")
        );
        let cassation = parse_formation(
            Some("CC"),
            Some("soc"),
            None,
            None,
            Some("Formation de section"),
        );
        assert_eq!(cassation.formation_uid, Some("formation:SECTION"));
        assert_eq!(
            cassation.chamber_label().as_deref(),
            Some("Chambre sociale")
        );
    }

    /// Every solemn formation names the room, none is left as a formation
    /// beside it, and the label rendered reads back as the same variant.
    #[test]
    fn les_solennelles_nomment_la_salle() {
        let cas = [
            (composed("CE", "Assemblee"), Solemn::Assemblee),
            (
                parse_formation(Some("CC"), Some("pl"), None, None, None),
                Solemn::AssembleePleniere,
            ),
            (composed("CE", "Formation spécialisée"), Solemn::Specialisee),
            (
                composed("CE", "Section du contentieux"),
                Solemn::SectionContentieux,
            ),
            (
                composed("CE", "Président de la section du contentieux"),
                Solemn::PresidentSectionContentieux,
            ),
            (
                parse_formation(Some("CC"), Some("creun"), None, None, None),
                Solemn::ChambresReunies,
            ),
            (
                parse_formation(Some("CC"), Some("mi"), None, None, None),
                Solemn::ChambreMixte,
            ),
            (composed("TCOM", "Formation mixte"), Solemn::FormationMixte),
            (composed("CE", "Formation consultative"), Solemn::Avis),
        ];
        for (axes, solemn) in cas {
            assert_eq!(axes.chamber.solemn, Some(solemn), "{solemn:?}");
            assert_eq!(axes.formation_uid, None, "{solemn:?}");
            assert_eq!(axes.office_uid, None, "{solemn:?}");
            let libelle = axes.chamber_label().unwrap();
            assert_eq!(
                composed("CE", &libelle).chamber.solemn,
                Some(solemn),
                "« {libelle} »"
            );
        }
    }

    #[test]
    fn ce_sous_section_seule() {
        let axes = composed("CE", "10 ss");
        assert_eq!(axes.chamber_label().as_deref(), Some("10e sous-section"));
    }

    #[test]
    fn ca_pole_chambre() {
        let axes = composed("CA", "Pôle 1 - Chambre 11");
        assert_eq!(axes.chamber_label().as_deref(), Some("Pôle 1, 11e chambre"));
    }

    /// Un libellé rendu se relit sur lui-même, sans perdre de coordonnée.
    #[test]
    fn le_libelle_rendu_se_relit() {
        for (juridiction, source) in [
            ("CA", "Pôle 1 - Chambre 11"),
            ("CA", "Pôle 4 - chambre 9 - B"),
            ("CA", "Pôle 5 ch 3"),
            ("CA", "Pôle 2"),
            ("CA", "1ere chambre"),
            ("CA", "chambre 2-5"),
            ("CE", "3ème - 8ème chambres réunies"),
            ("CE", "2 / 6 ssr"),
            ("CE", "4ème ssjs"),
            ("TA", "4e section, 1re chambre"),
            ("TJ", "18e chambre, 2e section"),
        ] {
            let attendu = composed(juridiction, source).chamber_label();
            let relu = composed(juridiction, attendu.as_deref().unwrap()).chamber_label();
            assert_eq!(relu, attendu, "« {source} »");
        }
    }

    /// The section contains the chambers in the administrative order and divides
    /// one in the judicial order, whichever order the registry writes the two in.
    #[test]
    fn la_section_suit_l_ordre_juridictionnel() {
        for source in ["4e section - 1re chambre", "1re chambre - 4e section"] {
            let axes = composed("TA", source);
            assert_eq!(axes.chamber.section, None, "« {source} »");
            assert_eq!(
                axes.chamber.container,
                Some(Container::Section(4)),
                "« {source} »"
            );
            assert_eq!(
                axes.chamber.chamber,
                Some(ChamberId::Number(1)),
                "« {source} »"
            );
        }
        for source in ["18e chambre - 2e section", "2e section - 18e chambre"] {
            let axes = composed("TJ", source);
            assert_eq!(axes.chamber.container, None, "« {source} »");
            assert_eq!(
                axes.chamber.chamber,
                Some(ChamberId::Number(18)),
                "« {source} »"
            );
            assert_eq!(axes.chamber.section, Some(2), "« {source} »");
        }
        assert_eq!(
            composed("TA", "8e section").chamber.container,
            Some(Container::Section(8))
        );
        assert_eq!(composed("TJ", "8e section").chamber.section, Some(8));
    }

    #[test]
    fn ca_specialised_numbered_chamber() {
        let axes = composed("CA", "5EME chambre prud'homale");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("5e chambre prud'homale")
        );
    }

    #[test]
    fn ca_specialisation_alone_uses_referential_label() {
        let axes = composed("CA", "Chambre sociale");
        assert_eq!(axes.chamber_label().as_deref(), Some("Chambre sociale"));
    }

    #[test]
    fn ca_ch_abbrev_with_section() {
        let axes = composed("CA", "2ème CH - section 1");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("2e chambre, 1re section")
        );
    }

    #[test]
    fn ca_chamber_lettree() {
        let axes = composed("CA", "Chambre B");
        assert_eq!(axes.chamber_label().as_deref(), Some("Chambre B"));
    }

    #[test]
    fn tj_word_ordinal() {
        let axes = composed("TJ", "Première chambre");
        assert_eq!(axes.chamber_label().as_deref(), Some("1re chambre"));
        let axes = composed("TJ", "Quatrième chambre");
        assert_eq!(axes.chamber_label().as_deref(), Some("4e chambre"));
    }

    #[test]
    fn tj_roles() {
        assert_eq!(composed("TJ", "J.L.D.").office_uid, Some("office:JLD"));
        assert_eq!(composed("TJ", "JCP").office_uid, Some("office:JCP"));
        assert_eq!(composed("TJ", "J.E.X").office_uid, Some("office:JEX"));
        assert_eq!(
            composed("TJ", "Juge libertés & détention").office_uid,
            Some("office:JLD")
        );
        assert_eq!(
            composed("TJ", "Rétention_recoursjld").office_uid,
            Some("office:JLD")
        );
    }

    #[test]
    fn tj_greffe_compound() {
        let axes = composed("TJ", "PCP JCP ACR référé");
        assert_eq!(axes.office_uid, Some("office:JCP"));
    }

    #[test]
    fn tj_ctx_protection_sociale() {
        let axes = composed("TJ", "CTX protection sociale");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Chambre de la protection sociale")
        );
    }

    #[test]
    fn tj_proximite() {
        assert_eq!(
            composed("TJ", "PCP JTJ proxi fond")
                .chamber_label()
                .as_deref(),
            Some("Chambre de proximité")
        );
        assert_eq!(
            composed("TJ", "Pprox_fond").chamber_label().as_deref(),
            Some("Chambre de proximité")
        );
    }

    #[test]
    fn tcom_compound_chamber() {
        let axes = composed("TCOM", "Chambre 2-5");
        assert_eq!(axes.chamber_label().as_deref(), Some("Chambre 2-5"));
    }

    #[test]
    fn tcom_padded_number() {
        let axes = composed("TCOM", "Chambre 04");
        assert_eq!(axes.chamber_label().as_deref(), Some("4e chambre"));
    }

    #[test]
    fn tcom_refere_eclate() {
        assert_eq!(
            composed("TCOM", "R E F E R E").formation_uid,
            Some("formation:JUGE_UNIQUE")
        );
    }

    #[test]
    fn referes_sans_office() {
        let caa = composed("CAA", "Juge des référés");
        assert_eq!(caa.office_uid, None);
        assert_eq!(caa.formation_uid, Some("formation:JUGE_UNIQUE"));
        let tj = composed("TJ", "Service des référés");
        assert_eq!(tj.office_uid, None);
        assert_eq!(tj.formation_uid, Some("formation:JUGE_UNIQUE"));
    }

    /// A matter the source composes names the WHOLE room, in writing order;
    /// side by side names a room and its section, which is not the same thing.
    #[test]
    fn la_matiere_composee_nomme_la_salle_entiere() {
        let besancon = composed("CA", "1ère chambre civile et commerciale");
        assert_eq!(
            besancon.chamber.specialty,
            ["matiere:CIVILE", "matiere:COMMERCIALE"]
        );
        assert_eq!(
            besancon.chamber_label().as_deref(),
            Some("1re chambre civile et commerciale")
        );
        let reims = composed("CA", "Chambre-1 civile et com.");
        assert_eq!(
            reims.chamber.specialty,
            ["matiere:CIVILE", "matiere:COMMERCIALE"]
        );
        let rouen = composed("CA", "Ch. civile et commerciale");
        assert_eq!(
            rouen.chamber_label().as_deref(),
            Some("Chambre civile et commerciale")
        );
        let barre = composed("CA", "Chambre civile/commerciale");
        assert_eq!(
            barre.chamber_label().as_deref(),
            Some("Chambre civile et commerciale")
        );
        let presse = composed("TJ", "17ème Ch. Presse-civile");
        assert_eq!(presse.chamber.specialty, ["matiere:PRESSE"]);
        assert_eq!(
            presse.chamber_label().as_deref(),
            Some("17e chambre, presse")
        );
    }

    /// Neither a subdivision nor a role names a matter: « section JEX » gives a
    /// section and an office, « sect. Civile » a section and a matter.
    #[test]
    fn la_section_et_l_office_ne_font_pas_une_matiere() {
        let jex = composed("CA", "1ère chambre section JEX");
        assert!(jex.chamber.specialty.is_empty());
        assert_eq!(jex.chamber.chamber, Some(ChamberId::Number(1)));
        assert_eq!(jex.office_uid, Some("office:JEX"));
        let colle = composed("CA", "Chambre-1 JEX");
        assert!(colle.chamber.specialty.is_empty());
        assert_eq!(colle.office_uid, Some("office:JEX"));
        let civile = composed("CA", "1ere Chambre sect.Civile");
        assert_eq!(civile.chamber.chamber, Some(ChamberId::Number(1)));
        assert_eq!(civile.chamber.specialty, ["matiere:CIVILE"]);
    }

    /// An audience held away from the public is a procedure, and the numbering
    /// it sat under survives it leaving the seat.
    #[test]
    fn la_chambre_du_conseil_ne_nomme_pas_de_salle() {
        let seule = composed("TCOM", "CHAMBRE DU CONSEIL (OUVERTURES)");
        assert_eq!(seule.chamber_label(), None);
        assert_eq!(seule.procedure_uid, Some("procedure:CHAMBRE_DU_CONSEIL"));
        let numerotee = composed("TCOM", "DEBATS EN CHAMBRE DU CONSEIL - CHAMBRE 2");
        assert_eq!(numerotee.chamber_label().as_deref(), Some("2e chambre"));
        assert_eq!(
            numerotee.procedure_uid,
            Some("procedure:CHAMBRE_DU_CONSEIL")
        );
    }

    /// Named in full, the référés are a room; bare, they are a procedure suffix
    /// that names none.
    #[test]
    fn les_referes_nomment_une_salle_sur_leur_graphie_pleine() {
        let service = composed("TJ", "Service des référés");
        assert_eq!(
            service.chamber_label().as_deref(),
            Some("Chambre des référés")
        );
        assert_eq!(service.formation_uid, Some("formation:JUGE_UNIQUE"));
        assert_eq!(service.office_uid, None);
        let cour = composed("CA", "CHAMBRE DES REFERES");
        assert_eq!(cour.chamber.specialty, ["matiere:REFERES"]);
        assert_eq!(cour.formation_uid, Some("formation:JUGE_UNIQUE"));
        let nue = composed("CA", "Chambre 1-11 référés");
        assert!(nue.chamber.specialty.is_empty());
        assert_eq!(nue.formation_uid, None);
    }

    #[test]
    fn ta_eloignement_et_ju() {
        assert_eq!(
            composed("TA", "Eloignement 72 heures")
                .chamber_label()
                .as_deref(),
            Some("Étrangers et rétention")
        );
        assert_eq!(
            composed("TA", "Ju1").formation_uid,
            Some("formation:JUGE_UNIQUE")
        );
    }

    #[test]
    fn residue_is_all_none() {
        for s in [".", "Jeudi", "Affaire courante", "Salon d'honneur"] {
            let (axes, status) = parse_composed("TCOM", s);
            assert!(axes.is_empty(), "{s} devrait être résidu");
            assert_eq!(status, ParseStatus::Residue);
        }
    }

    #[test]
    fn ta_reconduite_frontiere() {
        let axes = composed("TA", "Reconduite à la frontière");
        assert_eq!(
            axes.chamber_label().as_deref(),
            Some("Étrangers et rétention")
        );
    }

    /// The registry of TA Rennes and the banner of Saint-Denis: `à` and `D'` are no chamber letter.
    #[test]
    fn a_preposition_and_an_elision_name_no_letter() {
        for key in [
            "Chambre à 3 juge des référés",
            "CHAMBRE D'APPEL DE SAINT-DENIS",
        ] {
            assert_eq!(composed("CA", key).chamber.chamber, None, "{key}");
        }
        assert_eq!(
            composed("CA", "Chambre A - Civile").chamber.chamber,
            Some(ChamberId::Letter('A'))
        );
    }

    #[test]
    fn a_key_says_more_than_the_number_when_it_names_a_sitting() {
        for key in [
            "Ch4.3 JCP",
            "Chambre 04 JEX",
            "1ère Chambre cab D",
            "Ch. 3 Cab. 5",
            "CH4 TJ FOND",
            "2ème Chambre Vente",
            "1re Section - 1re Chambre",
        ] {
            assert!(says_more_than_the_number(key), "{key}");
        }
        for key in [
            "Deuxième Chambre",
            "1ère Chambre",
            "2° chambre",
            "Chambre n° 2",
            "1ère Ch.",
        ] {
            assert!(!says_more_than_the_number(key), "{key}");
        }
    }
}
