//! Parser PUR du fond LEGI (bulk DILA) — `payload XML → LegiArticle / LegiCode`.
//! Aucun I/O : les octets sont fournis par `lj-ingest`. Réutilise l'arbre XML
//! minimal de [`lj_parse`] (`build_tree`/`XmlNode`), donc le même tolérant
//! `quick-xml` que le parser opendata.
//!
//! Les sentinelles de date (`2999-01-01` = pas de fin, `2222-02-22` = version
//! future à date inconnue) sont normalisées en `None` ici — frontière de parsing
//! unique.

use lj_core::error::CoreError;
use lj_core::normalizer::clean_texte;
use lj_parse::{build_tree, node_text, XmlNode};

/// Sentinelles `DATE_FIN` LEGI → « pas de date » : `2999-01-01` (pas de fin /
/// en vigueur) et `2222-02-22` (version future programmée, date inconnue).
const DATE_SENTINELS: &[&str] = &["2999-01-01", "2222-02-22"];

/// Une version d'article du fond LEGI (`LEGIARTI*.xml`, racine `<ARTICLE>`).
/// Dates en `String` ISO (`YYYY-MM-DD`) ; la conversion en type date se fait au
/// bord store. Même asymétrie que [`LegiTocEdge`] : `date_fin` absorbe les
/// sentinelles (`None` = pas de fin), `date_debut` les garde **brutes** — sur
/// `date_debut` la sentinelle ne dit pas « début inconnu » mais « pas de
/// fenêtre de validité », et le filtrage daté l'exclut naturellement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegiArticle {
    pub legiarti: String,
    pub legitext: String,
    pub num: String,
    pub num_key: String,
    pub titre_text: Option<String>,
    pub etat: String,
    pub date_debut: String,
    pub date_fin: Option<String>,
    pub texte: Option<String>,
    pub nota: Option<String>,
    pub liens: Vec<LegiLien>,
    /// The chronicle DILA declares for this article (`VERSIONS/VERSION/LIEN_ART`),
    /// every version of the number and not only the one this file carries: the
    /// only completeness test the bulk offers, what we hold against what the
    /// source announces.
    pub versions: Vec<LegiVersionRef>,
}

/// One version announced by an article's chronicle. `date_fin` absorbs the
/// sentinels like [`LegiArticle`], `None` meaning no end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegiVersionRef {
    pub legiarti: String,
    pub etat: String,
    pub date_debut: Option<String>,
    pub date_fin: Option<String>,
}

/// Un texte/code du fond LEGI (`LEGITEXT*.xml`, racine `<TEXTE_VERSION>`).
/// `versions_a_venir` = dates programmées de versions futures du texte
/// (`META_TEXTE_CHRONICLE/VERSIONS_A_VENIR`) — la sentinelle `2222-02-22` (date
/// inconnue) est conservée, rendue « à déterminer ».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegiCode {
    /// **CID chronique** (`META_TEXTE_CHRONICLE/CID`) — l'identité stable du
    /// texte, celle que portent aussi les articles (`CONTEXTE/TEXTE@cid`) et
    /// la TOC. Depuis que la DILA versionne les TNC, `META_COMMUN/ID` est un
    /// id de **version** distinct : s'ancrer dessus fend le texte en deux,
    /// fiche-coquille d'un côté et articles orphelins de l'autre.
    pub legitext: String,
    pub titre: String,
    /// `TITREFULL` descriptif quand il diffère du `TITRE` court (TNC : le
    /// TITRE d'un arrêté est nu « Arrêté du 7 juillet 2026 »).
    pub titre_full: Option<String>,
    pub nature: String,
    /// NOR (`META_COMMUN/NOR`) — identité cross-diffuseur, clé de résolution du
    /// linker pour les actes datés (arrêtés, décrets).
    pub nor: Option<String>,
    /// `META_TEXTE_CHRONICLE/DATE_TEXTE` and `DATE_PUBLI`, ISO, raw: a code
    /// carries the 2999-01-01 sentinel.
    pub date_texte: Option<String>,
    pub date_publi: Option<String>,
    pub derniere_modification: Option<String>,
    pub versions_a_venir: Vec<String>,
    pub liens: Vec<LegiLien>,
}

/// Grain de la cible d'un lien, dérivé du préfixe d'ID DILA (`*ARTI`/`*SCTA`)
/// sinon de la présence d'un `num` d'article.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LienTargetKind {
    Article,
    Section,
    Texte,
}

impl LienTargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LienTargetKind::Article => "article",
            LienTargetKind::Section => "section",
            LienTargetKind::Texte => "texte",
        }
    }
}

/// Un `<LIEN>` du bloc `<LIENS>` DILA : `typelien` brut + famille `verb`
/// normalisée + `direction` décodée du couple `(typelien, sens)`, cible en clé
/// pendante (IDs DILA, résolution au read-time). Attributs vides absorbés en
/// `None` à la frontière.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegiLien {
    pub typelien: String,
    pub verb: String,
    /// `outgoing` (le propriétaire agit sur la cible) ou `incoming` (il subit).
    pub direction: &'static str,
    pub target_kind: LienTargetKind,
    pub target_uid: Option<String>,
    pub target_text_uid: Option<String>,
    pub target_num: Option<String>,
    pub target_nature: Option<String>,
    pub target_label: String,
    pub target_date: Option<String>,
    pub target_nor: Option<String>,
}

/// Nature d'un enfant dans l'arbre structurel d'un texte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TocChildKind {
    Article,
    Section,
}

impl TocChildKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TocChildKind::Article => "article",
            TocChildKind::Section => "section",
        }
    }
}

/// Une arête de l'arbre structurel : un enfant (`LIEN_ART` ou
/// `LIEN_SECTION_TA`) tel que listé par son propriétaire, avec sa fenêtre de
/// validité. `date_debut` est gardée **brute** (y compris les sentinelles
/// `2999`/`2222` : une version jamais entrée en vigueur reste exclue de tout
/// filtrage daté) ; `date_fin` absorbe les sentinelles (`None` = pas de fin).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegiTocEdge {
    pub child_kind: TocChildKind,
    pub child_uid: String,
    /// Sections : id chronique stable inter-versions (l'ancre du sommaire).
    pub child_cid: Option<String>,
    /// Articles : clé d'identité `identity_key(num)`.
    pub child_num_key: Option<String>,
    /// Num d'article ou titre de section.
    pub label: String,
    pub etat: String,
    pub date_debut: Option<String>,
    pub date_fin: Option<String>,
    pub niv: Option<i32>,
}

/// L'arbre structurel porté par un fichier `texte/struct` (`TEXTELR`) ou
/// `section_ta` (`SECTION_TA`) : le propriétaire et ses enfants directs, dans
/// l'ordre du fichier, qui est l'ordre de lecture réel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegiToc {
    pub owner_uid: String,
    pub text_uid: String,
    pub edges: Vec<LegiTocEdge>,
}

