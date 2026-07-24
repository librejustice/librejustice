//! Parser PUR du fond KALI (conventions collectives nationales, bulk DILA) —
//! `payload XML → KaliArticle / KaliConteneur` (ADR 0120). Aucun I/O : les octets
//! sont fournis par `lj-ingest`. Réutilise l'arbre XML minimal de
//! [`lj_core::parsing`] et les helpers structurels du parser LEGI (`find_anywhere`,
//! `clean_contenu`, `date_or_none`, `collect_titre_tm`) — la structure `<ARTICLE>`
//! d'un `KALIARTI` est celle d'un `LEGIARTI` (mêmes `META_ARTICLE`/`CONTEXTE`/
//! `BLOC_TEXTUEL`), seul le rattachement diffère (#11, pas de réimplémentation).
//!
//! Deux écarts au modèle LEGI, propres à KALI :
//! 1. **Rattachement au conteneur, pas au texte** : un `KALIARTI` porte dans son
//!    `CONTEXTE` à la fois un `<TEXTE cid="KALITEXT…">` (le texte de base ou un
//!    avenant) et un `<CONTENEUR cid="KALICONT…">` (la convention). On ancre le
//!    `text_uid` sur le **conteneur** (KALICONT) : c'est la convention qu'on cite
//!    (« art. X de la CCN des Y »), pas l'avenant.
//! 2. **Articles sans numéro** : beaucoup de `KALIARTI` ont `<NUM/>` vide
//!    (organisés par titre de section, pas numérotés). Le parseur les rend
//!    quand même (`num_key` vide) : le référentiel citable les saute côté
//!    pipeline d'ingest, mais l'assemblage du corps des TI (ADR 0223) les
//!    consomme — le skip est une décision du consommateur, pas du parseur.
//!
//! Les `ETAT` étendus de KALI (`VIGUEUR_ETEN` convention étendue par arrêté,
//! `VIGUEUR_NON_ETEN` en vigueur pour les seuls signataires) sont **repliés sur
//! `VIGUEUR`** : ce sont des articles en vigueur, qui doivent compter comme tels
//! pour le pick « texte vivant » et l'index partiel `status='VIGUEUR'`.

use crate::legi::{
    clean_contenu, collect_liens, collect_titre_tm, date_or_none, find_anywhere, LegiLien,
};
use lj_core::error::CoreError;
use lj_core::parsing::{build_tree, node_text};

/// Une version d'article du fond KALI (`KALIARTI*.xml`, racine `<ARTICLE>`).
/// `kalicont` = conteneur (la convention) sur lequel on ancre le `text_uid` —
/// cf. écart #1 du module. Dates en `String` ISO ; conversion au bord store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KaliArticle {
    pub kaliarti: String,
    pub kalicont: String,
    /// Texte porteur (`CONTEXTE/TEXTE@cid`, le texte de base ou un avenant) —
    /// sert à ordonner les blocs d'un conteneur (ADR 0223). Absent sur certains
    /// articles ancrés directement au conteneur.
    pub kalitext: Option<String>,
    /// Vide pour les articles non numérotés (sections titrées) ; idem `num_key`.
    pub num: String,
    pub num_key: String,
    pub titre_text: Option<String>,
    pub etat: String,
    pub date_debut: String,
    pub date_fin: Option<String>,
    pub texte: Option<String>,
    pub liens: Vec<LegiLien>,
}

/// Un conteneur du fond KALI (`KALICONT*.xml`, racine `<IDCC>`) = une convention
/// collective. `num_broch` = premier numéro de brochure (`NUM_BROCH`, l'« n° 3011 »
/// d'usage), sinon `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KaliConteneur {
    pub kalicont: String,
    pub titre: String,
    pub nature: String,
    pub etat: String,
    pub date_publi: Option<String>,
    pub num_broch: Option<String>,
    /// `KALITEXT` du sommaire (`STRUCTURE_TXT/…/LIEN_TXT@idtxt`), dans l'ordre
    /// du fichier : texte de base d'abord, puis textes attachés/salaires.
    /// L'ordre inter-textes de l'assemblage du corps des TI (ADR 0223).
    pub textes: Vec<String>,
}

