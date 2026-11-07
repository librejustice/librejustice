//! Usage names from French Wikipedia: decision pages, and the list of the major administrative decisions.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use lj_core::text::normalize_spaces;

/// A decision a page or a list line names, with what anchors it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WikiEntry {
    pub name: String,
    /// `CETATEXT…` or `JURITEXT…`.
    pub legifrance: Vec<String>,
    /// As `decisions.jurisdiction_code`: `ce`, `tc` or `cc`.
    pub court: Option<&'static str>,
    /// Reading day, ISO.
    pub date: Option<String>,
    pub dockets: Vec<String>,
}

static LEGIFRANCE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:CETATEXT|JURITEXT)\d{12}").unwrap());
static DOCKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{2}-\d{2}\.\d{3}\b|\b\d{2}-\d{5}\b|\b\d{4,6}\b").unwrap());
static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:1[789]|20)\d\d$").unwrap());
static DAY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{1,2})(?:er)?[\s|]+(\p{L}+)[\s|]+(\d{4})").unwrap());
static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static TEMPLATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{[^{}]*\}\}").unwrap());
static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[(?:[^|\]]*\|)?([^\]]*)\]\]").unwrap());
static EMPHASIS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"'{2,}").unwrap());
static PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?i:arr[êe]ts?|affaire|jurisprudence|d[ée]cision)\s+(?:(?i:dite?s?)\s+)?(?:(?:de la|du|des)\s+)?",
    )
    .unwrap()
});
static SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s*\([^)]*\)$|\s+du\s+(?:1er|\d{1,2})\s+\p{L}+\s+\d{4}$").unwrap()
});
static CONNECTOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:(?:et|puis)\s+|[,;]\s*)+").unwrap());
static REFERENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{4}|\d{2}-\d|\bn[o°]\.?\s*\d").unwrap());
static DATE_TEMPLATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{date\|([^}]*)\}\}").unwrap());
static LIST_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\*\s*\{\{date\|([^}]*)\}\}\s*:\s*(.*)$").unwrap());
static REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<ref(\s[^>]*?)?(?:/>|>(.*?)</ref>)").unwrap());
static REF_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"name\s*=\s*"?([^"\s/>]+)"?"#).unwrap());

const MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// A name without the word that designates it, nor the day or disambiguation after
/// it; empty for a reference, which writes a day or a number.
pub fn wiki_usage_name(text: &str) -> String {
    let plain = plain_text(text);
    let name = PREFIX.replace(plain.trim(), "");
    let name = normalize_spaces(&SUFFIX.replace(&name, ""));
    match REFERENCE.is_match(&name) || DAY.is_match(&name) {
        true => String::new(),
        false => name,
    }
}

/// The decision a page names, read from its infobox; `None` without one.
pub fn wiki_page_entry(title: &str, wikitext: &str) -> Option<WikiEntry> {
    let infobox = infobox(wikitext)?;
    let fields = infobox_fields(infobox);
    let field = |name: &str| fields.get(name).map(String::as_str).unwrap_or("");
    Some(WikiEntry {
        name: wiki_usage_name(title),
        legifrance: legifrance_ids(infobox),
        court: court_of(field("tribunal")),
        date: iso_day(field("date")),
        dockets: dockets(&format!("{} {}", field("code"), field("numéro"))),
    })
}

/// Each name of a list line takes the references written right after it.
pub fn wiki_list_entries(wikitext: &str) -> Vec<WikiEntry> {
    let definitions: HashMap<&str, Vec<String>> = REF
        .captures_iter(wikitext)
        .filter_map(|c| {
            let name = REF_NAME.captures(c.get(1)?.as_str())?.get(1)?.as_str();
            Some((name, legifrance_ids(c.get(2)?.as_str())))
        })
        .collect();
    let mut entries = Vec::new();
    for line in wikitext.lines() {
        let Some(c) = LIST_LINE.captures(line) else {
            continue;
        };
        let date = iso_day(&c[1]);
        let rest = c.get(2).map_or("", |m| m.as_str());
        let mut line_entries: Vec<WikiEntry> = Vec::new();
        let mut last = 0;
        for r in REF.captures_iter(rest) {
            let whole = r.get(0).expect("group 0");
            let ids = match r.get(2) {
                Some(body) => legifrance_ids(body.as_str()),
                None => r
                    .get(1)
                    .and_then(|a| REF_NAME.captures(a.as_str()))
                    .and_then(|n| definitions.get(n.get(1)?.as_str()).cloned())
                    .unwrap_or_default(),
            };
            let segment = &rest[last..whole.start()];
            let name = segment_name(segment);
            match line_entries.last_mut() {
                Some(entry) if name.is_empty() => entry.legifrance.extend(ids),
                _ => line_entries.push(WikiEntry {
                    name,
                    legifrance: ids,
                    date: DATE_TEMPLATE
                        .captures(segment)
                        .and_then(|d| iso_day(&d[1]))
                        .or_else(|| date.clone()),
                    ..Default::default()
                }),
            }
            last = whole.end();
        }
        if line_entries.is_empty() {
            line_entries.push(WikiEntry {
                name: segment_name(rest),
                date: date.clone(),
                ..Default::default()
            });
        }
        entries.extend(line_entries.into_iter().filter(|e| !e.name.is_empty()));
    }
    for entry in &mut entries {
        entry.legifrance.sort_unstable();
        entry.legifrance.dedup();
    }
    entries
}

/// The italic span of a segment when it has one: the list sets its names in italics.
fn segment_name(raw: &str) -> String {
    let unlinked = LINK.replace_all(raw, "$1");
    let named = match (unlinked.find("''"), unlinked.rfind("''")) {
        (Some(start), Some(end)) if end > start => &unlinked[start..end + 2],
        _ => unlinked.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '.' | ',' | ';')),
    };
    let plain = plain_text(named);
    wiki_usage_name(&CONNECTOR.replace(plain.trim(), ""))
}

