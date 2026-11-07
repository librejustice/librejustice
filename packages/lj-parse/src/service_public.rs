//! Service-Public.fr « vos droits » sheets (DILA schema 3.4): one `F<n>.xml`
//! publication into its text as read, its « Textes de référence » and its source.

use jiff::civil::Date;

use crate::{build_tree, XmlNode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePublicSheet {
    /// `F1848`.
    pub id: String,
    pub title: String,
    /// The sheet on its own site (`spUrl`).
    pub url: String,
    /// Last update, `dc:date` « modified YYYY-MM-DD ».
    pub updated: Date,
    pub body: String,
    pub references: Vec<SheetReference>,
}

/// A « Texte de référence ».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetReference {
    pub url: String,
    /// « Code du travail : articles L1132-1 à L1132-4 ».
    pub title: String,
    /// « Conditions (licenciement nul) ».
    pub complement: Option<String>,
}

/// « L1132-1 à L1132-4 », « R312-1 et R312-5 », « L342-1 et suivants ».
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArticleSpan {
    One(String),
    Range(String, String),
    /// The article and every one after it in its section.
    From(String),
}

impl SheetReference {
    /// The articles named after the title's colon, parentheses dropped, an exception with them.
    pub fn articles(&self) -> Vec<ArticleSpan> {
        let Some((_, spec)) = self.title.rsplit_once(':') else {
            return Vec::new();
        };
        let mut plain = String::new();
        let mut depth = 0usize;
        for c in spec.chars() {
            match c {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ if depth == 0 => plain.push(c),
                _ => {}
            }
        }
        let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut spans = Vec::new();
        for item in plain.split([',', '+']).flat_map(|p| p.split(" et ")) {
            let item = without_article_word(item);
            if item == "suivants" {
                if let Some(ArticleSpan::One(first)) = spans.pop() {
                    spans.push(ArticleSpan::From(first));
                }
            } else if let Some((first, last)) = item.split_once(" à ") {
                spans.push(ArticleSpan::Range(
                    without_article_word(first).to_string(),
                    without_article_word(last).to_string(),
                ));
            } else if !item.is_empty() {
                spans.push(ArticleSpan::One(item.to_string()));
            }
        }
        spans
    }
}

fn without_article_word(item: &str) -> &str {
    let mut item = item.trim();
    while let Some(rest) = ["articles ", "article ", "Articles ", "Article "]
        .iter()
        .find_map(|w| item.strip_prefix(w))
    {
        item = rest.trim();
    }
    item
}

/// The `type` of a guided search's navigation node.
const GUIDED_SEARCH: &str = "Recherche guidée";

/// The parts of a publication that carry its text, in reading order.
const TEXT_PARTS: &[&str] = &["Introduction", "Texte", "ListeSituations"];

/// Parses one publication. `Ok(None)` for a guided search or a sheet with no
/// text; an error names the missing field.
pub fn parse_service_public_sheet(raw: &[u8]) -> Result<Option<ServicePublicSheet>, String> {
    let root = build_tree(raw).ok_or("unreadable XML")?;
    if root.tag != "Publication" {
        return Err(format!("root <{}>, not <Publication>", root.tag));
    }
    if root.attr("type") == Some(GUIDED_SEARCH) {
        return Ok(None);
    }
    let id = root.attr("ID").ok_or("no ID")?.to_string();
    let url = root.attr("spUrl").ok_or("no spUrl")?.to_string();
    let title = child_text(&root, "title").ok_or("no dc:title")?;
    let modified = child_text(&root, "date").ok_or("no dc:date")?;
    let updated = modified
        .strip_prefix("modified ")
        .and_then(|d| Date::strptime("%Y-%m-%d", d).ok())
        .ok_or_else(|| format!("dc:date {modified:?}"))?;

    let mut blocks = Vec::new();
    for part in &root.children {
        if TEXT_PARTS.contains(&part.tag.as_str()) {
            render(part, &mut blocks);
        } else if part.tag == "LienExterneCommente" {
            blocks.push(external_link(part));
        }
    }
    blocks.retain(|b| !b.is_empty());
    if blocks.is_empty() {
        return Ok(None);
    }

    let references: Vec<SheetReference> = root
        .children
        .iter()
        .filter(|c| c.tag == "Reference")
        .filter_map(|r| {
            Some(SheetReference {
                url: r.attr("URL")?.trim().to_string(),
                title: child_text(r, "Titre")?,
                complement: child_text(r, "Complement"),
            })
        })
        .collect();

    let mut body = blocks.join("\n\n");
    if !references.is_empty() {
        body.push_str("\n\nTextes de référence");
        for r in &references {
            body.push_str("\n- ");
            body.push_str(&r.title);
            if let Some(c) = &r.complement {
                body.push_str(" (");
                body.push_str(c);
                body.push(')');
            }
        }
    }
    body.push_str(&format!(
        "\n\nSource : Service-Public.fr (DILA), fiche {id}, mise à jour le {}.\n{url}",
        updated.strftime("%d/%m/%Y")
    ));

    Ok(Some(ServicePublicSheet {
        id,
        title,
        url,
        updated,
        body,
        references,
    }))
}

