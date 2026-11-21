//! A row of A2AJ's Canadian case law dataset into a [`Decision`] for each language it holds, read
//! from the Decisia header its text opens on if any, keyed on what its two pages share.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use lj_core::decision::Decision;

use crate::scc_csc::{
    english, french, items, strings, DECISION_FLOOR, SCC_CODE, SCC_LABEL, SCC_PLACE, SCC_TYPE,
};

/// A court whose file the dataset holds, by the dataset's code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct A2ajCourt {
    pub dataset: &'static str,
    pub jurisdiction_type: &'static str,
    pub code: &'static str,
    pub label: &'static str,
    /// The court's English name, held in its `names`.
    pub english: &'static str,
    pub place: &'static str,
}

const fn court(
    dataset: &'static str,
    jurisdiction_type: &'static str,
    code: &'static str,
    label: &'static str,
    english: &'static str,
    place: &'static str,
) -> A2ajCourt {
    A2ajCourt {
        dataset,
        jurisdiction_type,
        code,
        label,
        english,
        place,
    }
}

const NOVA_SCOTIA: &str = "geo:CA-NS";
const NEWFOUNDLAND: &str = "geo:CA-NL";
const ONTARIO: &str = "geo:CA-ON";
const BRITISH_COLUMBIA: &str = "geo:CA-BC";
const YUKON: &str = "geo:CA-YT";

const fn ontario(
    dataset: &'static str,
    code: &'static str,
    label: &'static str,
    english: &'static str,
) -> A2ajCourt {
    court(
        dataset,
        "TRIBUNAL_ADMINISTRATIF",
        code,
        label,
        english,
        ONTARIO,
    )
}