/// Famille normalisée d'un `typelien` DILA : le couple verbe/nom (MODIFIE /
/// MODIFICATION…) se replie sur une seule famille — la voix ne porte pas
/// l'action mais l'orientation, décodée à part par [`lien_direction`].
/// `typelien` inconnu → minuscules tel quel (DILA peut étendre la liste ; la
/// valeur brute reste stockée à côté). Public : le pipeline JORF mappe les mêmes
/// `typelien` sur ses liens texte-niveau.
pub fn lien_verb(typelien: &str) -> String {
    match typelien {
        "CITATION" => "cite",
        "MODIFIE" | "MODIFICATION" => "modifie",
        "CREE" | "CREATION" => "cree",
        "ABROGE" | "ABROGATION" => "abroge",
        "CODIFICATION" | "CODIFIE" => "codifie",
        "CONCORDE" | "CONCORDANCE" => "concorde",
        "TRANSFERE" | "TRANSFERT" => "transfere",
        "DEPLACE" | "DEPLACEMENT" => "deplace",
        "RECTIFICATION" | "RECTIFIE" => "rectifie",
        "PERIME" | "PEREMPTION" => "perime",
        "RATIFIE" | "RATIFICATION" => "ratifie",
        "DENONCE" | "DENONCIATION" => "denonce",
        "ANNULE" | "ANNULATION" => "annule",
        "DISJOINT" | "DISJONCTION" => "disjoint",
        "ETEND" | "EXTENSION" => "etend",
        "TRANSPOSITION" => "transpose",
        "APPLICATION" => "applique",
        "SPEC_APPLI" => "spec_appli",
        "TXT_SOURCE" => "txt_source",
        "TXT_ASSOCIE" => "txt_associe",
        "PILOTE_SUIVEUR" => "pilote_suiveur",
        "HISTO" => "histo",
        other => return other.to_ascii_lowercase(),
    }
    .to_string()
}

/// Direction d'un lien, décodée du couple `(typelien, sens)`.
///
/// Le vocabulaire `typelien` de DILA vient en paires **verbe/nom** (MODIFIE /
/// MODIFICATION, ABROGE / ABROGATION…) et `sens` ne se lit qu'avec la voix :
///
/// - forme **verbale** — `sens` donne le rôle du **propriétaire** :
///   `source` = il agit (sortant), `cible` = il subit (entrant) ;
/// - forme **nominale** — `sens` donne le rôle de la **cible** :
///   `cible` = le propriétaire agit sur elle (sortant), `source` = elle agit
///   sur lui (entrant).
///
/// Un `typelien` hors vocabulaire suit la règle nominale (les singletons du stock
/// — APPLICATION, TXT_SOURCE… — sont nominaux ou structurels), le brut restant
/// stocké à côté ; un `sens` hors `{source, cible}` est une erreur franche.
///
/// **CITATION suit la règle verbale malgré sa morphologie** : le fichier du
/// **citant** porte `sens=source` (il agit), celui du **cité** porte `sens=cible`.
pub fn lien_direction(typelien: &str, sens: &str) -> Result<&'static str, CoreError> {
    let verbal = matches!(
        typelien,
        "MODIFIE"
            | "CREE"
            | "ABROGE"
            | "CODIFIE"
            | "CONCORDE"
            | "TRANSFERE"
            | "DEPLACE"
            | "RECTIFIE"
            | "PERIME"
            | "RATIFIE"
            | "DENONCE"
            | "ANNULE"
            | "DISJOINT"
            | "ETEND"
            | "COMPLETE"
            | "RENVOIT"
            | "PUBLIE"
            | "CITATION"
    );
    match sens {
        "source" => Ok(if verbal { "outgoing" } else { "incoming" }),
        "cible" => Ok(if verbal { "incoming" } else { "outgoing" }),
        other => Err(CoreError::Xml(format!(
            "LIEN typelien={typelien}: sens inconnu «{other}»"
        ))),
    }
}

fn lien_target_kind(uid: Option<&str>, num: Option<&str>) -> LienTargetKind {
    match uid {
        Some(id) if id.contains("ARTI") => LienTargetKind::Article,
        Some(id) if id.contains("SCTA") => LienTargetKind::Section,
        Some(_) => LienTargetKind::Texte,
        None if num.is_some() => LienTargetKind::Article,
        None => LienTargetKind::Texte,
    }
}

/// Collecte le bloc `<LIENS>` (premier en ordre document : top-level dans
/// `ARTICLE`, sous `META_TEXTE_VERSION` dans `TEXTE_VERSION`) en
/// [`LegiLien`]s, dans l'ordre du fichier. La direction sort du couple
/// `(typelien, sens)` ([`lien_direction`]). `pub(crate)` : partagé avec le parser
/// KALI, qui lit le même bloc.
pub(crate) fn collect_liens(root: &XmlNode) -> Result<Vec<LegiLien>, CoreError> {
    let Some(liens) = find_anywhere(root, "LIENS") else {
        return Ok(Vec::new());
    };
    liens
        .children
        .iter()
        .filter(|c| c.tag == "LIEN")
        .map(|l| {
            let attr = |k: &str| l.attr(k).map(str::to_string).filter(|v| !v.is_empty());
            let typelien = attr("typelien").unwrap_or_default();
            let target_uid = attr("id");
            let target_num = attr("num");
            Ok(LegiLien {
                verb: lien_verb(&typelien),
                direction: lien_direction(&typelien, l.attr("sens").unwrap_or_default())?,
                target_kind: lien_target_kind(target_uid.as_deref(), target_num.as_deref()),
                typelien,
                target_text_uid: attr("cidtexte"),
                target_nature: attr("naturetexte"),
                target_label: l.text().map(|t| t.trim().to_string()).unwrap_or_default(),
                target_date: date_or_none(attr("datesignatexte")),
                target_nor: attr("nortexte"),
                target_uid,
                target_num,
            })
        })
        .collect()
}

/// Sentinelle LEGI → `None` ; sinon la date telle quelle (ISO, conversion au
/// bord store). `pub(crate)` : partagé avec le parser JORF, mêmes sentinelles de
/// date.
pub(crate) fn date_or_none(raw: Option<String>) -> Option<String> {
    raw.filter(|d| !d.is_empty() && !DATE_SENTINELS.contains(&d.as_str()))
}

/// `clean_texte` d'un `CONTENU`, `None` si vide après nettoyage. `pub(crate)` :
/// partagé avec le parser JORF (même `BLOC_TEXTUEL/CONTENU`).
pub(crate) fn clean_contenu(node: Option<&XmlNode>) -> Option<String> {
    node.and_then(XmlNode::html_text)
        .map(|raw| clean_texte(&raw))
        .filter(|t| !t.is_empty())
}

/// Résout un chemin `A/B/C` dont le **premier** segment est cherché n'importe
/// où dans l'arbre (les conteneurs LEGI `META_COMMUN`/`META_ARTICLE`/`CONTEXTE`/
/// `BLOC_TEXTUEL` sont imbriqués sous `<META>`/`<META_SPEC>`, profondeur variable
/// selon ARTICLE/TEXTE_VERSION) ; les segments suivants suivent les enfants
/// directs comme [`XmlNode::find`]. Premier match en ordre document. `pub(crate)`
/// : partagé avec le parser JORF, de structure XML identique.
pub(crate) fn find_anywhere<'a>(root: &'a XmlNode, path: &str) -> Option<&'a XmlNode> {
    let (head, tail) = match path.split_once('/') {
        Some((h, t)) => (h, Some(t)),
        None => (path, None),
    };
    fn locate<'a>(node: &'a XmlNode, tag: &str) -> Option<&'a XmlNode> {
        if node.tag == tag {
            return Some(node);
        }
        node.children.iter().find_map(|c| locate(c, tag))
    }
    let anchor = locate(root, head)?;
    match tail {
        Some(rest) => anchor.find(rest),
        None => Some(anchor),
    }
}

/// Fil d'Ariane structurel : concatène tous les `TITRE_TM` du sous-arbre `node`
/// (Livre/Titre/Chapitre/Section imbriqués), dans l'ordre document, joints par
/// ` > `. `None` si aucun. `pub(crate)` : partagé avec le parser JORF.
pub(crate) fn collect_titre_tm(node: &XmlNode) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    fn walk(node: &XmlNode, parts: &mut Vec<String>) {
        for child in &node.children {
            if child.tag == "TITRE_TM" {
                if let Some(t) = child.text() {
                    parts.push(t);
                }
            }
            walk(child, parts);
        }
    }
    walk(node, &mut parts);
    (!parts.is_empty()).then(|| parts.join(" > "))
}