/// Appends the blocks of `node`: a paragraph or a title whole, a list item or a
/// table row as one line, every other element through its children.
fn render(node: &XmlNode, blocks: &mut Vec<String>) {
    match node.tag.as_str() {
        "Paragraphe" | "Titre" | "TitreFlottant" => blocks.push(flat(node)),
        "Liste" => {
            let items: Vec<String> = node
                .children
                .iter()
                .filter(|i| i.tag == "Item")
                .map(|i| format!("- {}", flat(i)))
                .collect();
            blocks.push(items.join("\n"));
        }
        "Tableau" => {
            let mut rows = Vec::new();
            for child in &node.children {
                match child.tag.as_str() {
                    "Rangée" => rows.push(
                        child
                            .children
                            .iter()
                            .filter(|c| c.tag == "Cellule")
                            .map(flat)
                            .collect::<Vec<_>>()
                            .join(" | "),
                    ),
                    "Titre" | "TitreFlottant" => blocks.push(flat(child)),
                    _ => {}
                }
            }
            blocks.push(rows.join("\n"));
        }
        // An online service or a contact inside the text: its title only.
        "ServiceEnLigne" | "OuSAdresser" | "PourEnSavoirPlus" => {
            if let Some(t) = node.children.iter().find(|c| c.tag == "Titre") {
                blocks.push(flat(t));
            }
        }
        _ => {
            for child in &node.children {
                render(child, blocks);
            }
        }
    }
}

/// « - Certificat d'aptitude professionnelle (CAP) (Ministère chargé de l'éducation) ».
fn external_link(node: &XmlNode) -> String {
    let link = node
        .children
        .iter()
        .find(|c| c.tag == "LienExterne")
        .map(flat)
        .unwrap_or_default();
    match node.children.iter().find(|c| c.tag == "Source").map(flat) {
        Some(source) if !source.is_empty() => format!("- {link} ({source})"),
        _ => format!("- {link}"),
    }
}

fn child_text(node: &XmlNode, tag: &str) -> Option<String> {
    node.children
        .iter()
        .find(|c| c.tag == tag)
        .map(flat)
        .filter(|t| !t.is_empty())
}

