//! Dérivation des arêtes `legal_toc_edge` d'un texte curé (ADR 0186).
//!
//! Le dataset curé porte la structure sous forme d'un `title_path` par article
//! (fil d'Ariane ` > `-séparé, produit par la curation Python). Ce module en
//! dérive l'arbre de divisions et l'émet dans le format d'écriture du store
//! (`replace_toc_edges`) : racine ancrée `owner_uid = text_uid` (même ancrage
//! que LEGI), divisions à cid-slug synthétique, articles joints par
//! `source_uid` — la vue-lecture et le sommaire réel fonctionnent tels quels.

use lj_store::repository::{TocEdgeRow, TocOwner};

/// Un article du dataset tel qu'inséré (clés finales, post dé-collision).
/// Mono-version = un seul élément à fenêtre ouverte ; multi-versions
/// (historique daté, ADR 0187) = une entrée par ligne `legal_article` insérée,
/// en ordre chronologique.
pub struct TocArticle {
    pub num: String,
    pub num_key: String,
    pub versions: Vec<TocVersion>,
    pub title_path: Option<String>,
}

/// Une version datée d'un article, telle qu'insérée : l'arête TOC émise porte
/// la même fenêtre — `toc_tree`/`toc_section_reading` à une date joignent la
/// version couvrante, exactement comme pour un article LEGI.
pub struct TocVersion {
    pub source_uid: String,
    pub status: String,
    pub date_debut: Option<chrono::NaiveDate>,
    pub date_fin: Option<chrono::NaiveDate>,
}

impl TocArticle {
    /// Statut « courant » de l'article (agrégat d'état des divisions) : la
    /// version à fenêtre encore ouverte, sinon la dernière.
    fn current_status(&self) -> &str {
        self.versions
            .iter()
            .rev()
            .find(|v| v.date_fin.is_none())
            .unwrap_or_else(|| self.versions.last().expect("article sans version"))
            .status
            .as_str()
    }
}

/// Longueur max d'un segment de cid (slug d'un intitulé de division).
const SEGMENT_MAX: usize = 40;

enum Node {
    Division(Division),
    Article(usize),
}

struct Division {
    label: String,
    children: Vec<Node>,
}

/// Dérive l'arbre TOC d'un texte curé. Vide si aucun article ne porte de
/// `title_path` (corpus à plat : rien à écrire, la purge suffit).
pub fn derive_corpus_toc(
    text_uid: &str,
    articles: &[TocArticle],
) -> Vec<(TocOwner, Vec<TocEdgeRow>)> {
    if articles.iter().all(|a| a.title_path.is_none()) {
        return Vec::new();
    }

    // Arbre : divisions réutilisées par intitulé sous leur parent (même
    // regroupement que le sommaire à plat du front — des blocs non contigus
    // d'un même intitulé fusionnent).
    let mut root: Vec<Node> = Vec::new();
    for (idx, art) in articles.iter().enumerate() {
        let mut level = &mut root;
        if let Some(path) = art.title_path.as_deref() {
            for segment in path.split(" > ").map(str::trim).filter(|s| !s.is_empty()) {
                let at = level
                    .iter()
                    .position(|n| matches!(n, Node::Division(d) if d.label == segment));
                let at = at.unwrap_or_else(|| {
                    level.push(Node::Division(Division {
                        label: segment.to_string(),
                        children: Vec::new(),
                    }));
                    level.len() - 1
                });
                let Node::Division(d) = &mut level[at] else {
                    unreachable!()
                };
                level = &mut d.children;
            }
        }
        level.push(Node::Article(idx));
    }

    let mut cids = std::collections::HashSet::new();
    let mut out = Vec::new();
    emit(
        text_uid,
        text_uid.to_string(),
        &root,
        &[],
        1,
        articles,
        &mut cids,
        &mut out,
    );
    out
}