/// The courts read.
pub const A2AJ_COURTS: [A2ajCourt; 49] = [
    court(
        "SCC",
        SCC_TYPE,
        SCC_CODE,
        SCC_LABEL,
        "Supreme Court of Canada",
        SCC_PLACE,
    ),
    court(
        "FCA",
        "COUR_APPEL",
        "ca_cour_appel_federale",
        "Cour d'appel fédérale (Canada)",
        "Federal Court of Appeal (Canada)",
        SCC_PLACE,
    ),
    court(
        "FC",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ca_tribunal_premiere_instance_federale",
        "Cour fédérale (Canada)",
        "Federal Court (Canada)",
        SCC_PLACE,
    ),
    court(
        "TCC",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ca_tribunal_premiere_instance_impot",
        "Cour canadienne de l'impôt",
        "Tax Court of Canada",
        SCC_PLACE,
    ),
    court(
        "CMAC",
        "COUR_APPEL",
        "ca_cour_appel_cour_martiale",
        "Cour d'appel de la cour martiale du Canada",
        "Court Martial Appeal Court of Canada",
        SCC_PLACE,
    ),
    court(
        "CART",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_revision_agricole",
        "Commission de révision agricole du Canada",
        "Canada Agricultural Review Tribunal",
        SCC_PLACE,
    ),
    court(
        "CHRT",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_droits_personne",
        "Tribunal canadien des droits de la personne",
        "Canadian Human Rights Tribunal",
        SCC_PLACE,
    ),
    court(
        "CIRB",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_relations_industrielles",
        "Conseil canadien des relations industrielles",
        "Canada Industrial Relations Board",
        SCC_PLACE,
    ),
    court(
        "CITT",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_commerce_exterieur",
        "Tribunal canadien du commerce extérieur",
        "Canadian International Trade Tribunal",
        SCC_PLACE,
    ),
    court(
        "CT",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_concurrence",
        "Tribunal de la concurrence (Canada)",
        "Competition Tribunal (Canada)",
        SCC_PLACE,
    ),
    court(
        "FPSLREB",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_relations_travail_secteur_public",
        "Commission des relations de travail et de l'emploi dans le secteur public fédéral (Canada)",
        "Federal Public Sector Labour Relations and Employment Board (Canada)",
        SCC_PLACE,
    ),
    court(
        "OHSTC",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_sante_securite_travail",
        "Tribunal de santé et sécurité au travail Canada",
        "Occupational Health and Safety Tribunal Canada",
        SCC_PLACE,
    ),
    court(
        "OIC",
        "AUTORITE_ADMINISTRATIVE",
        "ca_autorite_administrative_commissaire_information",
        "Commissaire à l'information du Canada",
        "Information Commissioner of Canada",
        SCC_PLACE,
    ),
    court(
        "PSDPT",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_protection_fonctionnaires_divulgateurs",
        "Tribunal de la protection des fonctionnaires divulgateurs (Canada)",
        "Public Servants Disclosure Protection Tribunal (Canada)",
        SCC_PLACE,
    ),
    court(
        "RAD",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_appel_refugies",
        "Section d'appel des réfugiés (CISR, Canada)",
        "Refugee Appeal Division (IRB, Canada)",
        SCC_PLACE,
    ),
    court(
        "RPD",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_protection_refugies",
        "Section de la protection des réfugiés (CISR, Canada)",
        "Refugee Protection Division (IRB, Canada)",
        SCC_PLACE,
    ),
    court(
        "SCT",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_revendications_particulieres",
        "Tribunal des revendications particulières (Canada)",
        "Specific Claims Tribunal (Canada)",
        SCC_PLACE,
    ),
    court(
        "SST",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_securite_sociale",
        "Tribunal de la sécurité sociale du Canada",
        "Social Security Tribunal of Canada",
        SCC_PLACE,
    ),
    court(
        "TATC",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_appel_transports",
        "Tribunal d'appel des transports du Canada",
        "Transportation Appeal Tribunal of Canada",
        SCC_PLACE,
    ),
    court(
        "TMOB",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_oppositions_marques",
        "Commission des oppositions des marques de commerce (Canada)",
        "Trademarks Opposition Board (Canada)",
        SCC_PLACE,
    ),
    court(
        "NSCA",
        "COUR_APPEL",
        "ca_cour_appel_nouvelle_ecosse",
        "Cour d'appel (Nouvelle-Écosse, Canada)",
        "Nova Scotia Court of Appeal",
        NOVA_SCOTIA,
    ),
    court(
        "NSSC",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ca_tribunal_premiere_instance_nouvelle_ecosse",
        "Cour suprême (Nouvelle-Écosse, Canada)",
        "Supreme Court of Nova Scotia",
        NOVA_SCOTIA,
    ),
    court(
        "NSPC",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ca_tribunal_premiere_instance_nouvelle_ecosse_provinciale",
        "Cour provinciale (Nouvelle-Écosse, Canada)",
        "Provincial Court of Nova Scotia",
        NOVA_SCOTIA,
    ),
    court(
        "NSFC",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ca_tribunal_premiere_instance_nouvelle_ecosse_famille",
        "Cour de la famille (Nouvelle-Écosse, Canada)",
        "Family Court of Nova Scotia",
        NOVA_SCOTIA,
    ),
    court(
        "NSSM",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ca_tribunal_premiere_instance_nouvelle_ecosse_petites_creances",
        "Cour des petites créances (Nouvelle-Écosse, Canada)",
        "Small Claims Court of Nova Scotia",
        NOVA_SCOTIA,
    ),
    court(
        "NLCA",
        "COUR_APPEL",
        "ca_cour_appel_terre_neuve_et_labrador",
        "Cour d'appel (Terre-Neuve-et-Labrador, Canada)",
        "Court of Appeal of Newfoundland and Labrador",
        NEWFOUNDLAND,
    ),
    court(
        "ONCA",
        "COUR_APPEL",
        "ca_cour_appel_ontario",
        "Cour d'appel (Ontario, Canada)",
        "Court of Appeal for Ontario",
        ONTARIO,
    ),
    court(
        "BCCA",
        "COUR_APPEL",
        "ca_cour_appel_colombie_britannique",
        "Cour d'appel (Colombie-Britannique, Canada)",
        "Court of Appeal for British Columbia",
        BRITISH_COLUMBIA,
    ),
    court(
        "BCSC",
        "TRIBUNAL_PREMIERE_INSTANCE",
        "ca_tribunal_premiere_instance_colombie_britannique",
        "Cour suprême (Colombie-Britannique, Canada)",
        "Supreme Court of British Columbia",
        BRITISH_COLUMBIA,
    ),
    court(
        "YKCA",
        "COUR_APPEL",
        "ca_cour_appel_yukon",
        "Cour d'appel (Yukon, Canada)",
        "Court of Appeal of Yukon",
        YUKON,
    ),
    court(
        "RLLR",
        "TRIBUNAL_ADMINISTRATIF",
        "ca_tribunal_administratif_protection_refugies",
        "Section de la protection des réfugiés (CISR, Canada)",
        "Refugee Protection Division (IRB, Canada)",
        SCC_PLACE,
    ),
    ontario(
        "ONACRB",
        "ca_tribunal_administratif_ontario_animaux",
        "Commission d'étude des soins aux animaux (Ontario, Canada)",
        "Animal Care Review Board (Ontario)",
    ),
    ontario(
        "ONARB",
        "ca_tribunal_administratif_ontario_evaluation_fonciere",
        "Commission de révision de l'évaluation foncière (Ontario, Canada)",
        "Assessment Review Board (Ontario)",
    ),
    ontario(
        "ONASLPDT",
        "ca_tribunal_administratif_ontario_discipline_audiologistes",
        "Tribunal de discipline des audiologistes et des orthophonistes de l'Ontario (Canada)",
        "Ontario Audiologists and Speech-Language Pathologists Discipline Tribunal",
    ),
    ontario(
        "ONCFSRB",
        "ca_tribunal_administratif_ontario_services_enfance_famille",
        "Commission de révision des services à l'enfance et à la famille (Ontario, Canada)",
        "Child and Family Services Review Board (Ontario)",
    ),
    ontario(
        "ONCPDT",
        "ca_tribunal_administratif_ontario_discipline_podologues",
        "Tribunal de discipline des podologues et des podiatres de l'Ontario (Canada)",
        "Ontario Chiropodists and Podiatrists Discipline Tribunal",
    ),
    ontario(
        "ONCRB",
        "ca_tribunal_administratif_ontario_placements_sous_garde",
        "Commission de révision des placements sous garde (Ontario, Canada)",
        "Custody Review Board (Ontario)",
    ),
    ontario(
        "ONDDT",
        "ca_tribunal_administratif_ontario_discipline_dietetistes",
        "Tribunal de discipline des diététistes de l'Ontario (Canada)",
        "Ontario Dietitians Discipline Tribunal",
    ),
    ontario(
        "ONFSC",
        "ca_tribunal_administratif_ontario_securite_incendie",
        "Commission de la sécurité-incendie (Ontario, Canada)",
        "Fire Safety Commission (Ontario)",
    ),
    ontario(
        "ONHRTO",
        "ca_tribunal_administratif_ontario_droits_personne",
        "Tribunal des droits de la personne de l'Ontario (Canada)",
        "Human Rights Tribunal of Ontario",
    ),
    ontario(
        "ONLAT",
        "ca_tribunal_administratif_ontario_appel_permis",
        "Tribunal d'appel en matière de permis (Ontario, Canada)",
        "Licence Appeal Tribunal (Ontario)",
    ),
    ontario(
        "ONLSTA",
        "ca_tribunal_administratif_ontario_barreau_appel",
        "Tribunal du Barreau, section d'appel (Ontario, Canada)",
        "Law Society Tribunal, Appeal Division (Ontario)",
    ),
    ontario(
        "ONLSTH",
        "ca_tribunal_administratif_ontario_barreau",
        "Tribunal du Barreau, section de première instance (Ontario, Canada)",
        "Law Society Tribunal, Hearing Division (Ontario)",
    ),
    ontario(
        "ONLTB",
        "ca_tribunal_administratif_ontario_location_immobiliere",
        "Commission de la location immobilière (Ontario, Canada)",
        "Landlord and Tenant Board (Ontario)",
    ),
    ontario(
        "ONMRITDT",
        "ca_tribunal_administratif_ontario_discipline_technologues_radiation",
        "Tribunal de discipline des technologues en radiation médicale et en imagerie de l'Ontario (Canada)",
        "Ontario Medical Radiation and Imaging Technologists Discipline Tribunal",
    ),
    ontario(
        "ONPDT",
        "ca_tribunal_administratif_ontario_discipline_physiotherapeutes",
        "Tribunal de discipline des physiothérapeutes de l'Ontario (Canada)",
        "Ontario Physiotherapists Discipline Tribunal",
    ),
    ontario(
        "ONPSDT",
        "ca_tribunal_administratif_ontario_discipline_medecins",
        "Tribunal de discipline des médecins de l'Ontario (Canada)",
        "Ontario Physicians and Surgeons Discipline Tribunal",
    ),
    ontario(
        "ONRPDT",
        "ca_tribunal_administratif_ontario_discipline_psychotherapeutes",
        "Tribunal de discipline des psychothérapeutes autorisés de l'Ontario (Canada)",
        "Ontario Registered Psychotherapists Discipline Tribunal",
    ),
    ontario(
        "ONSBT",
        "ca_tribunal_administratif_ontario_aide_sociale",
        "Tribunal de l'aide sociale (Ontario, Canada)",
        "Social Benefits Tribunal (Ontario)",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A2ajLanguage {
    French,
    English,
}

/// The labels of a Decisia header in one language, as the collections read write them.
struct Labels {
    end: &'static str,
    date: &'static [&'static str],
    neutral: &'static [&'static str],
    report: &'static [&'static str],
    dockets: &'static [&'static str],
    judges: &'static [&'static str],
    appeal_from: &'static [&'static str],
    subjects: &'static [&'static str],
    /// The labels whose rows are not kept, which end the row before them.
    others: &'static [&'static str],
}