/// Replie les `ETAT` « en vigueur » de KALI sur `VIGUEUR` (cf. doc module). Les
/// autres (`ABROGE`/`PERIME`/`REMPLACE`/`DENONCE`/`MODIFIE`) sont conservés tels
/// quels — ils ne matchent pas le filtre `status='VIGUEUR'`, ce qui est correct.
fn canon_etat(raw: &str) -> String {
    if raw.starts_with("VIGUEUR") {
        "VIGUEUR".to_string()
    } else {
        raw.to_string()
    }
}

/// Parse un `KALIARTI*.xml` (racine `<ARTICLE>`) en [`KaliArticle`].
///
/// Un article **sans numéro** (`<NUM/>` vide, section titrée) est rendu avec
/// `num_key` vide — au consommateur de le sauter (référentiel citable) ou de le
/// consommer (corps des TI, ADR 0223). `Err` ([`CoreError::Xml`]) si
/// `ID`/`DATE_DEBUT`/`CONTENEUR@cid` manquent — frontière de validation
/// source (#12).
pub fn parse_kali_article(raw: &[u8]) -> Result<KaliArticle, CoreError> {
    let root =
        build_tree(raw).ok_or_else(|| CoreError::Xml("ARTICLE KALI: XML illisible".into()))?;

    let kaliarti = node_text(find_anywhere(&root, "META_COMMUN/ID"))
        .ok_or_else(|| CoreError::Xml("ARTICLE KALI: META_COMMUN/ID manquant".into()))?;

    // NUM souvent vide en KALI (articles organisés par titre de section) :
    // num/num_key vides, rendus quand même (cf. doc).
    let num = node_text(find_anywhere(&root, "META_ARTICLE/NUM")).unwrap_or_default();
    let num_key = lj_core::article_key::identity_key(&num);

    let date_debut =
        node_text(find_anywhere(&root, "META_ARTICLE/DATE_DEBUT")).ok_or_else(|| {
            CoreError::Xml(format!(
                "ARTICLE KALI {kaliarti}: META_ARTICLE/DATE_DEBUT manquant"
            ))
        })?;
    // Rattachement au CONTENEUR (la convention), pas au TEXTE (l'avenant) — écart #1.
    let kalicont = find_anywhere(&root, "CONTEXTE/CONTENEUR")
        .and_then(|c| c.attr("cid"))
        .filter(|cid| !cid.is_empty())
        .ok_or_else(|| {
            CoreError::Xml(format!(
                "ARTICLE KALI {kaliarti}: CONTEXTE/CONTENEUR@cid manquant"
            ))
        })?
        .to_string();

    let kalitext = find_anywhere(&root, "CONTEXTE/TEXTE")
        .and_then(|t| t.attr("cid"))
        .filter(|cid| !cid.is_empty())
        .map(str::to_string);

    let etat =
        canon_etat(&node_text(find_anywhere(&root, "META_ARTICLE/ETAT")).unwrap_or_default());
    let date_fin = date_or_none(node_text(find_anywhere(&root, "META_ARTICLE/DATE_FIN")));
    let titre_text = find_anywhere(&root, "CONTEXTE").and_then(collect_titre_tm);

    Ok(KaliArticle {
        kaliarti,
        kalicont,
        kalitext,
        num,
        num_key,
        titre_text,
        etat,
        date_debut,
        date_fin,
        texte: clean_contenu(find_anywhere(&root, "BLOC_TEXTUEL/CONTENU")),
        liens: collect_liens(&root),
    })
}