/// Émet les arêtes du propriétaire `owner_uid` (ses enfants directs), puis
/// récurse sur ses divisions. Renvoie l'état agrégé du sous-arbre : `ABROGE`
/// si tous les articles descendants le sont, `VIGUEUR` sinon.
#[allow(clippy::too_many_arguments)]
fn emit(
    text_uid: &str,
    owner_uid: String,
    children: &[Node],
    path_slugs: &[String],
    depth: i32,
    articles: &[TocArticle],
    cids: &mut std::collections::HashSet<String>,
    out: &mut Vec<(TocOwner, Vec<TocEdgeRow>)>,
) -> &'static str {
    let mut edges = Vec::with_capacity(children.len());
    let mut all_abroge = true;
    for node in children {
        match node {
            Node::Article(idx) => {
                let art = &articles[*idx];
                all_abroge &= art.current_status() == "ABROGE";
                // Une arête par version insérée, fenêtrée comme elle : la CTE
                // datée sélectionne la version couvrante (law-at-date).
                for v in &art.versions {
                    edges.push(TocEdgeRow {
                        child_kind: "article".to_string(),
                        child_uid: v.source_uid.clone(),
                        child_cid: None,
                        child_num_key: Some(art.num_key.clone()),
                        label: art.num.clone(),
                        etat: v.status.clone(),
                        date_debut: v.date_debut,
                        date_fin: v.date_fin,
                        niv: Some(depth),
                    });
                }
            }
            Node::Division(d) => {
                let mut slugs = path_slugs.to_vec();
                slugs.push(segment_slug(&d.label));
                let cid = unique_cid(&slugs, cids);
                let child_uid = format!("{text_uid}#s:{cid}");
                let etat = emit(
                    text_uid,
                    child_uid.clone(),
                    &d.children,
                    &slugs,
                    depth + 1,
                    articles,
                    cids,
                    out,
                );
                all_abroge &= etat == "ABROGE";
                edges.push(TocEdgeRow {
                    child_kind: "section".to_string(),
                    child_uid,
                    child_cid: Some(cid),
                    child_num_key: None,
                    label: d.label.clone(),
                    etat: etat.to_string(),
                    date_debut: None,
                    date_fin: None,
                    niv: Some(depth),
                });
            }
        }
    }
    out.push((
        TocOwner {
            owner_uid,
            text_uid: text_uid.to_string(),
        },
        edges,
    ));

    if all_abroge && !children.is_empty() {
        "ABROGE"
    } else {
        "VIGUEUR"
    }
}

/// Slug d'un segment de cid : slug du libellé, tronqué à [`SEGMENT_MAX`] sur
/// une frontière de mot quand c'est possible.
fn segment_slug(label: &str) -> String {
    let slug = lj_extract::legi::slugify_code(label);
    if slug.len() <= SEGMENT_MAX {
        return slug;
    }
    match slug[..SEGMENT_MAX].rfind('-') {
        Some(cut) if cut > 0 => slug[..cut].to_string(),
        _ => slug[..SEGMENT_MAX].to_string(),
    }
}

