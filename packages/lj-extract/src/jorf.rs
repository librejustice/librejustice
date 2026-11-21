//! Parser PUR du fond JORF (bulk DILA) — `payload XML → JorfArticle /
//! JorfTexte`. Aucun I/O : les octets sont fournis par `lj-ingest`.
//!
//! Le fond JORF partage la **structure XML de LEGI** : mêmes tags
//! `META_COMMUN/ID`, `META_ARTICLE/NUM|ETAT|DATE_DEBUT|DATE_FIN`,
//! `CONTEXTE/TEXTE@cid`, `BLOC_TEXTUEL/CONTENU`, mêmes sentinelles de date. Ce
//! module réutilise les helpers de [`crate::legi`] (`find_anywhere`,
//! `date_or_none`, `clean_contenu`, `collect_titre_tm`) et n'ajoute que les
//! divergences JORF :
//! - **`NUM` souvent absent** (annonces, actes `TYPE=AUTONOME`) → `num:
//!   Option<String>` (LEGI le rend obligatoire ; JORF non) ; idem `DATE_DEBUT`
//!   (les annonces n'ont qu'une sentinelle) ;
//! - **métadonnées texte plus riches** : `TITREFULL` (libellé descriptif vs
//!   `TITRE` court), `NATURE` (DECRET/ARRETE/LOI/ANNONCES…), mots-clés `MC`
//!   (détection des traités) et le graphe de liens `LIENS/LIEN` (rattachement
//!   avenant→accord initial).

use std::collections::BTreeMap;

use crate::legi::{clean_contenu, collect_titre_tm, date_or_none, find_anywhere};
use lj_core::error::CoreError;
use lj_parse::{build_tree, node_text, XmlNode};
use serde::{Deserialize, Serialize};

/// Une version d'article du fond JORF (`JORFARTI*.xml`, racine `<ARTICLE>`).
/// Calque [`crate::legi::LegiArticle`] mais `num`/`num_key`/`date_debut` sont
/// optionnels (cf. en-tête module). `jorftext` = cid `CONTEXTE/TEXTE@cid` (le
/// texte JORF parent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JorfArticle {
    pub jorfarti: String,
    pub jorftext: String,
    pub num: Option<String>,
    pub num_key: Option<String>,
    pub titre_text: Option<String>,
    pub etat: String,
    pub date_debut: Option<String>,
    pub date_fin: Option<String>,
    pub texte: Option<String>,
    pub nota: Option<String>,
}

/// Un lien `META_TEXTE_VERSION/LIENS/LIEN` : graphe inter-textes JORF. Pour les
/// avenants d'un accord, `typelien ∈ {MODIFIE, MODIFICATION}` relie l'avenant à
/// l'accord initial. `date_signa` = sentinelle absorbée en `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JorfLien {
    pub cid: String,
    pub typelien: String,
    pub sens: String,
    pub date_signa: Option<String>,
    pub num_texte: Option<String>,
    pub libelle: Option<String>,
    /// `naturetexte` de la cible (DECRET, LOI, CODE…).
    pub nature: Option<String>,
    /// `num` : numéro d'article ciblé (liens CITATION vers un article de code).
    pub num: Option<String>,
    /// `id` : uid DILA de l'élément ciblé (texte ou article, ex. `LEGIARTI…`).
    pub target_id: Option<String>,
}

/// Un texte/version du fond JORF (`JORFTEXT*.xml`, racine `<TEXTE_VERSION>`). Le
/// corps vit dans les `JORFARTI` liés (pas ici) ; ce nœud porte les métadonnées.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JorfTexte {
    pub jorftext: String,
    pub titre: String,
    pub titre_full: Option<String>,
    pub nature: String,
    /// Numéro propre de l'acte (`META_TEXTE_CHRONICLE/NUM`, ex. `65-557`), absent pour
    /// les actes non numérotés (arrêté/décret « du X »). Brique d'`instrument_key`.
    pub num: Option<String>,
    /// NOR (`META_TEXTE_CHRONICLE/NOR`) — identifiant cross-diffuseur (même NOR
    /// LEGI↔JORF) ; workhorse du collapse d'identité.
    pub nor: Option<String>,
    /// ELI (`META_COMMUN/ID_ELI`) — identifiant autoritaire quand présent.
    pub eli: Option<String>,
    pub date_texte: Option<String>,
    pub date_publi: Option<String>,
    pub liens: Vec<JorfLien>,
}

/// Parse un `JORFARTI*.xml` (racine `<ARTICLE>`) en [`JorfArticle`]. Erreur
/// franche ([`CoreError::Xml`]) si `ID` ou `cid` manquent ; `NUM`/`DATE_DEBUT`
/// absents sont tolérés (`None`) — divergence JORF vs LEGI.
pub fn parse_jorf_article(raw: &[u8]) -> Result<JorfArticle, CoreError> {
    let root =
        build_tree(raw).ok_or_else(|| CoreError::Xml("ARTICLE JORF: XML illisible".into()))?;

    let jorfarti = node_text(find_anywhere(&root, "META_COMMUN/ID"))
        .ok_or_else(|| CoreError::Xml("ARTICLE JORF: META_COMMUN/ID manquant".into()))?;
    let texte_node = find_anywhere(&root, "CONTEXTE/TEXTE");
    let jorftext = texte_node
        .and_then(|t| t.attr("cid"))
        .ok_or_else(|| {
            CoreError::Xml(format!(
                "ARTICLE JORF {jorfarti}: CONTEXTE/TEXTE@cid manquant"
            ))
        })?
        .to_string();

    let num = node_text(find_anywhere(&root, "META_ARTICLE/NUM"));
    let num_key = num.as_deref().map(lj_norm::article_key::identity_key);
    let etat = node_text(find_anywhere(&root, "META_ARTICLE/ETAT")).unwrap_or_default();
    // Le fonds JORF ne date pas ses articles : `META_ARTICLE/DATE_DEBUT` et
    // `DATE_FIN` portent la sentinelle. Une entrée de journal est datée par sa
    // parution et n'a pas de fin de validité.
    let date_debut = date_or_none(node_text(find_anywhere(&root, "META_ARTICLE/DATE_DEBUT")))
        .or_else(|| {
            date_or_none(
                texte_node
                    .and_then(|t| t.attr("date_publi"))
                    .map(str::to_string),
            )
        });
    let date_fin = date_or_none(node_text(find_anywhere(&root, "META_ARTICLE/DATE_FIN")));
    let titre_text = find_anywhere(&root, "CONTEXTE").and_then(collect_titre_tm);

    Ok(JorfArticle {
        jorfarti,
        jorftext,
        num,
        num_key,
        titre_text,
        etat,
        date_debut,
        date_fin,
        texte: clean_contenu(find_anywhere(&root, "BLOC_TEXTUEL/CONTENU")),
        nota: clean_contenu(find_anywhere(&root, "NOTA/CONTENU")),
    })
}

