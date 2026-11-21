//! Seat of a formation in the organigram of its jurisdiction: the stored datum
//! (`decisions.seat`), starting at level 0, the jurisdiction itself.

use serde::de::{value::StrDeserializer, IntoDeserializer};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Pose du qualificatif sur une chambre : « 1re chambre civile » est un nom,
/// « 2e chambre, protection sociale » une chambre dont on note la matière.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agreement {
    Adjective(&'static str),
    Complement(&'static str),
    Apposition(&'static str),
}

impl Agreement {
    fn text(self) -> &'static str {
        match self {
            Agreement::Adjective(t) | Agreement::Complement(t) | Agreement::Apposition(t) => t,
        }
    }
}

/// Spécialisations `matiere:*` : (uid, dénomination sous sa cour, accord sur une
/// chambre). La dénomination est le miroir exact du seed `facet_value`.
pub const SPECIALTY_LABELS: &[(&str, &str, Agreement)] = &[
    (
        "matiere:CIVILE",
        "Chambre civile",
        Agreement::Adjective("civile"),
    ),
    (
        "matiere:SOCIALE",
        "Chambre sociale",
        Agreement::Adjective("sociale"),
    ),
    (
        "matiere:COMMERCIALE",
        "Chambre commerciale",
        Agreement::Adjective("commerciale"),
    ),
    (
        "matiere:COMMERCIALE_FINANCIERE_ECONOMIQUE",
        "Chambre commerciale, financière et économique",
        Agreement::Adjective("commerciale, financière et économique"),
    ),
    (
        "matiere:CRIMINELLE",
        "Chambre criminelle",
        Agreement::Adjective("criminelle"),
    ),
    (
        "matiere:CORRECTIONNELLE",
        "Chambre correctionnelle",
        Agreement::Adjective("correctionnelle"),
    ),
    (
        "matiere:PRUD_HOMALE",
        "Chambre prud'homale",
        Agreement::Adjective("prud'homale"),
    ),
    (
        "matiere:PROTECTION_SOCIALE",
        "Chambre de la protection sociale",
        Agreement::Apposition("protection sociale"),
    ),
    (
        "matiere:PROCEDURES_COLLECTIVES",
        "Chambre des procédures collectives",
        Agreement::Apposition("procédures collectives"),
    ),
    (
        "matiere:INSTRUCTION",
        "Chambre de l'instruction",
        Agreement::Complement("de l'instruction"),
    ),
    (
        "matiere:FAMILLE",
        "Chambre de la famille",
        Agreement::Complement("de la famille"),
    ),
    (
        "matiere:BAUX",
        "Chambre des baux",
        Agreement::Complement("des baux"),
    ),
    (
        "matiere:CONSTRUCTION",
        "Chambre de la construction",
        Agreement::Complement("de la construction"),
    ),
    (
        "matiere:ETRANGERS",
        "Étrangers et rétention",
        Agreement::Apposition("étrangers et rétention"),
    ),
    (
        "matiere:EXPROPRIATION",
        "Chambre de l'expropriation",
        Agreement::Apposition("expropriation"),
    ),
    (
        "matiere:PROXIMITE",
        "Chambre de proximité",
        Agreement::Apposition("proximité"),
    ),
    (
        "matiere:SURENDETTEMENT",
        "Chambre du surendettement",
        Agreement::Apposition("surendettement"),
    ),
    (
        "matiere:COPROPRIETE",
        "Chambre de la copropriété",
        Agreement::Apposition("copropriété"),
    ),
    (
        "matiere:URGENCES",
        "Chambre des urgences",
        Agreement::Apposition("urgences"),
    ),
    (
        "matiere:DALO",
        "Chambre du droit au logement (DALO)",
        Agreement::Apposition("droit au logement (DALO)"),
    ),
    (
        "matiere:MINEURS",
        "Chambre des mineurs",
        Agreement::Complement("des mineurs"),
    ),
    (
        "matiere:NATIONALITE",
        "Chambre de la nationalité",
        Agreement::Apposition("nationalité"),
    ),
    (
        "matiere:TERRES",
        "Chambre des terres",
        Agreement::Complement("des terres"),
    ),
    (
        "matiere:CIVI",
        "Commission d'indemnisation des victimes (CIVI)",
        Agreement::Apposition("indemnisation des victimes (CIVI)"),
    ),
    (
        "matiere:REFERES",
        "Chambre des référés",
        Agreement::Complement("des référés"),
    ),
    (
        "matiere:PRESSE",
        "Chambre de la presse",
        Agreement::Apposition("presse"),
    ),
];

pub fn specialty_label(uid: &str) -> Option<&'static str> {
    SPECIALTY_LABELS
        .iter()
        .find(|(u, _, _)| *u == uid)
        .map(|(_, l, _)| *l)
}

pub fn specialty_agreement(uid: &str) -> Option<Agreement> {
    SPECIALTY_LABELS
        .iter()
        .find(|(u, _, _)| *u == uid)
        .map(|(_, _, a)| *a)
}

/// Uid statique d'une spécialisation, désignée par son uid complet ou par son
/// suffixe nu, qui est la clé d'une chambre qu'aucune coordonnée ne positionne.
pub fn static_specialty(uid: &str) -> Option<&'static str> {
    SPECIALTY_LABELS
        .iter()
        .map(|(u, _, _)| *u)
        .find(|u| *u == uid || suffix(u) == uid)
}

/// Ordinal français plein texte : « 1re », « 2e »… sans exposant Unicode, la
/// même forme alimentant `search_title`/BM25.
fn ordinal(n: u32) -> String {
    if n == 1 {
        "1re".to_string()
    } else {
        format!("{n}e")
    }
}

/// Numéros d'une formation réunie, en ordre canonique.
fn sorted(nums: &[u32]) -> Vec<u32> {
    let mut v = nums.to_vec();
    v.sort_unstable();
    v.dedup();
    v
}

fn join(nums: &[u32], sep: char) -> String {
    nums.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(&sep.to_string())
}

fn suffix(uid: &str) -> &str {
    uid.split_once(':').map_or(uid, |(_, s)| s)
}

/// What holds the chambers: a cour d'appel's pôle, a tribunal administratif's
/// section, the General Court and the Civil Service Tribunal beside the Court of Justice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Container {
    Pole(u32),
    Section(u32),
    Tribunal,
    TribunalFonctionPublique,
}

impl Container {
    fn label(&self) -> String {
        match self {
            Container::Pole(n) => format!("Pôle {n}"),
            Container::Section(n) => format!("{} section", ordinal(*n)),
            Container::Tribunal => "Tribunal".to_string(),
            Container::TribunalFonctionPublique => "Tribunal de la fonction publique".to_string(),
        }
    }

    /// The General Court and the Civil Service Tribunal are names, which keep their capital.
    fn is_proper_name(self) -> bool {
        matches!(
            self,
            Container::Tribunal | Container::TribunalFonctionPublique
        )
    }