/// Parse un `LEGIARTI*.xml` (racine `<ARTICLE>`) en [`LegiArticle`]. Erreur
/// franche ([`CoreError::Xml`]) si `ID`/`DATE_DEBUT`/`cid` manquent — la
/// frontière de validation source. `NUM` peut être vide (`<NUM/>`) : forme DILA
/// légitime des TNC — annexes, placeholders d'images, visas/formules/signatures,
/// et quelques vieilles versions au numéro resté dans la prose. Le pipeline les
/// écarte : sans num_key ils ne sont ni citables ni servables.
pub fn parse_legi_article(raw: &[u8]) -> Result<LegiArticle, CoreError> {
    let root = build_tree(raw).ok_or_else(|| CoreError::Xml("ARTICLE: XML illisible".into()))?;

    let legiarti = node_text(find_anywhere(&root, "META_COMMUN/ID"))
        .ok_or_else(|| CoreError::Xml("ARTICLE: META_COMMUN/ID manquant".into()))?;
    let num = node_text(find_anywhere(&root, "META_ARTICLE/NUM")).unwrap_or_default();
    // Présence obligatoire, valeur gardée brute : la sentinelle sur `DATE_DEBUT`
    // marque un article qui n'a pas de fenêtre de validité — pour l'essentiel des
    // dispositifs modificatifs consommés (« a modifié les dispositions suivantes »),
    // auxquels DILA ne donne pas d'`ETAT` non plus.
    let date_debut =
        node_text(find_anywhere(&root, "META_ARTICLE/DATE_DEBUT")).ok_or_else(|| {
            CoreError::Xml(format!(
                "ARTICLE {legiarti}: META_ARTICLE/DATE_DEBUT manquant"
            ))
        })?;
    let legitext = find_anywhere(&root, "CONTEXTE/TEXTE")
        .and_then(|t| t.attr("cid"))
        .ok_or_else(|| CoreError::Xml(format!("ARTICLE {legiarti}: CONTEXTE/TEXTE@cid manquant")))?
        .to_string();

    let etat = node_text(find_anywhere(&root, "META_ARTICLE/ETAT")).unwrap_or_default();
    let date_fin = date_or_none(node_text(find_anywhere(&root, "META_ARTICLE/DATE_FIN")));
    let titre_text = find_anywhere(&root, "CONTEXTE").and_then(collect_titre_tm);
    let num_key = lj_norm::article_key::identity_key(&num);

    Ok(LegiArticle {
        legiarti,
        legitext,
        num,
        num_key,
        titre_text,
        etat,
        date_debut,
        date_fin,
        texte: clean_contenu(find_anywhere(&root, "BLOC_TEXTUEL/CONTENU")),
        nota: clean_contenu(find_anywhere(&root, "NOTA/CONTENU")),
        liens: collect_liens(&root)?,
        versions: collect_versions(&root),
    })
}

/// The `VERSIONS` block, flattened: one entry per `LIEN_ART`, in document order.
/// An entry without `id` is skipped, having nothing to compare a held row to.
fn collect_versions(root: &XmlNode) -> Vec<LegiVersionRef> {
    let Some(bloc) = find_anywhere(root, "VERSIONS") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for version in &bloc.children {
        for lien in &version.children {
            if lien.tag != "LIEN_ART" {
                continue;
            }
            let attr = |k: &str| lien.attr(k).filter(|v| !v.is_empty()).map(str::to_string);
            let Some(legiarti) = attr("id") else { continue };
            out.push(LegiVersionRef {
                legiarti,
                etat: lien.attr("etat").unwrap_or_default().to_string(),
                date_debut: attr("debut"),
                date_fin: date_or_none(attr("fin")),
            });
        }
    }
    out
}

/// Collecte les enfants directs d'un conteneur `STRUCT`/`STRUCTURE_TA` en
/// [`LegiTocEdge`]s, dans l'ordre document. Un `LIEN_ART` sans `id` ou sans
/// `num` est **sauté** (article non numéroté, jamais ingéré) ; un
/// `LIEN_SECTION_TA` sans `id`/`cid` est une erreur franche : la structure serait
/// inexploitable.
fn collect_toc_edges(container: &XmlNode) -> Result<Vec<LegiTocEdge>, CoreError> {
    let mut edges = Vec::new();
    for c in &container.children {
        let attr = |k: &str| c.attr(k).filter(|v| !v.is_empty()).map(str::to_string);
        let etat = c.attr("etat").unwrap_or_default().to_string();
        let date_debut = attr("debut");
        let date_fin = date_or_none(attr("fin"));
        match c.tag.as_str() {
            "LIEN_ART" => {
                let (Some(id), Some(num)) = (attr("id"), attr("num")) else {
                    continue;
                };
                edges.push(LegiTocEdge {
                    child_kind: TocChildKind::Article,
                    child_uid: id,
                    child_cid: None,
                    child_num_key: Some(lj_norm::article_key::identity_key(&num)),
                    label: num,
                    etat,
                    date_debut,
                    date_fin,
                    niv: None,
                });
            }
            "LIEN_SECTION_TA" => {
                let req = |k: &str| {
                    attr(k).ok_or_else(|| CoreError::Xml(format!("LIEN_SECTION_TA: @{k} manquant")))
                };
                edges.push(LegiTocEdge {
                    child_kind: TocChildKind::Section,
                    child_uid: req("id")?,
                    child_cid: Some(req("cid")?),
                    child_num_key: None,
                    label: c.text().unwrap_or_default(),
                    etat,
                    date_debut,
                    date_fin,
                    niv: c.attr("niv").and_then(|n| n.parse().ok()),
                });
            }
            _ => {}
        }
    }
    Ok(edges)
}

/// Parse un `texte/struct/LEGITEXT*.xml` (racine `<TEXTELR>`) en [`LegiToc`] :
/// le premier niveau de l'arbre d'un texte. Le propriétaire est le **cid
/// chronique** (`META_TEXTE_CHRONICLE/CID` — celui que portent les articles en
/// `text_uid`), pas l'ID de version du fichier : les fichiers struct des
/// différentes versions d'un texte portent le même arbre daté complet et se
/// remplacent.
pub fn parse_legi_textelr(raw: &[u8]) -> Result<LegiToc, CoreError> {
    let root = build_tree(raw).ok_or_else(|| CoreError::Xml("TEXTELR: XML illisible".into()))?;
    let cid = node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/CID"))
        .ok_or_else(|| CoreError::Xml("TEXTELR: META_TEXTE_CHRONICLE/CID manquant".into()))?;
    let edges = match find_anywhere(&root, "STRUCT") {
        Some(s) => {
            collect_toc_edges(s).map_err(|e| CoreError::Xml(format!("TEXTELR {cid}: {e}")))?
        }
        None => Vec::new(),
    };
    Ok(LegiToc {
        owner_uid: cid.clone(),
        text_uid: cid,
        edges,
    })
}