fn plain_text(text: &str) -> String {
    let mut text = COMMENT.replace_all(text, "").into_owned();
    while TEMPLATE.is_match(&text) {
        text = TEMPLATE.replace_all(&text, "").into_owned();
    }
    let text = LINK.replace_all(&text, "$1");
    EMPHASIS.replace_all(&text, "").into_owned()
}

/// The first decision infobox of a page, braces included.
fn infobox(wikitext: &str) -> Option<&str> {
    let start = wikitext.find("{{Infobox Décision")?;
    let bytes = wikitext.as_bytes();
    let (mut depth, mut i) = (0usize, start);
    while i + 1 < bytes.len() {
        match &bytes[i..i + 2] {
            b"{{" => {
                depth += 1;
                i += 2;
            }
            b"}}" => {
                depth -= 1;
                i += 2;
                if depth == 0 {
                    return Some(&wikitext[start..i]);
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// The `name = value` fields, split on the bars outside nested templates and links.
fn infobox_fields(infobox: &str) -> HashMap<String, String> {
    let body = COMMENT.replace_all(infobox, "");
    let body = &body[2..body.len() - 2];
    let bytes = body.as_bytes();
    let (mut depth, mut i, mut start) = (0usize, 0usize, 0usize);
    let mut parts = Vec::new();
    while i < bytes.len() {
        let pair = bytes.get(i..i + 2);
        if matches!(pair, Some(b"{{") | Some(b"[[")) {
            depth += 1;
            i += 2;
        } else if matches!(pair, Some(b"}}") | Some(b"]]")) {
            depth = depth.saturating_sub(1);
            i += 2;
        } else {
            if bytes[i] == b'|' && depth == 0 {
                parts.push(&body[start..i]);
                start = i + 1;
            }
            i += 1;
        }
    }
    parts.push(&body[start..]);
    parts
        .into_iter()
        .skip(1)
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))
        .collect()
}

fn legifrance_ids(text: &str) -> Vec<String> {
    LEGIFRANCE_ID
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

fn court_of(tribunal: &str) -> Option<&'static str> {
    let t = tribunal.to_lowercase().replace('’', "'");
    if t.contains("tribunal des conflits") {
        Some("tc")
    } else if t.contains("conseil d'") {
        Some("ce")
    } else if t.contains("cassation") {
        Some("cc")
    } else {
        None
    }
}

fn iso_day(text: &str) -> Option<String> {
    let c = DAY.captures(text)?;
    let month = MONTHS.iter().position(|m| *m == c[2].to_lowercase())? + 1;
    let day: u32 = c[1].parse().ok()?;
    (1..=31)
        .contains(&day)
        .then(|| format!("{}-{month:02}-{day:02}", &c[3]))
}

fn dockets(text: &str) -> Vec<String> {
    DOCKET
        .find_iter(&plain_text(text))
        .map(|m| m.as_str())
        .filter(|n| !YEAR.is_match(n))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_usage_name_drops_what_designates_it() {
        for (text, name) in [
            ("Arrêt Nicolo", "Nicolo"),
            ("arrêt AETR", "AETR"),
            ("Arrêt Croissant du 7 juillet 1978", "Croissant"),
            ("Arrêts amiante (2002)", "amiante"),
            ("Affaire de la crèche Baby Loup", "crèche Baby Loup"),
            ("Arrêt ''Luze et Devaux''", "Luze et Devaux"),
            ("Huglo{{référence nécessaire}}", "Huglo"),
            (
                "Cour de cassation, civile, Chambre sociale, 3 juillet 2019, 17-22.481, Inédit",
                "",
            ),
            ("Cass. soc., 25 nov. 2020, no 18-13769, PBRI", ""),
            ("no 2017-171 PDR", ""),
            (
                "Ministre de l'agriculture c/ Dame Lamotte",
                "Ministre de l'agriculture c/ Dame Lamotte",
            ),
        ] {
            assert_eq!(wiki_usage_name(text), name, "{text:?}");
        }
    }

    #[test]
    fn a_list_line_pairs_each_name_with_the_reference_after_it() {
        let line = "* {{date|20 avril 1956}} : ''Époux Bertin''<ref group=L>Arrêt {{Légifrance|base=JADE|url=https://www.legifrance.gouv.fr/affichJuriAdmin.do?idTexte=CETATEXT000007636583&dateTexte=|texte=''Époux Bertin''}}.</ref> et ''Ministre de l'agriculture c/ consorts Grimouard''<ref group=L>Arrêt {{Légifrance|base=JADE|url=https://www.legifrance.gouv.fr/affichJuriAdmin.do?idTexte=CETATEXT000007637338&dateTexte=}}.</ref>.";
        let entries = wiki_list_entries(line);
        let got: Vec<(&str, &[String])> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.legifrance.as_slice()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Époux Bertin", &["CETATEXT000007636583".to_string()][..]),
                (
                    "Ministre de l'agriculture c/ consorts Grimouard",
                    &["CETATEXT000007637338".to_string()][..]
                ),
            ]
        );
        assert!(entries
            .iter()
            .all(|e| e.date.as_deref() == Some("1956-04-20")));
    }

    #[test]
    fn the_words_around_the_italic_name_are_not_the_name() {
        let line = "* {{date|28 février 1992}} : ''S.A. Rothmans International France''<ref group=L>{{Légifrance|url=https://x/?idTexte=CETATEXT000007815079}}</ref>, et également l'arrêt ''S.A. Philip Morris France''<ref group=L>{{Légifrance|url=https://x/?idTexte=CETATEXT000007817502}}</ref>.\n\
                    * {{date|17 février 1995}} : [[Arrêts Hardouin et Marie du 17 février 1995|''Hardouin'' et ''Marie'']]<ref group=L>{{Légifrance|url=https://x/?idTexte=CETATEXT000007840249}}</ref>.";
        let names: Vec<String> = wiki_list_entries(line)
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(
            names,
            vec![
                "S.A. Rothmans International France",
                "S.A. Philip Morris France",
                "Hardouin et Marie"
            ]
        );
    }

    #[test]
    fn a_second_name_of_the_line_keeps_its_own_day() {
        let line = "* {{date|18 janvier 2001}} : ''Commune de Venelles''<ref group=L>{{Légifrance|url=https://x/?idTexte=CETATEXT000008036551}}.</ref> et {{date|5 mars 2001}}, ''Saez''<ref group=L>{{Légifrance|url=https://x/?idTexte=CETATEXT000008067600}}.</ref>.";
        let got: Vec<(String, Option<String>)> = wiki_list_entries(line)
            .into_iter()
            .map(|e| (e.name, e.date))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Commune de Venelles".into(), Some("2001-01-18".into())),
                ("Saez".into(), Some("2001-03-05".into())),
            ]
        );
    }

    #[test]
    fn a_reused_reference_carries_the_ids_of_its_definition() {
        let text = "* {{date|20 octobre 1989}} : ''[[Arrêt Nicolo|Nicolo]]''<ref name=nicolo group=L>Arrêt {{Légifrance|url=https://www.legifrance.gouv.fr/affichJuriAdmin.do?idTexte=CETATEXT000007742504}}.</ref>.\n\
                    * {{date|20 octobre 1989}} : ''Nicolo''<ref name=nicolo group=L/>.";
        let entries = wiki_list_entries(text);
        assert_eq!(entries.len(), 2);
        for e in entries {
            assert_eq!(e.name, "Nicolo");
            assert_eq!(e.legifrance, vec!["CETATEXT000007742504"]);
        }
    }

    #[test]
    fn a_line_without_reference_names_by_its_day() {
        let line = "* {{date|19 mai 1933}} : ''[[René Benjamin#La jurisprudence « Benjamin »|Benjamin]]''.";
        assert_eq!(
            wiki_list_entries(line),
            vec![WikiEntry {
                name: "Benjamin".into(),
                date: Some("1933-05-19".into()),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn the_infobox_gives_the_court_the_day_and_the_legifrance_id() {
        let page = "Intro.\n{{Infobox Décision juridique\n<!-- PRESENTATION --> | désignation courte = Arrêt Costedoat\n | titre = M. Z. contre M. Costedoat\n | code = \n | tribunal = [[Cour de cassation (France)|Cour de cassation]]\n | formation = [[Assemblée plénière]]\n | date = {{date|25|février|2000}}\n | lire en ligne = {{Légifrance|url=https://www.legifrance.gouv.fr/juri/id/JURITEXT000007043704/|texte=Costedoat}}\n}}\nCorps de la page, qui cite JURITEXT000007041111.";
        assert_eq!(
            wiki_page_entry("Arrêt Costedoat", page),
            Some(WikiEntry {
                name: "Costedoat".into(),
                legifrance: vec!["JURITEXT000007043704".into()],
                court: Some("cc"),
                date: Some("2000-02-25".into()),
                dockets: Vec::new(),
            })
        );
        assert_eq!(
            wiki_page_entry("Arrêt Costedoat", "Intro sans infobox."),
            None
        );
    }

    #[test]
    fn the_infobox_number_is_read_beside_the_year() {
        let page = "{{Infobox Décision juridique\n | tribunal = [[Tribunal des conflits (France)|Tribunal des conflits]]\n | date = 8 février 1873\n | code = n° 00012, 1873\n}}";
        let entry = wiki_page_entry("Arrêt Blanco", page).unwrap();
        assert_eq!(
            (entry.court, entry.date.as_deref(), entry.dockets),
            (Some("tc"), Some("1873-02-08"), vec!["00012".to_string()])
        );
    }
}