const FRENCH: Labels = Labels {
    end: "Contenu de la décision",
    date: &["Date", "Date de décision", "Date de la décision"],
    neutral: &["Référence neutre", "Référence"],
    report: &["Recueil", "Référence de recueil", "Autres références"],
    dockets: &[
        "Numéro de dossier",
        "Numéros de dossier",
        "Numéro(s) de dossier",
        "Numéro(s) du cas",
        "N° Dossier TATC",
        "Numéro de dossier de la revendication",
        "Numéro de référence",
    ],
    judges: &[
        "Juges",
        "Juges et Officiers taxateurs",
        "Décideurs",
        "Décideur(s)",
        "Membre",
        "Membre/Agent d'audience",
        "Membre du Tribunal",
        "Devant",
    ],
    appeal_from: &["En appel de"],
    subjects: &["Sujets", "Termes analytiques"],
    others: &[
        "Collection",
        "Base de données \u{2013} Cour (s)",
        "Date de publication",
        "Date de dépôt",
        "Décision rapportée",
        "Numéro d'instance",
        "Numéro de demande / Numéro d'enregistrement",
        "Notes",
        "Voir aussi",
        "Fiche analytique",
        "Résumé :",
        "Type de document",
        "Type de la décision",
        "Type de décision",
        "Statut de la décision",
        "Type de cas",
        "Type d'affaire",
        "Motifs de discrimination",
        "Relations",
        "Décisions connexes",
        "Division",
        "Langue d'origine",
        "Mots-clés",
        "Phase",
        "Province/ Territoire",
        "Article(s) de la Loi sur la concurrence",
        "Marque de commerce",
        "Documentation",
    ],
};