/// Parse un `section_ta/LEGISCTA*.xml` (racine `<SECTION_TA>`) en [`LegiToc`] :
/// les enfants d'une **version** de section (le propriétaire est l'`ID` de
/// version — la jointure de la CTE se fait sur lui ; le `cid` stable vit sur
/// l'arête parente). `text_uid` = `CONTEXTE/TEXTE@cid` du texte porteur.
pub fn parse_legi_section_ta(raw: &[u8]) -> Result<LegiToc, CoreError> {
    let root = build_tree(raw).ok_or_else(|| CoreError::Xml("SECTION_TA: XML illisible".into()))?;
    let owner_uid = node_text(root.find("ID"))
        .ok_or_else(|| CoreError::Xml("SECTION_TA: ID manquant".into()))?;
    let text_uid = find_anywhere(&root, "CONTEXTE/TEXTE")
        .and_then(|t| t.attr("cid"))
        .ok_or_else(|| {
            CoreError::Xml(format!(
                "SECTION_TA {owner_uid}: CONTEXTE/TEXTE@cid manquant"
            ))
        })?
        .to_string();
    let edges = match find_anywhere(&root, "STRUCTURE_TA") {
        Some(s) => collect_toc_edges(s)
            .map_err(|e| CoreError::Xml(format!("SECTION_TA {owner_uid}: {e}")))?,
        None => Vec::new(),
    };
    Ok(LegiToc {
        owner_uid,
        text_uid,
        edges,
    })
}