/// The descendant text on one line.
fn flat(node: &XmlNode) -> String {
    node.text()
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHEET: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Publication xmlns:dc="http://purl.org/dc/elements/1.1/" ID="F1848" type="Fiche d'information conditionnée" spUrl="https://www.service-public.gouv.fr/particuliers/vosdroits/F1848">
<dc:title>Licenciement pour motif personnel nul</dc:title>
<dc:date>modified 2026-06-19</dc:date>
<Introduction><Texte><Paragraphe>Lorsque l'employeur licencie un salarié, il doit préciser le motif.</Paragraphe></Texte></Introduction>
<Texte><Paragraphe>Le juge peut <MiseEnEvidence>annuler</MiseEnEvidence> le licenciement.</Paragraphe></Texte>
<ListeSituations affichage="onglet"><Situation><Titre>Nul</Titre><Texte>
<Chapitre><Titre><Paragraphe>Qu'est-ce qu'un licenciement nul ?</Paragraphe></Titre>
<Paragraphe>Les cas de <LienIntra LienID="R60353">nullité</LienIntra> sont :</Paragraphe>
<Liste type="puce"><Item><Paragraphe>En raison d'une discrimination</Paragraphe></Item><Item><Paragraphe>En violation d'une liberté</Paragraphe></Item></Liste>
<BlocCas affichage="radio"><Cas><Titre>Le salarié est réintégré</Titre><Paragraphe>Il a droit à une indemnité.</Paragraphe></Cas></BlocCas>
<Tableau><Rangée type="header"><Cellule><Paragraphe>Ancienneté</Paragraphe></Cellule><Cellule><Paragraphe>Indemnité</Paragraphe></Cellule></Rangée><Rangée><Cellule><Paragraphe>1 an</Paragraphe></Cellule><Cellule><Paragraphe>1 mois</Paragraphe></Cellule></Rangée></Tableau>
<ServiceEnLigne ID="R49067" type="Simulateur"><Titre>Simulateur d'indemnités</Titre></ServiceEnLigne>
</Chapitre></Texte></Situation></ListeSituations>
<Reference type="Texte de référence" URL="https://www.legifrance.gouv.fr/codes/id/LEGISCTA000006177836/" ID="R2808"><Titre>Code du travail : articles L1132-1 à L1132-4</Titre><Complement>Conditions (licenciement nul)</Complement></Reference>
<Reference type="Texte de référence" URL="https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000006901117" ID="R56470"><Titre>Code du travail : article L1134-4</Titre></Reference>
<QuestionReponse ID="F33681" audience="Particuliers">Une autre question ?</QuestionReponse>
</Publication>"#;

    #[test]
    fn a_sheet_reads_as_its_blocks_then_its_references_then_its_source() {
        let sheet = parse_service_public_sheet(SHEET.as_bytes())
            .unwrap()
            .unwrap();
        assert_eq!(sheet.id, "F1848");
        assert_eq!(sheet.title, "Licenciement pour motif personnel nul");
        assert_eq!(sheet.updated, jiff::civil::date(2026, 6, 19));
        assert_eq!(
            sheet.body,
            "Lorsque l'employeur licencie un salarié, il doit préciser le motif.\n\n\
             Le juge peut annuler le licenciement.\n\n\
             Nul\n\n\
             Qu'est-ce qu'un licenciement nul ?\n\n\
             Les cas de nullité sont :\n\n\
             - En raison d'une discrimination\n- En violation d'une liberté\n\n\
             Le salarié est réintégré\n\n\
             Il a droit à une indemnité.\n\n\
             Ancienneté | Indemnité\n1 an | 1 mois\n\n\
             Simulateur d'indemnités\n\n\
             Textes de référence\n\
             - Code du travail : articles L1132-1 à L1132-4 (Conditions (licenciement nul))\n\
             - Code du travail : article L1134-4\n\n\
             Source : Service-Public.fr (DILA), fiche F1848, mise à jour le 19/06/2026.\n\
             https://www.service-public.gouv.fr/particuliers/vosdroits/F1848"
        );
        assert_eq!(
            sheet.references[1],
            SheetReference {
                url: "https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000006901117".into(),
                title: "Code du travail : article L1134-4".into(),
                complement: None,
            }
        );
    }

    #[test]
    fn a_reference_title_names_its_articles_as_ranges_lists_and_open_ends() {
        let spans = |title: &str| {
            SheetReference {
                url: String::new(),
                title: title.into(),
                complement: None,
            }
            .articles()
        };
        let one = |n: &str| ArticleSpan::One(n.into());
        let range = |a: &str, b: &str| ArticleSpan::Range(a.into(), b.into());
        assert_eq!(
            spans("Code du travail : articles L1132-1 à L1132-4"),
            [range("L1132-1", "L1132-4")]
        );
        assert_eq!(
            spans("Code civil : articles 230 et 232"),
            [one("230"), one("232")]
        );
        assert_eq!(
            spans("Code de l'action sociale et des familles : articles R225-1 à R225-8 + D225-6"),
            [range("R225-1", "R225-8"), one("D225-6")]
        );
        assert_eq!(
            spans("Code de la santé publique : articles R2141-36 et suivants"),
            [ArticleSpan::From("R2141-36".into())]
        );
        assert_eq!(
            spans("Code de commerce : articles L622-3 à L622-9 (à l'exception de l'article L. 622-6-1)"),
            [range("L622-3", "L622-9")]
        );
        assert_eq!(
            spans("Code du travail : article R4215-1 à article R4215-17"),
            [range("R4215-1", "R4215-17")]
        );
        assert_eq!(
            spans("Code général des impôts, annexe 2 : articles 242-0 M à 242-0 Z ter"),
            [range("242-0 M", "242-0 Z ter")]
        );
        assert_eq!(spans("Code du travail"), []);
    }

    #[test]
    fn a_guided_search_is_not_a_sheet() {
        let xml = r#"<Publication xmlns:dc="http://purl.org/dc/elements/1.1/" ID="F35977" type="Recherche guidée" spUrl="https://x/F35977"><dc:title>Quelle aide ?</dc:title><dc:date>modified 2026-01-01</dc:date><Introduction><Texte><Paragraphe>Répondez.</Paragraphe></Texte></Introduction></Publication>"#;
        assert_eq!(parse_service_public_sheet(xml.as_bytes()), Ok(None));
    }

    #[test]
    fn a_sheet_of_external_links_lists_them() {
        let xml = r#"<Publication xmlns:dc="http://purl.org/dc/elements/1.1/" ID="F10041" type="Fiche avec liens externes" spUrl="https://www.service-public.gouv.fr/particuliers/vosdroits/F10041"><dc:title>Diplômes professionnels</dc:title><dc:date>modified 2026-03-02</dc:date><LienExterneCommente><LienExterne URL="https://www.education.gouv.fr/cap">Certificat d'aptitude professionnelle (CAP)</LienExterne><Source ID="R30638">Ministère chargé de l'éducation</Source></LienExterneCommente></Publication>"#;
        let sheet = parse_service_public_sheet(xml.as_bytes()).unwrap().unwrap();
        assert!(sheet.body.starts_with(
            "- Certificat d'aptitude professionnelle (CAP) (Ministère chargé de l'éducation)\n\nSource"
        ));
    }
}