const ENGLISH: Labels = Labels {
    end: "Decision Content",
    date: &["Date", "Decision date", "Decision Rendered"],
    neutral: &["Neutral citation", "Citation"],
    report: &["Report", "Report citation", "Other citations"],
    dockets: &[
        "Case number",
        "Case Number(s)",
        "File numbers",
        "File number(s)",
        "File Number",
        "TATC File Number",
        "Claim file number",
        "Docket",
        "Reference number",
    ],
    judges: &[
        "Judges",
        "Judges and Taxing Officers",
        "Decision-makers",
        "Decision-maker(s)",
        "Judge/Registrar/Adjudicator",
        "Member",
        "Member/Hearing officer",
        "Tribunal Member",
        "Before",
    ],
    appeal_from: &["On appeal from"],
    subjects: &["Subjects", "Subject Terms"],
    others: &[
        "Collection",
        "Court (s) Database",
        "Publication Date",
        "Date Filed",
        "Reported Decision",
        "Proceeding Number",
        "Application number/Registration number",
        "Notes",
        "See also",
        "Digest",
        "Summary:",
        "Court",
        "Document Type",
        "Decision type",
        "Decision Type",
        "Decision status",
        "Case Type",
        "Type of Application",
        "Hearing Type",
        "Grounds",
        "Relations",
        "Related decisions",
        "Division",
        "Original Language",
        "Keywords",
        "Phase",
        "Province/ Territory",
        "Section(s) of the Competition Act",
        "Trademark",
        "Case Documents",
    ],
};

impl A2ajCourt {
    /// The IRB's divisions: the citation column is the file number, and their text has no header.
    fn docket_cited(&self) -> bool {
        matches!(self.dataset, "RAD" | "RPD")
    }
}