/// Parse un `LEGITEXT*.xml` (racine `<TEXTE_VERSION>`) en [`LegiCode`],
/// **ancré sur le CID chronique** — même identité que les articles et la TOC ;
/// pour un code, CID = ID, pour un TNC versionné le CID est le JORFTEXT stable.
/// Erreur franche ([`CoreError::Xml`]) si `META_TEXTE_CHRONICLE/CID` ou
/// `META_TEXTE_VERSION/TITRE` manquent.
pub fn parse_legi_texte(raw: &[u8]) -> Result<LegiCode, CoreError> {
    let root =
        build_tree(raw).ok_or_else(|| CoreError::Xml("TEXTE_VERSION: XML illisible".into()))?;

    let legitext = node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/CID"))
        .ok_or_else(|| CoreError::Xml("TEXTE_VERSION: META_TEXTE_CHRONICLE/CID manquant".into()))?;
    let titre = node_text(find_anywhere(&root, "META_TEXTE_VERSION/TITRE")).ok_or_else(|| {
        CoreError::Xml(format!(
            "TEXTE_VERSION {legitext}: META_TEXTE_VERSION/TITRE manquant"
        ))
    })?;
    let titre_full = node_text(find_anywhere(&root, "META_TEXTE_VERSION/TITREFULL"))
        .filter(|t| !t.is_empty() && t != &titre);
    let nature = node_text(find_anywhere(&root, "META_COMMUN/NATURE")).unwrap_or_default();
    // LEGI comme JORF placent le NOR sous META_TEXTE_CHRONICLE ; repli
    // META_COMMUN pour les membres qui l'y portent.
    let nor = node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/NOR"))
        .or_else(|| node_text(find_anywhere(&root, "META_COMMUN/NOR")))
        .filter(|n| !n.is_empty());
    let date_texte = node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/DATE_TEXTE"))
        .filter(|d| !d.is_empty());
    let date_publi = node_text(find_anywhere(&root, "META_TEXTE_CHRONICLE/DATE_PUBLI"))
        .filter(|d| !d.is_empty());
    let derniere_modification = node_text(find_anywhere(
        &root,
        "META_TEXTE_CHRONICLE/DERNIERE_MODIFICATION",
    ));
    let versions_a_venir = find_anywhere(&root, "VERSIONS_A_VENIR")
        .map(|n| {
            n.children
                .iter()
                .filter(|c| c.tag == "VERSION_A_VENIR")
                .filter_map(XmlNode::text)
                .collect()
        })
        .unwrap_or_default();
    Ok(LegiCode {
        legitext,
        titre,
        titre_full,
        nature,
        nor,
        date_texte,
        date_publi,
        derniere_modification,
        versions_a_venir,
        liens: collect_liens(&root)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARTICLE_MODIFIE: &str = r#"<ARTICLE>
  <META>
    <META_COMMUN><ID>LEGIARTI000006832947</ID><NATURE>Article</NATURE></META_COMMUN>
    <META_SPEC><META_ARTICLE>
      <NUM>L131-4</NUM><ETAT>MODIFIE</ETAT>
      <DATE_DEBUT>2004-07-02</DATE_DEBUT><DATE_FIN>2018-08-06</DATE_FIN>
    </META_ARTICLE></META_SPEC>
  </META>
  <CONTEXTE>
    <TEXTE cid="LEGITEXT000006074220">
      <TM><TITRE_TM>Partie législative</TITRE_TM>
        <TM><TITRE_TM>Livre Ier</TITRE_TM>
          <TM><TITRE_TM>Titre III</TITRE_TM></TM>
        </TM>
      </TM>
    </TEXTE>
  </CONTEXTE>
  <BLOC_TEXTUEL><CONTENU>Le premier alinéa.&lt;br/&gt;Le second alinéa.</CONTENU></BLOC_TEXTUEL>
  <NOTA><CONTENU></CONTENU></NOTA>
</ARTICLE>"#;

    #[test]
    fn parse_article_maps_full_structure() {
        let a = parse_legi_article(ARTICLE_MODIFIE.as_bytes()).expect("article");
        assert_eq!(a.legiarti, "LEGIARTI000006832947");
        assert_eq!(a.legitext, "LEGITEXT000006074220");
        assert_eq!(a.num, "L131-4");
        assert_eq!(a.etat, "MODIFIE");
        assert_eq!(a.date_debut, "2004-07-02");
        assert_eq!(a.date_fin.as_deref(), Some("2018-08-06"));
        assert_eq!(
            a.titre_text.as_deref(),
            Some("Partie législative > Livre Ier > Titre III")
        );
        // CONTENU porte du HTML léger entity-escapé (&lt;br/&gt;) : build_tree décode
        // l'entité → texte « <br/> », que clean_texte mue en saut de ligne.
        assert_eq!(
            a.texte.as_deref(),
            Some("Le premier alinéa.\nLe second alinéa.")
        );
        assert_eq!(a.nota, None);
    }

    /// The chronicle is what the source announces, so the sentinel end is absorbed
    /// like everywhere else and an entry without `id` is dropped: it could not be
    /// compared to a held row.
    #[test]
    fn parse_article_collects_the_declared_chronicle() {
        let xml = br#"<ARTICLE>
  <META_COMMUN><ID>LEGIARTI000006746513</ID></META_COMMUN>
  <META_ARTICLE><NUM>R142-9</NUM><ETAT>MODIFIE</ETAT>
    <DATE_DEBUT>1996-09-11</DATE_DEBUT><DATE_FIN>2005-10-01</DATE_FIN></META_ARTICLE>
  <CONTEXTE><TEXTE cid="LEGITEXT000006073189"/></CONTEXTE>
  <VERSIONS>
    <VERSION etat="MODIFIE">
      <LIEN_ART debut="1985-12-21" etat="MODIFIE" fin="1996-09-11" id="LEGIARTI000006746512" num="R142-9"/>
    </VERSION>
    <VERSION etat="MODIFIE">
      <LIEN_ART debut="1996-09-11" etat="MODIFIE" fin="2005-10-01" id="LEGIARTI000006746513" num="R142-9"/>
    </VERSION>
    <VERSION etat="VIGUEUR">
      <LIEN_ART debut="2020-01-01" etat="VIGUEUR" fin="2999-01-01" id="LEGIARTI000041404184" num="R142-9"/>
    </VERSION>
    <VERSION etat="MODIFIE"><LIEN_ART debut="2019-01-01" etat="MODIFIE" num="R142-9"/></VERSION>
  </VERSIONS>
</ARTICLE>"#;
        let a = parse_legi_article(xml).expect("article");
        assert_eq!(a.versions.len(), 3);
        assert_eq!(a.versions[0].legiarti, "LEGIARTI000006746512");
        assert_eq!(a.versions[0].date_fin.as_deref(), Some("1996-09-11"));
        assert_eq!(a.versions[2].etat, "VIGUEUR");
        assert_eq!(a.versions[2].date_fin, None);

        let sans_bloc = parse_legi_article(ARTICLE_MODIFIE.as_bytes()).expect("article");
        assert!(sans_bloc.versions.is_empty());
    }

    #[test]
    fn parse_article_num_key_is_normalized() {
        let xml = br#"<ARTICLE>
  <META_COMMUN><ID>LEGIARTI000000000001</ID></META_COMMUN>
  <META_ARTICLE><NUM>Art. L. 131-4</NUM><ETAT>VIGUEUR</ETAT>
    <DATE_DEBUT>2020-01-01</DATE_DEBUT></META_ARTICLE>
  <CONTEXTE><TEXTE cid="LEGITEXT000006074220"/></CONTEXTE>
</ARTICLE>"#;
        let a = parse_legi_article(xml).expect("article");
        assert_eq!(a.num, "Art. L. 131-4");
        assert_eq!(a.num_key, "art-l-131-4");
    }

    #[test]
    fn date_fin_sentinel_2999_becomes_none() {
        let xml = br#"<ARTICLE>
  <META_COMMUN><ID>LEGIARTI000000000002</ID></META_COMMUN>
  <META_ARTICLE><NUM>1240</NUM><ETAT>VIGUEUR</ETAT>
    <DATE_DEBUT>2016-10-01</DATE_DEBUT><DATE_FIN>2999-01-01</DATE_FIN></META_ARTICLE>
  <CONTEXTE><TEXTE cid="LEGITEXT000006070721"/></CONTEXTE>
</ARTICLE>"#;
        let a = parse_legi_article(xml).expect("article");
        assert_eq!(a.date_fin, None);
    }

    #[test]
    fn date_fin_sentinel_2222_becomes_none() {
        let xml = br#"<ARTICLE>
  <META_COMMUN><ID>LEGIARTI000000000003</ID></META_COMMUN>
  <META_ARTICLE><NUM>L822-1</NUM><ETAT>VIGUEUR</ETAT>
    <DATE_DEBUT>2025-01-01</DATE_DEBUT><DATE_FIN>2222-02-22</DATE_FIN></META_ARTICLE>
  <CONTEXTE><TEXTE cid="LEGITEXT000006070721"/></CONTEXTE>
</ARTICLE>"#;
        let a = parse_legi_article(xml).expect("article");
        assert_eq!(a.date_fin, None);
    }

    #[test]
    fn date_debut_sentinel_is_kept_raw() {
        // Dispositif modificatif consommé : la DILA pose DATE_DEBUT=2999-01-01 et pas
        // d'ETAT. Ce n'est pas un début inconnu (elle a `0001-01-01` pour ça) mais une
        // absence de fenêtre de validité : la ligne existe pour porter son arête
        // `MODIFIE`, pas pour être en vigueur.
        let xml = br#"<ARTICLE>
  <META_COMMUN><ID>LEGIARTI000000000006</ID></META_COMMUN>
  <META_ARTICLE><NUM>2</NUM><DATE_DEBUT>2999-01-01</DATE_DEBUT>
    <DATE_FIN>2999-01-01</DATE_FIN></META_ARTICLE>
  <CONTEXTE><TEXTE cid="JORFTEXT000000000001"/></CONTEXTE>
</ARTICLE>"#;
        let a = parse_legi_article(xml).expect("article");
        assert_eq!(a.date_debut, "2999-01-01");
        assert_eq!(a.date_fin, None);
    }

    #[test]
    fn article_empty_num_parses_with_empty_key() {
        // Forme DILA légitime des TNC (annexes, placeholders, visas) : `<NUM/>` vide
        // n'est pas une erreur, le pipeline écarte la ligne sur `num_key` vide.
        let xml = br#"<ARTICLE>
  <META_COMMUN><ID>LEGIARTI000000000007</ID></META_COMMUN>
  <META_ARTICLE><NUM/><ETAT>ABROGE</ETAT>
    <DATE_DEBUT>1992-01-02</DATE_DEBUT><DATE_FIN>2020-09-01</DATE_FIN></META_ARTICLE>
  <CONTEXTE><TEXTE cid="JORFTEXT000000000002"/></CONTEXTE>
</ARTICLE>"#;
        let a = parse_legi_article(xml).expect("article");
        assert_eq!(a.num, "");
        assert_eq!(a.num_key, "");
    }

    #[test]
    fn article_missing_id_is_hard_error() {
        let xml = br#"<ARTICLE>
  <META_ARTICLE><NUM>1240</NUM><DATE_DEBUT>2016-10-01</DATE_DEBUT></META_ARTICLE>
  <CONTEXTE><TEXTE cid="LEGITEXT000006070721"/></CONTEXTE>
</ARTICLE>"#;
        assert!(matches!(parse_legi_article(xml), Err(CoreError::Xml(_))));
    }

    #[test]
    fn article_missing_cid_is_hard_error() {
        let xml = br#"<ARTICLE>
  <META_COMMUN><ID>LEGIARTI000000000004</ID></META_COMMUN>
  <META_ARTICLE><NUM>1240</NUM><DATE_DEBUT>2016-10-01</DATE_DEBUT></META_ARTICLE>
  <CONTEXTE><TEXTE/></CONTEXTE>
</ARTICLE>"#;
        assert!(matches!(parse_legi_article(xml), Err(CoreError::Xml(_))));
    }

    #[test]
    fn article_missing_date_debut_is_hard_error() {
        let xml = br#"<ARTICLE>
  <META_COMMUN><ID>LEGIARTI000000000005</ID></META_COMMUN>
  <META_ARTICLE><NUM>1240</NUM><ETAT>VIGUEUR</ETAT></META_ARTICLE>
  <CONTEXTE><TEXTE cid="LEGITEXT000006070721"/></CONTEXTE>
</ARTICLE>"#;
        assert!(matches!(parse_legi_article(xml), Err(CoreError::Xml(_))));
    }

    // Membre JORF réel (ARTICLE), tags identiques à LEGIARTI : le parser LEGI
    // s'applique tel quel. Ici une version d'article de traité (cid JORFTEXT, dates
    // réelles, ETAT VIGUEUR).
    const JORF_ARTICLE: &str = r#"<ARTICLE>
  <META>
    <META_COMMUN><ID>JORFARTI000000694291</ID><ORIGINE>JORF</ORIGINE><NATURE>Article</NATURE></META_COMMUN>
    <META_SPEC><META_ARTICLE>
      <NUM>6</NUM><ETAT>VIGUEUR</ETAT>
      <DATE_DEBUT>2002-08-09</DATE_DEBUT><DATE_FIN>2999-01-01</DATE_FIN>
    </META_ARTICLE></META_SPEC>
  </META>
  <CONTEXTE>
    <TEXTE cid="JORFTEXT000000694290" nature="DECRET">
      <TITRE_TXT>Décret portant publication de l'accord franco-algérien</TITRE_TXT>
    </TEXTE>
  </CONTEXTE>
  <BLOC_TEXTUEL><CONTENU>Les ressortissants algériens bénéficient des dispositions du présent article.</CONTENU></BLOC_TEXTUEL>
</ARTICLE>"#;

    #[test]
    fn parse_legi_article_handles_jorf_member() {
        let a = parse_legi_article(JORF_ARTICLE.as_bytes()).expect("article JORF");
        assert_eq!(a.legiarti, "JORFARTI000000694291");
        assert_eq!(a.legitext, "JORFTEXT000000694290");
        assert_eq!(a.num, "6");
        assert_eq!(a.etat, "VIGUEUR");
        assert_eq!(a.date_debut, "2002-08-09");
        assert_eq!(a.date_fin, None);
        assert_eq!(
            a.texte.as_deref(),
            Some("Les ressortissants algériens bénéficient des dispositions du présent article.")
        );
    }

    // Bloc LIENS de l'ordonnance 2016-131 art. 2, copié du stock : le même
    // « a modifié » y est encodé des deux façons — verbe+source (MODIFIE le Code
    // civil) et nom+cible (MODIFICATION du Code de la consommation), toutes deux
    // sortantes. S'y ajoutent un « modifié par » (verbe+cible, entrant) et une
    // CITATION cible, verbale : « cité par », entrant. La sentinelle 2999-01-01 des
    // codes est absorbée.
    const ARTICLE_AVEC_LIENS: &str = r#"<ARTICLE>
  <META>
    <META_COMMUN><ID>LEGIARTI000032006591</ID></META_COMMUN>
    <META_SPEC><META_ARTICLE>
      <NUM>2</NUM><ETAT>VIGUEUR</ETAT><DATE_DEBUT>2016-02-12</DATE_DEBUT>
    </META_ARTICLE></META_SPEC>
  </META>
  <CONTEXTE><TEXTE cid="LEGITEXT000032004939"/></CONTEXTE>
  <LIENS>
    <LIEN cidtexte="LEGITEXT000006070721" datesignatexte="2999-01-01" id="LEGISCTA000032040812" naturetexte="CODE" nortexte="" num="" numtexte="" sens="source" typelien="MODIFIE">Code civil -  Chapitre II : La formation du contrat (V)</LIEN>
    <LIEN cidtexte="LEGITEXT000006069565" datesignatexte="2999-01-01" id="LEGIARTI000032890812" naturetexte="CODE" nortexte="" num="L212-1" numtexte="" sens="cible" typelien="MODIFICATION">Code de la consommation - art. L212-1 (VD)</LIEN>
    <LIEN cidtexte="LEGITEXT000036829582" datesignatexte="2018-04-20" id="LEGIARTI000036829885" naturetexte="LOI" nortexte="JUSX1705255L" num="16" numtexte="2018-287" sens="cible" typelien="MODIFIE">LOI n°2018-287 du 20 avril 2018 - art. 16</LIEN>
    <LIEN cidtexte="LEGITEXT000006069577" datesignatexte="2999-01-01" id="" naturetexte="CODE" num="1600-0 H" sens="cible" typelien="CITATION">Code général des impôts, CGI. - art. 1600-0 H (V)</LIEN>
  </LIENS>
</ARTICLE>"#;

    #[test]
    fn parse_article_collects_liens_in_document_order() {
        let a = parse_legi_article(ARTICLE_AVEC_LIENS.as_bytes()).expect("article");
        assert_eq!(a.liens.len(), 4);

        let modifie = &a.liens[0];
        assert_eq!(modifie.typelien, "MODIFIE");
        assert_eq!(modifie.verb, "modifie");
        assert_eq!(modifie.direction, "outgoing");
        assert_eq!(modifie.target_kind, LienTargetKind::Section);
        assert_eq!(modifie.target_uid.as_deref(), Some("LEGISCTA000032040812"));
        assert_eq!(
            modifie.target_text_uid.as_deref(),
            Some("LEGITEXT000006070721")
        );
        assert_eq!(modifie.target_nature.as_deref(), Some("CODE"));
        assert_eq!(modifie.target_date, None);
        assert_eq!(modifie.target_nor, None);

        let modification = &a.liens[1];
        assert_eq!(modification.typelien, "MODIFICATION");
        assert_eq!(modification.verb, "modifie");
        assert_eq!(modification.direction, "outgoing");
        assert_eq!(modification.target_kind, LienTargetKind::Article);
        assert_eq!(modification.target_num.as_deref(), Some("L212-1"));

        let modifie_par = &a.liens[2];
        assert_eq!(modifie_par.typelien, "MODIFIE");
        assert_eq!(modifie_par.verb, "modifie");
        assert_eq!(modifie_par.direction, "incoming");
        assert_eq!(modifie_par.target_date.as_deref(), Some("2018-04-20"));
        assert_eq!(modifie_par.target_nor.as_deref(), Some("JUSX1705255L"));

        let cite = &a.liens[3];
        assert_eq!(cite.verb, "cite");
        assert_eq!(cite.direction, "incoming");
        assert_eq!(cite.target_kind, LienTargetKind::Article);
        assert_eq!(cite.target_uid, None);
        assert_eq!(cite.target_num.as_deref(), Some("1600-0 H"));
    }

    #[test]
    fn lien_direction_decodes_voice_and_sens() {
        assert_eq!(lien_direction("MODIFIE", "source").unwrap(), "outgoing");
        assert_eq!(lien_direction("MODIFIE", "cible").unwrap(), "incoming");
        assert_eq!(lien_direction("MODIFICATION", "cible").unwrap(), "outgoing");
        assert_eq!(
            lien_direction("MODIFICATION", "source").unwrap(),
            "incoming"
        );
        assert_eq!(lien_direction("CITATION", "source").unwrap(), "outgoing");
        assert_eq!(lien_direction("CITATION", "cible").unwrap(), "incoming");
        assert_eq!(lien_direction("NOUVEAUTE", "cible").unwrap(), "outgoing");
        assert!(lien_direction("MODIFIE", "").is_err());
    }

    #[test]
    fn article_sans_bloc_liens_est_vide() {
        let a = parse_legi_article(ARTICLE_MODIFIE.as_bytes()).expect("article");
        assert!(a.liens.is_empty());
    }

    #[test]
    fn parse_texte_collects_liens() {
        // TEXTE_VERSION d'un décret ABROGE : LIENS sous META_TEXTE_VERSION — abrogé
        // par un décret, issu d'une loi.
        let xml = r#"<TEXTE_VERSION>
  <META>
    <META_COMMUN><ID>LEGITEXT000005615128</ID><NATURE>DECRET</NATURE></META_COMMUN>
    <META_SPEC>
    <META_TEXTE_CHRONICLE><CID>LEGITEXT000005615128</CID></META_TEXTE_CHRONICLE>
    <META_TEXTE_VERSION>
      <TITRE>Décret n°94-46 du 5 janvier 1994</TITRE>
      <LIENS>
        <LIEN cidtexte="JORFTEXT000000646051" datesignatexte="2007-03-19" id="LEGIARTI000006238452" naturetexte="DECRET" num="26" numtexte="2007-358" sens="cible" typelien="ABROGATION">Décret n°2007-358 du 19 mars 2007 - art. 26 (V)</LIEN>
        <LIEN cidtexte="JORFTEXT000000541524" datesignatexte="1992-07-13" id="" naturetexte="LOI" numtexte="92-654" sens="source" typelien="TXT_SOURCE">Loi n°92-654 du 13 juillet 1992</LIEN>
      </LIENS>
    </META_TEXTE_VERSION></META_SPEC>
  </META>
</TEXTE_VERSION>"#;
        let c = parse_legi_texte(xml.as_bytes()).expect("texte");
        assert_eq!(c.liens.len(), 2);
        assert_eq!(c.liens[0].verb, "abroge");
        assert_eq!(c.liens[0].target_kind, LienTargetKind::Article);
        assert_eq!(c.liens[1].verb, "txt_source");
        assert_eq!(c.liens[1].target_kind, LienTargetKind::Texte);
        assert_eq!(
            c.liens[1].target_text_uid.as_deref(),
            Some("JORFTEXT000000541524")
        );
    }

    #[test]
    fn typelien_inconnu_garde_le_brut_en_minuscules() {
        assert_eq!(lien_verb("HISTO"), "histo");
        assert_eq!(lien_verb("NOUVEAU_TYPE_DILA"), "nouveau_type_dila");
    }

    #[test]
    fn parse_texte_maps_code_metadata() {
        let xml = br#"<TEXTE_VERSION>
  <META>
    <META_COMMUN><ID>LEGITEXT000006074220</ID><NATURE>CODE</NATURE></META_COMMUN>
    <META_SPEC>
      <META_TEXTE_CHRONICLE>
        <CID>LEGITEXT000006074220</CID>
        <DERNIERE_MODIFICATION>2026-06-11</DERNIERE_MODIFICATION>
      </META_TEXTE_CHRONICLE>
      <META_TEXTE_VERSION>
        <TITRE>Code de l'environnement</TITRE><ETAT>VIGUEUR</ETAT>
      </META_TEXTE_VERSION>
    </META_SPEC>
  </META>
</TEXTE_VERSION>"#;
        let c = parse_legi_texte(xml).expect("texte");
        assert_eq!(c.legitext, "LEGITEXT000006074220");
        assert_eq!(c.titre, "Code de l'environnement");
        assert_eq!(c.nature, "CODE");
        assert_eq!(c.derniere_modification.as_deref(), Some("2026-06-11"));
        assert_eq!(c.nor, None);
    }

    #[test]
    fn parse_texte_nor_from_chronicle() {
        // Layout LEGI réel : le NOR vit sous META_TEXTE_CHRONICLE, PAS sous
        // META_COMMUN, qui est le layout JORF.
        let xml = r#"<TEXTE_VERSION>
  <META>
    <META_COMMUN><ID>LEGITEXT000005615128</ID><NATURE>DECRET</NATURE></META_COMMUN>
    <META_SPEC>
      <META_TEXTE_CHRONICLE>
        <CID>JORFTEXT000000546652</CID>
        <NUM>94-46</NUM>
        <NOR>ECOC9300166D</NOR>
        <DATE_PUBLI>1994-01-19</DATE_PUBLI>
        <DATE_TEXTE>1994-01-05</DATE_TEXTE>
        <DERNIERE_MODIFICATION>2007-03-20</DERNIERE_MODIFICATION>
      </META_TEXTE_CHRONICLE>
      <META_TEXTE_VERSION>
        <TITRE>Décret n°94-46 du 5 janvier 1994</TITRE><ETAT>ABROGE</ETAT>
      </META_TEXTE_VERSION>
    </META_SPEC>
  </META>
</TEXTE_VERSION>"#;
        let c = parse_legi_texte(xml.as_bytes()).expect("texte");
        assert_eq!(c.nor.as_deref(), Some("ECOC9300166D"));
        assert_eq!(c.date_texte.as_deref(), Some("1994-01-05"));
        assert_eq!(c.date_publi.as_deref(), Some("1994-01-19"));
    }

    #[test]
    fn parse_texte_collects_versions_a_venir() {
        // Dates programmées sous META_TEXTE_CHRONICLE. La sentinelle 2222-02-22 (date
        // inconnue) est conservée, rendue « à déterminer » côté UI.
        let xml = r#"<TEXTE_VERSION>
  <META>
    <META_COMMUN><ID>LEGITEXT000050940859</ID><NATURE>ARRETE</NATURE></META_COMMUN>
    <META_SPEC>
      <META_TEXTE_CHRONICLE>
        <CID>LEGITEXT000050940859</CID>
        <VERSIONS_A_VENIR>
          <VERSION_A_VENIR>2026-01-01</VERSION_A_VENIR>
          <VERSION_A_VENIR>2222-02-22</VERSION_A_VENIR>
        </VERSIONS_A_VENIR>
      </META_TEXTE_CHRONICLE>
      <META_TEXTE_VERSION><TITRE>Arrêté du 5 mai 2021</TITRE></META_TEXTE_VERSION>
    </META_SPEC>
  </META>
</TEXTE_VERSION>"#;
        let c = parse_legi_texte(xml.as_bytes()).expect("texte");
        assert_eq!(c.versions_a_venir, vec!["2026-01-01", "2222-02-22"]);
    }

    #[test]
    fn parse_texte_tnc_anchors_on_cid_and_prefers_titrefull() {
        // TNC versionné DILA : META_COMMUN/ID est un id de VERSION, l'identité est le
        // CID chronique, celui des articles et de la TOC ; le TITRE d'un TNC est nu,
        // TITREFULL porte le descriptif.
        let xml = r#"<TEXTE_VERSION>
  <META>
    <META_COMMUN><ID>LEGITEXT000054411191</ID><NATURE>ARRETE</NATURE></META_COMMUN>
    <META_SPEC>
      <META_TEXTE_CHRONICLE><CID>JORFTEXT000054407988</CID><NOR>AGRT2617818A</NOR></META_TEXTE_CHRONICLE>
      <META_TEXTE_VERSION>
        <TITRE>Arrêté du 7 juillet 2026</TITRE>
        <TITREFULL>Arrêté du 7 juillet 2026 relatif à la modification temporaire du cahier des charges de l'appellation d'origine protégée (AOP) « Piment d'Espelette »</TITREFULL>
      </META_TEXTE_VERSION>
    </META_SPEC>
  </META>
</TEXTE_VERSION>"#;
        let c = parse_legi_texte(xml.as_bytes()).expect("texte");
        assert_eq!(c.legitext, "JORFTEXT000054407988");
        assert_eq!(c.titre, "Arrêté du 7 juillet 2026");
        assert!(c.titre_full.as_deref().unwrap().contains("Espelette"));
    }

    #[test]
    fn parse_texte_titrefull_identique_absorbe_en_none() {
        let xml = br#"<TEXTE_VERSION>
  <META>
    <META_COMMUN><ID>LEGITEXT000006074220</ID><NATURE>CODE</NATURE></META_COMMUN>
    <META_SPEC>
      <META_TEXTE_CHRONICLE><CID>LEGITEXT000006074220</CID></META_TEXTE_CHRONICLE>
      <META_TEXTE_VERSION><TITRE>Code de l'environnement</TITRE><TITREFULL>Code de l'environnement</TITREFULL></META_TEXTE_VERSION>
    </META_SPEC>
  </META>
</TEXTE_VERSION>"#;
        let c = parse_legi_texte(xml).expect("texte");
        assert_eq!(c.titre_full, None);
    }

    #[test]
    fn texte_missing_titre_is_hard_error() {
        let xml = br#"<TEXTE_VERSION>
  <META_COMMUN><ID>LEGITEXT000006074220</ID><NATURE>CODE</NATURE></META_COMMUN>
  <META_TEXTE_CHRONICLE><CID>LEGITEXT000006074220</CID></META_TEXTE_CHRONICLE>
</TEXTE_VERSION>"#;
        assert!(matches!(parse_legi_texte(xml), Err(CoreError::Xml(_))));
    }

    #[test]
    fn texte_missing_cid_is_hard_error() {
        // Le CID chronique est l'identité : absent = donnée inexploitable, pas de repli
        // sur l'id de version.
        let xml = br#"<TEXTE_VERSION>
  <META_COMMUN><ID>LEGITEXT000006074220</ID><NATURE>CODE</NATURE></META_COMMUN>
  <META_TEXTE_VERSION><TITRE>Code de l'environnement</TITRE></META_TEXTE_VERSION>
</TEXTE_VERSION>"#;
        assert!(matches!(parse_legi_texte(xml), Err(CoreError::Xml(_))));
    }

    // TEXTELR réel : STRUCT mêle LIEN_ART multi-versions (art. 3 : 1996/1997/2001,
    // plus deux versions jamais entrées en vigueur debut=2999) et une
    // LIEN_SECTION_TA. Le propriétaire est le cid chronique (JORFTEXT), pas l'ID de
    // version.
    const TEXTELR: &str = r#"<TEXTELR>
  <META>
    <META_COMMUN><ID>LEGITEXT000005620686</ID><NATURE>ORDONNANCE</NATURE></META_COMMUN>
    <META_SPEC><META_TEXTE_CHRONICLE>
      <CID>JORFTEXT000000193176</CID><NUM>96-267</NUM>
    </META_TEXTE_CHRONICLE></META_SPEC>
  </META>
  <STRUCT>
    <LIEN_ART debut="2999-01-01" etat="" fin="2999-01-01" id="LEGIARTI000006495398" num="1" origine="LEGI"/>
    <LIEN_ART debut="2001-07-13" etat="VIGUEUR" fin="2999-01-01" id="LEGIARTI000006495402" num="3" origine="LEGI"/>
    <LIEN_ART debut="1996-03-31" etat="MODIFIE" fin="1997-01-01" id="LEGIARTI000006495400" num="3" origine="LEGI"/>
    <LIEN_SECTION_TA cid="LEGISCTA000006094230" debut="1996-03-31" etat="VIGUEUR" fin="2999-01-01" id="LEGISCTA000006094230" niv="1" url="/LEGI/SCTA/00/00/06/09/42/LEGISCTA000006094230.xml">TITRE II : Dispositions relatives au nouveau code pénal.</LIEN_SECTION_TA>
  </STRUCT>
</TEXTELR>"#;

    #[test]
    fn parse_textelr_owner_is_chronicle_cid() {
        let toc = parse_legi_textelr(TEXTELR.as_bytes()).expect("textelr");
        assert_eq!(toc.owner_uid, "JORFTEXT000000193176");
        assert_eq!(toc.text_uid, "JORFTEXT000000193176");
        assert_eq!(toc.edges.len(), 4);

        // Version jamais en vigueur : debut sentinelle GARDÉE (le filtrage daté
        // l'exclut naturellement), fin sentinelle absorbée.
        let jamais = &toc.edges[0];
        assert_eq!(jamais.child_kind, TocChildKind::Article);
        assert_eq!(jamais.date_debut.as_deref(), Some("2999-01-01"));
        assert_eq!(jamais.date_fin, None);
        assert_eq!(jamais.label, "1");
        assert_eq!(jamais.child_num_key.as_deref(), Some("1"));

        let v1996 = &toc.edges[2];
        assert_eq!(v1996.date_debut.as_deref(), Some("1996-03-31"));
        assert_eq!(v1996.date_fin.as_deref(), Some("1997-01-01"));
        assert_eq!(v1996.etat, "MODIFIE");

        let section = &toc.edges[3];
        assert_eq!(section.child_kind, TocChildKind::Section);
        assert_eq!(section.child_uid, "LEGISCTA000006094230");
        assert_eq!(section.child_cid.as_deref(), Some("LEGISCTA000006094230"));
        assert_eq!(section.niv, Some(1));
        assert_eq!(
            section.label,
            "TITRE II : Dispositions relatives au nouveau code pénal."
        );
    }

    // SECTION_TA réel : deux versions d'un même chapitre coexistent au même niveau
    // avec leurs fenêtres (renommage 1995) ; et cas cid ≠ id, version postérieure
    // d'une section chronique.
    const SECTION_TA: &str = r#"<SECTION_TA>
<ID>LEGISCTA000006138328</ID>
<TITRE_TA>Titre II : Les différentes formes juridiques de l'exploitation agricole</TITRE_TA>
<STRUCTURE_TA>
<LIEN_ART debut="2025-03-26" etat="VIGUEUR" fin="2999-01-01" id="LEGIARTI000051371885" num="L320-1" origine="LEGI"/>
<LIEN_SECTION_TA cid="LEGISCTA000006152230" debut="1993-07-23" etat="ABROGE" fin="1995-02-02" id="LEGISCTA000006152230" niv="4" url="/LEGI/SCTA/00/00/06/15/22/LEGISCTA000006152230.xml">Chapitre II : Les groupements fonciers agricoles.</LIEN_SECTION_TA>
<LIEN_SECTION_TA cid="LEGISCTA000024603481" debut="2014-01-01" etat="VIGUEUR" fin="2999-01-01" id="LEGISCTA000028424593" niv="4" url="/LEGI/SCTA/00/00/28/42/45/LEGISCTA000028424593.xml">Chapitre II : Les groupements fonciers agricoles et les groupements fonciers ruraux.</LIEN_SECTION_TA>
</STRUCTURE_TA>
<CONTEXTE>
<TEXTE autorite="" cid="LEGITEXT000006071367" nature="CODE">
<TITRE_TXT c_titre_court="Code rural" debut="1979-12-01" fin="2010-05-08" id_txt="LEGITEXT000006071367">Code rural (nouveau)</TITRE_TXT>
</TEXTE>
</CONTEXTE>
</SECTION_TA>"#;

    #[test]
    fn parse_section_ta_owner_is_version_id() {
        let toc = parse_legi_section_ta(SECTION_TA.as_bytes()).expect("section_ta");
        // Propriétaire = ID de version (la jointure CTE se fait dessus) ;
        // text_uid = cid du texte porteur.
        assert_eq!(toc.owner_uid, "LEGISCTA000006138328");
        assert_eq!(toc.text_uid, "LEGITEXT000006071367");
        assert_eq!(toc.edges.len(), 3);

        let art = &toc.edges[0];
        assert_eq!(art.child_kind, TocChildKind::Article);
        assert_eq!(art.label, "L320-1");
        assert_eq!(art.child_num_key.as_deref(), Some("l320-1"));

        let abroge = &toc.edges[1];
        assert_eq!(abroge.etat, "ABROGE");
        assert_eq!(abroge.date_fin.as_deref(), Some("1995-02-02"));

        let versionne = &toc.edges[2];
        assert_eq!(versionne.child_cid.as_deref(), Some("LEGISCTA000024603481"));
        assert_eq!(versionne.child_uid, "LEGISCTA000028424593");
    }

    #[test]
    fn lien_art_sans_num_est_saute() {
        // Annexes non numérotées (TNC) : LIEN_ART sans @num — l'article n'est jamais
        // ingéré, l'arête est sautée sans invalider le reste du fichier.
        let xml = br#"<SECTION_TA>
<ID>LEGISCTA000036164366</ID>
<STRUCTURE_TA>
<LIEN_ART debut="2017-12-13" etat="VIGUEUR" fin="2999-01-01" id="LEGIARTI000036164466" origine="LEGI"/>
<LIEN_ART debut="2017-12-13" etat="VIGUEUR" fin="2999-01-01" id="LEGIARTI000036164467" num="2" origine="LEGI"/>
</STRUCTURE_TA>
<CONTEXTE><TEXTE cid="JORFTEXT000036162560"/></CONTEXTE>
</SECTION_TA>"#;
        let toc = parse_legi_section_ta(xml).expect("section_ta");
        assert_eq!(toc.edges.len(), 1);
        assert_eq!(toc.edges[0].label, "2");
    }

    #[test]
    fn section_ta_sans_structure_est_vide() {
        let xml = br#"<SECTION_TA>
<ID>LEGISCTA000000000001</ID>
<TITRE_TA>Section vide</TITRE_TA>
<CONTEXTE><TEXTE cid="LEGITEXT000006071367"/></CONTEXTE>
</SECTION_TA>"#;
        let toc = parse_legi_section_ta(xml).expect("section_ta");
        assert!(toc.edges.is_empty());
    }

    #[test]
    fn textelr_missing_cid_is_hard_error() {
        let xml = br#"<TEXTELR><META><META_COMMUN><ID>LEGITEXT000005620686</ID></META_COMMUN></META></TEXTELR>"#;
        assert!(matches!(parse_legi_textelr(xml), Err(CoreError::Xml(_))));
    }
}
