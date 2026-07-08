//! Parse structuré de la formation (ADR 0170) : décompose les champs SOURCE
//! (code chambre CC, chambre de bandeau Judilibre, formation greffe
//! Judilibre/DILA) en quatre axes — position de la chambre (display canonique
//! recomposé), spécialisation (`chambre:*`), type de formation
//! (`formation:*`), rôle (`office:*` / `voie:*`).
//!
//! de référence 0157 §4 : vocabulaire fermé, comparaisons pliées
//! (`compiled::fold_stable`), une poignée de regex génériques de position —
//! jamais de regex par entrée. Résidu de greffe illisible (jours d'audience
//! TCOM, noms de magistrats TA, « Affaire courante »…) = tous axes `None`,
//! assumé.

use std::sync::OnceLock;

use regex::Regex;

use crate::compiled::fold_stable;

/// Axes structurés d'une formation. Les uids sont des valeurs complètes de
/// `facet_value` (FK) ; `chamber_position` est l'unique forme affichable,
/// recomposée — jamais la chaîne source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormationAxes {
    pub chamber_position: Option<String>,
    pub chambre_uid: Option<&'static str>,
    pub formation_uid: Option<&'static str>,
    pub office_uid: Option<&'static str>,
    pub voie_uid: Option<&'static str>,
}

impl FormationAxes {
    pub fn is_empty(&self) -> bool {
        self.chamber_position.is_none()
            && self.chambre_uid.is_none()
            && self.formation_uid.is_none()
            && self.office_uid.is_none()
            && self.voie_uid.is_none()
    }
}

// ─────────────────────────── référentiels embarqués ─────────────────────────

/// Spécialisations `chambre:*` : (uid, label référentiel, adjectif accolable à
/// une position numérotée — « 5ᵉ chambre prud'homale »). Le seed SQL de la
/// migration reprend exactement ces (uid, label).
pub const CHAMBRE_SEED: &[(&str, &str, Option<&str>)] = &[
    ("chambre:CIVILE", "Chambre civile", Some("civile")),
    ("chambre:SOCIALE", "Chambre sociale", Some("sociale")),
    (
        "chambre:COMMERCIALE",
        "Chambre commerciale",
        Some("commerciale"),
    ),
    (
        "chambre:CRIMINELLE",
        "Chambre criminelle",
        Some("criminelle"),
    ),
    (
        "chambre:CORRECTIONNELLE",
        "Chambre correctionnelle",
        Some("correctionnelle"),
    ),
    (
        "chambre:PRUD_HOMALE",
        "Chambre prud'homale",
        Some("prud'homale"),
    ),
    ("chambre:PROTECTION_SOCIALE", "Protection sociale", None),
    (
        "chambre:PROCEDURES_COLLECTIVES",
        "Procédures collectives",
        None,
    ),
    ("chambre:INSTRUCTION", "Chambre de l'instruction", None),
    ("chambre:FAMILLE", "Chambre de la famille", None),
    ("chambre:BAUX", "Chambre des baux", None),
    ("chambre:CONSTRUCTION", "Chambre de la construction", None),
    ("chambre:ETRANGERS", "Étrangers et rétention", None),
    ("chambre:CONSEIL", "Chambre du conseil", None),
    ("chambre:EXPROPRIATION", "Expropriation", None),
    ("chambre:PROXIMITE", "Proximité", None),
    ("chambre:SURENDETTEMENT", "Surendettement", None),
    ("chambre:COPROPRIETE", "Copropriété", None),
    ("chambre:URGENCES", "Urgences", None),
    ("chambre:DALO", "Droit au logement (DALO)", None),
    ("chambre:MINEURS", "Chambre des mineurs", None),
    ("chambre:NATIONALITE", "Nationalité", None),
    ("chambre:TERRES", "Chambre des terres", None),
    ("chambre:CIVI", "Indemnisation des victimes (CIVI)", None),
];

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
    ("formation:MIXTE", "Formation mixte"),
    ("formation:SSR", "Sous-sections réunies"),
    ("formation:CHAMBRES_REUNIES", "Chambres réunies"),
    ("formation:ASSEMBLEE", "Assemblée du contentieux"),
    ("formation:SPECIALISEE", "Formation spécialisée"),
];

/// Offices nouveaux portés par ce parse (les autres `office:*` existent déjà,
/// ADR 0163). Même contrat de seed.
pub const OFFICE_SEED_EXTRA: &[(&str, &str)] = &[
    ("office:JUGE_REFERES", "Juge des référés"),
    (
        "office:PRESIDENT_SECTION_CONTENTIEUX",
        "Président de la section du contentieux",
    ),
    ("office:JUGE_EXPROPRIATION", "Juge de l'expropriation"),
];

fn chambre_label(uid: &str) -> &'static str {
    CHAMBRE_SEED
        .iter()
        .find(|(u, _, _)| *u == uid)
        .map(|(_, l, _)| *l)
        .expect("uid chambre hors seed")
}

fn chambre_adjective(uid: &str) -> Option<&'static str> {
    CHAMBRE_SEED
        .iter()
        .find(|(u, _, _)| *u == uid)
        .and_then(|(_, _, a)| *a)
}

// ─────────────────────────────── positions ──────────────────────────────────

/// Position structurelle dans la juridiction, recomposée en display canonique.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Position {
    Chambre(u32),
    ChambreLettre(char),
    /// TCOM « Chambre 2-5 » : numérotation composée chambre-section.
    ChambreComposee(u32, u32),
    /// CA « 2ème CH - section 1 » / TA « 4e section - 1re chambre » — le
    /// display suit l'ordre source (au TA la section contient les chambres).
    ChambreSection {
        chambre: u32,
        section: u32,
        section_d_abord: bool,
    },
    Pole {
        pole: u32,
        chambre: Option<u32>,
    },
    Section(u32),
    /// CE historique : sous-section jugeant seule (« 2 ss », « 4ème ssjs »).
    SousSection(u32),
    /// CE historique : sous-sections réunies (« 2 / 6 ssr », « 7 8 9 ssr »).
    SousSectionsReunies(Vec<u32>),
    /// CE post-2016 : chambres réunies numérotées (« 3ème - 8ème chambres
    /// réunies »).
    ChambresReunies(Vec<u32>),
}

/// Ordinal français plein texte : « 1re », « 2e » … (pas d'exposant Unicode —
/// la même forme alimente `search_title`/BM25, les requêtes utilisateur
/// s'écrivent « 2e chambre »).
fn ordinal(n: u32) -> String {
    if n == 1 {
        "1re".to_string()
    } else {
        format!("{n}e")
    }
}

