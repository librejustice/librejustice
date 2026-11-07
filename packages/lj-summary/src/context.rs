//! What the model reads besides the decision's text: its title, the name it is known
//! by, and how the court or the registry indexed it.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::{analysis_titrages, build_summary_input, names_by_code};

/// One provenance of a decision: its source and the fields it carries.
pub struct Provenance<'a> {
    pub source: &'a str,
    pub fields: &'a Value,
}

/// PCJA codes heading a JADE titrage: `19-04-02-03-02 `, `39-05-01-015, 65-02 `.
static PCJA_CODES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*\d{2}(?:-\d{2,3})*(?:\s*,\s*\d{2}(?:-\d{2,3})*)*\s+").unwrap()
});
/// Rank heading a DILA titrage of a decision indexed under several: `1° `.
static RANK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\d+\s*°\s*").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static TRAILING_ACRONYM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\(([A-Z]{2,6})\)$").unwrap());
static LEADING_ACRONYM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([A-Z]{2,6})\)").unwrap());

/// The provenances [`classification`] reads, and the fields it reads of them.
pub const CONTEXT_SOURCES: [&str; 7] = [
    "judilibre",
    "dila-cass",
    "dila-inca",
    "dila-capp",
    "dila-jade",
    "ariane-web",
    "cedh",
];
pub const CONTEXT_FIELDS: [&str; 7] = [
    "titlesAndSummaries",
    "themes",
    "SOMMAIRE",
    "SCT",
    "commentaires",
    "article",
    "conclusion",
];

const LINE_MAX: usize = 400;
/// Past 60 characters a name pushes the opening beyond 160: over 40 long CJUE and
/// Strasbourg names, 7 openings refused with the names whole, 2 once cut.
pub(crate) const NAME_MAX: usize = 60;
pub(crate) const PARTIES: [&str; 3] = [" contre ", " c. ", " v. "];
const PARTY_ENDS: [&str; 7] = [", ", " et ", " ET ", " and ", " e.a.", " (", " - "];
const MORE_PARTIES: [&str; 4] = [" et ", " ET ", " and ", " e.a."];
/// The short names the Court and the doctrine cite the commonest parties by.
const SHORT_NAMES: [(&str, &str); 20] = [
    ("Commission des Communautés européennes", "Commission"),
    ("Commission européenne", "Commission"),
    ("Conseil de l'Union européenne", "Conseil"),
    ("Conseil des Communautés européennes", "Conseil"),
    ("Parlement européen", "Parlement"),
    (
        "Office de l'Union européenne pour la propriété intellectuelle",
        "EUIPO",
    ),
    (
        "Office de l'harmonisation dans le marché intérieur (marques, dessins et modèles)",
        "OHMI",
    ),
    ("République italienne", "Italie"),
    ("République française", "France"),
    ("République fédérale d'Allemagne", "Allemagne"),
    ("Royaume de Belgique", "Belgique"),
    ("République hellénique", "Grèce"),
    ("Royaume d'Espagne", "Espagne"),
    ("Grand-Duché de Luxembourg", "Luxembourg"),
    (
        "Royaume-Uni de Grande-Bretagne et d'Irlande du Nord",
        "Royaume-Uni",
    ),
    ("Royaume des Pays-Bas", "Pays-Bas"),
    ("République portugaise", "Portugal"),
    ("République d'Autriche", "Autriche"),
    ("Royaume de Danemark", "Danemark"),
    ("République de Pologne", "Pologne"),
];
/// A Conseil d'Etat analysis can file one decision under ten rubrics.
const TITRAGES_MAX: usize = 6;

const TITRAGE: &str = "[Titrage]";
const NATURE: &str = "[Nature de l'affaire]";
const ARTICLES: &str = "[Articles de la Convention]";
const CONCLUSION: &str = "[Conclusion]";

/// The labels of the header lines, which a model sometimes echoes before its answer.
pub(crate) const LABELS: [&str; 6] = [
    "[Décision]",
    "[Nom d'usage]",
    TITRAGE,
    NATURE,
    ARTICLES,
    CONCLUSION,
];