impl A2ajLanguage {
    /// The code a row's columns and a provenance's uid end on.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::French => "fr",
            Self::English => "en",
        }
    }

    fn iso(self) -> &'static str {
        match self {
            Self::French => "fra",
            Self::English => "eng",
        }
    }

    fn labels(self) -> &'static Labels {
        match self {
            Self::French => &FRENCH,
            Self::English => &ENGLISH,
        }
    }

    fn other(self) -> Self {
        match self {
            Self::French => Self::English,
            Self::English => Self::French,
        }
    }

    /// Whether `text` reads the other language.
    fn foreign(self, text: &str) -> bool {
        match self {
            Self::French => english(text),
            Self::English => french(text),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A2ajRefused {
    CourtNotRead(String),
    /// Neither the URL nor the neutral citation gives the number both pages share.
    NoItem,
    NoDate,
    NoText,
    /// The text reads the other language, which the row holds a text in.
    OtherLanguage,
}

#[derive(Debug, Clone, PartialEq)]
pub struct A2ajDecision {
    pub court: &'static A2ajCourt,
    pub decision: Decision,
    pub source_fields: Value,
    /// The style of cause.
    pub title: String,
}

static ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/item/(\d+)/|[?&]decision-id=(\d+)").unwrap());

static NEUTRAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{4}) \S+ (\d+)$").unwrap());

/// The Landlord and Tenant Board's document id, `069N300000zKt5MIAS`.
static BINARY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/exportedBinaries[^/]*/([0-9A-Za-z]{15,18})/").unwrap());

/// An Ontario tribunal's file name without its language, `2026-ONLAT-25-011265-AABS`.
static FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://decisions\.a2aj\.ca/[^/]+/(.+?)-(?:EN|FR)\.(?:docx|pdf)$").unwrap()
});

/// The key a row's two pages share: the IRB's file number, the Decisia item or decision id, the
/// LTB's document id, the neutral citation's year and number (`2023-27`), else the file name.
fn item(court: &A2ajCourt, url: &str, first_url: &str, citation: Option<&str>) -> Option<String> {
    if court.docket_cited() {
        return citation.map(str::to_string);
    }
    if let Some(c) = ITEM.captures(url) {
        return c
            .get(1)
            .or_else(|| c.get(2))
            .map(|m| m.as_str().to_string());
    }
    if let Some(c) = BINARY.captures(first_url) {
        return Some(c[1].to_string());
    }
    citation
        .and_then(|c| NEUTRAL.captures(c))
        .map(|c| format!("{}-{}", &c[1], &c[2]))
        .or_else(|| FILE.captures(first_url).map(|c| c[1].to_string()))
}

/// `a2aj/SCC/20353/en`.
pub fn a2aj_source_uid(dataset: &str, item: &str, language: A2ajLanguage) -> String {
    format!("a2aj/{dataset}/{item}/{}", language.suffix())
}

pub fn a2aj_court(dataset: &str) -> Option<&'static A2ajCourt> {
    A2AJ_COURTS.iter().find(|c| c.dataset == dataset)
}

/// The English name of each court, for its `names`.
pub fn a2aj_names() -> Vec<(&'static str, Value)> {
    A2AJ_COURTS
        .iter()
        .map(|c| (c.code, json!({ "en": { "name": c.english } })))
        .collect()
}

/// The header's rows by label, each its lines, and the text after it; the whole text, without
/// rows, when it holds no header.
fn header(text: &str, labels: &Labels) -> (Vec<(String, Vec<String>)>, String) {
    let lines: Vec<&str> = text.lines().collect();
    let Some(end) = lines.iter().position(|l| l.trim() == labels.end) else {
        return (Vec::new(), text.trim().to_string());
    };
    let known = |l: &str| {
        [
            labels.date,
            labels.neutral,
            labels.report,
            labels.dockets,
            labels.judges,
            labels.appeal_from,
            labels.subjects,
            labels.others,
        ]
        .iter()
        .any(|set| set.contains(&l))
    };
    let mut rows: Vec<(String, Vec<String>)> = Vec::new();
    for line in lines[1..end].iter().map(|l| l.trim()) {
        if known(line) {
            rows.push((line.to_string(), Vec::new()));
        } else if let Some((_, values)) = rows.last_mut() {
            if !line.is_empty() {
                values.push(line.to_string());
            }
        }
    }
    (rows, lines[end + 1..].join("\n").trim().to_string())
}

fn decision_of(
    court: &A2ajCourt,
    item: &str,
    language: A2ajLanguage,
    date: String,
    dockets: Vec<String>,
    text: String,
) -> Decision {
    Decision {
        source_uid: a2aj_source_uid(court.dataset, item, language),
        member_name: item.to_string(),
        ecli: None,
        jurisdiction_source_code: Some(court.code.to_string()),
        chamber: None,
        nac: None,
        jurisdiction_name: Some(court.label.to_string()),
        jurisdiction_type: Some(court.jurisdiction_type.to_string()),
        jurisdiction_location: None,
        numero_dossier: dockets.first().cloned(),
        numero_dossiers: (!dockets.is_empty()).then_some(dockets),
        numero_role: None,
        date_lecture: Some(date),
        date_audience: None,
        date_mise_jour: None,
        formation: None,
        type_decision: None,
        type_recours: None,
        solution: None,
        publication_codes: Vec::new(),
        avocat_requerant: None,
        listed_counsel: Vec::new(),
        themes: Vec::new(),
        attacked: None,
        texte_integral_raw: text.clone(),
        texte_integral_clean: text,
        sections: Vec::new(),
        metadata_header: String::new(),
        parse_warnings: Vec::new(),
    }
}

