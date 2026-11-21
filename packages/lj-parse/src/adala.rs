//! Morocco's legal texts as the Justice ministry's Adala portal files them: folders of PDFs.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// A folder's page: the folders under it and its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdalaFolder {
    pub folders: Vec<u64>,
    pub files: Vec<AdalaFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdalaFile {
    pub id: u64,
    pub name: String,
    /// Under the portal's `/api/`, `uploads/2024/02/28/Code de procédure civile-1709129409071.pdf`.
    pub path: String,
    pub updated: Option<String>,
}

/// A file as a norm: its uid head, its nature, and its number, `None` for a code or a constitution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdalaIdentity {
    pub code: String,
    pub nature: &'static str,
    pub number: Option<String>,
}

static NEXT_DATA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<script id="__NEXT_DATA__" type="application/json">(.*?)</script>"#).unwrap()
});
static PROMULGATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)promulgation\s+de\s+la\s+loi(\s+organique)?(?:\s+de\s+finances(?:\s+rectificative)?)?\s+n(?:\s*[°º]|o)\s*(\d[\d\-\x{2011}.]*\d)",
    )
    .unwrap()
});
static NUMBERED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(loi\s+organique|loi|dahir(?:\s+portant\s+loi)?|d[ée]cret(?:-loi)?|arr[êe]t[ée](?:\s+conjoint|\s+viziriel|\s+r[ée]sidentiel)?|d[ée]cision|circulaire)\s+n(?:\s*[°º]|o)\s*(\d[\d\-\x{2011}/.]*\d)").unwrap()
});
/// A law a title names after its article, `La loi n° 51-00 relatif à …`.
static OWN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:la|le|l['\x{2019}])\s*(loi\s+organique|loi)\s+n(?:\s*[°º]|o)\s*(\d[\d\-\x{2011}/.]*\d)").unwrap()
});
/// A name that opens on an act other than a law, whose text names the law it applies.
static ACT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:d[ée]cret|arr[êe]t[ée]|circulaire|d[ée]cision)\b").unwrap()
});
/// An arrêté numbered after its author, `Arrêté du ministre de l'équipement nº 02-11 du …`.
static MINISTERIAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(arr[êe]t[ée](?:\s+conjoint)?)\s+(?:du|de\s+la|des)\s+(?:ministres?|chef\s+du\s+gouvernement|premier\s+ministre)\b([^(]{0,240}?)\bn(?:\s*[°º]|o)\s*(\d[\d\-\x{2011}/.]*\d)").unwrap()
});
static CITED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(loi|dahir|d[ée]cret|article)").unwrap());
static CONSTITUTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*constitution\s+(?:de\s+)?(\d{4})\b").unwrap());
static CONSOLIDATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)version\s+consolid[ée]e\s+(?:en\s+date\s+)?du\s+(1er|\d{1,2})\s+(\p{L}+)\s+(\d{4})",
    )
    .unwrap()
});
static PROMULGATES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\best\s+(?:promulgu|approuvé\s+le\s+texte\s+formant)").unwrap()
});
static SIGNED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*Fait\s+à\s+\p{Lu}").unwrap());
const STOPWORDS: [&str; 10] = ["DE", "DES", "DU", "LA", "LE", "LES", "L", "D", "ET", "AU"];

