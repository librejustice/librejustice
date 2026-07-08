//! Parser PUR du fond LEGI (bulk DILA) — `payload XML → LegiArticle / LegiCode`
//! (ADR 0092). Aucun I/O : les octets sont fournis par `lj-ingest`. Réutilise
//! l'arbre XML minimal de [`lj_core::parsing`] (`build_tree`/`XmlNode`), donc le
//! même tolérant `quick-xml` que le parser opendata — pas de réimplémentation.
//!
//! Mapping XPath : cf. audit `docs/working-notes/data-audit/legi.md` et la note
//! de grounding `2026-06-13_legi-impl-grounding.md`. Les sentinelles de date
//! (`2999-01-01` = pas de fin, `2222-02-22` = version future à date inconnue)
//! sont normalisées en `None` ici — frontière de parsing unique (#12).

use crate::extract::normalize_article;
use lj_core::error::CoreError;
use lj_core::normalizer::clean_texte;
use lj_core::parsing::{build_tree, node_text, XmlNode};

/// Sentinelles `DATE_FIN` LEGI → « pas de date » : `2999-01-01` (pas de fin /
/// en vigueur) et `2222-02-22` (version future programmée, date inconnue).
const DATE_SENTINELS: &[&str] = &["2999-01-01", "2222-02-22"];

/// Une version d'article du fond LEGI (`LEGIARTI*.xml`, racine `<ARTICLE>`).
/// Dates en `String` ISO (`YYYY-MM-DD`) ; la conversion en type date se fait au
/// bord store. `date_fin = None` ⇔ sentinelle absorbée ⇔ pas de fin.
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
}

/// Un texte/code du fond LEGI (`LEGITEXT*.xml`, racine `<TEXTE_VERSION>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegiCode {
    pub legitext: String,
    pub titre: String,
    pub nature: String,
    pub derniere_modification: Option<String>,
}

/// Sentinelle LEGI → `None` ; sinon la date telle quelle (ISO, conversion au
/// bord store). `pub(crate)` : partagé avec le parser JORF (ADR 0109 §2, mêmes
/// sentinelles de date).
pub(crate) fn date_or_none(raw: Option<String>) -> Option<String> {
    raw.filter(|d| !DATE_SENTINELS.contains(&d.as_str()))
}

/// `clean_texte` d'un `CONTENU`, `None` si vide après nettoyage. `pub(crate)` :
/// partagé avec le parser JORF (même `BLOC_TEXTUEL/CONTENU`, ADR 0109 §2).
pub(crate) fn clean_contenu(node: Option<&XmlNode>) -> Option<String> {
    node_text(node)
        .map(|raw| clean_texte(&raw))
        .filter(|t| !t.is_empty())
}

/// Résout un chemin `A/B/C` dont le **premier** segment est cherché n'importe
/// où dans l'arbre (les conteneurs LEGI `META_COMMUN`/`META_ARTICLE`/`CONTEXTE`/
/// `BLOC_TEXTUEL` sont imbriqués sous `<META>`/`<META_SPEC>`, profondeur variable
/// selon ARTICLE/TEXTE_VERSION) ; les segments suivants suivent les enfants
/// directs comme [`XmlNode::find`]. Premier match en ordre document. `pub(crate)`
/// : partagé avec le parser JORF (structure XML identique, ADR 0109 §2).
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
/// ` > `. `None` si aucun. `pub(crate)` : partagé avec le parser JORF (ADR 0109 §2).
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
/// franche ([`CoreError::Xml`]) si `ID`/`NUM`/`DATE_DEBUT`/`cid` manquent — la
/// frontière de validation source (#12), pas de fallback silencieux.
pub fn parse_legi_article(raw: &[u8]) -> Result<LegiArticle, CoreError> {
    let root = build_tree(raw).ok_or_else(|| CoreError::Xml("ARTICLE: XML illisible".into()))?;

    let legiarti = node_text(find_anywhere(&root, "META_COMMUN/ID"))
        .ok_or_else(|| CoreError::Xml("ARTICLE: META_COMMUN/ID manquant".into()))?;
    let num = node_text(find_anywhere(&root, "META_ARTICLE/NUM"))
        .ok_or_else(|| CoreError::Xml(format!("ARTICLE {legiarti}: META_ARTICLE/NUM manquant")))?;
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
    let num_key = normalize_article(&num);

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
    })
}

/// Parse un `LEGITEXT*.xml` (racine `<TEXTE_VERSION>`) en [`LegiCode`]. Erreur
/// franche ([`CoreError::Xml`]) si `META_COMMUN/ID` ou `META_TEXTE_VERSION/TITRE`
/// manquent.
pub fn parse_legi_texte(raw: &[u8]) -> Result<LegiCode, CoreError> {
    let root =
        build_tree(raw).ok_or_else(|| CoreError::Xml("TEXTE_VERSION: XML illisible".into()))?;

    let legitext = node_text(find_anywhere(&root, "META_COMMUN/ID"))
        .ok_or_else(|| CoreError::Xml("TEXTE_VERSION: META_COMMUN/ID manquant".into()))?;
    let titre = node_text(find_anywhere(&root, "META_TEXTE_VERSION/TITRE")).ok_or_else(|| {
        CoreError::Xml(format!(
            "TEXTE_VERSION {legitext}: META_TEXTE_VERSION/TITRE manquant"
        ))
    })?;
    let nature = node_text(find_anywhere(&root, "META_COMMUN/NATURE")).unwrap_or_default();
    let derniere_modification = node_text(find_anywhere(
        &root,
        "META_TEXTE_CHRONICLE/DERNIERE_MODIFICATION",
    ));
    Ok(LegiCode {
        legitext,
        titre,
        nature,
        derniere_modification,
    })
}