    fn token(&self) -> String {
        match self {
            Container::Pole(n) => format!("P{n}"),
            Container::Section(n) => format!("SC{n}"),
            Container::Tribunal => "TRIBUNAL".to_string(),
            Container::TribunalFonctionPublique => "TFP".to_string(),
        }
    }
}

/// Comment la chambre se désigne. Les lettres sont stockées en majuscule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChamberId {
    Number(u32),
    Letter(char),
    /// TCOM « Chambre 2-5 » : numérotation composée chambre-section.
    Composed(u32, u32),
}

impl ChamberId {
    fn label(&self) -> String {
        match self {
            ChamberId::Number(n) => format!("{} chambre", ordinal(*n)),
            ChamberId::Letter(c) => format!("Chambre {c}"),
            ChamberId::Composed(a, b) => format!("Chambre {a}-{b}"),
        }
    }
}

/// Formations siégeant ensemble. Un singleton de sous-sections est la
/// sous-section jugeant seule du Conseil d'État d'avant 2016.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Joined {
    SubSections(Vec<u32>),
    Chambers(Vec<u32>),
}

impl Joined {
    fn label(&self) -> String {
        match self {
            Joined::SubSections(v) => match sorted(v).as_slice() {
                [seul] => format!("{} sous-section", ordinal(*seul)),
                nums => format!("Sous-sections {} réunies", join(nums, '/')),
            },
            Joined::Chambers(v) => format!("Chambres {} réunies", join(&sorted(v), '/')),
        }
    }
}

/// Formations that take the chamber's place instead of sitting in one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Solemn {
    Assemblee,
    AssembleePleniere,
    Specialisee,
    SectionContentieux,
    PresidentSectionContentieux,
    ChambresReunies,
    ChambreMixte,
    FormationMixte,
    Avis,
    GrandeChambre,
    President,
    VicePresident,
    AdmissionPourvois,
    ChambrePourvois,
    ChambrePrejudicielle,
    ChambreIntermediaire,
}

impl Solemn {
    fn label(self) -> &'static str {
        match self {
            Solemn::Assemblee => "Assemblée du contentieux",
            Solemn::AssembleePleniere => "Assemblée plénière",
            Solemn::Specialisee => "Formation spécialisée",
            Solemn::SectionContentieux => "Section du contentieux",
            Solemn::PresidentSectionContentieux => "Président de la section du contentieux",
            Solemn::ChambresReunies => "Chambres réunies",
            Solemn::ChambreMixte => "Chambre mixte",
            Solemn::FormationMixte => "Formation mixte",
            Solemn::Avis => "Formation consultative",
            Solemn::GrandeChambre => "Grande chambre",
            Solemn::President => "Président",
            Solemn::VicePresident => "Vice-président",
            Solemn::AdmissionPourvois => "Chambre d'admission des pourvois",
            Solemn::ChambrePourvois => "Chambre des pourvois",
            Solemn::ChambrePrejudicielle => "Chambre préjudicielle",
            Solemn::ChambreIntermediaire => "Chambre intermédiaire",
        }
    }

    /// Path token, which is the serde form of the variant.
    pub fn token(self) -> &'static str {
        match self {
            Solemn::Assemblee => "ASSEMBLEE",
            Solemn::AssembleePleniere => "ASSEMBLEE_PLENIERE",
            Solemn::Specialisee => "SPECIALISEE",
            Solemn::SectionContentieux => "SECTION_CONTENTIEUX",
            Solemn::PresidentSectionContentieux => "PRESIDENT_SECTION_CONTENTIEUX",
            Solemn::ChambresReunies => "CHAMBRES_REUNIES",
            Solemn::ChambreMixte => "CHAMBRE_MIXTE",
            Solemn::FormationMixte => "FORMATION_MIXTE",
            Solemn::Avis => "AVIS",
            Solemn::GrandeChambre => "GRANDE_CHAMBRE",
            Solemn::President => "PRESIDENT",
            Solemn::VicePresident => "VICE_PRESIDENT",
            Solemn::AdmissionPourvois => "ADMISSION_POURVOIS",
            Solemn::ChambrePourvois => "CHAMBRE_POURVOIS",
            Solemn::ChambrePrejudicielle => "CHAMBRE_PREJUDICIELLE",
            Solemn::ChambreIntermediaire => "CHAMBRE_INTERMEDIAIRE",
        }
    }

    /// Reads a token back, which is how the extraction names the variant it
    /// has just read in a source field.
    pub fn from_token(token: &str) -> Option<Solemn> {
        let form: StrDeserializer<serde::de::value::Error> = token.into_deserializer();
        Solemn::deserialize(form).ok()
    }
}

/// Historic renamings, read by the path derivation alone: the seat stores the
/// name of its date, the path one key per room. `SS` is the prefix of the
/// sub-sections the Conseil d'État turned into chambers in 2016, and the
/// commercial chamber of the Cour de cassation took its full name in 1989.
const RENAMED: &[(&str, &str)] = &[
    ("CRR", "CNDA"),
    ("SS", "C"),
    ("CHAMBRES_REUNIES", "ASSEMBLEE_PLENIERE"),
    ("COMMERCIALE_FINANCIERE_ECONOMIQUE", "COMMERCIALE"),
];

fn renamed(token: &str) -> &str {
    RENAMED
        .iter()
        .find(|(old, _)| *old == token)
        .map_or(token, |(_, new)| *new)
}

/// Streams of one room, read by the path derivation alone like `RENAMED`:
/// prud'hommes and social security are the two contentieux of a cour d'appel's
/// chambre sociale and the two spellings of a tribunal's pôle social. Angers
/// keys them « Chambre Prud'homale » and « Chambre Sécurité sociale » and prints
/// « CHAMBRE SOCIALE » on both arrêts. The seat keeps the stream, the path
/// carries the room. A numbered chamber keeps its matter: the number identifies
/// it, and the stream does not split it. A tribunal administratif has neither
/// room, its social contentieux keep their name.
const FOLDED: &[(&str, &str)] = &[
    ("matiere:PRUD_HOMALE", "matiere:SOCIALE"),
    ("matiere:PROTECTION_SOCIALE", "matiere:SOCIALE"),
];

