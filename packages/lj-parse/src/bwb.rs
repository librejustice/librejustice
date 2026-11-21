//! The BWB, the Dutch national law database: the regelingen the `wetten.overheid.nl` sitemap
//! lists, the consolidated states a regeling's manifest names, and the articles of one state.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::consolidated::{Child, ConsolidatedArticle, Lines, Tree};

/// A regeling the sitemap lists: its BWB id and the day it last changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BwbListed {
    pub id: String,
    pub lastmod: String,
}

/// A consolidated state: its first day, its last day unless open-ended, its XML's SHA-512 and path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BwbState {
    pub applies_from: String,
    pub until: Option<String>,
    pub hash: String,
    pub path: String,
}

/// One state of a regeling, its articles in document order, an annex one piece `Bijlage <n>`, a
/// text without any article one piece numbered `""`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BwbToestand {
    pub title: String,
    pub soort: String,
    /// The day it was signed, else the day the regeling was first published.
    pub adopted: Option<String>,
    pub articles: Vec<ConsolidatedArticle>,
    /// The day each article's wording came into force, by number.
    pub since: HashMap<String, String>,
    /// The articles left out because an earlier one holds their number.
    pub repeated: Vec<String>,
}

const CONTAINERS: &[&str] = &[
    "boek",
    "deel",
    "titeldeel",
    "hoofdstuk",
    "afdeling",
    "paragraaf",
    "sub-paragraaf",
    "divisie",
];
const SKIP: &[&str] = &["meta-data"];
const MONTHS: [&str; 12] = [
    "januari",
    "februari",
    "maart",
    "april",
    "mei",
    "juni",
    "juli",
    "augustus",
    "september",
    "oktober",
    "november",
    "december",
];
static SIGNED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bvan (\d{1,2}) (januari|februari|maart|april|mei|juni|juli|augustus|september|oktober|november|december) (\d{4})\b").unwrap()
});
static ABBREVIATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<afkorting(?:\s[^>]*)?>([^<]*)</afkorting>").unwrap());
const OPEN_END: &str = "9999-12-31";

fn text(tree: &Tree, idx: usize) -> String {
    tree.text(idx, SKIP)
}

fn kop_part(tree: &Tree, kop: usize, tag: &str) -> String {
    tree.child(kop, tag)
        .map(|i| text(tree, i))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// `Titel 1. Verbintenissen in het algemeen`, `§ 2`, or the title alone.
fn heading(tree: &Tree, kop: usize) -> String {
    let named = format!(
        "{} {}",
        kop_part(tree, kop, "label"),
        kop_part(tree, kop, "nr").trim_end_matches('.')
    );
    let named = named.trim();
    let titel = kop_part(tree, kop, "titel");
    match (named.is_empty(), titel.is_empty()) {
        (false, false) => format!("{named}. {titel}"),
        (false, true) => named.to_string(),
        _ => titel,
    }
}

pub fn bwb_sitemap(xml: &str) -> Result<Vec<BwbListed>, String> {
    let tree = Tree::parse(xml)?;
    let mut listed = Vec::new();
    for i in 0..tree.nodes.len() {
        if tree.nodes[i].tag != "url" {
            continue;
        }
        let loc = tree
            .child(i, "loc")
            .map(|l| text(&tree, l))
            .ok_or("a url without its loc")?;
        let id = loc.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        listed.push(BwbListed {
            id: id.to_string(),
            lastmod: tree
                .child(i, "lastmod")
                .map(|l| text(&tree, l))
                .unwrap_or_default(),
        });
    }
    Ok(listed)
}

/// The states of today's view, oldest first: a state rewritten for its day (`_1` after `_0`)
/// replaces the earlier writing.
pub fn bwb_manifest(xml: &str) -> Result<Vec<BwbState>, String> {
    let tree = Tree::parse(xml)?;
    let mut by_day: BTreeMap<String, (u32, BwbState)> = BTreeMap::new();
    for e in 0..tree.nodes.len() {
        if tree.nodes[e].tag != "expression" {
            continue;
        }
        let label = tree.attr(e, "label").ok_or("an expression without label")?;
        let (_, n) = label
            .rsplit_once('_')
            .ok_or_else(|| format!("expression label {label:?}"))?;
        let n: u32 = n
            .parse()
            .map_err(|_| format!("expression label {label:?}"))?;
        let meta = tree
            .child(e, "metadata")
            .ok_or("an expression without metadata")?;
        let field = |tag: &str| tree.child(meta, tag).map(|i| text(&tree, i));
        let Some(xml) = tree
            .children(e, "manifestation")
            .find(|&m| tree.attr(m, "label") == Some("xml"))
        else {
            continue;
        };
        let Some(item) = tree
            .child(xml, "item")
            .filter(|&i| tree.attr(i, "_deleted") != Some("true"))
        else {
            continue;
        };
        let applies_from = field("datum_inwerkingtreding")
            .ok_or_else(|| format!("expression {label} without its day"))?;
        let hash = tree
            .child(xml, "metadata")
            .and_then(|m| tree.child(m, "hashcode"))
            .map(|h| text(&tree, h))
            .ok_or_else(|| format!("expression {label} without its hash"))?;
        let state = BwbState {
            until: field("einddatum").filter(|d| d != OPEN_END),
            hash,
            path: format!(
                "{label}/xml/{}",
                tree.attr(item, "label").unwrap_or_default()
            ),
            applies_from: applies_from.clone(),
        };
        if by_day.get(&applies_from).is_none_or(|(held, _)| *held < n) {
            by_day.insert(applies_from, (n, state));
        }
    }
    Ok(by_day.into_values().map(|(_, s)| s).collect())
}

struct Render<'a> {
    tree: &'a Tree,
}