/// Slug déterministe d'un titre de code (décision de conception #1, ADR 0092) :
/// minuscules, accents retirés, ` ` / `'` → `-`, ne garde que `[a-z0-9-]`,
/// collapse les `-` consécutifs, trim les `-` de bord. Ex. `Code civil` →
/// `code-civil`.
pub fn slugify_code(titre: &str) -> String {
    let mut out = String::with_capacity(titre.len());
    let mut prev_dash = false;
    for ch in titre.chars() {
        let lowered = strip_accent(ch).to_ascii_lowercase();
        if lowered.is_ascii_alphanumeric() {
            out.push(lowered);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Replie un caractère latin accentué sur sa base ASCII ; les autres caractères
/// sont rendus tels quels (et seront écartés par le filtre `[a-z0-9-]`).
fn strip_accent(ch: char) -> char {
    match ch {
        'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        'œ' => 'o',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixture ARTICLE minimale reproduisant la structure auditée (legi.md:39-63) :
    // art. L131-4 / LEGIARTI000006832947, ETAT=MODIFIE, DATE_FIN réelle.
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
        // DATE_FIN réelle → conservée (pas une sentinelle).
        assert_eq!(a.date_fin.as_deref(), Some("2018-08-06"));
        assert_eq!(
            a.titre_text.as_deref(),
            Some("Partie législative > Livre Ier > Titre III")
        );
        // CONTENU porte du HTML léger entity-escapé (&lt;br/&gt;) : build_tree
        // décode l'entité → texte « <br/> », que clean_texte mue en saut de ligne.
        assert_eq!(
            a.texte.as_deref(),
            Some("Le premier alinéa.\nLe second alinéa.")
        );
        // NOTA vide → None.
        assert_eq!(a.nota, None);
    }

    #[test]
    fn parse_article_num_key_is_normalized() {
        // num_key = normalize_article(num) : « Art. L. 131-4 » → forme canonique,
        // identique à celle du libellé cité côté décisions (pont ADR 0092).
        let xml = br#"<ARTICLE>
  <META_COMMUN><ID>LEGIARTI000000000001</ID></META_COMMUN>
  <META_ARTICLE><NUM>Art. L. 131-4</NUM><ETAT>VIGUEUR</ETAT>
    <DATE_DEBUT>2020-01-01</DATE_DEBUT></META_ARTICLE>
  <CONTEXTE><TEXTE cid="LEGITEXT000006074220"/></CONTEXTE>
</ARTICLE>"#;
        let a = parse_legi_article(xml).expect("article");
        assert_eq!(a.num, "Art. L. 131-4");
        assert_eq!(a.num_key, normalize_article("Art. L. 131-4"));
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
        // VERSIONS_A_VENIR : 2222-02-22 = version future, date inconnue → None.
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

    // Membre JORF réel (ARTICLE), tags identiques à LEGIARTI (ADR 0109 §2) : le
    // parser LEGI s'applique tel quel (réutilisé, pas dupliqué — #11). Ici une
    // version d'article de traité (cid JORFTEXT, dates réelles, ETAT VIGUEUR).
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
        // Le fond JORF (bulk DILA, ADR 0109) partage la structure XML de LEGI :
        // le même parser le lit. `legitext` porte alors le cid JORFTEXT.
        let a = parse_legi_article(JORF_ARTICLE.as_bytes()).expect("article JORF");
        assert_eq!(a.legiarti, "JORFARTI000000694291");
        assert_eq!(a.legitext, "JORFTEXT000000694290");
        assert_eq!(a.num, "6");
        assert_eq!(a.etat, "VIGUEUR");
        assert_eq!(a.date_debut, "2002-08-09");
        // Sentinelle 2999-01-01 → pas de fin (version en vigueur).
        assert_eq!(a.date_fin, None);
        assert_eq!(
            a.texte.as_deref(),
            Some("Les ressortissants algériens bénéficient des dispositions du présent article.")
        );
    }

    #[test]
    fn parse_texte_maps_code_metadata() {
        // TEXTE_VERSION code (legi.md:65-78) : nature CODE, titre, dernière modif.
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
    }

    #[test]
    fn texte_missing_titre_is_hard_error() {
        let xml = br#"<TEXTE_VERSION>
  <META_COMMUN><ID>LEGITEXT000006074220</ID><NATURE>CODE</NATURE></META_COMMUN>
</TEXTE_VERSION>"#;
        assert!(matches!(parse_legi_texte(xml), Err(CoreError::Xml(_))));
    }

    #[test]
    fn slugify_strips_accents_punct_and_collapses() {
        assert_eq!(slugify_code("Code civil"), "code-civil");
        assert_eq!(slugify_code("Code pénal"), "code-penal");
        assert_eq!(
            slugify_code("Code de l'environnement"),
            "code-de-l-environnement"
        );
        assert_eq!(
            slugify_code("Code de commerce (ancien)"),
            "code-de-commerce-ancien"
        );
        // bords et séparateurs multiples → un seul tiret, pas de tiret de bord.
        assert_eq!(slugify_code("  Code   civil  "), "code-civil");
    }
}