/// A folder's page as its Next.js data holds it, its files in `language`, `FR` or `AR`.
pub fn adala_folder(html: &str, language: &str) -> Result<AdalaFolder, String> {
    let data = NEXT_DATA
        .captures(html)
        .ok_or("a page without __NEXT_DATA__")?;
    let data: Value = serde_json::from_str(&data[1]).map_err(|e| e.to_string())?;
    let content = data
        .pointer("/props/pageProps/content")
        .ok_or("a page without content")?;
    let folders = content
        .get("folders")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|f| f.get("id")?.as_u64()).collect())
        .unwrap_or_default();
    let files = content
        .get("files")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|f| f.get("deleted").and_then(Value::as_bool) != Some(true))
                .filter(|f| f.get("language").and_then(Value::as_str) == Some(language))
                .filter_map(|f| {
                    Some(AdalaFile {
                        id: f.get("id")?.as_u64()?,
                        name: f
                            .get("name")?
                            .as_str()?
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" "),
                        path: f.get("path")?.as_str()?.to_string(),
                        updated: f
                            .get("updatedAt")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(AdalaFolder { folders, files })
}

fn nature(kind: &str) -> (&'static str, &'static str) {
    let kind = lj_fold::fold(kind).to_uppercase();
    match kind.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["LOI", "ORGANIQUE"] => ("LOI", "LOI_ORGANIQUE"),
        ["LOI"] => ("LOI", "LOI"),
        ["DAHIR", ..] => ("DAHIR", "DAHIR"),
        ["DECRET-LOI"] => ("DECRET-LOI", "DECRET_LOI"),
        ["DECRET"] => ("DECRET", "DECRET"),
        ["ARRETE", ..] => ("ARRETE", "ARRETE"),
        ["DECISION"] => ("DECISION", "DECISION"),
        _ => ("CIRCULAIRE", "CIRCULAIRE"),
    }
}

/// A number as Morocco writes it, `58.11` or `1\u{2011}11\u{2011}170`, with dashes.
fn dashed(number: &str) -> String {
    number.replace(['\u{2011}', '.'], "-")
}

fn numbered(caps: &regex::Captures) -> AdalaIdentity {
    let (code, nature) = nature(&caps[1]);
    AdalaIdentity {
        code: code.to_string(),
        nature,
        number: Some(dashed(&caps[2])),
    }
}

fn ministerial(line: &str) -> Option<AdalaIdentity> {
    let c = MINISTERIAL.captures(line)?;
    (!CITED.is_match(&c[2])).then(|| AdalaIdentity {
        code: "ARRETE".to_string(),
        nature: "ARRETE",
        number: Some(dashed(&c[3])),
    })
}

fn promulgated(caps: &regex::Captures) -> AdalaIdentity {
    AdalaIdentity {
        code: "LOI".to_string(),
        nature: match caps.get(1) {
            Some(_) => "LOI_ORGANIQUE",
            None => "LOI",
        },
        number: Some(dashed(&caps[2])),
    }
}

/// Where a consolidated text starts listing the acts that amended it, whose numbers are not
/// its own: `Modifiée par :`, `Tel qu'il a été modifié par :`.
static AMENDED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)(?:\btel(?:le)?s?\s+qu(?:e|['\x{2019}]\s*(?:il|elle)s?\s+(?:a|ont)\s+été)|^)\s*modifi\S*\s+(?:et\s+compl\S*\s+)?par\b").unwrap()
});
/// The law a dahir promulgates, else the act the name numbers, else a code or constitution by
/// title, else the text's first lines before the acts that amended it: the act a decree's or
/// an arrêté's name announces, else the law a dahir promulgates, the act a line numbers, the
/// law a title names.
pub fn adala_identity(name: &str, text: &str) -> Option<AdalaIdentity> {
    if let Some(c) = PROMULGATED.captures(name) {
        return Some(promulgated(&c));
    }
    if let Some(c) = NUMBERED.captures(name).or_else(|| OWN.captures(name)) {
        return Some(numbered(&c));
    }
    if let Some(identity) = ministerial(name) {
        return Some(identity);
    }
    if let Some(c) = CONSTITUTION.captures(name) {
        return Some(AdalaIdentity {
            code: format!("CONSTITUTION-{}", &c[1]),
            nature: "CONSTITUTION",
            number: None,
        });
    }
    let folded = lj_fold::fold(name).to_uppercase();
    if folded.starts_with("CODE ") {
        let words: Vec<&str> = folded
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty() && !STOPWORDS.contains(w))
            .collect();
        return Some(AdalaIdentity {
            code: words.join("-"),
            nature: "CODE",
            number: None,
        });
    }
    let head: String = text.chars().take(1500).collect();
    let head = AMENDED
        .find(&head)
        .map_or(head.as_str(), |m| &head[..m.start()]);
    let lines: Vec<&str> = head
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(8)
        .collect();
    let act = |laws: bool| {
        lines.iter().find_map(|l| {
            NUMBERED
                .captures(l)
                .map(|c| numbered(&c))
                .filter(|i| laws || !matches!(i.nature, "LOI" | "LOI_ORGANIQUE" | "DAHIR"))
                .or_else(|| ministerial(l))
        })
    };
    if ACT.is_match(name) {
        return act(false);
    }
    PROMULGATED
        .captures(head)
        .map(|c| promulgated(&c))
        .or_else(|| act(true))
        .or_else(|| {
            lines
                .iter()
                .find_map(|l| OWN.captures(l).map(|c| numbered(&c)))
        })
}