impl Position {
    fn display(&self, adjective: Option<&str>) -> String {
        let base = match self {
            Position::Chambre(n) => format!("{} chambre", ordinal(*n)),
            Position::ChambreLettre(c) => format!("Chambre {}", c.to_uppercase()),
            Position::ChambreComposee(a, b) => format!("Chambre {a}-{b}"),
            Position::ChambreSection {
                chambre,
                section,
                section_d_abord: false,
            } => format!(
                "{} chambre, {} section",
                ordinal(*chambre),
                ordinal(*section)
            ),
            Position::ChambreSection {
                chambre,
                section,
                section_d_abord: true,
            } => format!(
                "{} section, {} chambre",
                ordinal(*section),
                ordinal(*chambre)
            ),
            Position::Pole {
                pole,
                chambre: Some(c),
            } => format!("Pôle {pole} — {} chambre", ordinal(*c)),
            Position::Pole {
                pole,
                chambre: None,
            } => format!("Pôle {pole}"),
            Position::Section(n) => format!("{} section", ordinal(*n)),
            Position::SousSection(n) => format!("{} sous-section", ordinal(*n)),
            Position::SousSectionsReunies(nums) => {
                let joined = nums
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join("/");
                format!("Sous-sections {joined} réunies")
            }
            Position::ChambresReunies(nums) => {
                let joined = nums
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join("/");
                format!("Chambres {joined} réunies")
            }
        };
        match (self, adjective) {
            (Position::Chambre(_), Some(adj)) => format!("{base} {adj}"),
            _ => base,
        }
    }
}

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
    ss_reunies_sans_numero: Regex,
    sous_section: Regex,
    ss: Regex,
    pole: Regex,
    chambre_composee: Regex,
    chambre_ord: Regex,
    chambre_num: Regex,
    chambre_collee: Regex,
    chambre_mot: Regex,
    chambre_lettre: Regex,
    chambre_num_lettre: Regex,
    chambre_num_lettre_collee: Regex,
    chambre_ord_lettre: Regex,
    section_ord: Regex,
    section_num: Regex,
    section_courte: Regex,
    section_lettre: Regex,
    composee_lettre: Regex,
    spec_lettre: Regex,
    chambre_solo: Regex,
    nombres: Regex,
}

/// Suffixe ordinal OBLIGATOIRE (« 1ère », « 2eme », « 3e », « 2° ») : un
/// chiffre nu devant un mot-clé appartient souvent au mot-clé précédent
/// (« chambre 2 section 1 » : « 2 section » n'est PAS la 2e section).
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
        // « 7 8 9 ssr », « 3ème - 8ème - 9ème - 10ème ssr », « 1 / 2
        // sous-sections reunies ». Les numéros sortent de la capture par
        // `nombres`.
        ssr_liste: Regex::new(&format!(
            r"\b(\d{{1,2}}\s*{ORD_SUFFIX}(?:\s*(?:et|[-/,]|\s)\s*\d{{1,2}}\s*{ORD_SUFFIX})*)\s*(?:ssr\b|sous[- ]sections?\s+reunies)"
        ))
        .unwrap(),
        // CE post-2016 : « 3ème - 8ème chambres réunies », « 1ère et 4ème
        // chambres réunies ».
        chambres_reunies_liste: Regex::new(&format!(
            r"\b(\d{{1,2}}\s*{ORD_SUFFIX}(?:\s*(?:et|[-/,])\s*\d{{1,2}}\s*{ORD_SUFFIX})+)\s*chambres\s+reunies"
        ))
        .unwrap(),
        // « sous-sections réunies » sans numéros.
        ss_reunies_sans_numero: Regex::new(r"\bsous[- ]sections?\s+reunies").unwrap(),
        // « 2ème sous-section ».
        sous_section: Regex::new(&format!(
            r"\b(\d{{1,2}})\s*{ORD_SUFFIX}\s*sous[- ]section\b"
        ))
        .unwrap(),
        // « 2 ss », « 10 ss. », « 4ème ssjs » — sous-section jugeant seule.
        ss: Regex::new(&format!(r"\b(\d{{1,2}})\s*{ORD_SUFFIX}\s*ss(?:js)?\b")).unwrap(),
        // « pole 1 - chambre 11 », « pole 5 ch 3 », « pole 2 ».
        pole: Regex::new(&format!(
            r"\bpole[\s.]*n?°?\s*0*(\d{{1,2}})(?:\D{{1,15}}?{CHAMBRE_ALIAS}[\s.]*n?°?\s*0*(\d{{1,2}}))?"
        ))
        .unwrap(),
        // TCOM « chambre 2-5 », CA « chambre civile 1-6 », « Ch civ. 1-4 »,
        // « Ch.protection sociale 4-7 » (jusqu'à deux mots interposés, points
        // de greffe tolérés), lettre de section collée (« chambre 4-8a »).
        chambre_composee: Regex::new(&format!(
            r"\b{CHAMBRE_MOT_SEUL}(?:[\s.]+[a-z']+){{0,2}}[\s.]*0*(\d{{1,2}})\s*-\s*0*(\d{{1,2}})[a-h]?\b"
        ))
        .unwrap(),
        // « 1ere chambre », « 2eme CH », « 3e chambre », « 2° chambre ».
        chambre_ord: Regex::new(&format!(
            r"\b0*(\d{{1,2}})\s*{ORD_SUFFIX_REQ}\s*{CHAMBRE_ALIAS}\b"
        ))
        .unwrap(),
        // « chambre 04 », « chambre n° 2 », « ch. 1 », « civil2 »,
        // « chambre-1 » (tiret de jonction — la composée « 2-5 » passe
        // avant), lettre de section collée tolérée (« Ch 9b » ; e = ordinal
        // et h = heure exclus).
        chambre_num: Regex::new(&format!(
            r"\b{CHAMBRE_ALIAS}[\s.\-]*n?°?\s*0*(\d{{1,2}})[a-dfg]?\b"
        ))
        .unwrap(),
        // « 13CH JCP civil » — numéro collé au mot chambre.
        chambre_collee: Regex::new(r"\b0*(\d{1,2})(?:chambre|chbre|ch)\b").unwrap(),
        // « premiere chambre », « troisieme chambre ».
        chambre_mot: Regex::new(&format!(
            r"\b(premiere|seconde|deuxieme|troisieme|quatrieme|cinquieme|sixieme|septieme|huitieme|neuvieme|dixieme|onzieme|douzieme)\s+{CHAMBRE_ALIAS}\b"
        ))
        .unwrap(),
        // « chambre b » (chambres lettrées de CA).
        chambre_lettre: Regex::new(&format!(r"\b{CHAMBRE_MOT_SEUL}\s+([a-h])\b")).unwrap(),
        // « Chambre 9 - B », « Pôle 4 - chambre 9 - A », « Chambre 1 A » —
        // lettre de section derrière le numéro de chambre. Séparateur REQUIS :
        // collé au chiffre, « e » est un ordinal (« chambre 2e section »).
        chambre_num_lettre: Regex::new(&format!(
            r"\b{CHAMBRE_MOT_SEUL}[\s.]*n?°?\s*0*\d{{1,2}}(?:\s*[-.]\s*|\s+)([a-h])(?:[^a-z0-9']|$)"
        ))
        .unwrap(),
        // « Ch 9b », « Chambre 5b » — lettre de section collée au numéro
        // (e = ordinal et h = heure exclus).
        chambre_num_lettre_collee: Regex::new(&format!(
            r"\b{CHAMBRE_MOT_SEUL}[\s.]*n?°?\s*0*\d{{1,2}}([a-dfg])\b"
        ))
        .unwrap(),
        // « 1re chambre B », « 8e chambre C » — lettre derrière le mot
        // chambre d'une position ordinale.
        chambre_ord_lettre: Regex::new(&format!(
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
        // « Chambre sociale section B », « 2ème chambre sect. A ».
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
        chambre_solo: Regex::new(&format!(r"\b0*(\d{{1,2}})\s*{ORD_SUFFIX_REQ}\b")).unwrap(),
        nombres: Regex::new(r"\d{1,2}").unwrap(),
    })
}