fn folded(uid: &'static str) -> &'static str {
    FOLDED
        .iter()
        .find(|(from, _)| *from == uid)
        .map_or(uid, |(_, to)| *to)
}

/// The legal order a court sits in, the first segment of its seat paths: its
/// type's, or for a foreign court the place its code starts with.
pub fn jurisdiction_order(jurisdiction_type: &str, jurisdiction_code: Option<&str>) -> String {
    match renamed(jurisdiction_type) {
        "CJUE" | "COMEU" => "EU".to_string(),
        "CEDH" => "INTL".to_string(),
        "ADLC" | "AMF" | "ASSISES" | "CA" | "CAA" | "CC" | "CCD" | "CE" | "CNDA" | "CNIL"
        | "CONSTIT" | "CPH" | "DDD" | "TA" | "TC" | "TCOM" | "TJ" => "FR".to_string(),
        foreign if lj_vocab::courts::is_foreign(foreign) => lj_vocab::courts::foreign_order(
            jurisdiction_code
                .unwrap_or_else(|| panic!("a foreign court without a code: {foreign}")),
        ),
        other => panic!("court type without a legal order: {other}"),
    }
}

/// The trunk of a seat path: the legal order, the court type, then the court
/// when its code names one. A filter translating a jurisdiction into the path
/// axis derives it here, so a translated value and an indexed path can never
/// disagree.
pub fn jurisdiction_trunk(jurisdiction_type: &str, jurisdiction_code: Option<&str>) -> Vec<String> {
    if jurisdiction_type.is_empty() {
        return Vec::new();
    }
    let mut trunk = vec![
        jurisdiction_order(jurisdiction_type, jurisdiction_code),
        renamed(jurisdiction_type).to_string(),
    ];
    let code = match lj_vocab::courts::is_foreign(jurisdiction_type) {
        true => jurisdiction_code.map(lj_vocab::courts::foreign_court_rest),
        false => jurisdiction_code,
    };
    trunk.extend(court_token(jurisdiction_type, code));
    trunk
}

/// Segment 1, the court: its `<type>_` prefix dropped and uppercased. Absent
/// when the code names the type itself, a jurisdiction that has one court.
fn court_token(jurisdiction_type: &str, jurisdiction_code: Option<&str>) -> Option<String> {
    let code = jurisdiction_code?;
    let kind = jurisdiction_type.to_lowercase();
    let name = code.strip_prefix(&format!("{kind}_")).unwrap_or(code);
    (!name.is_empty() && name != kind).then(|| name.to_uppercase())
}

/// Slots that fill in freely, in whichever order they are given. Serde is the
/// validation boundary of the stored column: an unknown field or an unknown
/// speciality is refused, never absorbed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seat {
    pub jurisdiction_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jurisdiction_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<Container>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chamber: Option<ChamberId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solemn: Option<Solemn>,
    /// Ordered, and holds two entries when the source writes a composed matter
    /// that names the whole room, « 1re chambre civile et commerciale ».
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "write_specialty",
        deserialize_with = "read_specialty"
    )]
    pub specialty: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub letter: Option<char>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub joined: Option<Joined>,
}

/// Bare stored form, `CIVILE` and not `matiere:CIVILE`.
fn write_specialty<S: Serializer>(uids: &[&'static str], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(uids.iter().copied().map(suffix))
}

fn read_specialty<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<&'static str>, D::Error> {
    Vec::<String>::deserialize(d)?
        .into_iter()
        .map(|s| {
            static_specialty(&s)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown speciality: {s}")))
        })
        .collect()
}

impl Seat {
    /// True when no room slot is filled: level 0 alone locates a jurisdiction,
    /// not a formation inside it.
    pub fn is_empty(&self) -> bool {
        self.container.is_none()
            && self.chamber.is_none()
            && self.solemn.is_none()
            && self.specialty.is_empty()
            && self.section.is_none()
            && self.letter.is_none()
            && self.joined.is_none()
    }

    /// Libellé affichable, fonction totale des emplacements.
    pub fn label(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(c) = self.container {
            parts.push(c.label());
        }
        match (self.chamber, self.solemn) {
            (Some(ch), _) => parts.push(qualify(ch, &self.specialty)),
            (None, Some(s)) => parts.push(s.label().to_string()),
            (None, None) if !self.specialty.is_empty() => {
                let mut l = specialty_label(self.specialty[0])?.to_string();
                if let Some(rest) = qualifier(&self.specialty[1..]) {
                    l = format!("{l} et {rest}");
                }
                parts.push(if parts.is_empty() {
                    l
                } else {
                    lowercase_first(&l)
                });
            }
            (None, None) => {}
        }
        if let Some(n) = self.section {
            parts.push(format!("{} section", ordinal(n)));
        }
        if let Some(r) = &self.joined {
            parts.push(r.label());
        }
        let base = parts.join(", ");
        match (base.is_empty(), self.letter) {
            (true, Some(l)) => Some(format!("Section {l}")),
            (true, None) => None,
            // Une section nue porte sa lettre collée, « 9e section D ».
            (false, Some(l)) if self.chamber.is_none() && self.section.is_some() => {
                Some(format!("{base} {l}"))
            }
            (false, Some(l)) => Some(format!("{base}, section {l}")),
            (false, None) => Some(base),
        }
    }

    /// The label as a title reads it after the jurisdiction: « pôle 5, 3e chambre », but
    /// « Tribunal, 5e chambre ».
    pub fn title_label(&self) -> Option<String> {
        let label = self.label()?;
        Some(
            match self.container.is_some_and(Container::is_proper_name) {
                true => label,
                false => crate::titles::mid_title(&label),
            },
        )
    }