/// The decision a row holds in `language`, `None` when the row holds no text in it.
pub fn parse_a2aj(
    row: &Map<String, Value>,
    language: A2ajLanguage,
) -> Result<Option<A2ajDecision>, A2ajRefused> {
    let column = |name: &str| {
        row.get(&format!("{name}_{}", language.suffix()))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let Some(whole) = column("unofficial_text") else {
        return Ok(None);
    };
    let dataset = row.get("dataset").and_then(Value::as_str).unwrap_or("");
    let court = a2aj_court(dataset).ok_or_else(|| A2ajRefused::CourtNotRead(dataset.into()))?;
    let url = column("url").ok_or(A2ajRefused::NoItem)?;
    let first = |columns: [&str; 2]| {
        columns
            .iter()
            .filter_map(|c| row.get(*c).and_then(Value::as_str).map(str::trim))
            .find(|c| !c.is_empty())
    };
    let citation = first(["citation_en", "citation_fr"]);
    let first_url = first(["url_en", "url_fr"]).unwrap_or(url);
    let item = item(court, url, first_url, citation).ok_or(A2ajRefused::NoItem)?;
    let labels = language.labels();
    let (rows, text) = header(whole, labels);
    let field = |set: &[&str]| -> Option<String> {
        rows.iter()
            .find(|(label, _)| set.contains(&label.as_str()))
            .map(|(_, values)| values.join("\n"))
            .filter(|v| !v.is_empty())
    };
    let date = field(labels.date)
        .into_iter()
        .chain(column("document_date").map(|d| d.chars().take(10).collect()))
        .find(|d| d.parse::<jiff::civil::Date>().is_ok())
        .ok_or(A2ajRefused::NoDate)?;
    if text.chars().filter(|c| !c.is_whitespace()).count() < DECISION_FLOOR {
        return Err(A2ajRefused::NoText);
    }
    let page = language;
    let language = match page.foreign(&text) {
        false => page,
        true if row
            .get(&format!("unofficial_text_{}", page.other().suffix()))
            .and_then(Value::as_str)
            .is_some_and(|t| !t.trim().is_empty()) =>
        {
            return Err(A2ajRefused::OtherLanguage)
        }
        true => page.other(),
    };
    let mut dockets = items(field(labels.dockets).as_deref(), &[',', ';']);
    if dockets.is_empty() && court.docket_cited() {
        dockets.extend(citation.map(str::to_string));
    }
    let neutral = field(labels.neutral).or_else(|| {
        column("citation")
            .filter(|c| NEUTRAL.is_match(c))
            .map(str::to_string)
    });
    let title = column("name").unwrap_or_default().to_string();
    let source_fields = json!({
        "a2aj_dataset": court.dataset,
        "item": item,
        "title": title,
        "date": date,
        "neutral_citation": neutral,
        "report_citation": field(labels.report),
        "dockets": dockets,
        "judges": items(field(labels.judges).as_deref(), &[';']),
        "appeal_from": field(labels.appeal_from),
        "subjects": items(field(labels.subjects).as_deref(), &[]),
        "source_url": url,
        "lang": language.iso(),
    });
    Ok(Some(A2ajDecision {
        court,
        decision: decision_of(court, &item, language, date, dockets, text),
        source_fields,
        title,
    }))
}

/// Branch of [`Decision::from_source_fields`] for `a2aj/`, on what the parse stored.
pub(crate) fn from_source_fields_a2aj(
    full_text: &str,
    source_fields: &Value,
    source_uid: &str,
) -> Decision {
    let court = source_fields
        .get("a2aj_dataset")
        .and_then(Value::as_str)
        .and_then(a2aj_court)
        .unwrap_or_else(|| panic!("source_fields a2aj without a court read ({source_uid})"));
    let item = source_fields
        .get("item")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("source_fields a2aj without an item ({source_uid})"));
    let date = source_fields
        .get("date")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("source_fields a2aj without a date ({source_uid})"));
    let language = match source_uid.rsplit('/').next() {
        Some("fr") => A2ajLanguage::French,
        Some("en") => A2ajLanguage::English,
        _ => panic!("an a2aj uid ends on its language ({source_uid})"),
    };
    decision_of(
        court,
        item,
        language,
        date.to_string(),
        strings(source_fields, "dockets"),
        full_text.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FromSourceFields;

    const REASONS_FR: &str =
        "[1] Au terme d'un procès en cour martiale générale, l'appelant a été \
        reconnu coupable de trafic de stupéfiants. Le juge militaire lui a imposé une peine \
        d'emprisonnement de cinq mois, et l'appelant soutient que la peine est trop sévère pour \
        les faits qui lui sont reprochés dans cette affaire.";
    const REASONS_EN: &str = "[1] At the end of a trial by general court martial, the appellant \
        was convicted of trafficking. The military judge sentenced him to five months of \
        imprisonment, and the appellant submits that the sentence is too harsh for the facts \
        with which he was charged in this case.";

    fn row(fr: &str, en: &str) -> Map<String, Value> {
        let value = json!({
            "dataset": "CMAC",
            "name_fr": "R. c. Lee",
            "name_en": "R. v. Lee",
            "url_fr": "https://decisions.cmac-cacm.ca/cmac-cacm/cmac-cacm/fr/item/490093/index.do",
            "url_en": "https://decisions.cmac-cacm.ca/cmac-cacm/cmac-cacm/en/item/490093/index.do",
            "document_date_fr": "2010-04-22T00:00:00Z",
            "document_date_en": "2010-04-22T00:00:00Z",
            "unofficial_text_fr": fr,
            "unofficial_text_en": en,
        });
        value.as_object().unwrap().clone()
    }

    #[test]
    fn each_language_of_a_row_is_a_decision_read_from_its_header() {
        let fr = format!(
            "R. c. Lee\nCollection\nCour d'appel de la cour martiale\nDate\n2010-04-22\n\
             Référence neutre\n2010 CACM 5\nRéférence de recueil\n7 CACM 463\n\
             Numéros de dossier\nCMAC-523\nDécideurs\nO'Reilly, James W.; Mactavish, Anne L.\n\
             Contenu de la décision\nDate : 20100422\n{REASONS_FR}"
        );
        let en = format!(
            "R. v. Lee\nCollection\nCourt Martial Appeal Court\nDate\n2010-04-22\n\
             Neutral citation\n2010 CMAC 5\nFile numbers\nCMAC-523, CMAC-524\n\
             Decision Content\n{REASONS_EN}"
        );
        let row = row(&fr, &en);
        let french = parse_a2aj(&row, A2ajLanguage::French).unwrap().unwrap();
        assert_eq!(french.decision.source_uid, "a2aj/CMAC/490093/fr");
        assert_eq!(french.court.code, "ca_cour_appel_cour_martiale");
        assert_eq!(french.decision.date_lecture.as_deref(), Some("2010-04-22"));
        assert!(french
            .decision
            .texte_integral_clean
            .starts_with("Date : 20100422\n[1]"));
        let sf = &french.source_fields;
        assert_eq!(sf["neutral_citation"], "2010 CACM 5");
        assert_eq!(sf["report_citation"], "7 CACM 463");
        assert_eq!(sf["dockets"], json!(["CMAC-523"]));
        assert_eq!(
            sf["judges"],
            json!(["O'Reilly, James W.", "Mactavish, Anne L."])
        );
        assert_eq!(sf["lang"], "fra");
        let english = parse_a2aj(&row, A2ajLanguage::English).unwrap().unwrap();
        assert_eq!(english.decision.source_uid, "a2aj/CMAC/490093/en");
        assert_eq!(english.title, "R. v. Lee");
        assert_eq!(
            english.source_fields["dockets"],
            json!(["CMAC-523", "CMAC-524"])
        );
        assert_eq!(english.source_fields["report_citation"], Value::Null);
        let rebuilt = Decision::from_source_fields(
            &english.decision.texte_integral_clean,
            &english.source_fields,
            &english.decision.source_uid,
        );
        assert_eq!(rebuilt, english.decision);
    }

    #[test]
    fn a_text_in_the_other_language_is_that_language_unless_the_row_holds_one() {
        let alone = row("", REASONS_FR);
        assert_eq!(parse_a2aj(&alone, A2ajLanguage::French), Ok(None));
        let french = parse_a2aj(&alone, A2ajLanguage::English).unwrap().unwrap();
        assert_eq!(french.decision.source_uid, "a2aj/CMAC/490093/fr");
        assert_eq!(french.source_fields["lang"], "fra");
        assert_eq!(french.title, "R. v. Lee");
        let doubled = row(REASONS_FR, REASONS_FR);
        assert_eq!(
            parse_a2aj(&doubled, A2ajLanguage::English),
            Err(A2ajRefused::OtherLanguage)
        );
    }

    #[test]
    fn an_irb_row_is_keyed_on_its_file_number_and_a_neutral_citation_is_read_from_its_column() {
        let irb = json!({
            "dataset": "RAD",
            "citation_en": "MB5-02459",
            "url_en": "2041116.txt",
            "url_fr": "2041115.txt",
            "document_date_en": "2016-02-12T00:00:00Z",
            "unofficial_text_en": format!("RAD File No. / No de dossier de la SAR : MB5-02459\n{REASONS_EN}"),
        });
        let english = parse_a2aj(irb.as_object().unwrap(), A2ajLanguage::English)
            .unwrap()
            .unwrap();
        assert_eq!(english.decision.source_uid, "a2aj/RAD/MB5-02459/en");
        assert_eq!(english.source_fields["dockets"], json!(["MB5-02459"]));
        assert_eq!(english.source_fields["neutral_citation"], Value::Null);
        let oic = json!({
            "dataset": "OIC",
            "citation_en": "2021 OIC 23",
            "url_en": "https://www.oic-ci.gc.ca/en/decisions/decision-pursuant-61-2021-oic-23",
            "document_date_en": "2021-08-04T00:00:00Z",
            "unofficial_text_en": REASONS_EN,
        });
        let english = parse_a2aj(oic.as_object().unwrap(), A2ajLanguage::English)
            .unwrap()
            .unwrap();
        assert_eq!(english.decision.source_uid, "a2aj/OIC/2021-23/en");
        assert_eq!(english.source_fields["neutral_citation"], "2021 OIC 23");
        assert_eq!(english.source_fields["dockets"], json!([]));
    }

    #[test]
    fn an_ontario_tribunal_s_row_is_keyed_on_its_document_its_neutral_citation_or_its_file() {
        let key = |value: Value, language| {
            parse_a2aj(value.as_object().unwrap(), language)
                .unwrap()
                .unwrap()
                .decision
                .source_uid
        };
        let ltb = |language| {
            key(
                json!({
                    "dataset": "ONLTB",
                    "citation_en": "2026 ONLTB 46364",
                    "url_en": "https://d3hf2qahezjxpx.cloudfront.net/attachments/exportedBinaries20260825_200359/069N3000011kZ6aIAE/LTB-L-027888-26-HR.pdf",
                    "url_fr": "https://d3hf2qahezjxpx.cloudfront.net/attachments/exportedBinaries20260825_200359/069N3000011w2GlIAI",
                    "document_date_en": "2026-08-20T00:00:00Z",
                    "document_date_fr": "2026-08-20T00:00:00Z",
                    "unofficial_text_en": REASONS_EN,
                    "unofficial_text_fr": REASONS_FR,
                }),
                language,
            )
        };
        assert_eq!(
            ltb(A2ajLanguage::English),
            "a2aj/ONLTB/069N3000011kZ6aIAE/en"
        );
        assert_eq!(
            ltb(A2ajLanguage::French),
            "a2aj/ONLTB/069N3000011kZ6aIAE/fr"
        );
        let hrto = json!({
            "dataset": "ONHRTO",
            "citation_fr": "2026 TDPO 1450",
            "url_fr": "https://decisions.a2aj.ca/ONHRTO/2026-TDPO-1450-FR.docx",
            "document_date_fr": "2026-07-02T00:00:00Z",
            "unofficial_text_fr": REASONS_FR,
        });
        assert_eq!(key(hrto, A2ajLanguage::French), "a2aj/ONHRTO/2026-1450/fr");
        let lat = json!({
            "dataset": "ONLAT",
            "citation_en": "2026 ONLAT 25-011265/AABS",
            "url_en": "https://decisions.a2aj.ca/ONLAT/2026-ONLAT-25-011265-AABS-EN.docx",
            "document_date_en": "2026-05-11T00:00:00Z",
            "unofficial_text_en": REASONS_EN,
        });
        assert_eq!(
            key(lat, A2ajLanguage::English),
            "a2aj/ONLAT/2026-ONLAT-25-011265-AABS/en"
        );
    }
}