fn word_ordinal(w: &str) -> Option<u32> {
    WORD_ORDINALS.iter().find(|(s, _)| *s == w).map(|(_, n)| *n)
}

fn parse_position(folded: &str) -> Option<Position> {
    let p = position_patterns();
    if let Some(c) = p.ssr_liste.captures(folded) {
        let nums: Vec<u32> = p
            .nombres
            .find_iter(&c[1])
            .filter_map(|m| m.as_str().parse().ok())
            .collect();
        return Some(match nums.as_slice() {
            [seul] => Position::SousSection(*seul),
            _ => Position::SousSectionsReunies(nums),
        });
    }
    if let Some(c) = p.chambres_reunies_liste.captures(folded) {
        let nums: Vec<u32> = p
            .nombres
            .find_iter(&c[1])
            .filter_map(|m| m.as_str().parse().ok())
            .collect();
        if nums.len() > 1 {
            return Some(Position::ChambresReunies(nums));
        }
    }
    if let Some(c) = p.sous_section.captures(folded) {
        if let Some(n) = c[1].parse().ok().filter(|n| *n > 0) {
            return Some(Position::SousSection(n));
        }
    }
    if let Some(c) = p.ss.captures(folded) {
        if let Some(n) = c[1].parse().ok().filter(|n| *n > 0) {
            return Some(Position::SousSection(n));
        }
    }
    if let Some(c) = p.pole.captures(folded) {
        if let Some(n) = c[1].parse().ok().filter(|n| *n > 0) {
            return Some(Position::Pole {
                pole: n,
                chambre: c
                    .get(2)
                    .and_then(|m| m.as_str().parse().ok())
                    .filter(|n| *n > 0),
            });
        }
    }
    if let Some(c) = p.chambre_composee.captures(folded) {
        let a: u32 = c[1].parse().ok()?;
        let b: u32 = c[2].parse().ok()?;
        // Un zéro est un placeholder de greffe, pas une position.
        if a > 0 && b > 0 {
            return Some(Position::ChambreComposee(a, b));
        }
    }
    // Chambre et section numérotées : capture + OFFSET, pour restituer
    // l'ordre source (TA : « 4e section - 1re chambre », la section contient
    // les chambres).
    let chambre: Option<(u32, usize)> = p
        .chambre_ord
        .captures(folded)
        .or_else(|| p.chambre_num.captures(folded))
        .or_else(|| p.chambre_collee.captures(folded))
        .and_then(|c| {
            let n: u32 = c[1].parse().ok()?;
            // « Chambre 0 » / « Chambre 00 » : placeholder de greffe.
            (n > 0).then(|| (n, c.get(0).unwrap().start()))
        })
        .or_else(|| {
            p.chambre_mot
                .captures(folded)
                .and_then(|c| Some((word_ordinal(&c[1])?, c.get(0)?.start())))
        });
    // Mot-clé d'abord (« section 1 » : le numéro APRÈS le mot), l'ordinal en
    // repli — un chiffre nu devant « section » appartient au mot-clé
    // précédent (« chambre 2 section 1 »).
    let mut section: Option<(u32, usize)> = p
        .section_num
        .captures(folded)
        .or_else(|| p.section_ord.captures(folded))
        .and_then(|c| {
            let n: u32 = c[1].parse().ok()?;
            (n > 0).then(|| (n, c.get(0).unwrap().start()))
        });
    if section.is_none() && chambre.is_some() {
        // « 11ème civ. S1 » : la graphie courte ne vaut section qu'adossée
        // à une chambre — seule, « S3 » est du code de greffe.
        section = p.section_courte.captures(folded).and_then(|c| {
            let n: u32 = c[1].parse().ok()?;
            (n > 0).then(|| (n, c.get(0).unwrap().start()))
        });
    }
    match (chambre, section) {
        (Some((c, ch_at)), Some((s, se_at))) => {
            return Some(Position::ChambreSection {
                chambre: c,
                section: s,
                section_d_abord: se_at < ch_at,
            })
        }
        (Some((c, _)), None) => return Some(Position::Chambre(c)),
        (None, Some((s, _))) => return Some(Position::Section(s)),
        (None, None) => {}
    }
    if let Some(c) = p.chambre_lettre.captures(folded) {
        return Some(Position::ChambreLettre(c[1].chars().next()?));
    }
    None
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
    ("protection sociale", "chambre:PROTECTION_SOCIALE"),
    ("protection soc", "chambre:PROTECTION_SOCIALE"),
    ("securite sociale", "chambre:PROTECTION_SOCIALE"),
    ("sec soc", "chambre:PROTECTION_SOCIALE"),
    ("secu", "chambre:PROTECTION_SOCIALE"),
    ("ctx technique", "chambre:PROTECTION_SOCIALE"),
    ("contentieux sociaux", "chambre:PROTECTION_SOCIALE"),
    ("tarification", "chambre:PROTECTION_SOCIALE"),
    ("tass", "chambre:PROTECTION_SOCIALE"),
    ("procedures collectives", "chambre:PROCEDURES_COLLECTIVES"),
    ("procedure collective", "chambre:PROCEDURES_COLLECTIVES"),
    ("procedure collectives", "chambre:PROCEDURES_COLLECTIVES"),
    ("procedures collect", "chambre:PROCEDURES_COLLECTIVES"),
    ("proc collectives", "chambre:PROCEDURES_COLLECTIVES"),
    ("proc coll", "chambre:PROCEDURES_COLLECTIVES"),
    ("pcl", "chambre:PROCEDURES_COLLECTIVES"),
    ("liquidation judiciaire", "chambre:PROCEDURES_COLLECTIVES"),
    ("liquidations judiciaires", "chambre:PROCEDURES_COLLECTIVES"),
    ("liquid judiciaire", "chambre:PROCEDURES_COLLECTIVES"),
    ("redressement judiciaire", "chambre:PROCEDURES_COLLECTIVES"),
    (
        "redressements judiciaires",
        "chambre:PROCEDURES_COLLECTIVES",
    ),
    ("cessation des paiements", "chambre:PROCEDURES_COLLECTIVES"),
    ("juge commissaire", "chambre:PROCEDURES_COLLECTIVES"),
    ("prud'homale", "chambre:PRUD_HOMALE"),
    ("prud'hommale", "chambre:PRUD_HOMALE"),
    ("prud'hommes", "chambre:PRUD_HOMALE"),
    ("prudhomale", "chambre:PRUD_HOMALE"),
    ("prudhommale", "chambre:PRUD_HOMALE"),
    ("prudhommes", "chambre:PRUD_HOMALE"),
    ("loyers commerciaux", "chambre:BAUX"),
    ("loyer commerciaux", "chambre:BAUX"),
    ("baux commerciaux", "chambre:BAUX"),
    ("baux ruraux", "chambre:BAUX"),
    ("baux", "chambre:BAUX"),
    ("tpbr", "chambre:BAUX"),
    ("sociale", "chambre:SOCIALE"),
    ("social", "chambre:SOCIALE"),
    ("commerciale", "chambre:COMMERCIALE"),
    ("commercial", "chambre:COMMERCIALE"),
    ("commerce", "chambre:COMMERCIALE"),
    ("economique", "chambre:COMMERCIALE"),
    ("ecocom", "chambre:COMMERCIALE"),
    ("chcom", "chambre:COMMERCIALE"),
    ("civile", "chambre:CIVILE"),
    ("civiles", "chambre:CIVILE"),
    ("civil", "chambre:CIVILE"),
    ("civ", "chambre:CIVILE"),
    ("criminelle", "chambre:CRIMINELLE"),
    ("correctionnelle", "chambre:CORRECTIONNELLE"),
    ("correctionnel", "chambre:CORRECTIONNELLE"),
    ("appels correctionnels", "chambre:CORRECTIONNELLE"),
    ("chambre correct", "chambre:CORRECTIONNELLE"),
    ("instruction", "chambre:INSTRUCTION"),
    ("famille", "chambre:FAMILLE"),
    ("affaires familiales", "chambre:FAMILLE"),
    ("aff familiales", "chambre:FAMILLE"),
    ("divorces", "chambre:FAMILLE"),
    ("divorce", "chambre:FAMILLE"),
    ("filiation", "chambre:FAMILLE"),
    ("construction", "chambre:CONSTRUCTION"),
    ("etrangers", "chambre:ETRANGERS"),
    ("etranger", "chambre:ETRANGERS"),
    ("retention administrative", "chambre:ETRANGERS"),
    ("retentions", "chambre:ETRANGERS"),
    ("retention", "chambre:ETRANGERS"),
    ("reconduite a la frontiere", "chambre:ETRANGERS"),
    ("reconduites a la frontiere", "chambre:ETRANGERS"),
    ("reconduites", "chambre:ETRANGERS"),
    ("reconduite", "chambre:ETRANGERS"),
    ("eloignement", "chambre:ETRANGERS"),
    ("oqtf", "chambre:ETRANGERS"),
    ("asile", "chambre:ETRANGERS"),
    ("96h", "chambre:ETRANGERS"),
    ("96 h", "chambre:ETRANGERS"),
    ("expropriations", "chambre:EXPROPRIATION"),
    ("expropriation", "chambre:EXPROPRIATION"),
    ("expro", "chambre:EXPROPRIATION"),
    ("chambre du conseil", "chambre:CONSEIL"),
    ("chbre du conseil", "chambre:CONSEIL"),
    ("ch du conseil", "chambre:CONSEIL"),
    ("proximite", "chambre:PROXIMITE"),
    ("proxi", "chambre:PROXIMITE"),
    ("pprox", "chambre:PROXIMITE"),
    ("prox", "chambre:PROXIMITE"),
    ("tprox", "chambre:PROXIMITE"),
    ("tprx", "chambre:PROXIMITE"),
    ("tpx", "chambre:PROXIMITE"),
    ("surendettement", "chambre:SURENDETTEMENT"),
    ("surendettemment", "chambre:SURENDETTEMENT"),
    ("retablissement personnel", "chambre:SURENDETTEMENT"),
    ("surend", "chambre:SURENDETTEMENT"),
    ("surendet", "chambre:SURENDETTEMENT"),
    ("surdt", "chambre:SURENDETTEMENT"),
    ("copropriete", "chambre:COPROPRIETE"),
    ("dalo", "chambre:DALO"),
    ("mineurs", "chambre:MINEURS"),
    ("nationalite", "chambre:NATIONALITE"),
    ("elections pro", "chambre:SOCIALE"),
    ("urgences", "chambre:URGENCES"),
    ("urgence", "chambre:URGENCES"),
    ("chambre des terres", "chambre:TERRES"),
    ("tribunal foncier", "chambre:TERRES"),
    ("civip", "chambre:CIVI"),
    ("civi", "chambre:CIVI"),
];

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
    ("section du contentieux", "formation:SECTION"),
    ("formation pleniere", "formation:PLENIERE"),
    ("pleniere de chambre", "formation:PLENIERE"),
    ("assemblee pleniere", "formation:PLENIERE"),
    ("assemblee du contentieux", "formation:ASSEMBLEE"),
    ("formation specialisee", "formation:SPECIALISEE"),
    ("formation mixte", "formation:MIXTE"),
    ("chambre mixte", "formation:MIXTE"),
    ("chambres reunies", "formation:CHAMBRES_REUNIES"),
    ("chambre reunies", "formation:CHAMBRES_REUNIES"),
    ("formation elargie", "formation:PLENIERE"),
    ("formations reunies", "formation:CHAMBRES_REUNIES"),
    ("formation a 2 chambres", "formation:CHAMBRES_REUNIES"),
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
    ("15 jours", "formation:JUGE_UNIQUE"),
    ("72 heures", "formation:JUGE_UNIQUE"),
    ("ju oqtf", "formation:JUGE_UNIQUE"),
    // Abréviation de greffe TA : « 10ème chambre (JU) ».
    ("ju", "formation:JUGE_UNIQUE"),
];