    /// Every path the seat sits under, ancestors unfolded and deduplicated. One
    /// function and one set, so that no caller can forget the unfolding; empty
    /// when nothing locates the jurisdiction itself.
    pub fn paths(&self) -> Vec<String> {
        if self.jurisdiction_type.is_empty() {
            return Vec::new();
        }
        let mut trunk =
            jurisdiction_trunk(&self.jurisdiction_type, self.jurisdiction_code.as_deref());
        trunk.extend(self.division());
        let mut paths: Vec<String> = Vec::new();
        for room in self.rooms() {
            let mut segments = trunk.clone();
            segments.extend(room);
            for depth in 1..=segments.len() {
                let path = segments[..depth].join(".");
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
        paths
    }

    /// The division the rooms sit in, when the court has one. A composed
    /// numbering names it in its FIRST number: « Chambre 1-5 » is the fifth room
    /// of the first division, which Paris writes « Pôle 1 - Chambre 5 » on the
    /// same decision, the same day, for the same room. Unfolded into two rooms,
    /// that number would sit where a chamber goes, on the chamber that carries it.
    fn division(&self) -> Option<String> {
        match (self.container, self.chamber) {
            (Some(container), _) => Some(container.token()),
            (None, Some(ChamberId::Composed(a, _))) => Some(format!("P{a}")),
            (None, _) => None,
        }
    }

    /// Segments below the division, one entry per sibling path: formations
    /// sitting together belong under each of the chambers that compose them.
    fn rooms(&self) -> Vec<Vec<String>> {
        if let Some(chamber) = self.chamber {
            return vec![match chamber {
                ChamberId::Number(n) => vec![self.named(format!("C{n}"))],
                ChamberId::Letter(l) => vec![self.named(format!("C{l}"))],
                // No matter here: it names the pair, never one of the two, and
                // the label of a composed numbering drops it too.
                ChamberId::Composed(_, b) => vec![format!("C{b}")],
            }];
        }
        if let Some(solemn) = self.solemn {
            return vec![vec![renamed(solemn.token()).to_string()]];
        }
        match &self.joined {
            Some(Joined::SubSections(v)) => split_joined(v, renamed("SS")),
            Some(Joined::Chambers(v)) => split_joined(v, "C"),
            None => vec![self.folded_token().into_iter().collect()],
        }
    }

    /// Chamber and matters in the SAME segment, joined as the label joins them.
    fn named(&self, chamber: String) -> String {
        match self.specialty_token() {
            Some(matters) => format!("{chamber}_{matters}"),
            None => chamber,
        }
    }

    fn specialty_token(&self) -> Option<String> {
        (!self.specialty.is_empty()).then(|| {
            self.specialty
                .iter()
                .copied()
                .map(suffix)
                .collect::<Vec<_>>()
                .join("_")
        })
    }

    /// The matter-only room, its streams folded into their room.
    fn folded_token(&self) -> Option<String> {
        let folds = matches!(self.jurisdiction_type.as_str(), "CA" | "TJ");
        let mut tokens: Vec<&str> = Vec::new();
        for uid in &self.specialty {
            let token = renamed(suffix(if folds { folded(uid) } else { uid }));
            if !tokens.contains(&token) {
                tokens.push(token);
            }
        }
        (!tokens.is_empty()).then(|| tokens.join("_"))
    }
}

fn split_joined(nums: &[u32], tag: &str) -> Vec<Vec<String>> {
    sorted(nums)
        .iter()
        .map(|n| vec![format!("{tag}{n}")])
        .collect()
}

/// Qualificatif d'une chambre par sa spécialisation. Une numérotation composée
/// porte déjà deux nombres, rien ne s'y accroche que l'apposition.
fn qualify(ch: ChamberId, uids: &[&str]) -> String {
    let base = ch.label();
    let Some(q) = qualifier(uids) else {
        return base;
    };
    match (ch, specialty_agreement(uids[0])) {
        (_, Some(Agreement::Apposition(_))) => format!("{base}, {q}"),
        (ChamberId::Composed(..), _) => base,
        _ => format!("{base} {q}"),
    }
}

/// « civile et commerciale » : the matters of one room, joined. `None` as soon
/// as one of them is unknown, the caller then naming the room without them.
fn qualifier(uids: &[&str]) -> Option<String> {
    let texts: Vec<&str> = uids
        .iter()
        .map(|u| specialty_agreement(u).map(Agreement::text))
        .collect::<Option<_>>()?;
    (!texts.is_empty()).then(|| texts.join(" et "))
}

fn lowercase_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(p) => p.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn number(s: &str) -> Option<u32> {
    (s.len() <= 2 && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
        .filter(|n| *n > 0)
}

fn chamber_payload(s: &str) -> Option<ChamberId> {
    if let Some((a, b)) = s.split_once('-') {
        return Some(ChamberId::Composed(number(a)?, number(b)?));
    }
    match number(s) {
        Some(n) => Some(ChamberId::Number(n)),
        None => letter(s).map(ChamberId::Letter),
    }
}

/// Lettre de section ou de chambre : un caractère alphabétique, en majuscule.
fn letter(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let c = chars.next().filter(char::is_ascii_alphabetic)?;
    chars.next().is_none().then(|| c.to_ascii_uppercase())
}

/// Label of the LAST node of a path, the tree already showing its ancestors.
/// `None` on an order, a jurisdiction type and a court, which the API resolves
/// from its own referential, and on anything outside the grammar of a segment.
pub fn seat_path_label(path: &str) -> Option<String> {
    let (_, path) = path.split_once('.')?;
    let segment = path.rsplit('.').next()?;
    if !path.contains('.') && crate::titles::jurisdiction_type_label(segment).is_some() {
        return None;
    }
    if let Some(n) = segment.strip_prefix("SC").and_then(number) {
        return Some(Container::Section(n).label());
    }
    if let Some(container) = [Container::Tribunal, Container::TribunalFonctionPublique]
        .into_iter()
        .find(|c| c.token() == segment)
    {
        return Some(container.label());
    }
    if let Some(n) = segment.strip_prefix('P').and_then(number) {
        return Some(Container::Pole(n).label());
    }
    if let Some(solemn) = Solemn::from_token(segment) {
        return Some(solemn.label().to_string());
    }
    let (head, matters) = segment
        .split_once('_')
        .map_or((segment, None), |(h, m)| (h, Some(m)));
    if let Some(chamber) = head.strip_prefix('C').and_then(chamber_payload) {
        let uids = match matters {
            Some(m) => read_matters(m)?,
            None => Vec::new(),
        };
        return Some(qualify(chamber, &uids));
    }
    let uids = read_matters(segment)?;
    // The pôle social is what a tribunal judiciaire has had since 2019 where a
    // cour d'appel has a chambre sociale.
    if uids == ["matiere:SOCIALE"] && path.starts_with("TJ.") {
        return Some("Pôle social".to_string());
    }
    // One node holds the room under both its names, and shows the current one.
    if uids == ["matiere:COMMERCIALE"] && path.starts_with("CC.") {
        return specialty_label("matiere:COMMERCIALE_FINANCIERE_ECONOMIQUE").map(str::to_string);
    }
    let mut label = specialty_label(uids[0])?.to_string();
    if let Some(rest) = qualifier(&uids[1..]) {
        label = format!("{label} et {rest}");
    }
    Some(label)
}

/// Matters glued to a segment, read longest first: a suffix can be the head of
/// a longer one, `CIVI` of `CIVILE`.
pub fn read_matters(mut rest: &str) -> Option<Vec<&'static str>> {
    let mut uids: Vec<&'static str> = Vec::new();
    while !rest.is_empty() {
        let uid = SPECIALTY_LABELS
            .iter()
            .map(|(u, _, _)| *u)
            .filter(|u| {
                rest.strip_prefix(suffix(u))
                    .is_some_and(|r| r.is_empty() || r.starts_with('_'))
            })
            .max_by_key(|u| suffix(u).len())?;
        rest = &rest[suffix(uid).len()..];
        rest = rest.strip_prefix('_').unwrap_or(rest);
        uids.push(uid);
    }
    (!uids.is_empty()).then_some(uids)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOLEMN: [(Solemn, &str); 16] = [
        (Solemn::Assemblee, "Assemblée du contentieux"),
        (Solemn::AssembleePleniere, "Assemblée plénière"),
        (Solemn::Specialisee, "Formation spécialisée"),
        (Solemn::SectionContentieux, "Section du contentieux"),
        (
            Solemn::PresidentSectionContentieux,
            "Président de la section du contentieux",
        ),
        (Solemn::ChambresReunies, "Chambres réunies"),
        (Solemn::ChambreMixte, "Chambre mixte"),
        (Solemn::FormationMixte, "Formation mixte"),
        (Solemn::Avis, "Formation consultative"),
        (Solemn::GrandeChambre, "Grande chambre"),
        (Solemn::President, "Président"),
        (Solemn::VicePresident, "Vice-président"),
        (
            Solemn::AdmissionPourvois,
            "Chambre d'admission des pourvois",
        ),
        (Solemn::ChambrePourvois, "Chambre des pourvois"),
        (Solemn::ChambrePrejudicielle, "Chambre préjudicielle"),
        (Solemn::ChambreIntermediaire, "Chambre intermédiaire"),
    ];

    fn seat(chamber: ChamberId) -> Seat {
        Seat {
            chamber: Some(chamber),
            ..Default::default()
        }
    }

    fn room(chamber: Option<ChamberId>, specialty: &[&'static str]) -> Seat {
        Seat {
            chamber,
            specialty: specialty.to_vec(),
            ..Default::default()
        }
    }

    fn at(jurisdiction_type: &str) -> Seat {
        Seat {
            jurisdiction_type: jurisdiction_type.to_string(),
            ..Default::default()
        }
    }

    fn court(jurisdiction_type: &str, code: &str) -> Seat {
        Seat {
            jurisdiction_code: Some(code.to_string()),
            ..at(jurisdiction_type)
        }
    }

    /// The room alone, which is what the last segment of a path names: the tree
    /// carries the container, and the label keeps the section and the letter.
    fn room_of(seat: &Seat) -> Seat {
        Seat {
            container: None,
            section: None,
            letter: None,
            ..seat.clone()
        }
    }

    /// One seat per shape of room, each with the set `paths()` must unfold.
    fn paths_table() -> Vec<(Seat, Vec<&'static str>)> {
        vec![
            (court("CNDA", "cnda"), vec!["FR", "FR.CNDA"]),
            (
                Seat {
                    container: Some(Container::Tribunal),
                    chamber: Some(ChamberId::Number(4)),
                    ..court("CJUE", "cjue")
                },
                vec!["EU", "EU.CJUE", "EU.CJUE.TRIBUNAL", "EU.CJUE.TRIBUNAL.C4"],
            ),
            (
                Seat {
                    container: Some(Container::TribunalFonctionPublique),
                    solemn: Some(Solemn::President),
                    ..court("CJUE", "cjue")
                },
                vec!["EU", "EU.CJUE", "EU.CJUE.TFP", "EU.CJUE.TFP.PRESIDENT"],
            ),
            (
                Seat {
                    chamber: Some(ChamberId::Number(1)),
                    specialty: vec!["matiere:CIVILE"],
                    ..court("CC", "cc")
                },
                vec!["FR", "FR.CC", "FR.CC.C1_CIVILE"],
            ),
            (
                Seat {
                    specialty: vec!["matiere:SOCIALE"],
                    ..court("CC", "cc")
                },
                vec!["FR", "FR.CC", "FR.CC.SOCIALE"],
            ),
            (
                Seat {
                    specialty: vec!["matiere:PRUD_HOMALE", "matiere:SOCIALE"],
                    ..court("CA", "ca_angers")
                },
                vec!["FR", "FR.CA", "FR.CA.ANGERS", "FR.CA.ANGERS.SOCIALE"],
            ),
            (
                Seat {
                    specialty: vec!["matiere:PROTECTION_SOCIALE"],
                    ..court("TJ", "tj_marseille")
                },
                vec!["FR", "FR.TJ", "FR.TJ.MARSEILLE", "FR.TJ.MARSEILLE.SOCIALE"],
            ),
            (
                Seat {
                    chamber: Some(ChamberId::Number(9)),
                    specialty: vec!["matiere:PROTECTION_SOCIALE"],
                    ..court("CA", "ca_rennes")
                },
                vec![
                    "FR",
                    "FR.CA",
                    "FR.CA.RENNES",
                    "FR.CA.RENNES.C9_PROTECTION_SOCIALE",
                ],
            ),
            (
                Seat {
                    specialty: vec!["matiere:PROTECTION_SOCIALE"],
                    ..court("TA", "ta_nimes")
                },
                vec![
                    "FR",
                    "FR.TA",
                    "FR.TA.NIMES",
                    "FR.TA.NIMES.PROTECTION_SOCIALE",
                ],
            ),
            (
                Seat {
                    container: Some(Container::Pole(5)),
                    chamber: Some(ChamberId::Number(6)),
                    ..court("CA", "ca_paris")
                },
                vec![
                    "FR",
                    "FR.CA",
                    "FR.CA.PARIS",
                    "FR.CA.PARIS.P5",
                    "FR.CA.PARIS.P5.C6",
                ],
            ),
            (
                Seat {
                    container: Some(Container::Section(1)),
                    chamber: Some(ChamberId::Number(1)),
                    ..court("TA", "ta_paris")
                },
                vec![
                    "FR",
                    "FR.TA",
                    "FR.TA.PARIS",
                    "FR.TA.PARIS.SC1",
                    "FR.TA.PARIS.SC1.C1",
                ],
            ),
            (
                Seat {
                    chamber: Some(ChamberId::Number(1)),
                    specialty: vec!["matiere:CIVILE", "matiere:COMMERCIALE"],
                    ..court("CA", "ca_besancon")
                },
                vec![
                    "FR",
                    "FR.CA",
                    "FR.CA.BESANCON",
                    "FR.CA.BESANCON.C1_CIVILE_COMMERCIALE",
                ],
            ),
            (
                Seat {
                    chamber: Some(ChamberId::Letter('D')),
                    specialty: vec!["matiere:COMMERCIALE"],
                    ..court("CA", "ca_angers")
                },
                vec!["FR", "FR.CA", "FR.CA.ANGERS", "FR.CA.ANGERS.CD_COMMERCIALE"],
            ),
            (
                Seat {
                    container: Some(Container::Pole(6)),
                    chamber: Some(ChamberId::Number(3)),
                    specialty: vec!["matiere:PROTECTION_SOCIALE"],
                    letter: Some('B'),
                    ..court("TJ", "tj_paris")
                },
                vec![
                    "FR",
                    "FR.TJ",
                    "FR.TJ.PARIS",
                    "FR.TJ.PARIS.P6",
                    "FR.TJ.PARIS.P6.C3_PROTECTION_SOCIALE",
                ],
            ),
            (
                Seat {
                    chamber: Some(ChamberId::Composed(1, 2)),
                    specialty: vec!["matiere:CIVILE"],
                    ..court("CA", "ca_aix_en_provence")
                },
                vec![
                    "FR",
                    "FR.CA",
                    "FR.CA.AIX_EN_PROVENCE",
                    "FR.CA.AIX_EN_PROVENCE.P1",
                    "FR.CA.AIX_EN_PROVENCE.P1.C2",
                ],
            ),
            (
                Seat {
                    chamber: Some(ChamberId::Number(2)),
                    section: Some(3),
                    letter: Some('B'),
                    ..court("TA", "ta_paris")
                },
                vec!["FR", "FR.TA", "FR.TA.PARIS", "FR.TA.PARIS.C2"],
            ),
            (
                Seat {
                    section: Some(8),
                    ..court("TA", "ta_paris")
                },
                vec!["FR", "FR.TA", "FR.TA.PARIS"],
            ),
            (
                Seat {
                    joined: Some(Joined::SubSections(vec![3, 5])),
                    ..court("CE", "ce")
                },
                vec!["FR", "FR.CE", "FR.CE.C3", "FR.CE.C5"],
            ),
            (
                Seat {
                    joined: Some(Joined::SubSections(vec![3])),
                    ..court("CE", "ce")
                },
                vec!["FR", "FR.CE", "FR.CE.C3"],
            ),
            (
                Seat {
                    joined: Some(Joined::Chambers(vec![3, 8])),
                    ..court("CE", "ce")
                },
                vec!["FR", "FR.CE", "FR.CE.C3", "FR.CE.C8"],
            ),
            (
                Seat {
                    solemn: Some(Solemn::Assemblee),
                    ..court("CE", "ce")
                },
                vec!["FR", "FR.CE", "FR.CE.ASSEMBLEE"],
            ),
            (
                Seat {
                    solemn: Some(Solemn::ChambresReunies),
                    ..court("CC", "cc")
                },
                vec!["FR", "FR.CC", "FR.CC.ASSEMBLEE_PLENIERE"],
            ),
        ]
    }

    #[test]
    fn the_stored_seat_makes_a_json_round_trip() {
        let mut cas = vec![
            at("CNDA"),
            Seat {
                jurisdiction_code: Some("tj_paris".to_string()),
                ..at("TJ")
            },
            Seat {
                container: Some(Container::Pole(5)),
                chamber: Some(ChamberId::Number(6)),
                ..at("CA")
            },
            Seat {
                container: Some(Container::Section(4)),
                chamber: Some(ChamberId::Letter('D')),
                section: Some(1),
                letter: Some('B'),
                ..at("TA")
            },
            Seat {
                chamber: Some(ChamberId::Composed(2, 5)),
                ..at("TCOM")
            },
            Seat {
                joined: Some(Joined::SubSections(vec![3, 5])),
                ..at("CE")
            },
            Seat {
                joined: Some(Joined::Chambers(vec![3, 8])),
                ..at("CE")
            },
            Seat {
                chamber: Some(ChamberId::Number(1)),
                specialty: vec!["matiere:CIVILE"],
                ..at("CA")
            },
            Seat {
                chamber: Some(ChamberId::Number(1)),
                specialty: vec!["matiere:CIVILE", "matiere:COMMERCIALE"],
                ..at("CA")
            },
        ];
        cas.extend(SOLEMN.map(|(s, _)| Seat {
            solemn: Some(s),
            ..at("CE")
        }));
        for seat in cas {
            let json = serde_json::to_value(&seat).unwrap();
            assert_eq!(
                serde_json::from_value::<Seat>(json.clone()).unwrap(),
                seat,
                "{json}"
            );
        }
    }

    #[test]
    fn level_zero_alone_serialises_to_its_two_fields() {
        let jurisdiction = Seat {
            jurisdiction_code: Some("tj_paris".to_string()),
            ..at("TJ")
        };
        assert_eq!(
            serde_json::to_string(&jurisdiction).unwrap(),
            r#"{"jurisdiction_type":"TJ","jurisdiction_code":"tj_paris"}"#
        );
        assert!(jurisdiction.is_empty());
        let room = Seat {
            solemn: Some(Solemn::AssembleePleniere),
            ..at("CC")
        };
        assert!(!room.is_empty());
        assert_eq!(
            serde_json::to_string(&room).unwrap(),
            r#"{"jurisdiction_type":"CC","solemn":"ASSEMBLEE_PLENIERE"}"#
        );
    }

    #[test]
    fn the_serde_boundary_refuses_what_it_does_not_know() {
        assert!(serde_json::from_str::<Seat>(r#"{"jurisdiction_type":"CA","chambre":2}"#).is_err());
        assert!(serde_json::from_str::<Seat>(
            r#"{"jurisdiction_type":"CA","specialty":["ASTRAL"]}"#
        )
        .is_err());
        let stored = r#"{"jurisdiction_type":"CA","chamber":{"number":1},"specialty":["CIVILE","COMMERCIALE"]}"#;
        let read: Seat = serde_json::from_str(stored).unwrap();
        assert_eq!(read.specialty, ["matiere:CIVILE", "matiere:COMMERCIALE"]);
        assert_eq!(serde_json::to_string(&read).unwrap(), stored);
    }

    #[test]
    fn a_composed_matter_names_the_whole_room() {
        let numbered = Seat {
            chamber: Some(ChamberId::Number(1)),
            specialty: vec!["matiere:CIVILE", "matiere:COMMERCIALE"],
            ..at("CA")
        };
        assert_eq!(
            numbered.label().as_deref(),
            Some("1re chambre civile et commerciale")
        );
        let unnumbered = Seat {
            specialty: vec!["matiere:CIVILE", "matiere:COMMERCIALE"],
            ..at("CA")
        };
        assert_eq!(
            unnumbered.label().as_deref(),
            Some("Chambre civile et commerciale")
        );
    }

    #[test]
    fn a_solemn_formation_labels_the_room_itself() {
        for (solemn, label) in SOLEMN {
            let seat = Seat {
                solemn: Some(solemn),
                ..at("CE")
            };
            assert_eq!(seat.label().as_deref(), Some(label), "{solemn:?}");
        }
    }

    #[test]
    fn a_title_lowers_the_seat_unless_it_opens_on_a_court_s_name() {
        let pole = Seat {
            container: Some(Container::Pole(5)),
            chamber: Some(ChamberId::Number(3)),
            ..Default::default()
        };
        assert_eq!(pole.title_label().as_deref(), Some("pôle 5, 3e chambre"));
        let tribunal = Seat {
            container: Some(Container::Tribunal),
            chamber: Some(ChamberId::Number(5)),
            ..Default::default()
        };
        assert_eq!(
            tribunal.title_label().as_deref(),
            Some("Tribunal, 5e chambre")
        );
    }

    #[test]
    fn an_unnumbered_room_is_named_by_its_matter() {
        let nommee = Seat {
            specialty: vec!["matiere:SOCIALE"],
            ..Default::default()
        };
        assert_eq!(nommee.label().as_deref(), Some("Chambre sociale"));
        assert_eq!(Seat::default().label(), None);
    }

    #[test]
    fn the_matter_qualifies_the_numbered_room() {
        let deuxieme = Seat {
            chamber: Some(ChamberId::Number(2)),
            specialty: vec!["matiere:CIVILE"],
            ..Default::default()
        };
        assert_eq!(deuxieme.label().as_deref(), Some("2e chambre civile"));
        let sociale = Seat {
            chamber: Some(ChamberId::Number(2)),
            specialty: vec!["matiere:SOCIALE"],
            ..Default::default()
        };
        assert_ne!(deuxieme, sociale);
        assert_eq!(sociale.label().as_deref(), Some("2e chambre sociale"));
        let nue = Seat {
            chamber: Some(ChamberId::Number(2)),
            ..Default::default()
        };
        assert_eq!(nue.label().as_deref(), Some("2e chambre"));
    }

    /// Two permutations are one formation, under one label.
    #[test]
    fn les_formations_reunies_sont_un_ensemble() {
        let a = Seat {
            joined: Some(Joined::SubSections(vec![4, 1])),
            ..Default::default()
        };
        let b = Seat {
            joined: Some(Joined::SubSections(vec![1, 4])),
            ..Default::default()
        };
        assert_eq!(a.label().as_deref(), Some("Sous-sections 1/4 réunies"));
        assert_eq!(b.label(), a.label());
        let ce = Seat {
            joined: Some(Joined::Chambers(vec![10, 9])),
            ..Default::default()
        };
        assert_eq!(ce.label().as_deref(), Some("Chambres 9/10 réunies"));
    }

    #[test]
    fn la_specialisation_accorde_le_libelle_de_la_position() {
        let cas = [
            (
                room(Some(ChamberId::Number(2)), &["matiere:CIVILE"]),
                "2e chambre civile",
            ),
            (
                room(Some(ChamberId::Number(2)), &["matiere:PROTECTION_SOCIALE"]),
                "2e chambre, protection sociale",
            ),
            (
                room(Some(ChamberId::Number(1)), &["matiere:FAMILLE"]),
                "1re chambre de la famille",
            ),
            (
                Seat {
                    section: Some(4),
                    ..room(None, &["matiere:URGENCES"])
                },
                "Chambre des urgences, 4e section",
            ),
            (
                room(Some(ChamberId::Composed(4, 7)), &["matiere:CIVILE"]),
                "Chambre 4-7",
            ),
            (
                room(Some(ChamberId::Letter('D')), &["matiere:CIVILE"]),
                "Chambre D civile",
            ),
            (
                Seat {
                    container: Some(Container::Pole(1)),
                    ..room(Some(ChamberId::Number(11)), &["matiere:SOCIALE"])
                },
                "Pôle 1, 11e chambre sociale",
            ),
            (room(None, &["matiere:SOCIALE"]), "Chambre sociale"),
        ];
        for (seat, attendu) in cas {
            assert_eq!(seat.label().as_deref(), Some(attendu), "{seat:?}");
        }
    }

    #[test]
    fn les_emplacements_se_remplissent_librement() {
        let cas = [
            (
                Seat {
                    container: Some(Container::Pole(5)),
                    ..seat(ChamberId::Number(6))
                },
                "Pôle 5, 6e chambre",
            ),
            (
                Seat {
                    container: Some(Container::Section(4)),
                    ..seat(ChamberId::Number(1))
                },
                "4e section, 1re chambre",
            ),
            (
                Seat {
                    section: Some(1),
                    ..seat(ChamberId::Number(2))
                },
                "2e chambre, 1re section",
            ),
            (
                Seat {
                    section: Some(9),
                    letter: Some('D'),
                    ..Default::default()
                },
                "9e section D",
            ),
            (
                Seat {
                    letter: Some('B'),
                    ..Default::default()
                },
                "Section B",
            ),
            (seat(ChamberId::Letter('D')), "Chambre D"),
            (
                Seat {
                    joined: Some(Joined::SubSections(vec![3])),
                    ..Default::default()
                },
                "3e sous-section",
            ),
        ];
        for (seat, attendu) in cas {
            assert_eq!(seat.label().as_deref(), Some(attendu), "{seat:?}");
        }
    }

    #[test]
    fn the_unfolded_paths_carry_every_ancestor_once() {
        for (seat, expected) in paths_table() {
            assert_eq!(seat.paths(), expected, "{seat:?}");
        }
    }

    #[test]
    fn the_depth_follows_the_jurisdiction() {
        let cases = [
            (court("CNDA", "cnda"), "FR.CNDA", 2),
            (
                Seat {
                    chamber: Some(ChamberId::Number(1)),
                    specialty: vec!["matiere:CIVILE"],
                    ..court("CC", "cc")
                },
                "FR.CC.C1_CIVILE",
                3,
            ),
            (
                Seat {
                    container: Some(Container::Pole(5)),
                    chamber: Some(ChamberId::Number(6)),
                    ..court("CA", "ca_paris")
                },
                "FR.CA.PARIS.P5.C6",
                5,
            ),
            (
                Seat {
                    container: Some(Container::Section(1)),
                    chamber: Some(ChamberId::Number(1)),
                    ..court("TA", "ta_paris")
                },
                "FR.TA.PARIS.SC1.C1",
                5,
            ),
        ];
        for (seat, deepest, depth) in cases {
            assert_eq!(seat.paths().last().map(String::as_str), Some(deepest));
            assert_eq!(deepest.split('.').count(), depth);
        }
    }

    /// Two permutations are the same formation, hence the same siblings.
    #[test]
    fn a_permutation_of_joined_formations_gives_the_same_set() {
        let one = Seat {
            joined: Some(Joined::SubSections(vec![4, 1])),
            ..court("CE", "ce")
        };
        let other = Seat {
            joined: Some(Joined::SubSections(vec![1, 4])),
            ..court("CE", "ce")
        };
        assert_eq!(one.paths(), ["FR", "FR.CE", "FR.CE.C1", "FR.CE.C4"]);
        assert_eq!(one.paths(), other.paths());
    }

    #[test]
    fn no_path_is_malformed() {
        assert!(Seat::default().paths().is_empty());
        let mut seats: Vec<Seat> = paths_table().into_iter().map(|(s, _)| s).collect();
        seats.push(Seat {
            jurisdiction_code: Some(String::new()),
            ..at("TJ")
        });
        for path in seats.iter().flat_map(Seat::paths) {
            assert!(
                !path.starts_with('.') && !path.contains("..") && !path.ends_with('.'),
                "{path}"
            );
        }
    }

    /// The cross test that replaces the round trip: the deepest path names the
    /// same room as the label of the seat does. A composed numbering is the
    /// exception that states the rule: its first number is the division, so it
    /// sits on the segment ABOVE, and the two read together say « Chambre 1-2 ».
    #[test]
    fn the_last_segment_reads_back_as_the_room_of_the_label() {
        for (seat, _) in paths_table() {
            // Sitting together folds several paths, a renamed formation says
            // its current name, and a folded stream says its room.
            if seat.joined.is_some()
                || seat.solemn == Some(Solemn::ChambresReunies)
                || (seat.chamber.is_none()
                    && matches!(seat.jurisdiction_type.as_str(), "CA" | "TJ")
                    && seat.specialty.iter().any(|u| folded(u) != *u))
            {
                continue;
            }
            let paths = seat.paths();
            let last = paths.last().unwrap();
            if let Some(ChamberId::Composed(a, b)) = seat.chamber {
                let mut above = last.rsplit('.');
                assert_eq!(above.next(), Some(format!("C{b}").as_str()), "{seat:?}");
                assert_eq!(above.next(), Some(format!("P{a}").as_str()), "{seat:?}");
                continue;
            }
            assert_eq!(seat_path_label(last), room_of(&seat).label(), "{seat:?}");
        }
    }

    #[test]
    fn the_social_stream_labels_its_room_by_court() {
        assert_eq!(
            seat_path_label("FR.CA.ANGERS.SOCIALE").as_deref(),
            Some("Chambre sociale")
        );
        assert_eq!(
            seat_path_label("FR.TJ.MARSEILLE.SOCIALE").as_deref(),
            Some("Pôle social")
        );
        assert_eq!(
            seat_path_label("FR.CA.RENNES.C9_PROTECTION_SOCIALE").as_deref(),
            Some("9e chambre, protection sociale")
        );
    }

    #[test]
    fn the_commercial_chamber_keeps_one_node_under_its_two_names() {
        let named = |uid| Seat {
            specialty: vec![uid],
            ..court("CC", "cc")
        };
        let before = named("matiere:COMMERCIALE");
        let after = named("matiere:COMMERCIALE_FINANCIERE_ECONOMIQUE");
        assert_eq!(before.label().as_deref(), Some("Chambre commerciale"));
        assert_eq!(
            after.label().as_deref(),
            Some("Chambre commerciale, financière et économique")
        );
        assert_eq!(before.paths(), after.paths());
        assert_eq!(
            seat_path_label("FR.CC.COMMERCIALE").as_deref(),
            Some("Chambre commerciale, financière et économique")
        );
        assert_eq!(
            seat_path_label("FR.CA.ANGERS.COMMERCIALE").as_deref(),
            Some("Chambre commerciale")
        );
    }

    #[test]
    fn a_segment_labels_the_room_it_names() {
        let cases = [
            ("P5", "Pôle 5"),
            ("SC1", "1re section"),
            ("C1", "1re chambre"),
            ("C1_CIVILE", "1re chambre civile"),
            ("C1_CIVILE_COMMERCIALE", "1re chambre civile et commerciale"),
            ("C3_PROTECTION_SOCIALE", "3e chambre, protection sociale"),
            ("CD", "Chambre D"),
            ("SOCIALE", "Chambre sociale"),
            ("CIVI", "Commission d'indemnisation des victimes (CIVI)"),
            ("ASSEMBLEE", "Assemblée du contentieux"),
            ("ASSEMBLEE_PLENIERE", "Assemblée plénière"),
            ("TRIBUNAL", "Tribunal"),
            ("TFP", "Tribunal de la fonction publique"),
            ("GRANDE_CHAMBRE", "Grande chambre"),
        ];
        for (segment, expected) in cases {
            assert_eq!(
                seat_path_label(&format!("FR.CA.PARIS.{segment}")).as_deref(),
                Some(expected),
                "{segment}"
            );
        }
        for path in [
            "FR",
            "FR.CA",
            "FR.CC",
            "FR.CE",
            "FR.TJ",
            "FR.CA.PARIS",
            "FR.CA.AIX_EN_PROVENCE",
            "FR.CA.PARIS.",
            "FR.CA.PARIS.S3",
            "FR.CA.PARIS.LD",
        ] {
            assert!(seat_path_label(path).is_none(), "{path}");
        }
    }

    #[test]
    fn a_jurisdiction_translates_into_the_trunk_of_its_paths() {
        assert_eq!(
            jurisdiction_trunk("CA", Some("ca_paris")),
            ["FR", "CA", "PARIS"]
        );
        assert_eq!(
            jurisdiction_trunk("TJ", Some("tj_nice")),
            ["FR", "TJ", "NICE"]
        );
        assert_eq!(
            jurisdiction_trunk("ASSISES", Some("assises_seine_saint_denis")),
            ["FR", "ASSISES", "SEINE_SAINT_DENIS"]
        );
        assert_eq!(
            jurisdiction_trunk("CCD", Some("ccd_vaucluse")),
            ["FR", "CCD", "VAUCLUSE"]
        );
        assert_eq!(jurisdiction_trunk("CC", Some("cc")), ["FR", "CC"]);
        assert_eq!(jurisdiction_trunk("CE", None), ["FR", "CE"]);
        assert_eq!(jurisdiction_trunk("CRR", Some("crr")), ["FR", "CNDA"]);
        assert_eq!(jurisdiction_trunk("CJUE", None), ["EU", "CJUE"]);
        assert_eq!(jurisdiction_trunk("CEDH", None), ["INTL", "CEDH"]);
        assert_eq!(
            jurisdiction_trunk("COUR_SUPREME", Some("sn_cour_supreme")),
            ["SN", "COUR_SUPREME"]
        );
        assert_eq!(
            jurisdiction_trunk("COUR_APPEL", Some("sn_cour_appel_dakar")),
            ["SN", "COUR_APPEL", "DAKAR"]
        );
        assert_eq!(
            jurisdiction_trunk("CCJA", Some("ohada_ccja")),
            ["OHADA", "CCJA"]
        );
        assert!(jurisdiction_trunk("", Some("ca_paris")).is_empty());
        for (seat, chemins) in paths_table() {
            let trunk =
                jurisdiction_trunk(&seat.jurisdiction_type, seat.jurisdiction_code.as_deref());
            if let (Some(premier), Some(tete)) = (chemins.first(), trunk.first()) {
                assert!(premier.starts_with(tete), "{premier} / {tete}");
            }
        }
    }

    #[test]
    fn the_path_carries_a_matter_only_when_the_label_says_it() {
        for (seat, _) in paths_table() {
            let deepest = seat.paths().last().cloned().unwrap_or_default();
            let label = seat.label().unwrap_or_default();
            for uid in &seat.specialty {
                let said = specialty_agreement(uid).unwrap().text();
                if deepest.contains(suffix(uid)) {
                    assert!(label.contains(said), "{deepest} / {label}");
                }
            }
        }
        let composed = Seat {
            chamber: Some(ChamberId::Composed(1, 2)),
            specialty: vec!["matiere:CIVILE"],
            ..court("CA", "ca_aix_en_provence")
        };
        assert_eq!(
            composed.paths().last().map(String::as_str),
            Some("FR.CA.AIX_EN_PROVENCE.P1.C2")
        );
        assert_eq!(composed.label().as_deref(), Some("Chambre 1-2"));
    }

    #[test]
    fn the_renaming_reaches_the_path_and_not_the_seat_nor_its_label() {
        let reunies = Seat {
            solemn: Some(Solemn::ChambresReunies),
            ..court("CC", "cc")
        };
        assert_eq!(reunies.paths(), ["FR", "FR.CC", "FR.CC.ASSEMBLEE_PLENIERE"]);
        assert_eq!(reunies.label().as_deref(), Some("Chambres réunies"));
        let sub_sections = Seat {
            joined: Some(Joined::SubSections(vec![3, 5])),
            ..court("CE", "ce")
        };
        assert_eq!(
            sub_sections.paths(),
            ["FR", "FR.CE", "FR.CE.C3", "FR.CE.C5"]
        );
        assert_eq!(
            sub_sections.label().as_deref(),
            Some("Sous-sections 3/5 réunies")
        );
        let crr = court("CRR", "crr");
        assert_eq!(crr.paths(), ["FR", "FR.CNDA"]);
        assert_eq!(crr.jurisdiction_type, "CRR");
    }

    #[test]
    fn the_path_token_is_the_serde_form() {
        for (solemn, _) in SOLEMN {
            assert_eq!(
                serde_json::to_string(&solemn).unwrap(),
                format!("\"{}\"", solemn.token())
            );
            assert_eq!(Solemn::from_token(solemn.token()), Some(solemn));
        }
    }
}