impl Render<'_> {
    fn walk(&self, idx: usize, out: &mut Lines) {
        for child in &self.tree.nodes[idx].children {
            match child {
                Child::Text(t) => out.push(t),
                Child::Node(i) => self.node(*i, out),
            }
        }
    }

    fn node(&self, i: usize, out: &mut Lines) {
        match self.tree.nodes[i].tag.as_str() {
            "meta-data" | "colspec" | "plaatje" | "illustratie" => {}
            "lidnr" | "li.nr" => {
                out.line_break();
                out.mark(&text(self.tree, i));
            }
            "kop" => {
                out.line_break();
                out.push(&heading(self.tree, i));
                out.line_break();
            }
            "sup" => {
                out.glue();
                self.walk(i, out);
            }
            "entry" => {
                out.push(" ");
                self.walk(i, out);
                out.push(" ");
            }
            "al" | "lid" | "li" | "row" | "tussenkop" | "definitie-item" | "artikel" => {
                out.line_break();
                self.walk(i, out);
                out.line_break();
            }
            _ => self.walk(i, out),
        }
    }

    /// The article's own title, then its body, without its label and number.
    fn article(&self, idx: usize) -> String {
        let mut lines = Lines::new(|line| line);
        for child in &self.tree.nodes[idx].children {
            match child {
                Child::Node(c) if self.tree.nodes[*c].tag == "kop" => {
                    let titel = kop_part(self.tree, *c, "titel");
                    if !titel.is_empty() {
                        lines.push(&titel);
                        lines.line_break();
                    }
                }
                Child::Node(c) => self.node(*c, &mut lines),
                Child::Text(t) => lines.push(t),
            }
        }
        lines.finish().join("\n")
    }

    fn annex(&self, idx: usize) -> String {
        let mut lines = Lines::new(|line| line);
        self.walk(idx, &mut lines);
        lines.finish().join("\n")
    }
}

/// `162`, `375` from a label `Artikel 375`, `Enig artikel`, or an unnumbered article's title.
fn article_num(tree: &Tree, kop: usize) -> Option<String> {
    let nr = kop_part(tree, kop, "nr");
    let nr = nr.trim_end_matches('.').trim();
    if !nr.is_empty() {
        return Some(nr.to_string());
    }
    let label = kop_part(tree, kop, "label");
    let numbered = label
        .get(..7)
        .filter(|head| head.eq_ignore_ascii_case("artikel"))
        .map(|_| label[7..].trim());
    match numbered {
        Some(n) if !n.is_empty() => Some(n.to_string()),
        Some(_) => Some(kop_part(tree, kop, "titel")).filter(|t| !t.is_empty()),
        None => Some(label).filter(|l| !l.is_empty()),
    }
}

fn title_path(tree: &Tree, idx: usize) -> Option<String> {
    let mut labels: Vec<String> = tree
        .ancestors(idx)
        .filter(|&a| CONTAINERS.contains(&tree.nodes[a].tag.as_str()))
        .filter_map(|a| tree.child(a, "kop"))
        .map(|k| heading(tree, k).replace('>', "›"))
        .filter(|h| !h.is_empty())
        .collect();
    labels.reverse();
    (!labels.is_empty()).then(|| labels.join(" > "))
}