/// Parse un `JORFTEXT*.xml` (racine `<TEXTE_VERSION>`) en [`JorfTexte`]. Erreur
/// franche si `META_COMMUN/ID` manque, ou si `META_TEXTE_VERSION/TITRE` **et**
/// `TITREFULL` manquent tous deux — le stock porte des `<TITRE/>` vides avec
/// un `TITREFULL` rempli, le titre long fait foi.
pub fn parse_jorf_texte(raw: &[u8]) -> Result<JorfTexte, CoreError> {
    let root = build_tree(raw)
        .ok_or_else(|| CoreError::Xml("TEXTE_VERSION JORF: XML illisible".into()))?;

    let jorftext = node_text(find_anywhere(&root, "META_COMMUN/ID"))
        .ok_or_else(|| CoreError::Xml("TEXTE_VERSION JORF: META_COMMUN/ID manquant".into()))?;
    let titre = node_text(find_anywhere(&root, "META_TEXTE_VERSION/TITRE"))
        .or_else(|| node_text(find_anywhere(&root, "META_TEXTE_VERSION/TITREFULL")))
        .ok_or_else(|| {
            CoreError::Xml(format!(
                "TEXTE_VERSION JORF {jorftext}: META_TEXTE_VERSION/TITRE et TITREFULL manquants"
            ))
        })?;

    Ok(JorfTexte {
        jorftext,
        titre,
        titre_full: node_text(find_anywhere(&root, "META_TEXTE_VERSION/TITREFULL")),
        nature: node_text(find_anywhere(&root, "META_COMMUN/NATURE")).unwrap_or_default(),
        num: node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/NUM")),
        nor: node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/NOR")),
        eli: node_text(find_anywhere(&root, "META_COMMUN/ID_ELI")),
        date_texte: node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/DATE_TEXTE")),
        date_publi: node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/DATE_PUBLI")),
        liens: collect_liens(&root),
    })
}