/// cid unique dans le texte : chemin de slugs joint par `--`, suffixe
/// numérique en cas de collision (intitulés distincts au même slug).
fn unique_cid(slugs: &[String], cids: &mut std::collections::HashSet<String>) -> String {
    let base = slugs.join("--");
    let mut cid = base.clone();
    let mut n = 2;
    while !cids.insert(cid.clone()) {
        cid = format!("{base}-{n}");
        n += 1;
    }
    cid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art(num: &str, path: Option<&str>) -> TocArticle {
        TocArticle {
            num: num.to_string(),
            num_key: num.to_lowercase(),
            versions: vec![TocVersion {
                source_uid: format!("t#{num}"),
                status: "VIGUEUR".to_string(),
                date_debut: None,
                date_fin: None,
            }],
            title_path: path.map(str::to_string),
        }
    }

    fn edges_of<'a>(out: &'a [(TocOwner, Vec<TocEdgeRow>)], owner: &str) -> &'a Vec<TocEdgeRow> {
        &out.iter().find(|(o, _)| o.owner_uid == owner).unwrap().1
    }

    #[test]
    fn corpus_plat_sans_title_path_ne_produit_rien() {
        let arts = vec![art("1", None), art("2", None)];
        assert!(derive_corpus_toc("t", &arts).is_empty());
    }

    #[test]
    fn arbre_deux_niveaux_ancre_racine_et_jointures() {
        let arts = vec![
            art("1", Some("LIVRE I. - DES PERSONNES > TITRE I.")),
            art("2", Some("LIVRE I. - DES PERSONNES > TITRE I.")),
            art("3", Some("LIVRE I. - DES PERSONNES > TITRE II.")),
        ];
        let out = derive_corpus_toc("be/cc", &arts);

        // Racine ancrée owner_uid = text_uid, une seule division LIVRE I.
        let root = edges_of(&out, "be/cc");
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].child_kind, "section");
        assert_eq!(root[0].child_cid.as_deref(), Some("livre-i-des-personnes"));
        assert_eq!(root[0].niv, Some(1));

        // LIVRE I possède TITRE I puis TITRE II, dans l'ordre de lecture.
        let livre = edges_of(&out, "be/cc#s:livre-i-des-personnes");
        assert_eq!(
            livre.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
            ["TITRE I.", "TITRE II."]
        );

        // Les articles se joignent par source_uid et num_key.
        let titre1 = edges_of(&out, "be/cc#s:livre-i-des-personnes--titre-i");
        assert_eq!(titre1.len(), 2);
        assert_eq!(titre1[0].child_kind, "article");
        assert_eq!(titre1[0].child_uid, "t#1");
        assert_eq!(titre1[0].child_num_key.as_deref(), Some("1"));
        assert_eq!(titre1[0].niv, Some(3));
    }

    #[test]
    fn divisions_non_contigues_fusionnent() {
        let arts = vec![
            art("1", Some("TITRE I.")),
            art("2", Some("TITRE II.")),
            art("3", Some("TITRE I.")),
        ];
        let out = derive_corpus_toc("t", &arts);
        let root = edges_of(&out, "t");
        assert_eq!(root.len(), 2);
        let titre1 = edges_of(&out, "t#s:titre-i");
        assert_eq!(
            titre1.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
            ["1", "3"]
        );
    }

    #[test]
    fn article_sans_chemin_reste_a_la_racine() {
        let arts = vec![art("1", Some("TITRE I.")), art("2", None)];
        let out = derive_corpus_toc("t", &arts);
        let root = edges_of(&out, "t");
        assert_eq!(root.len(), 2);
        assert_eq!(root[1].child_kind, "article");
        assert_eq!(root[1].label, "2");
    }

    #[test]
    fn cids_collisionnes_suffixes() {
        // Deux intitulés distincts, même slug (ponctuation différente).
        let arts = vec![art("1", Some("CHAPITRE I.")), art("2", Some("CHAPITRE, I"))];
        let out = derive_corpus_toc("t", &arts);
        let root = edges_of(&out, "t");
        assert_eq!(root[0].child_cid.as_deref(), Some("chapitre-i"));
        assert_eq!(root[1].child_cid.as_deref(), Some("chapitre-i-2"));
    }

    #[test]
    fn segment_long_tronque_sur_mot() {
        let s = segment_slug(
            "TITRE PRELIMINAIRE. - DE LA PUBLICATION, DES EFFETS ET DE L'APPLICATION DES LOIS",
        );
        assert!(s.len() <= SEGMENT_MAX);
        assert!(!s.ends_with('-'));
        assert_eq!(s, "titre-preliminaire-de-la-publication");
    }

    #[test]
    fn division_toute_abrogee_marquee_abroge() {
        let mut a1 = art("1", Some("CHAPITRE I."));
        a1.versions[0].status = "ABROGE".to_string();
        let mut a2 = art("2", Some("CHAPITRE I."));
        a2.versions[0].status = "ABROGE".to_string();
        let a3 = art("3", Some("CHAPITRE II."));
        let out = derive_corpus_toc("t", &[a1, a2, a3]);
        let root = edges_of(&out, "t");
        assert_eq!(root[0].etat, "ABROGE");
        assert_eq!(root[1].etat, "VIGUEUR");
    }

    #[test]
    fn article_multi_versions_une_arete_fenetree_par_version() {
        let d = |iso: &str| chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d").unwrap();
        let mut a = art("1134", Some("CHAPITRE III."));
        a.versions = vec![
            TocVersion {
                source_uid: "t#1134".to_string(),
                status: "VIGUEUR".to_string(),
                date_debut: None,
                date_fin: Some(d("2023-01-01")),
            },
            TocVersion {
                source_uid: "t#1134@2023-01-01".to_string(),
                status: "ABROGE".to_string(),
                date_debut: Some(d("2023-01-01")),
                date_fin: None,
            },
        ];
        let out = derive_corpus_toc("t", &[a]);
        let ch = edges_of(&out, "t#s:chapitre-iii");
        assert_eq!(ch.len(), 2);
        assert_eq!(ch[0].child_uid, "t#1134");
        assert_eq!(ch[0].etat, "VIGUEUR");
        assert_eq!(ch[0].date_fin, Some(d("2023-01-01")));
        assert_eq!(ch[1].child_uid, "t#1134@2023-01-01");
        assert_eq!(ch[1].date_debut, Some(d("2023-01-01")));
        assert_eq!(ch[1].date_fin, None);
        // L'agrégat de division suit la version COURANTE (abrogée).
        let root = edges_of(&out, "t");
        assert_eq!(root[0].etat, "ABROGE");
    }
}