/// The day the intitule names, `Besluit van 16 december 1991, houdende …`.
fn signed(intitule: &str) -> Option<String> {
    let c = SIGNED.captures(intitule)?;
    let month = MONTHS.iter().position(|m| *m == &c[2])? + 1;
    Some(format!(
        "{}-{month:02}-{:02}",
        &c[3],
        c[1].parse::<u32>().ok()?
    ))
}

fn lapsed(tree: &Tree, idx: usize) -> bool {
    tree.attr(idx, "status") == Some("vervallen")
}

/// The abbreviations a regeling's WTI lists in its general part, read from the head of the file.
pub fn bwb_abbreviations(head: &str) -> Result<Vec<String>, String> {
    let end = head
        .find("</algemene-informatie>")
        .ok_or("the head ends before the general part")?;
    let mut names = Vec::new();
    for c in ABBREVIATION.captures_iter(&head[..end]) {
        let name = quick_xml::escape::unescape(c[1].trim())
            .map_err(|e| format!("afkorting {:?}: {e}", &c[1]))?
            .into_owned();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    Ok(names)
}

/// A lapsed article (`Vervallen`) is left out, and an article an annex holds is read with it.
pub fn bwb_toestand(xml: &str) -> Result<BwbToestand, String> {
    let tree = Tree::parse(xml)?;
    let wetgeving = (0..tree.nodes.len())
        .find(|&i| tree.nodes[i].tag == "wetgeving")
        .ok_or("no wetgeving")?;
    let first = |tag: &str| (0..tree.nodes.len()).find(|&i| tree.nodes[i].tag == tag);
    let (citeertitel, intitule) = (first("citeertitel"), first("intitule"));
    let signing = [intitule, citeertitel]
        .into_iter()
        .flatten()
        .filter_map(|i| tree.attr(i, "ondertekening_bron"))
        .find(|d| d.len() == 10)
        .map(str::to_string);
    let published = [intitule, citeertitel]
        .into_iter()
        .flatten()
        .filter(|&i| tree.attr(i, "effect") == Some("nieuwe-regeling"))
        .filter_map(|i| tree.attr(i, "publicatie_bron"))
        .find(|d| d.len() == 10)
        .map(str::to_string);
    let original = tree
        .child(wetgeving, "meta-data")
        .and_then(|m| tree.child(m, "brondata"))
        .into_iter()
        .flat_map(|b| tree.children(b, "oorspronkelijk"))
        .flat_map(|o| tree.children(o, "publicatie"))
        .next();
    let original_day = |tag: &str| {
        original
            .and_then(|p| tree.child(p, tag))
            .and_then(|d| tree.attr(d, "isodatum"))
            .filter(|d| d.len() == 10)
            .map(str::to_string)
    };
    let title = citeertitel
        .map(|i| text(&tree, i))
        .filter(|t| !t.is_empty())
        .or_else(|| intitule.map(|i| text(&tree, i)))
        .filter(|t| !t.is_empty())
        .ok_or("no citeertitel nor intitule")?;
    let render = Render { tree: &tree };
    let mut articles = Vec::new();
    let mut seen = HashSet::new();
    let mut repeated = Vec::new();
    let mut since = HashMap::new();
    for idx in 0..tree.nodes.len() {
        let tag = tree.nodes[idx].tag.as_str();
        if !matches!(tag, "artikel" | "bijlage")
            || lapsed(&tree, idx)
            || tree
                .ancestors(idx)
                .any(|a| matches!(tree.nodes[a].tag.as_str(), "artikel" | "bijlage"))
        {
            continue;
        }
        let kop = tree.child(idx, "kop");
        let (num, texte) = if tag == "bijlage" {
            let nr = kop.map(|k| kop_part(&tree, k, "nr")).unwrap_or_default();
            let num = format!("Bijlage {nr}").trim().to_string();
            (Some(num), render.annex(idx))
        } else {
            (kop.and_then(|k| article_num(&tree, k)), render.article(idx))
        };
        let Some(num) = num else { continue };
        if texte.is_empty() {
            continue;
        }
        if !seen.insert(num.clone()) {
            repeated.push(num);
            continue;
        }
        if let Some(day) = tree.attr(idx, "inwerking").filter(|d| d.len() == 10) {
            since.insert(num.clone(), day.to_string());
        }
        articles.push(ConsolidatedArticle {
            num,
            title_path: title_path(&tree, idx),
            texte,
            nota: None,
        });
    }
    if articles.is_empty() {
        let body = ["wettekst", "regeling-tekst", "circulaire-tekst"]
            .iter()
            .find_map(|tag| (0..tree.nodes.len()).find(|&i| tree.nodes[i].tag == *tag))
            .map(|i| render.annex(i))
            .unwrap_or_default();
        if !body.is_empty() {
            articles.push(ConsolidatedArticle {
                num: String::new(),
                title_path: None,
                texte: body,
                nota: None,
            });
        }
    }
    Ok(BwbToestand {
        title,
        soort: tree
            .attr(wetgeving, "soort")
            .unwrap_or_default()
            .to_string(),
        adopted: signing
            .or_else(|| intitule.and_then(|i| signed(&text(&tree, i))))
            .or_else(|| original_day("ondertekeningsdatum"))
            .or(published)
            .or_else(|| original_day("uitgiftedatum")),
        articles,
        since,
        repeated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expression(label: &str, from: &str, until: &str, hash: &str) -> String {
        format!(
            r#"<expression label="{label}"><metadata><datum_inwerkingtreding>{from}</datum_inwerkingtreding><einddatum>{until}</einddatum></metadata><manifestation label="xml"><metadata><hashcode>{hash}</hashcode></metadata><item label="BWBR0000001_{label}.xml" _deleted="false" /></manifestation></expression>"#
        )
    }

    #[test]
    fn a_manifest_gives_today_s_view_oldest_first() {
        let xml = format!(
            r#"<work label="BWBR0000001"><metadata/>{}{}{}{}</work>"#,
            expression("2006-09-20_0", "2006-09-20", "9999-12-31", "c"),
            expression("2004-05-01_0", "2004-05-01", "2006-09-19", "a"),
            expression("2004-05-01_1", "2004-05-01", "2006-09-19", "b"),
            expression("2002-01-18_0", "2002-01-18", "2004-04-30", "z"),
        );
        let states = bwb_manifest(&xml).unwrap();
        let view: Vec<(&str, Option<&str>, &str)> = states
            .iter()
            .map(|s| (s.applies_from.as_str(), s.until.as_deref(), s.hash.as_str()))
            .collect();
        assert_eq!(
            view,
            [
                ("2002-01-18", Some("2004-04-30"), "z"),
                ("2004-05-01", Some("2006-09-19"), "b"),
                ("2006-09-20", None, "c"),
            ]
        );
        assert_eq!(
            states[1].path,
            "2004-05-01_1/xml/BWBR0000001_2004-05-01_1.xml"
        );
    }

    const TOESTAND: &str = r#"<toestand bwb-id="BWBR0005289"><wetgeving soort="wet">
<intitule>Wet van 22 november 1991, houdende <meta-data><jcis><jci/></jcis></meta-data></intitule>
<citeertitel status="officieel">Burgerlijk Wetboek Boek 6</citeertitel>
<wet-besluit><wettekst><boek><kop><label>Boek</label><nr>6</nr><titel>Algemeen gedeelte van het verbintenissenrecht </titel></kop>
<titeldeel><kop><label>Titel</label><nr>3</nr><titel>Onrechtmatige daad</titel></kop>
<afdeling><kop><label>Afdeling</label><nr>1</nr><titel>Algemene bepalingen </titel></kop>
<artikel status="goed" inwerking="1992-01-01"><kop><label>Artikel</label><nr status="officieel">162</nr></kop>
<lid><lidnr>1</lidnr><al>Hij die jegens een ander een onrechtmatige daad pleegt, is verplicht de schade te vergoeden. </al></lid>
<lid><lidnr>2</lidnr><al>Als onrechtmatige daad worden aangemerkt:</al><lijst><li><li.nr>a.</li.nr><al>een inbreuk op een recht;</al></li><li><li.nr>b.</li.nr><al>een doen in strijd met een wettelijke plicht<sup>1</sup>.</al></li></lijst></lid>
<meta-data><jcis><jci>jci1.3:c:BWBR0005289&amp;artikel=162</jci></jcis></meta-data>
</artikel>
<artikel status="vervallen"><kop><label>Artikel</label><nr>162a</nr></kop><al><redactie type="vervanging">Vervallen</redactie></al></artikel>
<artikel status="goed"><kop><label>Artikel</label><nr>163</nr><titel>Relativiteit</titel></kop><al>Geen verplichting tot schadevergoeding bestaat.</al></artikel>
</afdeling></titeldeel></boek>
<artikel status="goed"><kop><label>Artikel</label><nr>163</nr></kop><al>Twice numbered.</al></artikel>
</wettekst></wet-besluit>
<bijlage status="goed"><kop><label>Bijlage</label><nr>2</nr><titel>Bevoegdheidsregeling</titel></kop>
<artikel><kop><label>Artikel</label><nr>1</nr></kop><al>Het beroep wordt ingesteld bij de rechtbank.</al></artikel>
<table><tgroup><colspec/><tbody><row><entry>Wet A</entry><entry>artikel 2</entry></row></tbody></tgroup></table>
</bijlage>
</wetgeving></toestand>"#;

    #[test]
    fn a_state_gives_its_title_kind_day_and_articles_under_their_containers() {
        let t = bwb_toestand(TOESTAND).unwrap();
        assert_eq!(t.title, "Burgerlijk Wetboek Boek 6");
        assert_eq!(t.soort, "wet");
        assert_eq!(t.adopted.as_deref(), Some("1991-11-22"));
        assert_eq!(t.repeated, ["163"]);
        assert_eq!(t.since.get("162").map(String::as_str), Some("1992-01-01"));
        let nums: Vec<&str> = t.articles.iter().map(|a| a.num.as_str()).collect();
        assert_eq!(nums, ["162", "163", "Bijlage 2"]);
        let a = &t.articles[0];
        assert_eq!(
            a.title_path.as_deref(),
            Some(
                "Boek 6. Algemeen gedeelte van het verbintenissenrecht > Titel 3. Onrechtmatige daad > Afdeling 1. Algemene bepalingen"
            )
        );
        assert_eq!(
            a.texte,
            "1 Hij die jegens een ander een onrechtmatige daad pleegt, is verplicht de schade te vergoeden.\n\
             2 Als onrechtmatige daad worden aangemerkt:\n\
             a. een inbreuk op een recht;\n\
             b. een doen in strijd met een wettelijke plicht1."
        );
        assert_eq!(
            t.articles[1].texte,
            "Relativiteit\nGeen verplichting tot schadevergoeding bestaat."
        );
        assert_eq!(
            t.articles[2].texte,
            "Bijlage 2. Bevoegdheidsregeling\nArtikel 1\nHet beroep wordt ingesteld bij de rechtbank.\nWet A artikel 2"
        );
    }

    #[test]
    fn a_wti_head_gives_the_abbreviations_of_its_general_part() {
        let head = r#"<wetstechnische-informatie bwb-id="BWBR0005289"><algemene-informatie>
<afkortingen><afkorting>BW</afkorting><afkorting>BW Boek 6</afkorting><afkorting> BW6 </afkorting><afkorting>BW</afkorting></afkortingen>
<citeertitel status="officieel">Burgerlijk Wetboek Boek 6</citeertitel></algemene-informatie>
<gerelateerde-regelgeving><afkorting>Awb</afkorting>"#;
        assert_eq!(bwb_abbreviations(head).unwrap(), ["BW", "BW Boek 6", "BW6"]);
        assert!(bwb_abbreviations("<algemene-informatie><afkortingen>").is_err());
    }

    #[test]
    fn an_unsigned_regeling_is_dated_by_its_first_publication_only() {
        let state = |effect: &str| {
            format!(
                r#"<toestand><wetgeving soort="zbo"><intitule effect="{effect}" publicatie_bron="2026-10-14">Boetebeleid AFM</intitule>
<regeling><regeling-tekst><al>De AFM legt een boete op.</al></regeling-tekst></regeling></wetgeving></toestand>"#
            )
        };
        let first = bwb_toestand(&state("nieuwe-regeling")).unwrap();
        assert_eq!(first.adopted.as_deref(), Some("2026-10-14"));
        assert_eq!(first.articles[0].num, "");
        assert_eq!(bwb_toestand(&state("wijziging")).unwrap().adopted, None);
    }

    #[test]
    fn a_regeling_retitled_since_is_dated_by_its_original_publication() {
        let state = |effect: &str, signed: &str| {
            format!(
                r#"<toestand><wetgeving soort="circulaire"><meta-data><brondata><oorspronkelijk>
<publicatie effect="{effect}"><uitgiftedatum isodatum="2026-03-02"/>{signed}</publicatie>
</oorspronkelijk></brondata></meta-data>
<intitule effect="wijziging" publicatie_bron="2026-03-03">Circulaire BIO2</intitule>
<regeling><regeling-tekst><al>De BIO2 geldt.</al></regeling-tekst></regeling></wetgeving></toestand>"#
            )
        };
        let adopted = |effect, signed| bwb_toestand(&state(effect, signed)).unwrap().adopted;
        let signature = r#"<ondertekeningsdatum isodatum="2026-01-09"/>"#;
        assert_eq!(
            adopted("nieuwe-regeling", signature).as_deref(),
            Some("2026-01-09")
        );
        assert_eq!(adopted("nieuw", "").as_deref(), Some("2026-03-02"));
        assert_eq!(adopted("wijziging", "").as_deref(), Some("2026-03-02"));
    }
}