/// Rôles → office. Phrases pliées, englobants d'abord (le JLD avant « juge »).
const OFFICE_PHRASES: &[(&str, &str)] = &[
    (
        "president de la section du contentieux",
        "office:PRESIDENT_SECTION_CONTENTIEUX",
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
    ("juge des referes", "office:JUGE_REFERES"),
    ("juges des referes", "office:JUGE_REFERES"),
    ("service des referes", "office:JUGE_REFERES"),
    ("chambre des referes", "office:JUGE_REFERES"),
    ("referes", "office:JUGE_REFERES"),
    ("refere", "office:JUGE_REFERES"),
    ("refers", "office:JUGE_REFERES"),
];

/// Voies lisibles directement dans la formation greffe.
const VOIE_PHRASES: &[(&str, &str)] = &[("qpc", "voie:QPC")];

/// Chaîne ENTIÈRE (pliée, séparateurs normalisés) → axe. Pour les mots trop
/// ambigus en simple `contains` (« Section » seul = jugement en Section du
/// contentieux CE, mais « 8e section » = position).
const WHOLE_STRING: &[(&str, &str)] = &[
    ("section", "formation:SECTION"),
    ("avis section", "formation:SECTION"),
    ("assemblee", "formation:ASSEMBLEE"),
    ("pleniere", "formation:PLENIERE"),
    ("rj", "chambre:PROCEDURES_COLLECTIVES"),
    ("lj", "chambre:PROCEDURES_COLLECTIVES"),
    ("rj lj", "chambre:PROCEDURES_COLLECTIVES"),
    ("rjlj", "chambre:PROCEDURES_COLLECTIVES"),
    ("rlj", "chambre:PROCEDURES_COLLECTIVES"),
    ("sauvegarde", "chambre:PROCEDURES_COLLECTIVES"),
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
    ("ceseda", "chambre:ETRANGERS"),
];

// ────────────────────────────── chambres CC ─────────────────────────────────

/// Vocabulaire fermé Cassation : code Judilibre + label plié → axes. Le
/// display CC reste le label institutionnel verbatim (pas d'ordinal recomposé).
struct CcChamber {
    code: &'static str,
    folded_label: &'static str,
    display: Option<&'static str>,
    chambre_uid: Option<&'static str>,
    formation_uid: Option<&'static str>,
    office_uid: Option<&'static str>,
}

const CC_CHAMBERS: &[CcChamber] = &[
    CcChamber {
        code: "civ1",
        folded_label: "premiere chambre civile",
        display: Some("Première chambre civile"),
        chambre_uid: Some("chambre:CIVILE"),
        formation_uid: None,
        office_uid: None,
    },
    CcChamber {
        code: "civ2",
        folded_label: "deuxieme chambre civile",
        display: Some("Deuxième chambre civile"),
        chambre_uid: Some("chambre:CIVILE"),
        formation_uid: None,
        office_uid: None,
    },
    CcChamber {
        code: "civ3",
        folded_label: "troisieme chambre civile",
        display: Some("Troisième chambre civile"),
        chambre_uid: Some("chambre:CIVILE"),
        formation_uid: None,
        office_uid: None,
    },
    CcChamber {
        code: "soc",
        folded_label: "chambre sociale",
        display: Some("Chambre sociale"),
        chambre_uid: Some("chambre:SOCIALE"),
        formation_uid: None,
        office_uid: None,
    },
    CcChamber {
        code: "comm",
        folded_label: "chambre commerciale",
        display: Some("Chambre commerciale"),
        chambre_uid: Some("chambre:COMMERCIALE"),
        formation_uid: None,
        office_uid: None,
    },
    CcChamber {
        code: "comm",
        folded_label: "chambre commerciale, financiere et economique",
        display: Some("Chambre commerciale"),
        chambre_uid: Some("chambre:COMMERCIALE"),
        formation_uid: None,
        office_uid: None,
    },
    CcChamber {
        code: "cr",
        folded_label: "chambre criminelle",
        display: Some("Chambre criminelle"),
        chambre_uid: Some("chambre:CRIMINELLE"),
        formation_uid: None,
        office_uid: None,
    },
    CcChamber {
        code: "mi",
        folded_label: "chambre mixte",
        display: Some("Chambre mixte"),
        chambre_uid: None,
        formation_uid: Some("formation:MIXTE"),
        office_uid: None,
    },
    CcChamber {
        code: "pl",
        folded_label: "assemblee pleniere",
        display: Some("Assemblée plénière"),
        chambre_uid: None,
        formation_uid: Some("formation:PLENIERE"),
        office_uid: None,
    },
    CcChamber {
        code: "ord",
        folded_label: "ordonnance du premier president",
        display: None,
        chambre_uid: None,
        formation_uid: None,
        office_uid: Some("office:PREMIER_PRESIDENT"),
    },
    CcChamber {
        code: "ordo",
        folded_label: "ordonnance du premier president",
        display: None,
        chambre_uid: None,
        formation_uid: None,
        office_uid: Some("office:PREMIER_PRESIDENT"),
    },
    CcChamber {
        code: "creun",
        folded_label: "chambre reunies",
        display: Some("Chambres réunies"),
        chambre_uid: None,
        formation_uid: Some("formation:CHAMBRES_REUNIES"),
        office_uid: None,
    },
    CcChamber {
        code: "creun",
        folded_label: "chambres reunies",
        display: Some("Chambres réunies"),
        chambre_uid: None,
        formation_uid: Some("formation:CHAMBRES_REUNIES"),
        office_uid: None,
    },
    CcChamber {
        code: "allciv",
        folded_label: "toutes chambres civiles",
        display: None,
        chambre_uid: Some("chambre:CIVILE"),
        formation_uid: None,
        office_uid: None,
    },
];

fn cc_by_code(code: &str) -> Option<&'static CcChamber> {
    let lower = code.to_lowercase();
    CC_CHAMBERS.iter().find(|c| c.code == lower)
}