/// Liens `META_TEXTE_VERSION/LIENS/LIEN` (plusieurs) avec leurs attributs.
fn collect_liens(root: &lj_parse::XmlNode) -> Vec<JorfLien> {
    find_anywhere(root, "LIENS")
        .map(|n| {
            n.children
                .iter()
                .filter(|c| c.tag == "LIEN")
                .map(|c| JorfLien {
                    cid: c.attr("cidtexte").unwrap_or_default().to_string(),
                    typelien: c.attr("typelien").unwrap_or_default().to_string(),
                    sens: c.attr("sens").unwrap_or_default().to_string(),
                    date_signa: date_or_none(c.attr("datesignatexte").map(str::to_string)),
                    num_texte: c
                        .attr("numtexte")
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    libelle: c.text(),
                    nature: c
                        .attr("naturetexte")
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    num: c.attr("num").filter(|s| !s.is_empty()).map(str::to_string),
                    target_id: c.attr("id").filter(|s| !s.is_empty()).map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// An item of a JORF text or of one of its sections, in published order: an
/// article with its `NUM` when it has one, or a section with the title and
/// level its parent gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JorfItem {
    Article {
        article: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        num: Option<String>,
    },
    Section {
        section: String,
        title: String,
        level: u8,
    },
}

/// The `LIEN_ART` and `LIEN_SECTION_TA` children of a `STRUCT` or a
/// `STRUCTURE_TA`, in document order.
fn collect_items(node: Option<&XmlNode>) -> Result<Vec<JorfItem>, CoreError> {
    let Some(node) = node else {
        return Ok(Vec::new());
    };
    let mut items = Vec::new();
    for c in &node.children {
        match c.tag.as_str() {
            "LIEN_ART" => {
                let Some(id) = non_empty(c.attr("id")) else {
                    continue;
                };
                items.push(JorfItem::Article {
                    article: id,
                    num: non_empty(c.attr("num")),
                });
            }
            "LIEN_SECTION_TA" => {
                let section = non_empty(c.attr("id"))
                    .ok_or_else(|| CoreError::Xml("LIEN_SECTION_TA without id".into()))?;
                let level = level(c, &section)?;
                items.push(JorfItem::Section {
                    section,
                    title: c.text().unwrap_or_default(),
                    level,
                });
            }
            _ => {}
        }
    }
    Ok(items)
}

fn level(node: &XmlNode, owner: &str) -> Result<u8, CoreError> {
    node.attr("niv")
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| CoreError::Xml(format!("{owner}: niv missing or not a level")))
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// A `texte/struct/JORFTEXT*.xml` (root `<TEXTELR>`): the text's own items in
/// published order, which the `JORFARTI` ids do not follow. A section declares
/// its own articles in its `JORFSCTA`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JorfStructOrder {
    pub jorftext: String,
    pub items: Vec<JorfItem>,
}

impl JorfStructOrder {
    /// The text's own articles, outside its sections: `(JORFARTI, NUM)`.
    pub fn articles(&self) -> impl Iterator<Item = (&str, Option<&str>)> {
        self.items.iter().filter_map(|i| match i {
            JorfItem::Article { article, num } => Some((article.as_str(), num.as_deref())),
            JorfItem::Section { .. } => None,
        })
    }
}

/// Parses a `texte/struct/JORFTEXT*.xml`. An empty or absent `STRUCT` gives no
/// item: a text of the scanned era, published as a facsimile only.
pub fn parse_jorf_struct(raw: &[u8]) -> Result<JorfStructOrder, CoreError> {
    let root =
        build_tree(raw).ok_or_else(|| CoreError::Xml("TEXTELR JORF: XML illisible".into()))?;
    let jorftext = node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/CID"))
        .ok_or_else(|| CoreError::Xml("TEXTELR JORF: META_TEXTE_CHRONICLE/CID manquant".into()))?;
    let items = collect_items(find_anywhere(&root, "STRUCT"))?;
    Ok(JorfStructOrder { jorftext, items })
}

/// What a section holds, stored under its `JORFSCTA`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JorfSectionBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub items: Vec<JorfItem>,
}

/// A `section_ta/JORFSCTA*.xml` (root `<SECTION_TA>`) and the text it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JorfSection {
    pub id: String,
    pub jorftext: String,
    pub body: JorfSectionBody,
}

pub fn parse_jorf_section(raw: &[u8]) -> Result<JorfSection, CoreError> {
    let root =
        build_tree(raw).ok_or_else(|| CoreError::Xml("SECTION_TA JORF: unreadable XML".into()))?;
    let id = node_text(find_anywhere(&root, "SECTION_TA/ID"))
        .ok_or_else(|| CoreError::Xml("SECTION_TA JORF: ID missing".into()))?;
    let jorftext = find_anywhere(&root, "CONTEXTE/TEXTE")
        .and_then(|t| non_empty(t.attr("cid")))
        .ok_or_else(|| {
            CoreError::Xml(format!("SECTION_TA JORF {id}: CONTEXTE/TEXTE@cid missing"))
        })?;
    let body = JorfSectionBody {
        title: node_text(find_anywhere(&root, "SECTION_TA/TITRE_TA")),
        comment: find_anywhere(&root, "SECTION_TA/COMMENTAIRE").and_then(XmlNode::html_text),
        items: collect_items(find_anywhere(&root, "SECTION_TA/STRUCTURE_TA"))?,
    };
    Ok(JorfSection { id, jorftext, body })
}

/// The text and nota of an article as published, stored under its `JORFARTI`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JorfArticleText {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nota: Option<String>,
}

impl JorfArticle {
    /// `None` for an empty article.
    pub fn published_text(&self) -> Option<JorfArticleText> {
        (self.texte.is_some() || self.nota.is_some()).then(|| JorfArticleText {
            text: self.texte.clone(),
            nota: self.nota.clone(),
        })
    }
}

/// A node of an issue's table of contents: a heading with its level and what
/// it holds, or an entry naming a text with the title the issue printed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JorfTocNode {
    Heading {
        heading: String,
        level: u8,
        items: Vec<JorfTocNode>,
    },
    Entry {
        text: String,
        title: String,
    },
}

/// A JO issue, `conteneur/JORF/CONT/…/JORFCONT*.xml` (root `<JO>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JorfIssue {
    pub issue_uid: String,
    pub num: Option<String>,
    pub date_publi: Option<String>,
    pub eli: Option<String>,
    pub title: String,
    pub toc: Vec<JorfTocNode>,
}

impl JorfIssue {
    /// The texts the table of contents lists, in its order.
    pub fn texts(&self) -> Vec<&str> {
        fn walk<'a>(nodes: &'a [JorfTocNode], out: &mut Vec<&'a str>) {
            for n in nodes {
                match n {
                    JorfTocNode::Heading { items, .. } => walk(items, out),
                    JorfTocNode::Entry { text, .. } => out.push(text),
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.toc, &mut out);
        out
    }
}

fn toc_nodes(node: &XmlNode) -> Result<Vec<JorfTocNode>, CoreError> {
    let mut nodes = Vec::new();
    for c in &node.children {
        match c.tag.as_str() {
            "TM" => nodes.push(JorfTocNode::Heading {
                heading: node_text(c.find("TITRE_TM")).unwrap_or_default(),
                level: level(c, "TM")?,
                items: toc_nodes(c)?,
            }),
            "LIEN_TXT" => nodes.push(JorfTocNode::Entry {
                text: non_empty(c.attr("idtxt"))
                    .ok_or_else(|| CoreError::Xml("LIEN_TXT without idtxt".into()))?,
                title: c.attr("titretxt").unwrap_or_default().trim().to_string(),
            }),
            _ => {}
        }
    }
    Ok(nodes)
}

pub fn parse_jorf_conteneur(raw: &[u8]) -> Result<JorfIssue, CoreError> {
    let root = build_tree(raw).ok_or_else(|| CoreError::Xml("JO JORF: unreadable XML".into()))?;
    let issue_uid = node_text(find_anywhere(&root, "META_COMMUN/ID"))
        .ok_or_else(|| CoreError::Xml("JO JORF: META_COMMUN/ID missing".into()))?;
    let title = node_text(find_anywhere(&root, "META_CONTENEUR/TITRE"))
        .ok_or_else(|| CoreError::Xml(format!("JO JORF {issue_uid}: TITRE missing")))?;
    let toc = match find_anywhere(&root, "STRUCTURE_TXT") {
        Some(s) => toc_nodes(s).map_err(|e| CoreError::Xml(format!("JO JORF {issue_uid}: {e}")))?,
        None => Vec::new(),
    };
    Ok(JorfIssue {
        num: node_text(find_anywhere(&root, "META_CONTENEUR/NUM")),
        date_publi: date_or_none(node_text(find_anywhere(&root, "META_CONTENEUR/DATE_PUBLI"))),
        eli: node_text(find_anywhere(&root, "META_COMMUN/ID_ELI")),
        issue_uid,
        title,
        toc,
    })
}

/// A text's `ENTREPRISE` block, when it is marked as concerning companies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JorfEnterprise {
    pub dates: Vec<String>,
    pub domains: Vec<String>,
}