/// Parse un `KALICONT*.xml` (racine `<IDCC>`) en [`KaliConteneur`]. Erreur franche
/// ([`CoreError::Xml`]) si `META_COMMUN/ID` ou `META_CONTENEUR/TITRE` manquent.
pub fn parse_kali_conteneur(raw: &[u8]) -> Result<KaliConteneur, CoreError> {
    let root =
        build_tree(raw).ok_or_else(|| CoreError::Xml("CONTENEUR KALI: XML illisible".into()))?;

    let kalicont = node_text(find_anywhere(&root, "META_COMMUN/ID"))
        .ok_or_else(|| CoreError::Xml("CONTENEUR KALI: META_COMMUN/ID manquant".into()))?;
    let titre = node_text(find_anywhere(&root, "META_CONTENEUR/TITRE")).ok_or_else(|| {
        CoreError::Xml(format!(
            "CONTENEUR KALI {kalicont}: META_CONTENEUR/TITRE manquant"
        ))
    })?;
    let nature = node_text(find_anywhere(&root, "META_COMMUN/NATURE")).unwrap_or_default();
    let etat =
        canon_etat(&node_text(find_anywhere(&root, "META_CONTENEUR/ETAT")).unwrap_or_default());
    let date_publi = node_text(find_anywhere(&root, "META_CONTENEUR/DATE_PUBLI"));
    let num_broch = node_text(find_anywhere(&root, "NUMS_BROCH/NUM_BROCH"));
    let textes = collect_lien_txt(&root);

    Ok(KaliConteneur {
        kalicont,
        titre,
        nature,
        etat,
        date_publi,
        num_broch,
        textes,
    })
}