fn cc_by_label(folded: &str) -> Option<&'static CcChamber> {
    CC_CHAMBERS.iter().find(|c| c.folded_label == folded)
}

// ───────────────────────────────── parse ────────────────────────────────────

fn apply_cc(cc: &CcChamber, axes: &mut FormationAxes) {
    axes.chamber_position = cc.display.map(str::to_string);
    axes.chambre_uid = cc.chambre_uid;
    axes.formation_uid = axes.formation_uid.or(cc.formation_uid);
    axes.office_uid = axes.office_uid.or(cc.office_uid);
}

/// État positionnel accumulé sur les parts (premier écrivain), composé en
/// display par [`finish`] : position structurelle + section lettrée
/// (« Chambre sociale section B », « 2ème chambre section A »).
#[derive(Debug, Clone, Default)]
struct PositionState {
    position: Option<Position>,
    section_lettre: Option<char>,
}

/// Parse une part (champ chambre, bandeau ou formation greffe) et fusionne
/// ses axes dans `axes` (premier écrivain par axe) ; le positionnel sort dans
/// `state` (premier écrivain), le display n'est composé qu'en [`finish`] une
/// fois TOUTES les parts lues — l'adjectif de spécialisation peut venir d'une
/// autre part que la position (« 1ère Chambre » greffe + « chambre civile »
/// bandeau → « 1re chambre civile »). Retourne `true` si la part a produit au
/// moins un signal (statut de couverture).
fn parse_part(part: &str, axes: &mut FormationAxes, state: &mut PositionState) -> bool {
    let folded_raw = fold_stable(part);
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
    // display verbatim céderait la position (« 1re chambre » + « chambre
    // sociale » bandeau).
    if let Some(cc) = cc_by_label(&folded) {
        if axes.chamber_position.is_none() && axes.chambre_uid.is_none() && state.position.is_none()
        {
            apply_cc(cc, axes);
        } else {
            for uid in [cc.chambre_uid, cc.formation_uid, cc.office_uid]
                .into_iter()
                .flatten()
            {
                merge_uid(axes, uid);
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
            merge_uid(axes, uid);
            return true;
        }
    }

    // Position structurelle — sur le folded BRUT (les « 2 / 6 ssr » portent
    // leur slash, les « chambre 2-5 » leur tiret).
    let part_position = parse_position(&folded_raw);

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
                merge_uid(axes, uid);
                lexical.replace_range(a..b, &" ".repeat(b - a));
                hit = true;
                break;
            }
            if collapsed
                .as_deref()
                .is_some_and(|c| contains_phrase(c, phrase))
            {
                merge_uid(axes, uid);
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
                    merge_uid(axes, uid);
                    hit = true;
                    break;
                }
            }
            // « Ju1 », « JU2 », « Ju-6 semaines » — juge unique numéroté TA.
            if !hit
                && compact.starts_with("ju")
                && compact.chars().nth(2).is_some_and(|c| c.is_ascii_digit())
            {
                merge_uid(axes, "formation:JUGE_UNIQUE");
                hit = true;
            }
        }
    }

    if let Some(pos) = part_position {
        // Les sous-sections réunies portent aussi leur type de formation.
        if matches!(pos, Position::SousSectionsReunies(..)) && axes.formation_uid.is_none() {
            axes.formation_uid = Some("formation:SSR");
        }
        if state.position.is_none() && axes.chamber_position.is_none() {
            state.position = Some(pos);
        }
        hit = true;
    } else {
        if hit
            && position_patterns()
                .ss_reunies_sans_numero
                .is_match(&folded_raw)
            && axes.formation_uid.is_none()
        {
            // Sous-sections réunies sans numéros : formation seule, pas de
            // position.
            axes.formation_uid = Some("formation:SSR");
        }
        // Ordinal suffixé isolé porté par une spécialisation (« 2EME
        // protection sociale ») : le numéro est celui de la chambre
        // spécialisée.
        if state.position.is_none() && axes.chamber_position.is_none() && axes.chambre_uid.is_some()
        {
            if let Some(c) = position_patterns().chambre_solo.captures(&folded_raw) {
                if let Some(n) = c[1].parse().ok().filter(|n| *n > 0) {
                    state.position = Some(Position::Chambre(n));
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
    let lettre = pat
        .section_lettre
        .captures(&folded_raw)
        .or_else(|| pat.composee_lettre.captures(&folded_raw))
        .or_else(|| {
            pat.chambre_num_lettre
                .captures(&folded_raw)
                .filter(|c| garde_a(c))
        })
        .or_else(|| pat.chambre_num_lettre_collee.captures(&folded_raw))
        .or_else(|| {
            pat.chambre_ord_lettre
                .captures(&folded_raw)
                .filter(|c| garde_a(c))
        })
        .or_else(|| pat.spec_lettre.captures(&folded_raw).filter(|c| garde_a(c)));
    if let Some(c) = lettre {
        if state.section_lettre.is_none() && axes.chamber_position.is_none() {
            state.section_lettre = c[1].chars().next();
        }
        hit = true;
    }

    hit
}

/// Compose le display de chambre une fois toutes les parts fusionnées :
/// position structurelle (adjectivée par la spécialisation — « 1re chambre
/// civile »), sinon label référentiel de la spécialisation seule ; la section
/// lettrée se suffixe (« Chambre sociale, section B », « 1re section D »).
fn finish(axes: &mut FormationAxes, state: PositionState) {
    if axes.chamber_position.is_some() {
        return;
    }
    let base = match (&state.position, axes.chambre_uid) {
        // Section numérotée d'une chambre spécialisée sans numéro :
        // « Chambre sociale-2ème sect » → « Chambre sociale, 2e section ».
        (Some(Position::Section(n)), Some(uid)) => {
            Some(format!("{}, {} section", chambre_label(uid), ordinal(*n)))
        }
        // Chambre numérotée (simple ou composée) d'une spécialisation sans
        // adjectif accolable : le label se compose — « 1re chambre de la
        // famille » quand il est lui-même une chambre, « 2e chambre —
        // protection sociale » / « Chambre 4-7 — protection sociale » sinon.
        (Some(pos @ (Position::Chambre(_) | Position::ChambreComposee(..))), Some(uid))
            if chambre_adjective(uid).is_none() =>
        {
            let base = pos.display(None);
            let label = chambre_label(uid);
            Some(match (label.strip_prefix("Chambre "), pos) {
                (Some(rest), Position::Chambre(_)) => format!("{base} {rest}"),
                (Some(_), _) => base,
                (None, _) => {
                    let mut chars = label.chars();
                    let first = chars.next().expect("label chambre vide");
                    format!("{base} — {}{}", first.to_lowercase(), chars.as_str())
                }
            })
        }
        (Some(pos), _) => Some(pos.display(axes.chambre_uid.and_then(chambre_adjective))),
        (None, Some(uid)) => Some(chambre_label(uid).to_string()),
        (None, None) => None,
    };
    axes.chamber_position = match (base, state.section_lettre) {
        (Some(b), Some(l)) if matches!(state.position, Some(Position::Section(_))) => {
            Some(format!("{b} {}", l.to_uppercase()))
        }
        (Some(b), Some(l)) => Some(format!("{b}, section {}", l.to_uppercase())),
        (Some(b), None) => Some(b),
        (None, Some(l)) => Some(format!("Section {}", l.to_uppercase())),
        (None, None) => None,
    };
}

/// Premier écrivain par axe : un uid déjà posé n'est jamais écrasé.
fn merge_uid(axes: &mut FormationAxes, uid: &'static str) {
    if let Some(rest) = uid.strip_prefix("chambre:") {
        let _ = rest;
        if axes.chambre_uid.is_none() {
            axes.chambre_uid = Some(uid);
        }
    } else if uid.starts_with("formation:") {
        if axes.formation_uid.is_none() {
            axes.formation_uid = Some(uid);
        }
    } else if uid.starts_with("office:") {
        if axes.office_uid.is_none() {
            axes.office_uid = Some(uid);
        }
    } else if uid.starts_with("voie:") && axes.voie_uid.is_none() {
        axes.voie_uid = Some(uid);
    }
}

/// Entrée production : champs SOURCE de la décision. `juridiction_code` =
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
    juridiction_type: Option<&str>,
    juridiction_code: Option<&str>,
    chamber: Option<&str>,
    bandeau_chamber: Option<&str>,
    formation: Option<&str>,
) -> FormationAxes {
    let mut axes = FormationAxes::default();
    let mut state = PositionState::default();
    if juridiction_type == Some("CC") {
        if let Some(cc) = juridiction_code.and_then(cc_by_code) {
            apply_cc(cc, &mut axes);
        }
    } else {
        let field_hit = chamber.is_some_and(|c| parse_part(c, &mut axes, &mut state));
        if let Some(b) = bandeau_chamber {
            // Copie du positionnel greffe : le parse bandeau le VOIT (garde du
            // label CC verbatim) mais ses écritures sont jetées si le champ
            // greffe a parlé.
            let mut bandeau_state = state.clone();
            parse_part(b, &mut axes, &mut bandeau_state);
            if !field_hit {
                state = bandeau_state;
            }
        }
    }
    if let Some(f) = formation {
        parse_part(&strip_rnsm(f), &mut axes, &mut state);
    }
    finish(&mut axes, state);
    axes
}

/// Suffixe RNSM Cassation (rejet non spécialement motivé) : qualificatif de
/// publication, pas un axe de formation.
fn strip_rnsm(part: &str) -> String {
    fold_stable(part)
        .replace("hors rnsm", "")
        .replace("rnsm", "")
}

/// Statut de couverture d'un parse pour la sonde (ADR 0170 étape 1).
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
/// La production ne passe jamais par ici — elle lit les champs source.
pub fn parse_composed(juridiction_type: &str, composed: &str) -> (FormationAxes, ParseStatus) {
    let parts: Vec<&str> = if composed.contains(" — ") {
        composed.split(" — ").collect()
    } else if juridiction_type == "CC" && composed.contains(" - ") {
        composed.split(" - ").collect()
    } else {
        vec![composed]
    };
    let mut axes = FormationAxes::default();
    let mut state = PositionState::default();
    let mut hits = 0usize;
    for part in &parts {
        if parse_part(&strip_rnsm(part), &mut axes, &mut state) {
            hits += 1;
        }
    }
    finish(&mut axes, state);
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
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Deuxième chambre civile")
        );
        assert_eq!(axes.chambre_uid, Some("chambre:CIVILE"));
        assert_eq!(axes.formation_uid, Some("formation:RESTREINTE"));
    }

    #[test]
    fn cc_ordonnance_premier_president() {
        let axes = parse_formation(Some("CC"), Some("ordo"), None, None, None);
        assert_eq!(axes.chamber_position, None);
        assert_eq!(axes.office_uid, Some("office:PREMIER_PRESIDENT"));
    }

    #[test]
    fn bandeau_adjective_le_champ_greffe() {
        // Greffe : position seule ; bandeau : spécialisation seule. La fusion
        // adjectivise la position et pose le badge.
        let axes = parse_formation(
            Some("CA"),
            None,
            Some("1ère Chambre"),
            Some("Chambre civile"),
            None,
        );
        assert_eq!(axes.chamber_position.as_deref(), Some("1re chambre civile"));
        assert_eq!(axes.chambre_uid, Some("chambre:CIVILE"));
    }

    #[test]
    fn bandeau_ne_deloge_pas_la_position_du_greffe() {
        // Bandeau à label CC verbatim (« chambre sociale ») : il apporte le
        // badge, pas son display — la position greffe reste le siège.
        let axes = parse_formation(
            Some("CA"),
            None,
            Some("2ème Chambre"),
            Some("chambre sociale"),
            None,
        );
        assert_eq!(axes.chamber_position.as_deref(), Some("2e chambre sociale"));
        assert_eq!(axes.chambre_uid, Some("chambre:SOCIALE"));
    }

    #[test]
    fn chambre_et_section_numerotees_sans_confusion() {
        // Le chiffre de la chambre devant le mot « section » ne vaut pas
        // numéro de section (bug « chambre 2 section 1 » → 2/2).
        let axes = composed("CA", "Chambre 2 section 1");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("2e chambre, 1re section")
        );
        // Ordre source TA : la section contient les chambres.
        let axes = composed("TA", "4e section - 1re chambre");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("4e section, 1re chambre")
        );
        let axes = composed("TJ", "18° chambre 1ère section");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("18e chambre, 1re section")
        );
        // Section abrégée.
        let axes = composed("CA", "Chambre sociale-2ème sect");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Chambre sociale, 2e section")
        );
    }

    #[test]
    fn ssr_et_chambres_reunies_generiques() {
        let axes = composed("CE", "1ère - 6ème ssr");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Sous-sections 1/6 réunies")
        );
        let axes = composed("CE", "10 / 7 sous-sections réunies");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Sous-sections 10/7 réunies")
        );
        let axes = composed("CE", "3ème - 8ème - 9ème - 10ème ssr");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Sous-sections 3/8/9/10 réunies")
        );
        let axes = composed("CE", "3ème - 8ème chambres réunies");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Chambres 3/8 réunies")
        );
        assert_eq!(axes.formation_uid, Some("formation:CHAMBRES_REUNIES"));
        let axes = composed("CE", "1ère et 4ème chambres réunies");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Chambres 1/4 réunies")
        );
    }

    #[test]
    fn office_consomme_son_span() {
        // « section du contentieux » consommé par l'office ne vote plus la
        // formation SECTION.
        let axes = composed("CE", "President de la section du contentieux");
        assert_eq!(
            axes.office_uid,
            Some("office:PRESIDENT_SECTION_CONTENTIEUX")
        );
        assert_eq!(axes.formation_uid, None);
    }

    #[test]
    fn graphies_eclatees_recollees() {
        let axes = composed("TCOM", "R E F E R E et procédure accélérée au fond");
        assert_eq!(axes.office_uid, Some("office:JUGE_REFERES"));
        let axes = composed("TJ", "Chambre J.A.F. cab 5");
        assert_eq!(axes.office_uid, Some("office:JAF"));
        let axes = composed("TJ", "R.J. L.J.");
        assert_eq!(axes.chambre_uid, Some("chambre:PROCEDURES_COLLECTIVES"));
    }

    #[test]
    fn lexique_gate_titres() {
        // Point après l'alias (« Ch. 3 »), ordinal en degré (« 2° »).
        let axes = composed("TJ", "Ch. 3 cab. 5");
        assert_eq!(axes.chamber_position.as_deref(), Some("3e chambre"));
        let axes = composed("CA", "2° chambre");
        assert_eq!(axes.chamber_position.as_deref(), Some("2e chambre"));
        // « civil2 » : alias civil numéroté.
        let axes = composed("TJ", "TJ - civil2");
        assert_eq!(axes.chamber_position.as_deref(), Some("2e chambre"));
        // Chambre composée avec adjectif interposé.
        let axes = composed("CA", "Chambre civile 1-6");
        assert_eq!(axes.chamber_position.as_deref(), Some("Chambre 1-6"));
        // Juge-commissaire → procédures collectives (tiret lexical).
        let axes = composed("TCOM", "Juge-commissaire");
        assert_eq!(axes.chambre_uid, Some("chambre:PROCEDURES_COLLECTIVES"));
        // Juge unique par régime : ≤ 10 000 €, R222-13.
        let axes = composed("TJ", "CTX gal inf/= 10 000€");
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
        let axes = composed("TA", "R222-13 (JU 2)");
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
        // Plénière en chaîne entière (CAA).
        let axes = composed("CAA", "Pleniere");
        assert_eq!(axes.formation_uid, Some("formation:PLENIERE"));
    }

    #[test]
    fn correctifs_gate_finaux() {
        // Abréviation JU de greffe TA.
        let axes = composed("TA", "10ème chambre (JU)");
        assert_eq!(axes.chamber_position.as_deref(), Some("10e chambre"));
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
        // Composée à lettre de section collée.
        let axes = composed("TCOM", "Chambre 4-8a");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Chambre 4-8, section A")
        );
        // Tiret de jonction alias-numéro.
        let axes = composed("CA", "Chambre-1 civile et com.");
        assert_eq!(axes.chamber_position.as_deref(), Some("1re chambre civile"));
        assert_eq!(axes.chambre_uid, Some("chambre:CIVILE"));
        // Ordinal suffixé isolé porté par la spécialisation.
        let axes = composed("TJ", "2EME protection sociale");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("2e chambre — protection sociale")
        );
        assert_eq!(axes.chambre_uid, Some("chambre:PROTECTION_SOCIALE"));
        // Label-chambre : composition directe, pas d'em-dash.
        let axes = composed("TJ", "3ème chambre famille");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("3e chambre de la famille")
        );
        // Siège du juge des référés à régime juge unique : les deux axes
        // sortent (précédence display côté lj-core).
        let axes = composed("TJ", "Chamb. référés(sup 10000)");
        assert_eq!(axes.office_uid, Some("office:JUGE_REFERES"));
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
        assert_eq!(axes.chamber_position, None);
    }

    #[test]
    fn correctifs_gate_v27() {
        // Chambre zéro = placeholder de greffe, pas une position.
        let axes = composed("TJ", "Chambre 0 referes");
        assert_eq!(axes.chamber_position, None);
        assert_eq!(axes.office_uid, Some("office:JUGE_REFERES"));
        assert_eq!(composed("TCOM", "Chambre 00").chamber_position, None);
        // Composée à alias pointé / spécialisation interposée (Aix).
        let axes = composed("CA", "Ch civ. 1-4 copropriété");
        assert_eq!(axes.chamber_position.as_deref(), Some("Chambre 1-4"));
        assert_eq!(axes.chambre_uid, Some("chambre:CIVILE"));
        let axes = composed("CA", "Ch.protection sociale 4-7");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Chambre 4-7 — protection sociale")
        );
        // Numéro collé au mot chambre.
        let axes = composed("TJ", "13CH JCP civil");
        assert_eq!(axes.chamber_position.as_deref(), Some("13e chambre civile"));
        assert_eq!(axes.office_uid, Some("office:JCP"));
        // Lettre de section derrière le numéro / mot chambre.
        let axes = composed("CA", "Pôle 4 - chambre 9 - B");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Pôle 4 — 9e chambre, section B")
        );
        let axes = composed("CA", "Chambre 1 A");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("1re chambre, section A")
        );
        let axes = composed("CA", "1re chambre B");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("1re chambre, section B")
        );
        // La chambre lettrée seule reste une chambre, pas une section.
        let axes = composed("CA", "Chambre B");
        assert_eq!(axes.chamber_position.as_deref(), Some("Chambre B"));
        // Spécialisation famille/filiation lettrée ; l'apostrophe ne fait
        // pas section (« baux d'habitation »).
        let axes = composed("TJ", "Ch. de la filiation G");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Chambre de la famille, section G")
        );
        let axes = composed("TJ", "Jcp-baux d'habitation");
        assert_eq!(axes.chamber_position.as_deref(), Some("Chambre des baux"));
        // L'ordinal « e » collé au chiffre n'est pas une lettre de section.
        let axes = composed("CA", "1re chambre 2e section");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("1re chambre, 2e section")
        );
        // Lettre collée au numéro de chambre (hors e/h).
        let axes = composed("TA", "Ch 9b magistrat statuant seul");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("9e chambre, section B")
        );
        assert_eq!(axes.formation_uid, Some("formation:JUGE_UNIQUE"));
        let axes = composed("TA", "Chambre 5b");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("5e chambre, section B")
        );
    }

    #[test]
    fn spec_lettree_sans_mot_cle() {
        let axes = composed("CA", "Sociale A salle 1");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Chambre sociale, section A")
        );
        let axes = composed("TJ", "1/2/2 nationalité B");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Nationalité, section B")
        );
    }

    #[test]
    fn section_lettree() {
        let axes = composed("CA", "CHAMBRE SOCIALE SECTION B");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Chambre sociale, section B")
        );
        assert_eq!(axes.chambre_uid, Some("chambre:SOCIALE"));

        let axes = composed("CA", "2ème chambre section A");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("2e chambre, section A")
        );

        let axes = composed("CA", "3ème Ch.section E");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("3e chambre, section E")
        );

        // Ordinal de section + lettre : suffixe sans virgule.
        let axes = composed("TJ", "1ERE SECTION D");
        assert_eq!(axes.chamber_position.as_deref(), Some("1re section D"));
    }

    #[test]
    fn bandeau_position_seulement_si_greffe_muet() {
        // Greffe muet (résidu) : la position du bandeau prend le relais.
        let axes = parse_formation(
            Some("CA"),
            None,
            Some("Affaire courante"),
            Some("Pôle 1 - Chambre 11"),
            None,
        );
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Pôle 1 — 11e chambre")
        );
        // Greffe parlant sans position : la position du bandeau est ignorée.
        let axes = parse_formation(
            Some("TJ"),
            None,
            Some("CTX PROTECTION SOCIALE"),
            Some("3ème chambre"),
            None,
        );
        assert_eq!(axes.chamber_position.as_deref(), Some("Protection sociale"));
        assert_eq!(axes.chambre_uid, Some("chambre:PROTECTION_SOCIALE"));
    }

    #[test]
    fn admin_ordinal_chamber() {
        let axes = composed("TA", "2ème chambre");
        assert_eq!(axes.chamber_position.as_deref(), Some("2e chambre"));
        assert!(axes.chambre_uid.is_none());
    }

    #[test]
    fn admin_chamber_with_formation() {
        let axes = composed("CAA", "1ère chambre - formation à 3");
        assert_eq!(axes.chamber_position.as_deref(), Some("1re chambre"));
        assert_eq!(axes.formation_uid, Some("formation:A_TROIS"));
    }

    #[test]
    fn ce_ssr() {
        let axes = composed("CE", "2 / 6 ssr");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Sous-sections 2/6 réunies")
        );
        assert_eq!(axes.formation_uid, Some("formation:SSR"));
    }

    #[test]
    fn ce_ssr_textuel() {
        let axes = composed("CE", "8ème et 3ème sous-sections réunies");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Sous-sections 8/3 réunies")
        );
        assert_eq!(axes.formation_uid, Some("formation:SSR"));
    }

    #[test]
    fn ce_section_seule_est_la_formation_de_jugement() {
        let axes = composed("CE", "Section");
        assert_eq!(axes.formation_uid, Some("formation:SECTION"));
        let axes = composed("CE", "Section du contentieux");
        assert_eq!(axes.formation_uid, Some("formation:SECTION"));
        let axes = composed("CE", "Assemblee");
        assert_eq!(axes.formation_uid, Some("formation:ASSEMBLEE"));
    }

    #[test]
    fn ce_sous_section_seule() {
        let axes = composed("CE", "10 ss");
        assert_eq!(axes.chamber_position.as_deref(), Some("10e sous-section"));
    }

    #[test]
    fn ca_pole_chambre() {
        let axes = composed("CA", "Pôle 1 - Chambre 11");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("Pôle 1 — 11e chambre")
        );
    }

    #[test]
    fn ca_specialised_numbered_chamber() {
        let axes = composed("CA", "5EME chambre prud'homale");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("5e chambre prud'homale")
        );
        assert_eq!(axes.chambre_uid, Some("chambre:PRUD_HOMALE"));
    }

    #[test]
    fn ca_specialisation_alone_uses_referential_label() {
        let axes = composed("CA", "Chambre sociale");
        assert_eq!(axes.chamber_position.as_deref(), Some("Chambre sociale"));
        assert_eq!(axes.chambre_uid, Some("chambre:SOCIALE"));
    }

    #[test]
    fn ca_ch_abbrev_with_section() {
        let axes = composed("CA", "2ème CH - section 1");
        assert_eq!(
            axes.chamber_position.as_deref(),
            Some("2e chambre, 1re section")
        );
    }

    #[test]
    fn ca_chambre_lettree() {
        let axes = composed("CA", "Chambre B");
        assert_eq!(axes.chamber_position.as_deref(), Some("Chambre B"));
    }

    #[test]
    fn tj_word_ordinal() {
        let axes = composed("TJ", "Première chambre");
        assert_eq!(axes.chamber_position.as_deref(), Some("1re chambre"));
        let axes = composed("TJ", "Quatrième chambre");
        assert_eq!(axes.chamber_position.as_deref(), Some("4e chambre"));
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
        assert_eq!(axes.chambre_uid, Some("chambre:PROTECTION_SOCIALE"));
        assert_eq!(axes.chamber_position.as_deref(), Some("Protection sociale"));
    }

    #[test]
    fn tj_proximite() {
        assert_eq!(
            composed("TJ", "PCP JTJ proxi fond").chambre_uid,
            Some("chambre:PROXIMITE")
        );
        assert_eq!(
            composed("TJ", "Pprox_fond").chambre_uid,
            Some("chambre:PROXIMITE")
        );
    }

    #[test]
    fn tcom_compound_chamber() {
        let axes = composed("TCOM", "Chambre 2-5");
        assert_eq!(axes.chamber_position.as_deref(), Some("Chambre 2-5"));
    }

    #[test]
    fn tcom_padded_number() {
        let axes = composed("TCOM", "Chambre 04");
        assert_eq!(axes.chamber_position.as_deref(), Some("4e chambre"));
    }

    #[test]
    fn tcom_refere_eclate() {
        assert_eq!(
            composed("TCOM", "R E F E R E").office_uid,
            Some("office:JUGE_REFERES")
        );
    }

    #[test]
    fn referes_map_to_office() {
        assert_eq!(
            composed("CAA", "Juge des référés").office_uid,
            Some("office:JUGE_REFERES")
        );
        assert_eq!(
            composed("TJ", "Service des référés").office_uid,
            Some("office:JUGE_REFERES")
        );
    }

    #[test]
    fn ta_eloignement_et_ju() {
        assert_eq!(
            composed("TA", "Eloignement 72 heures").chambre_uid,
            Some("chambre:ETRANGERS")
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
        assert_eq!(axes.chambre_uid, Some("chambre:ETRANGERS"));
    }
}