/// The blocks of a `TEXTE_VERSION` around its articles, and the key each is
/// stored under.
const BLOCKS: [(&str, &str); 7] = [
    ("NOTICE", "notice"),
    ("VISAS", "visas"),
    ("SIGNATAIRES", "signataires"),
    ("TP", "tp"),
    ("ABRO", "abro"),
    ("RECT", "rect"),
    ("SM", "sm"),
];

/// What a `TEXTE_VERSION` publishes beyond what `legal_text` keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JorfPublication {
    pub jorftext: String,
    pub nature: Option<String>,
    pub num_parution: Option<String>,
    pub num_sequence: Option<i32>,
    pub origine_publi: Option<String>,
    pub date_publi: Option<String>,
    pub eli: Option<String>,
    pub page_deb: Option<i32>,
    pub page_fin: Option<i32>,
    pub ministere: Option<String>,
    pub autorite: Option<String>,
    pub title: Option<String>,
    pub title_full: Option<String>,
    pub keywords: Vec<String>,
    pub enterprise: Option<JorfEnterprise>,
    pub blocks: BTreeMap<String, String>,
}

/// A hard error when the id is missing, or when a sequence or a page is not a
/// number.
pub fn parse_jorf_publication(raw: &[u8]) -> Result<JorfPublication, CoreError> {
    let root = build_tree(raw)
        .ok_or_else(|| CoreError::Xml("TEXTE_VERSION JORF: unreadable XML".into()))?;
    let jorftext = node_text(find_anywhere(&root, "META_COMMUN/ID"))
        .ok_or_else(|| CoreError::Xml("TEXTE_VERSION JORF: META_COMMUN/ID missing".into()))?;
    let chronicle =
        |tag: &str| node_text(find_anywhere(&root, &format!("META_TEXTE_CHRONICLE/{tag}")));
    let version = |tag: &str| node_text(find_anywhere(&root, &format!("META_TEXTE_VERSION/{tag}")));
    let number = |tag: &str| -> Result<Option<i32>, CoreError> {
        chronicle(tag)
            .map(|v| {
                v.parse().map_err(|_| {
                    CoreError::Xml(format!("TEXTE_VERSION JORF {jorftext}: {tag} {v:?}"))
                })
            })
            .transpose()
    };
    let children = |path: &str, tag: &str| -> Vec<String> {
        find_anywhere(&root, path)
            .map(|n| {
                n.children
                    .iter()
                    .filter(|c| c.tag == tag)
                    .filter_map(XmlNode::text)
                    .collect()
            })
            .unwrap_or_default()
    };
    let enterprise = find_anywhere(&root, "TEXTE_VERSION/ENTREPRISE")
        .filter(|e| e.attr("texte_entreprise") == Some("oui"))
        .map(|_| JorfEnterprise {
            dates: children("ENTREPRISE/DATES_EFFET", "DATE_EFFET"),
            domains: children("ENTREPRISE/DOMAINES", "DOMAINE"),
        });
    let blocks = BLOCKS
        .iter()
        .filter_map(|(tag, key)| {
            let path = format!("TEXTE_VERSION/{tag}/CONTENU");
            Some((key.to_string(), clean_contenu(find_anywhere(&root, &path))?))
        })
        .collect();
    Ok(JorfPublication {
        nature: node_text(find_anywhere(&root, "META_COMMUN/NATURE")),
        num_parution: chronicle("NUM_PARUTION"),
        num_sequence: number("NUM_SEQUENCE")?,
        origine_publi: chronicle("ORIGINE_PUBLI"),
        date_publi: date_or_none(chronicle("DATE_PUBLI")),
        eli: node_text(find_anywhere(&root, "META_COMMUN/ID_ELI")),
        page_deb: number("PAGE_DEB_PUBLI")?,
        page_fin: number("PAGE_FIN_PUBLI")?,
        ministere: version("MINISTERE"),
        autorite: version("AUTORITE"),
        title: version("TITRE"),
        title_full: version("TITREFULL"),
        keywords: children("META_TEXTE_VERSION/MCS_TXT", "MC"),
        enterprise,
        blocks,
        jorftext,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // TEXTE_VERSION réel : décret 69-243 publiant l'accord franco-algérien 1968.
    // MC « ACCORD INTERNATIONAL » + LIENS vers les avenants (typelien
    // MODIFIE/MODIFICATION).
    const ACCORD_ALGERIEN: &str = r#"<TEXTE_VERSION>
<META>
<META_COMMUN><ID>JORFTEXT000000694290</ID><ORIGINE>JORF</ORIGINE><NATURE>DECRET</NATURE></META_COMMUN>
<META_SPEC>
<META_TEXTE_CHRONICLE><CID>JORFTEXT000000694290</CID><NUM>69-243</NUM><DATE_PUBLI>1969-03-22</DATE_PUBLI><DATE_TEXTE>1969-03-18</DATE_TEXTE></META_TEXTE_CHRONICLE>
<META_TEXTE_VERSION>
<TITRE>Décret n° 69-243 du 18 mars 1969</TITRE>
<TITREFULL>Décret n° 69-243 du 18 mars 1969 portant publication de l'accord entre le Gouvernement de la République française et le Gouvernement de la République Démocratique et populaire algérienne, relatif à la circulation, à l'emploi et au séjour en France des ressortissants algériens et de leurs familles, signé à Alger le 27 décembre 1968</TITREFULL>
<DATE_DEBUT/><DATE_FIN/>
<MCS_TXT><MC>ACCORD INTERNATIONAL</MC><MC>FRANCE</MC><MC>ALGERIE</MC><MC>ACCORDS DE 1968</MC></MCS_TXT>
<LIENS>
<LIEN cidtexte="JORFTEXT000000333424" datesignatexte="1986-03-07" id="JORFTEXT000000333424" naturetexte="DECRET" num="" numtexte="86-320" sens="cible" typelien="MODIFIE">Décret n°86-320 du 7 mars 1986</LIEN>
<LIEN cidtexte="JORFTEXT000000599731" datesignatexte="2002-12-20" id="JORFTEXT000000599731" naturetexte="DECRET" num="" numtexte="2002-1500" sens="source" typelien="MODIFICATION">Décret n° 2002-1500 du 20 décembre 2002, v. init.</LIEN>
<LIEN cidtexte="LEGITEXT000006069577" datesignatexte="2999-01-01" id="LEGIARTI000006311152" naturetexte="CODE" num="948" numtexte="" sens="cible" typelien="CITATION">CGI - art. 948</LIEN>
</LIENS>
</META_TEXTE_VERSION>
</META_SPEC>
</META>
<NOTICE><CONTENU/></NOTICE>
</TEXTE_VERSION>"#;

    #[test]
    fn parse_texte_accord_maps_metadata_and_links() {
        let t = parse_jorf_texte(ACCORD_ALGERIEN.as_bytes()).expect("texte");
        assert_eq!(t.jorftext, "JORFTEXT000000694290");
        assert_eq!(t.nature, "DECRET");
        assert_eq!(t.date_publi.as_deref(), Some("1969-03-22"));
        assert_eq!(t.date_texte.as_deref(), Some("1969-03-18"));
        assert!(t
            .titre_full
            .as_deref()
            .unwrap()
            .contains("27 décembre 1968"));
        assert_eq!(t.liens.len(), 3);
        assert_eq!(t.liens[0].typelien, "MODIFIE");
        assert_eq!(t.liens[0].cid, "JORFTEXT000000333424");
        assert_eq!(t.liens[0].num_texte.as_deref(), Some("86-320"));
        assert_eq!(t.liens[0].date_signa.as_deref(), Some("1986-03-07"));
        assert_eq!(t.liens[1].typelien, "MODIFICATION");
        assert_eq!(t.liens[2].typelien, "CITATION");
        assert_eq!(t.liens[2].date_signa, None);
    }

    // TEXTE_VERSION réel : délibération CSA 2018-11. Le `META_COMMUN` du fond JORF
    // ne porte que ID/ANCIEN_ID/ORIGINE/URL/NATURE ; NOR vit sous
    // `META_TEXTE_CHRONICLE`, comme NUM et les dates.
    const DELIBERATION_CSA: &str = r#"<TEXTE_VERSION>
<META>
<META_COMMUN><ID>JORFTEXT000036889522</ID><ANCIEN_ID/><ORIGINE>JORF</ORIGINE><URL>texte/version/JORF/TEXT/00/00/36/88/95/JORFTEXT000036889522.xml</URL><NATURE>DELIBERATION</NATURE></META_COMMUN>
<META_SPEC>
<META_TEXTE_CHRONICLE><CID>JORFTEXT000036889522</CID><NUM>2018-11</NUM><NOR>CSAC1812426X</NOR><DATE_PUBLI>2018-05-10</DATE_PUBLI><DATE_TEXTE>2018-04-18</DATE_TEXTE></META_TEXTE_CHRONICLE>
<META_TEXTE_VERSION>
<TITRE>Délibération n°2018-11 du 18 avril 2018</TITRE>
<TITREFULL>Délibération n° 2018-11 du 18 avril 2018 du Conseil supérieur de l'audiovisuel relative à l'honnêteté et à l'indépendance de l'information et des programmes qui y concourent</TITREFULL>
</META_TEXTE_VERSION>
</META_SPEC>
</META>
</TEXTE_VERSION>"#;

    #[test]
    fn parse_texte_lit_le_nor_sous_la_chronique() {
        let t = parse_jorf_texte(DELIBERATION_CSA.as_bytes()).expect("texte");
        assert_eq!(t.nor.as_deref(), Some("CSAC1812426X"));
        assert_eq!(t.num.as_deref(), Some("2018-11"));
    }

    // TEXTE_VERSION réel : accord de coopération T2S (JORFTEXT000033243907),
    // `<TITRE/>` vide mais TITREFULL rempli.
    const ACCORD_SANS_TITRE: &str = r#"<TEXTE_VERSION>
<META>
<META_COMMUN><ID>JORFTEXT000033243907</ID><ORIGINE>JORF</ORIGINE><NATURE/></META_COMMUN>
<META_SPEC>
<META_TEXTE_CHRONICLE><CID>JORFTEXT000033243907</CID><NUM/><NOR>AMFX1627336X</NOR><DATE_PUBLI>2016-10-16</DATE_PUBLI><DATE_TEXTE>2999-01-01</DATE_TEXTE></META_TEXTE_CHRONICLE>
<META_TEXTE_VERSION>
<TITRE/>
<TITREFULL>Accord de coopération relatif à T2S entre surveillants et autorités compétentes pour la supervision des dépositaires centraux de titres</TITREFULL>
</META_TEXTE_VERSION>
</META_SPEC>
</META>
</TEXTE_VERSION>"#;

    #[test]
    fn parse_texte_titre_vide_replie_sur_titrefull() {
        let t = parse_jorf_texte(ACCORD_SANS_TITRE.as_bytes()).expect("texte");
        assert!(t.titre.starts_with("Accord de coopération relatif à T2S"));
    }

    // JORFARTI réel : annonce « accès protégé » à NUM vide (TYPE=AUTONOME),
    // DATE_DEBUT/FIN sentinelles, corps présent.
    const ARTICLE_ANNONCE: &str = r#"<ARTICLE>
<META>
<META_COMMUN><ID>JORFARTI000051887074</ID><ORIGINE>JORF</ORIGINE><NATURE>Article</NATURE></META_COMMUN>
<META_SPEC><META_ARTICLE><NUM/><DATE_DEBUT>2999-01-01</DATE_DEBUT><DATE_FIN>2999-01-01</DATE_FIN><TYPE>AUTONOME</TYPE></META_ARTICLE></META_SPEC>
</META>
<CONTEXTE><TEXTE cid="JORFTEXT000051887073" nature="ANNONCES"><TITRE_TXT>Demandes de changement de nom</TITRE_TXT></TEXTE></CONTEXTE>
<BLOC_TEXTUEL><CONTENU><p>Les actes individuels relatifs à l'état des personnes sont accessibles sur Légifrance.</p></CONTENU></BLOC_TEXTUEL>
</ARTICLE>"#;

    #[test]
    fn parse_article_tolerates_missing_num_and_sentinel_dates() {
        let a = parse_jorf_article(ARTICLE_ANNONCE.as_bytes()).expect("article");
        assert_eq!(a.jorfarti, "JORFARTI000051887074");
        assert_eq!(a.jorftext, "JORFTEXT000051887073");
        assert_eq!(a.num, None);
        assert_eq!(a.num_key, None);
        assert_eq!(a.date_debut, None);
        assert_eq!(a.date_fin, None);
        assert_eq!(
            a.texte.as_deref(),
            Some("Les actes individuels relatifs à l'état des personnes sont accessibles sur Légifrance.")
        );
    }

    #[test]
    fn parse_article_with_num_normalizes_key() {
        let xml = r#"<ARTICLE>
<META><META_COMMUN><ID>JORFARTI000000694291</ID></META_COMMUN>
<META_SPEC><META_ARTICLE><NUM>6</NUM><ETAT>VIGUEUR</ETAT><DATE_DEBUT>2002-08-09</DATE_DEBUT><DATE_FIN>2999-01-01</DATE_FIN></META_ARTICLE></META_SPEC></META>
<CONTEXTE><TEXTE cid="JORFTEXT000000694290"/></CONTEXTE>
<BLOC_TEXTUEL><CONTENU>Les ressortissants algériens bénéficient de plein droit.</CONTENU></BLOC_TEXTUEL>
</ARTICLE>"#;
        let a = parse_jorf_article(xml.as_bytes()).expect("article");
        assert_eq!(a.num.as_deref(), Some("6"));
        assert_eq!(a.num_key.as_deref(), Some("6"));
        assert_eq!(a.etat, "VIGUEUR");
        assert_eq!(a.date_debut.as_deref(), Some("2002-08-09"));
        assert_eq!(a.date_fin, None);
    }

    #[test]
    fn parse_article_dates_from_publication() {
        // JORFARTI réel (délibération CSA 2018-11, art. 1er) : DATE_DEBUT et DATE_FIN
        // sentinelles, ETAT absent. La parution portée par CONTEXTE/TEXTE@date_publi
        // date l'entrée.
        let xml = r#"<ARTICLE>
<META><META_COMMUN><ID>JORFARTI000036889523</ID></META_COMMUN>
<META_SPEC><META_ARTICLE><NUM>1</NUM><DATE_DEBUT>2999-01-01</DATE_DEBUT><DATE_FIN>2999-01-01</DATE_FIN></META_ARTICLE></META_SPEC></META>
<CONTEXTE><TEXTE cid="JORFTEXT000036889522" date_publi="2018-04-20" date_signature="2018-04-18" nature="DELIBERATION" nor="CSAC1812426X"/></CONTEXTE>
<BLOC_TEXTUEL><CONTENU>Les éditeurs de services veillent à l'honnêteté de l'information.</CONTENU></BLOC_TEXTUEL>
</ARTICLE>"#;
        let a = parse_jorf_article(xml.as_bytes()).expect("article");
        assert_eq!(a.date_debut.as_deref(), Some("2018-04-20"));
        assert_eq!(a.date_fin, None);
        assert_eq!(a.etat, "");
    }

    #[test]
    fn parse_struct_keeps_document_order_including_unnumbered() {
        // Extrait réel (JORFTEXT000000209787) : les LIEN_ART sans num sont rendus dans
        // l'ordre du fichier.
        let xml = r#"<TEXTELR>
<META><META_SPEC><META_TEXTE_CHRONICLE><CID>JORFTEXT000000209787</CID></META_TEXTE_CHRONICLE></META_SPEC></META>
<STRUCT>
<LIEN_ART debut="2999-01-01" etat="" fin="2999-01-01" id="JORFARTI000001045162" num="" origine="JORF"/>
<LIEN_ART debut="2999-01-01" etat="" fin="2999-01-01" id="JORFARTI000002207442" num="" origine="JORF"/>
<LIEN_ART debut="2999-01-01" etat="" fin="2999-01-01" id="JORFARTI000001817636" num="Annexe" origine="JORF"/>
</STRUCT>
</TEXTELR>"#;
        let s = parse_jorf_struct(xml.as_bytes()).expect("struct");
        assert_eq!(s.jorftext, "JORFTEXT000000209787");
        assert_eq!(
            s.articles().collect::<Vec<_>>(),
            vec![
                ("JORFARTI000001045162", None),
                ("JORFARTI000002207442", None),
                ("JORFARTI000001817636", Some("Annexe"))
            ]
        );
    }

    #[test]
    fn parse_struct_ne_liste_que_le_dispositif_pas_les_articles_de_l_annexe() {
        // STRUCT réelle de l'arrêté du 31 juillet 2026 (JORFTEXT000054750162) :
        // six articles au dispositif, et une annexe qui renumérote de 1 à 3
        // derrière son LIEN_SECTION_TA. Seuls les six sont ici.
        let xml = r#"<TEXTELR>
<META><META_SPEC><META_TEXTE_CHRONICLE><CID>JORFTEXT000054750162</CID></META_TEXTE_CHRONICLE></META_SPEC></META>
<STRUCT>
<LIEN_ART debut="2999-01-01" etat="" fin="2999-01-01" id="JORFARTI000054750167" num="1" origine="JORF"/>
<LIEN_ART debut="2999-01-01" etat="" fin="2999-01-01" id="JORFARTI000054750172" num="6" origine="JORF"/>
<LIEN_SECTION_TA cid="JORFSCTA000054750173" debut="2999-01-01" etat="" fin="2999-01-01" id="JORFSCTA000054750173" niv="1" url="/JORF/SCTA/00/00/54/75/01/JORFSCTA000054750173.xml">Annexe</LIEN_SECTION_TA>
</STRUCT>
</TEXTELR>"#;
        let s = parse_jorf_struct(xml.as_bytes()).expect("struct");
        assert_eq!(
            s.articles().map(|(_, num)| num).collect::<Vec<_>>(),
            vec![Some("1"), Some("6")]
        );
        assert_eq!(
            s.items.last(),
            Some(&JorfItem::Section {
                section: "JORFSCTA000054750173".into(),
                title: "Annexe".into(),
                level: 1,
            })
        );
    }

    #[test]
    fn parse_struct_empty_is_prenumerisation() {
        // STRUCT vide (traité pré-numérisation, ex. décret 76-963) → liste vide.
        let xml = r#"<TEXTELR>
<META><META_SPEC><META_TEXTE_CHRONICLE><CID>JORFTEXT000000307098</CID></META_TEXTE_CHRONICLE></META_SPEC></META>
<STRUCT/>
</TEXTELR>"#;
        let s = parse_jorf_struct(xml.as_bytes()).expect("struct");
        assert!(s.items.is_empty());
    }

    #[test]
    fn article_missing_id_is_hard_error() {
        let xml = br#"<ARTICLE><META_SPEC><META_ARTICLE><NUM>1</NUM></META_ARTICLE></META_SPEC>
<CONTEXTE><TEXTE cid="JORFTEXT000000000001"/></CONTEXTE></ARTICLE>"#;
        assert!(matches!(parse_jorf_article(xml), Err(CoreError::Xml(_))));
    }

    #[test]
    fn article_missing_cid_is_hard_error() {
        let xml = br#"<ARTICLE><META_COMMUN><ID>JORFARTI000000000001</ID></META_COMMUN>
<CONTEXTE><TEXTE/></CONTEXTE></ARTICLE>"#;
        assert!(matches!(parse_jorf_article(xml), Err(CoreError::Xml(_))));
    }

    // A real JORFCONT, JO n° 0235 of 8 October 2026, cut to two headings.
    const JO_0235: &str = r#"<JO>
<META><META_COMMUN><ID>JORFCONT000054960866</ID><ID_ELI>https://www.legifrance.gouv.fr/eli/jo/2026/10/8/0235</ID_ELI><NATURE>JO</NATURE></META_COMMUN>
<META_SPEC><META_CONTENEUR><TITRE>JORF n°0235 du 8 octobre 2026</TITRE><NUM>0235</NUM><DATE_PUBLI>2026-10-08</DATE_PUBLI></META_CONTENEUR></META_SPEC></META>
<STRUCTURE_TXT>
<TM niv="1"><TITRE_TM>Journal officiel "Lois et Décrets"</TITRE_TM>
<TM niv="2"><TITRE_TM>Décrets, arrêtés, circulaires</TITRE_TM>
<TM niv="3"><TITRE_TM>Textes généraux</TITRE_TM>
<TM niv="4"><TITRE_TM>Ministère de l'intérieur</TITRE_TM>
<LIEN_TXT idtxt="JORFTEXT000054960879" titretxt="Arrêté du 24 septembre 2026 modifiant l'arrêté du 4 février 2015"/>
<LIEN_TXT idtxt="JORFTEXT000054960887" titretxt="Décision du 5 octobre 2026 portant délégation de signature"/>
</TM></TM></TM>
<TM niv="2"><TITRE_TM>Annonces</TITRE_TM>
<LIEN_TXT idtxt="JORFTEXT000054961483" titretxt="Demandes de changement de nom (Accès protégé textes 58 à 78)"/>
</TM></TM>
</STRUCTURE_TXT>
</JO>"#;

    #[test]
    fn an_issue_keeps_its_headings_and_the_order_of_its_texts() {
        let issue = parse_jorf_conteneur(JO_0235.as_bytes()).expect("issue");
        assert_eq!(issue.issue_uid, "JORFCONT000054960866");
        assert_eq!(issue.num.as_deref(), Some("0235"));
        assert_eq!(issue.date_publi.as_deref(), Some("2026-10-08"));
        assert_eq!(
            issue.texts(),
            [
                "JORFTEXT000054960879",
                "JORFTEXT000054960887",
                "JORFTEXT000054961483"
            ]
        );
        let JorfTocNode::Heading { heading, items, .. } = &issue.toc[0] else {
            panic!("a heading first");
        };
        assert_eq!(heading, "Journal officiel \"Lois et Décrets\"");
        assert!(
            matches!(&items[1], JorfTocNode::Heading { heading, level: 2, .. } if heading == "Annonces")
        );
    }

    // A real TEXTE_VERSION, arrêté of 16 September 2026 (JORFTEXT000054960871),
    // cut; its note (1) sits inside SIGNATAIRES.
    const ARRETE_CYBER: &str = r#"<TEXTE_VERSION>
<META><META_COMMUN><ID>JORFTEXT000054960871</ID><ID_ELI>https://www.legifrance.gouv.fr/eli/arrete/2026/9/16/PRMI2623428A/jo/texte</ID_ELI><NATURE>ARRETE</NATURE></META_COMMUN>
<META_SPEC><META_TEXTE_CHRONICLE><CID>JORFTEXT000054960871</CID><NUM/><NUM_PARUTION>0235</NUM_PARUTION><NUM_SEQUENCE>1</NUM_SEQUENCE><NOR>PRMI2623428A</NOR><DATE_PUBLI>2026-10-08</DATE_PUBLI><DATE_TEXTE>2026-09-16</DATE_TEXTE><ORIGINE_PUBLI>JORF n°0235 du 8 octobre 2026</ORIGINE_PUBLI><PAGE_DEB_PUBLI/><PAGE_FIN_PUBLI/></META_TEXTE_CHRONICLE>
<META_TEXTE_VERSION><TITRE>Arrêté du 16 septembre 2026</TITRE><TITREFULL>Arrêté du 16 septembre 2026 relatif à l'approbation du cahier des charges</TITREFULL><AUTORITE/><MINISTERE>Premier ministre</MINISTERE><MCS_TXT/></META_TEXTE_VERSION></META_SPEC></META>
<NOTICE><CONTENU/></NOTICE>
<VISAS><CONTENU><p align="left"><br/>Le Premier ministre,<br/>Vu la loi n° 2010-237 du 9 mars 2010 ;<br/>Arrête :</p></CONTENU></VISAS>
<SIGNATAIRES><CONTENU><div align="left"><p>Fait le 16 septembre 2026.</p></div><div align="left"><p>B. BONNELL</p></div><p>(1) Le cahier des charges est consultable sur les sites internet de Bpifrance.</p></CONTENU></SIGNATAIRES>
<TP><CONTENU/></TP><ABRO><CONTENU/></ABRO><RECT><CONTENU/></RECT><SM><CONTENU/></SM>
<ENTREPRISE texte_entreprise="oui"><DATES_EFFET><DATE_EFFET>2016-08-12</DATE_EFFET></DATES_EFFET><DOMAINES><DOMAINE>Economie et finances</DOMAINE></DOMAINES></ENTREPRISE>
</TEXTE_VERSION>"#;

    #[test]
    fn a_publication_keeps_its_place_in_the_issue_and_its_blocks() {
        let p = parse_jorf_publication(ARRETE_CYBER.as_bytes()).expect("publication");
        assert_eq!(p.nature.as_deref(), Some("ARRETE"));
        assert_eq!(p.num_parution.as_deref(), Some("0235"));
        assert_eq!(p.num_sequence, Some(1));
        assert_eq!(p.page_deb, None);
        assert_eq!(p.ministere.as_deref(), Some("Premier ministre"));
        assert_eq!(p.autorite, None);
        assert_eq!(p.title.as_deref(), Some("Arrêté du 16 septembre 2026"));
        assert_eq!(
            p.blocks.keys().map(String::as_str).collect::<Vec<_>>(),
            ["signataires", "visas"]
        );
        assert_eq!(
            p.blocks["visas"],
            "Le Premier ministre,\nVu la loi n° 2010-237 du 9 mars 2010 ;\nArrête :"
        );
        assert!(p.blocks["signataires"].ends_with(
            "(1) Le cahier des charges est consultable sur les sites internet de Bpifrance."
        ));
        assert_eq!(
            p.enterprise,
            Some(JorfEnterprise {
                dates: vec!["2016-08-12".into()],
                domains: vec!["Economie et finances".into()],
            })
        );
    }

    #[test]
    fn a_section_holds_its_subsections_and_articles_in_order() {
        // A real SECTION_TA (JORFSCTA000054297716), one subsection and one article kept.
        let xml = r#"<SECTION_TA><ID>JORFSCTA000054297716</ID><TITRE_TA>Titre 1ER : DISPOSITIONS GÉNÉRALES</TITRE_TA><COMMENTAIRE/>
<STRUCTURE_TA>
<LIEN_SECTION_TA cid="JORFSCTA000054297718" id="JORFSCTA000054297718" niv="3">Chapitre A Règles générales</LIEN_SECTION_TA>
<LIEN_ART id="JORFARTI000054297730" num=""/>
</STRUCTURE_TA>
<CONTEXTE><TEXTE cid="JORFTEXT000054297670" nature="ARRETE"><TITRE_TXT>Arrêté du 8 juin 2026</TITRE_TXT></TEXTE></CONTEXTE>
</SECTION_TA>"#;
        let s = parse_jorf_section(xml.as_bytes()).expect("section");
        assert_eq!(s.jorftext, "JORFTEXT000054297670");
        assert_eq!(
            s.body.title.as_deref(),
            Some("Titre 1ER : DISPOSITIONS GÉNÉRALES")
        );
        assert_eq!(s.body.comment, None);
        assert_eq!(
            serde_json::to_value(&s.body.items).unwrap(),
            serde_json::json!([
                {"section": "JORFSCTA000054297718", "title": "Chapitre A Règles générales", "level": 3},
                {"article": "JORFARTI000054297730"}
            ])
        );
        let back: Vec<JorfItem> =
            serde_json::from_value(serde_json::to_value(&s.body.items).unwrap()).unwrap();
        assert_eq!(back, s.body.items);
    }
}