/// `LIEN_TXT@idtxt` du sommaire (`STRUCTURE_TXT`, imbriqués sous des `TM`),
/// dans l'ordre du fichier. Vide si le conteneur n'a pas de sommaire.
fn collect_lien_txt(root: &lj_core::parsing::XmlNode) -> Vec<String> {
    fn walk(node: &lj_core::parsing::XmlNode, out: &mut Vec<String>) {
        for child in &node.children {
            if child.tag == "LIEN_TXT" {
                if let Some(id) = child.attr("idtxt").filter(|v| !v.is_empty()) {
                    out.push(id.to_string());
                }
            }
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    if let Some(s) = find_anywhere(root, "STRUCTURE_TXT") {
        walk(s, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixture KALIARTI réelle (CCN, art. 5 en vigueur étendu) : on ancre sur le
    // CONTENEUR, pas sur le TEXTE ; VIGUEUR_ETEN se replie sur VIGUEUR.
    const ARTI: &str = r#"<ARTICLE>
  <META>
    <META_COMMUN><ID>KALIARTI000005833715</ID><ORIGINE>KALI</ORIGINE><NATURE>Article</NATURE></META_COMMUN>
    <META_SPEC><META_ARTICLE>
      <NUM>5</NUM><TITRE/><ETAT>VIGUEUR_ETEN</ETAT>
      <DATE_DEBUT>2011-07-26</DATE_DEBUT><DATE_FIN>2999-01-01</DATE_FIN>
    </META_ARTICLE></META_SPEC>
  </META>
  <CONTEXTE>
    <TEXTE cid="KALITEXT000024359407" nature="ACCORD">
      <TITRE_TXT>Convention collective nationale des entreprises de propreté</TITRE_TXT>
      <TM><TITRE_TM>Champ d'application</TITRE_TM></TM>
    </TEXTE>
    <CONTENEUR cid="KALICONT000005635585" titre="Convention collective nationale des entreprises de propreté"/>
  </CONTEXTE>
  <BLOC_TEXTUEL><CONTENU>La présente convention règle les rapports.&lt;br/&gt;Second alinéa.</CONTENU></BLOC_TEXTUEL>
</ARTICLE>"#;

    #[test]
    fn parse_arti_anchors_on_conteneur_and_folds_etat() {
        let a = parse_kali_article(ARTI.as_bytes()).expect("ok");
        assert_eq!(a.kaliarti, "KALIARTI000005833715");
        // text_uid = CONTENEUR (la convention), PAS le TEXTE/avenant — mais le
        // TEXTE porteur reste capté (ordre des blocs, ADR 0223).
        assert_eq!(a.kalicont, "KALICONT000005635585");
        assert_eq!(a.kalitext.as_deref(), Some("KALITEXT000024359407"));
        assert_eq!(a.num, "5");
        assert_eq!(a.num_key, "5");
        // VIGUEUR_ETEN → VIGUEUR.
        assert_eq!(a.etat, "VIGUEUR");
        assert_eq!(a.date_debut, "2011-07-26");
        // Sentinelle 2999 → pas de fin.
        assert_eq!(a.date_fin, None);
        assert_eq!(a.titre_text.as_deref(), Some("Champ d'application"));
        assert_eq!(
            a.texte.as_deref(),
            Some("La présente convention règle les rapports.\nSecond alinéa.")
        );
    }

    #[test]
    fn parse_arti_empty_num_is_returned_with_empty_key() {
        let xml = br#"<ARTICLE>
  <META_COMMUN><ID>KALIARTI000000000001</ID></META_COMMUN>
  <META_ARTICLE><NUM/><ETAT>VIGUEUR</ETAT><DATE_DEBUT>2000-09-01</DATE_DEBUT></META_ARTICLE>
  <CONTEXTE><CONTENEUR cid="KALICONT000005635082"/></CONTEXTE>
  <BLOC_TEXTUEL><CONTENU>Sans numero.</CONTENU></BLOC_TEXTUEL>
</ARTICLE>"#;
        // NUM vide → article rendu quand même, num_key vide (le skip citable
        // appartient au consommateur, ADR 0223) ; TEXTE absent → kalitext None.
        let a = parse_kali_article(xml).expect("ok");
        assert!(a.num_key.is_empty());
        assert_eq!(a.kalitext, None);
        assert_eq!(a.texte.as_deref(), Some("Sans numero."));
    }

    #[test]
    fn parse_arti_missing_conteneur_is_hard_error() {
        let xml = br#"<ARTICLE>
  <META_COMMUN><ID>KALIARTI000000000002</ID></META_COMMUN>
  <META_ARTICLE><NUM>1</NUM><ETAT>VIGUEUR</ETAT><DATE_DEBUT>2000-09-01</DATE_DEBUT></META_ARTICLE>
  <CONTEXTE><TEXTE cid="KALITEXT000005672388"/></CONTEXTE>
</ARTICLE>"#;
        assert!(matches!(parse_kali_article(xml), Err(CoreError::Xml(_))));
    }

    #[test]
    fn parse_conteneur_maps_metadata() {
        let xml = r#"<IDCC>
  <META>
    <META_COMMUN><ID>KALICONT000005635585</ID><ORIGINE>KALI</ORIGINE><NATURE>IDCC</NATURE></META_COMMUN>
    <META_SPEC><META_CONTENEUR>
      <TITRE>Convention collective nationale des entreprises de propreté</TITRE>
      <ETAT>VIGUEUR_ETEN</ETAT><NUM/><DATE_PUBLI>2011-09-01</DATE_PUBLI>
    </META_CONTENEUR></META_SPEC>
  </META>
  <STRUCTURE_TXT>
    <TM niv="1"><TITRE_TM>Texte de base</TITRE_TM>
      <LIEN_TXT idtxt="KALITEXT000024359407" titretxt="Convention collective"/>
    </TM>
    <TM niv="1"><TITRE_TM>Textes Attachés</TITRE_TM>
      <LIEN_TXT idtxt="KALITEXT000024359999" titretxt="Avenant n°1"/>
    </TM>
  </STRUCTURE_TXT>
  <NUMS_BROCH><NUM_BROCH>3173</NUM_BROCH></NUMS_BROCH>
</IDCC>"#;
        let c = parse_kali_conteneur(xml.as_bytes()).expect("conteneur");
        assert_eq!(c.kalicont, "KALICONT000005635585");
        assert_eq!(
            c.titre,
            "Convention collective nationale des entreprises de propreté"
        );
        assert_eq!(c.nature, "IDCC");
        assert_eq!(c.etat, "VIGUEUR");
        assert_eq!(c.date_publi.as_deref(), Some("2011-09-01"));
        assert_eq!(c.num_broch.as_deref(), Some("3173"));
        // Sommaire dans l'ordre du fichier : texte de base puis attachés.
        assert_eq!(
            c.textes,
            vec!["KALITEXT000024359407", "KALITEXT000024359999"]
        );
    }

    #[test]
    fn parse_conteneur_missing_titre_is_hard_error() {
        let xml = br#"<IDCC>
  <META_COMMUN><ID>KALICONT000005635585</ID><NATURE>IDCC</NATURE></META_COMMUN>
</IDCC>"#;
        assert!(matches!(parse_kali_conteneur(xml), Err(CoreError::Xml(_))));
    }
}