/// The strings a field holds: one, a list, or DILA's `{"#text": …}` nodes.
fn texts(value: &Value) -> Vec<&str> {
    match value {
        Value::String(s) => vec![s.as_str()],
        Value::Array(items) => items.iter().flat_map(texts).collect(),
        Value::Object(node) => node.get("#text").map(texts).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn titrage(raw: &str) -> Option<String> {
    let line = SPACES.replace_all(raw, " ");
    let line = PCJA_CODES.replace(&line, "");
    let line = RANK.replace(&line, "");
    let line = line
        .trim()
        .trim_matches(|c: char| c == '-' || c == '.' || c == ' ');
    if !line.chars().any(char::is_alphabetic) || line.chars().count() > LINE_MAX {
        return None;
    }
    Some(line.to_string())
}

/// What the court indexed a decision under, from the first source that carries it.
fn titrages(jurisdiction_type: &str, provenances: &[Provenance]) -> Vec<String> {
    let fields = |sources: &[&str]| -> Vec<&Value> {
        provenances
            .iter()
            .filter(|p| sources.contains(&p.source))
            .map(|p| p.fields)
            .collect()
    };
    let judilibre_titles = || -> Vec<String> {
        fields(&["judilibre"])
            .into_iter()
            .filter_map(|f| f.get("titlesAndSummaries")?.as_array())
            .flatten()
            .filter_map(|entry| titrage(&texts(entry.get("titles")?).join(" - ")))
            .collect()
    };
    let judilibre_keywords = || -> Vec<String> {
        fields(&["judilibre"])
            .into_iter()
            .filter_map(|f| f.get("themes"))
            .filter_map(|themes| titrage(&texts(themes).join(" - ")))
            .collect()
    };
    let dila = |sources: &[&str]| -> Vec<String> {
        fields(sources)
            .into_iter()
            .filter_map(|f| f.get("SOMMAIRE")?.get("SCT"))
            .flat_map(texts)
            .filter_map(titrage)
            .collect()
    };
    let jade = || -> Vec<String> {
        fields(&["dila-jade"])
            .into_iter()
            .filter_map(|f| f.get("SCT"))
            .flat_map(texts)
            .filter_map(titrage)
            .collect()
    };
    let ariane = || -> Vec<String> {
        fields(&["ariane-web"])
            .into_iter()
            .filter_map(|f| f.get("commentaires")?.as_array())
            .flatten()
            .filter(|c| c.get("kind").and_then(Value::as_str) == Some("analyse"))
            .filter_map(|c| c.get("body")?.as_str())
            .flat_map(analysis_titrages)
            .filter_map(|t| titrage(&t))
            .collect()
    };
    let first = |candidates: Vec<Vec<String>>| {
        candidates
            .into_iter()
            .find(|found| !found.is_empty())
            .unwrap_or_default()
    };
    match jurisdiction_type {
        "CC" => first(vec![
            judilibre_titles(),
            dila(&["dila-cass", "dila-inca"]),
            judilibre_keywords(),
        ]),
        "CE" => first(vec![ariane(), jade()]),
        "CA" => dila(&["dila-capp"]),
        _ => jade(),
    }
}

/// The classification lines of a decision, each led by its kind: the court's rubrics,
/// the registry's nature of the case, a Strasbourg judgment's articles and conclusions.
pub fn classification(jurisdiction_type: &str, provenances: &[Provenance]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut lines: Vec<String> = titrages(jurisdiction_type, provenances)
        .into_iter()
        .filter(|t| seen.insert(t.to_lowercase()))
        .take(TITRAGES_MAX)
        .map(|t| format!("{TITRAGE} {t}"))
        .collect();
    for p in provenances {
        match p.source {
            "judilibre" if jurisdiction_type != "CC" => {
                if let Some(nature) = p
                    .fields
                    .get("themes")
                    .and_then(|t| titrage(&texts(t).join(" - ")))
                {
                    lines.push(format!("{NATURE} {nature}"));
                }
            }
            "cedh" => {
                let list = |key: &str, sep: &str| {
                    let items: Vec<&str> = p
                        .fields
                        .get(key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .split(';')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .collect();
                    (!items.is_empty()).then(|| items.join(sep))
                };
                if let Some(articles) = list("article", ", ") {
                    lines.push(format!("{ARTICLES} {articles}"));
                }
                if let Some(conclusion) = list("conclusion", " ; ") {
                    lines.push(format!("{CONCLUSION} {conclusion}"));
                }
            }
            _ => {}
        }
    }
    lines
}

/// The name a decision is cited by, each party cut to its first name or its short name
/// when the whole runs past [`NAME_MAX`], and none when it still does or when the opening
/// could not carry it, a party named by initials reading as an anonymisation code.
fn usage_name(case_name: &str) -> Option<String> {
    let name = case_name.trim().trim_matches('"').trim();
    if name.is_empty() {
        return None;
    }
    let name = if name.chars().count() <= NAME_MAX {
        name.to_owned()
    } else {
        let (at, separator) = PARTIES
            .iter()
            .filter_map(|separator| name.find(separator).map(|at| (at, *separator)))
            .min()?;
        let short = format!(
            "{} c. {}",
            party(&name[..at]),
            party(&name[at + separator.len()..])
        );
        (short.chars().count() <= NAME_MAX).then_some(short)?
    };
    (!names_by_code(&name)).then_some(name)
}

fn party(party: &str) -> String {
    let party = party.trim().trim_matches('"').replace('\u{2019}', "'");
    let bare = TRAILING_ACRONYM.replace(&party, "");
    if let Some((_, short)) = SHORT_NAMES.iter().find(|(name, _)| *name == bare) {
        return (*short).to_owned();
    }
    if let Some(acronym) = TRAILING_ACRONYM
        .captures(&party)
        .filter(|acronym| party.find(" (") == acronym.get(0).map(|m| m.start()))
    {
        return acronym[1].to_owned();
    }
    let Some((at, end)) = PARTY_ENDS
        .iter()
        .filter_map(|end| party.find(end).filter(|&at| at > 0).map(|at| (at, *end)))
        .min()
    else {
        return party;
    };
    let rest = &party[at + end.len()..];
    let head = match LEADING_ACRONYM.captures(rest) {
        Some(acronym) if end == " (" => acronym[1].to_owned(),
        _ => party[..at].to_owned(),
    };
    let more = MORE_PARTIES.contains(&end)
        || (end != " - " && MORE_PARTIES.iter().any(|more| rest.contains(more)));
    if more {
        format!("{head} et autres")
    } else {
        head
    }
}

/// What introduces a decision to the model: its title, then the name it is known by and
/// its classification when it has them.
#[derive(Debug, Clone)]
pub struct Header {
    pub title: String,
    pub case_name: Option<String>,
    pub classification: Vec<String>,
}

impl Header {
    /// A decision's header, its classification read from its provenances.
    pub fn new(
        jurisdiction_type: &str,
        title: String,
        case_name: Option<String>,
        provenances: &[Provenance],
    ) -> Self {
        Self {
            title,
            case_name: case_name.as_deref().and_then(usage_name),
            classification: classification(jurisdiction_type, provenances),
        }
    }

    fn naming(&self) -> String {
        let mut head = format!("[Décision] {}", self.title);
        if let Some(name) = self
            .case_name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            head.push_str("\n[Nom d'usage] ");
            head.push_str(name);
        }
        head
    }

    /// The header, then the decision, cut to the cap.
    pub fn rewrite_input(&self, body: &str) -> String {
        let mut head = self.naming();
        for line in &self.classification {
            head.push('\n');
            head.push_str(line);
        }
        build_summary_input(&format!("{head}\n\n{body}"))
    }

    /// The decision under its header, then the court's commentary under the titrage it
    /// was published with.
    pub fn collage_input(&self, body: &str, commentary: &str) -> String {
        let classification: String = self
            .classification
            .iter()
            .map(|line| format!("{line}\n"))
            .collect();
        format!(
            "=== DÉCISION ===\n{}\n\n{}\n\n=== SOMMAIRE OFFICIEL DE LA JURIDICTION ===\n{classification}{commentary}\n",
            self.naming(),
            build_summary_input(body)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one(jurisdiction_type: &str, source: &str, fields: &Value) -> Vec<String> {
        classification(jurisdiction_type, &[Provenance { source, fields }])
    }

    #[test]
    fn cassation_reads_judilibre_titles_before_dila_and_keywords() {
        let judilibre = json!({
            "themes": ["copropriete", "syndicat secondaire"],
            "titlesAndSummaries": [{"titles": ["COPROPRIETE", "Syndicat secondaire", "Constitution"]}],
        });
        let dila =
            json!({"SOMMAIRE": {"SCT": {"@ID": "8A", "#text": "1° \n COPROPRIETE - Autre"}}});
        let both = classification(
            "CC",
            &[
                Provenance {
                    source: "dila-cass",
                    fields: &dila,
                },
                Provenance {
                    source: "judilibre",
                    fields: &judilibre,
                },
            ],
        );
        assert_eq!(
            both,
            ["[Titrage] COPROPRIETE - Syndicat secondaire - Constitution"]
        );
        assert_eq!(
            one("CC", "dila-cass", &dila),
            ["[Titrage] COPROPRIETE - Autre"]
        );
        let keywords = json!({"themes": ["travail", "travail clandestin"]});
        assert_eq!(
            one("CC", "judilibre", &keywords),
            ["[Titrage] travail - travail clandestin"]
        );
    }

    #[test]
    fn jade_titrage_drops_its_pcja_codes() {
        let jade = json!({"SCT": "39-05-01-015, 65-02 MARCHES ET CONTRATS - EXECUTION FINANCIERE -Concession d'autoroutes."});
        assert_eq!(
            one("CAA", "dila-jade", &jade),
            ["[Titrage] MARCHES ET CONTRATS - EXECUTION FINANCIERE -Concession d'autoroutes"]
        );
    }

    #[test]
    fn conseil_d_etat_reads_the_titrages_of_its_analyses() {
        let ariane = json!({"commentaires": [
            {"kind": "analyse", "body": "Fixation du forfait - Reconduction tacite.\n\nLa reconduction tacite des forfaits est applicable."},
            {"kind": "conclusions", "body": "Autre chose."},
        ]});
        assert_eq!(
            one("CE", "ariane-web", &ariane),
            ["[Titrage] Fixation du forfait - Reconduction tacite"]
        );
    }

    #[test]
    fn appeal_judgments_carry_the_nature_of_the_case() {
        let judilibre = json!({"themes": ["Contrats", "Vente", "Demande en paiement du prix"]});
        assert_eq!(
            one("CA", "judilibre", &judilibre),
            ["[Nature de l'affaire] Contrats - Vente - Demande en paiement du prix"]
        );
    }

    #[test]
    fn a_strasbourg_judgment_carries_its_articles_and_conclusions() {
        let cedh = json!({"article": "6;6-1;41", "conclusion": "Violation de l'art. 6-1;Préjudice moral - réparation pécuniaire"});
        assert_eq!(
            one("CEDH", "cedh", &cedh),
            [
                "[Articles de la Convention] 6, 6-1, 41",
                "[Conclusion] Violation de l'art. 6-1 ; Préjudice moral - réparation pécuniaire"
            ]
        );
    }

    #[test]
    fn a_long_name_is_cut_to_each_party_s_first_or_short_name() {
        for (name, cut) in [
            ("Commune de Chapet", Some("Commune de Chapet")),
            (
                "Commission des Communautés européennes contre Royaume-Uni de Grande-Bretagne et d'Irlande du Nord",
                Some("Commission c. Royaume-Uni"),
            ),
            (
                "Advanced Superabrasives, Inc. contre Office de l\u{2019}Union européenne pour la propriété intellectuelle",
                Some("Advanced Superabrasives c. EUIPO"),
            ),
            (
                "Galderma SA contre Office de l\u{2019}harmonisation dans le marché intérieur (marques, dessins et modèles) (OHMI)",
                Some("Galderma SA c. OHMI"),
            ),
            (
                "Association AFAÏA contre Institut national de l\u{2019}origine et de la qualité (INAO)",
                Some("Association AFAÏA c. INAO"),
            ),
            (
                "Ángel Barreira Pérez contre Instituto Nacional de la Seguridad Social (INSS) et Tesorería General de la Seguridad Social (TGSS)",
                Some("Ángel Barreira Pérez c. INSS et autres"),
            ),
            (
                "Ryanair DAC et Airport Marketing Services Ltd contre Commission européenne",
                Some("Ryanair DAC et autres c. Commission"),
            ),
            (
                "Adria-Wien Pipeline GmbH et Wietersdorfer & Peggauer Zementwerke GmbH contre Finanzlandesdirektion für Kärnten",
                None,
            ),
            (
                "Fédération nationale des accidentés du travail et des handicapés de France",
                None,
            ),
            ("Mme X. contre Commission des Communautés européennes", None),
        ] {
            assert_eq!(usage_name(name).as_deref(), cut, "{name}");
        }
    }

    #[test]
    fn the_header_names_the_decision_then_classifies_it() {
        let header = Header {
            title: "Conseil d'État, 12 mars 2020, 123456".into(),
            case_name: Some("Commune de Chapet".into()),
            classification: vec!["[Titrage] URBANISME - Permis de construire".into()],
        };
        assert_eq!(
            header.rewrite_input("Le texte."),
            "[Décision] Conseil d'État, 12 mars 2020, 123456\n[Nom d'usage] Commune de Chapet\n[Titrage] URBANISME - Permis de construire\n\nLe texte."
        );
        assert_eq!(
            header.collage_input("Le texte.", "Le sommaire."),
            "=== DÉCISION ===\n[Décision] Conseil d'État, 12 mars 2020, 123456\n[Nom d'usage] Commune de Chapet\n\nLe texte.\n\n=== SOMMAIRE OFFICIEL DE LA JURIDICTION ===\n[Titrage] URBANISME - Permis de construire\nLe sommaire.\n"
        );
    }
}