/// Where the law a file prints after the dahir promulgating it, or approving it as its annex,
/// begins, and the line the dahir is signed on, `Fait à Rabat, le 12 hija 1424 (3 février
/// 2004).`; `None` when the file opens with no such dahir.
pub fn adala_promulgation(text: &str) -> Option<(usize, &str)> {
    let mut promulgates = false;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        at += line.len();
        promulgates |= PROMULGATES.is_match(line);
        if SIGNED.is_match(line) {
            return promulgates.then_some((at, line.trim()));
        }
    }
    None
}

/// The day of the consolidation a text's first page names, `Version consolidée du 18 mars 2021`.
pub fn adala_consolidated(text: &str) -> Option<String> {
    let head: String = text.chars().take(1500).collect();
    let c = CONSOLIDATED.captures(&head)?;
    lj_core::dates::parse_date_fr(&format!("{} {} {}", &c[1], &c[2], &c[3])).map(|d| d.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_page_reads_its_folders_and_its_french_files() {
        let html = r#"<html><script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"content":{"id":415,
            "folders":[{"id":424},{"id":433}],
            "files":[{"id":2030,"name":"Code des Obligations et des Contrats","path":"uploads/2024/02/28/Code des Obligations-1709126934943.pdf","language":"FR","updatedAt":"2025-07-24T10:33:00.09Z","deleted":false},
                     {"id":2031,"name":"Loi","path":"uploads/x.pdf","language":"AR"},
                     {"id":2032,"name":"Gone","path":"uploads/y.pdf","language":"FR","deleted":true}]}}}}</script></html>"#;
        let folder = adala_folder(html, "FR").unwrap();
        assert_eq!(folder.folders, [424, 433]);
        assert_eq!(folder.files.len(), 1);
        assert_eq!(folder.files[0].id, 2030);
        assert_eq!(
            folder.files[0].updated.as_deref(),
            Some("2025-07-24T10:33:00.09Z")
        );
    }

    #[test]
    fn a_file_is_keyed_on_the_law_it_promulgates_else_its_act_else_its_title() {
        let id = |name: &str, text: &str| {
            adala_identity(name, text).map(|i| (i.code, i.nature, i.number))
        };
        assert_eq!(
            id("Dahir n° 1-22-34 du 23 chaoual 1443 (24 mai 2022) portant promulgation de la loi n° 95-17 relative à l’arbitrage", ""),
            Some(("LOI".into(), "LOI", Some("95-17".into())))
        );
        assert_eq!(
            id(
                "Décret n° 2-07-975 relatif à la composition de la commission",
                ""
            ),
            Some(("DECRET".into(), "DECRET", Some("2-07-975".into())))
        );
        assert_eq!(
            id("Code des Obligations et des Contrats", ""),
            Some(("CODE-OBLIGATIONS-CONTRATS".into(), "CODE", None))
        );
        assert_eq!(
            id("Constitution 2011", ""),
            Some(("CONSTITUTION-2011".into(), "CONSTITUTION", None))
        );
        assert_eq!(
            id(
                "La lutte contre les violences faites aux femmes",
                "Dahir n° 1-18-19 du 5 joumada II 1439 (22 février 2018) portant promulgation de la loi n° 103-13 relative à la lutte"
            ),
            Some(("LOI".into(), "LOI", Some("103-13".into())))
        );
        assert_eq!(
            id("Dahir no 1-11-165 du 16 kaada 1432 (14 octobre 2011) portant promulgation de la loi organique no 27-11 relative à la Chambre", ""),
            Some(("LOI".into(), "LOI_ORGANIQUE", Some("27-11".into())))
        );
        assert_eq!(
            id("Loi de finances 2013", "Dahir n° 1-12-57 portant promulgation de la loi de finances n° 115-12 pour l'année 2013"),
            Some(("LOI".into(), "LOI", Some("115-12".into())))
        );
        assert_eq!(
            id("Arrêté du ministre délégué auprès du Premier ministre, chargé des affaires économiques n° 1964-06 du 20 rejeb 1427", ""),
            Some(("ARRETE".into(), "ARRETE", Some("1964-06".into())))
        );
        assert_eq!(
            id(
                "Arrêté du ministre de l'agriculture pris pour l'application de la loi n° 28-07",
                "Le texte"
            ),
            None
        );
        assert_eq!(id("Grâces", "Le texte"), None);
        let amended = "Version consolidée en date du 26 octobre 2011\nLa loi n° 51-00 relatif à la location-accession\n\
            Telle qu\u{2019}elle a été modifiée par:\nDahir n° 1.11.170 portant promulgation de la loi n° 58.11 relative à la Cour de cassation";
        assert_eq!(
            id("Propriété immobilière", amended),
            Some(("LOI".into(), "LOI", Some("51-00".into())))
        );
        assert_eq!(
            id("Grâces", "Dahir n° 1-57-387 du 16 rejeb 1377 (6 février 1958) relatif aux grâces\n\
                Tel qu'il a été modifié par:\nDahir n° 1.11.170 portant promulgation de la loi n° 58.11"),
            Some(("DAHIR".into(), "DAHIR", Some("1-57-387".into())))
        );
        assert_eq!(
            id(
                "La société en nom collectif",
                "Version Consolidée en date du 19 Août 2021\nLOI N° 5-96 SUR LA SOCIETE EN NOM COLLECTIF\n\
                Modifiée par :\n- Dahir n° 1-21-75 du 3 hija 1442 (14 juillet 2021) portant\npromulgation de la loi n° 19-20 modifiant"
            ),
            Some(("LOI".into(), "LOI", Some("5-96".into())))
        );
        assert_eq!(
            id(
                "Décret pris pour l'application de la loi n° 104-12",
                "LOI N° 104-12 SUR LA LIBERTÉ DES PRIX\nDécret n° 2-14-652 du 8 safar 1436 (1er décembre 2014)\nVu le dahir n° 1-14-116 portant promulgation de la loi n° 104-12"
            ),
            Some(("DECRET".into(), "DECRET", Some("2-14-652".into())))
        );
        assert_eq!(
            id("La loi n° 112-12 relative aux coopératives", ""),
            Some(("LOI".into(), "LOI", Some("112-12".into())))
        );
        assert_eq!(
            id(
                "Décret pris pour l'application de la loi n° 20-13 du conseil de la concurrence",
                "La loi n° 20-13 relative au conseil de la concurrence\nDécret n° 2-15-109 du 16 chaabane 1436"
            ),
            Some(("DECRET".into(), "DECRET", Some("2-15-109".into())))
        );
        assert_eq!(
            id(
                "Arrêté de la ministre fixant la liste prévue par l'article 2",
                "La loi n° 44-12\nLe texte"
            ),
            None
        );
        assert_eq!(
            id("Dahir n° 1.11.170 portant promulgation de la loi n° 58.11 relative à la Cour de cassation", ""),
            Some(("LOI".into(), "LOI", Some("58-11".into())))
        );
        let text = "CODE DE LA FAMILLE\nDahir n° 1-16-2 portant promulgation de la loi n° 102-15\n\
            A DÉCIDÉ CE QUI SUIT:\nEst promulguée et sera publiée au Bulletin officiel la loi n° 70-03.\n\
            Fait à Rabat, le 12 hija 1424 (3 février 2004).\nPREAMBULE\nArticle premier\n";
        let (at, signed) = adala_promulgation(text).unwrap();
        assert_eq!(signed, "Fait à Rabat, le 12 hija 1424 (3 février 2004).");
        assert_eq!(&text[at..], "PREAMBULE\nArticle premier\n");
        let text =
            "Article premier\nEst approuvé le texte formant code pénal tel qu'il est publié en\n\
            annexe au présent dahir.\nFait à Rabat, le 28 joumada II 1382 (26 novembre 1962).\n\
            CODE PENAL\nArticle premier\n";
        let (at, _) = adala_promulgation(text).unwrap();
        assert_eq!(&text[at..], "CODE PENAL\nArticle premier\n");
        assert_eq!(
            adala_promulgation("DAHIR DES OBLIGATIONS\nFait à Rabat, le 12 août 1913.\n"),
            None
        );
        assert_eq!(
            adala_consolidated("CODE PENAL\nVersion consolidée en date du 20 avril 2023\n")
                .as_deref(),
            Some("2023-04-20")
        );
    }
}
