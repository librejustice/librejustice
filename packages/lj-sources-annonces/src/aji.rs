use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use lj_annonces::zones::zones_departements;
use lj_annonces::{echeance_ts, parse_heure, AnnonceBrute, ContenuAnnonce, NatureMarche};
use lj_core::dates::parse_date_fr;

use super::{apres_balise, dedup_source_uid, send_text, txt, DetailAnnonce, MoteurRun};
use lj_sources::error::{Result, SourceError};

const BASE: &str = "https://mapa.aji-france.com";
const POLITESSE: std::time::Duration = std::time::Duration::from_secs(20);
/// Garde-fou de pagination : la boucle sort de toute façon dès qu'une page
/// n'apporte plus d'uid neuf.
const MAX_PAGES: u64 = 150;

/// La lettre qui préfixe chaque ligne de produit d'une carte
/// (`S <span class="product_mapa">Transports</span>`) : `S`, `F` ou `T`.
fn nature(lettre: &str) -> Option<NatureMarche> {
    match lettre {
        "S" => Some(NatureMarche::Services),
        "F" => Some(NatureMarche::Fournitures),
        "T" => Some(NatureMarche::Travaux),
        _ => None,
    }
}

/// Une carte = un `<a href="/mapa/marche/<id>/show">` enveloppant toute la
/// consultation. Les liens de pagination partagent le préfixe
/// (`/mapa/marche/?page=4`) : c'est `<id>/show` qui distingue une carte.
fn parse_page(html: &str) -> Vec<AnnonceBrute> {
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^(\d+)/show""#).unwrap());
    static ENTETE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)card-header bg-primary text-white'>\s*<i>([^<]*)</i>\s*-\s*<b>(.*?)</b>")
            .unwrap()
    });
    static PUB: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Publié le (\d{2}/\d{2}/\d{4})").unwrap());
    static LIM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Echéance\s*:\s*<b>\s*le (\d{2}/\d{2}/\d{4})(?:\s*à\s*(\d{1,2}h\d{2}))?")
            .unwrap()
    });
    // Colonne de droite : deux `<div class="small">` — l'établissement puis sa
    // ville, le libellé de département ensuite.
    static ACHETEUR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<div class="small">(.*?)(?:<br>|</div>)"#).unwrap());
    static DEPT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<div class="small">.*?</div>\s*<div class="small">(.*?)</div>"#).unwrap()
    });
    static CODE_DEPT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\(([0-9]{2,3}|2A|2B)\)\s*$").unwrap());
    static NATURE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(\S+)\s*<span class="product_mapa">"#).unwrap());
    static PROC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<div class="float-left"><i>(.*?)</i></div>"#).unwrap());

    let mut out = Vec::new();
    for bloc in html.split(r#"<a href="/mapa/marche/"#).skip(1) {
        let Some(id) = ID.captures(bloc).map(|c| c[1].to_string()) else {
            continue;
        };
        let carte = apres_balise(bloc);
        let entete = ENTETE.captures(carte);
        let echeance = LIM.captures(carte).and_then(|c| {
            let d = parse_date_fr(&c[1])?;
            Some(echeance_ts(
                d,
                c.get(2).and_then(|h| parse_heure(h.as_str())),
            ))
        });
        out.push(AnnonceBrute {
            contenu: ContenuAnnonce::default(),
            descripteurs: Vec::new(),
            moteur: "aji",
            source_uid: format!("aji:{id}"),
            cles_jointure: Vec::new(),
            reference: entete
                .as_ref()
                .map(|c| txt(&c[1]))
                .filter(|r| !r.is_empty()),
            acheteur: ACHETEUR
                .captures(carte)
                .map(|c| txt(&c[1]))
                .filter(|a| !a.is_empty()),
            siren: None,
            objet: entete
                .as_ref()
                .map(|c| txt(&c[2]).chars().take(400).collect::<String>())
                .unwrap_or_default(),
            nature: NATURE.captures(carte).and_then(|c| nature(&c[1])),
            cpv: None,
            zones: zones_departements(
                DEPT.captures(carte)
                    .map(|c| txt(&c[1]))
                    .and_then(|l| CODE_DEPT.captures(&l).map(|c| c[1].to_string())),
            ),
            date_publication: PUB.captures(carte).and_then(|c| parse_date_fr(&c[1])),
            echeance,
            url_avis: Some(format!("{BASE}/mapa/marche/{id}/show")),
            // Les pièces (« Documents annexes ») sont nommées par leur fichier
            // et ne se déduisent pas de l'id ; seule la fiche les porte.
            url_rc: None,
            url_dce: None,
            url_depot: Some(format!("{BASE}/mapa/marche/{id}/show?tab_show=offer")),
            procedure: PROC
                .captures(carte)
                .map(|c| txt(&c[1]))
                .filter(|p| !p.is_empty()),
        });
    }
    out
}

/// Total annoncé par le bandeau de pagination — le dénominateur du sanity
/// check collecté/annoncé de la moisson.
fn total_declare(html: &str) -> Option<u64> {
    static TOTAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d[\d\s]{0,7})\s*consultations\b").unwrap());
    TOTAL
        .captures(&txt(html))
        .and_then(|c| c[1].replace(char::is_whitespace, "").parse().ok())
}

/// Ré-énumère le stock ouvert, page par page. Séquentiel, par politesse envers
/// l'hôte d'une association.
pub async fn enumerate() -> Result<MoteurRun> {
    let client = super::client();
    let premiere = send_text(
        client.get(format!("{BASE}/mapa/marche/?page=1")),
        "aji liste 1",
    )
    .await?;
    let annonce = total_declare(&premiere);

    let mut rows: Vec<AnnonceBrute> = Vec::new();
    let mut vus: HashSet<String> = HashSet::new();
    for r in parse_page(&premiere) {
        if vus.insert(r.source_uid.clone()) {
            rows.push(r);
        }
    }
    for p in 2..=MAX_PAGES {
        let html = send_text(
            client.get(format!("{BASE}/mapa/marche/?page={p}")),
            &format!("aji liste {p}"),
        )
        .await?;
        let avant = vus.len();
        for r in parse_page(&html) {
            if vus.insert(r.source_uid.clone()) {
                rows.push(r);
            }
        }
        if vus.len() == avant {
            break;
        }
    }

    Ok(MoteurRun {
        annonce,
        echecs: 0,
        hotes: Vec::new(),
        rows: dedup_source_uid(rows),
    })
}

/// Ce qu'une fiche apprend : le CPV, seul champ que la carte ne porte pas (la
/// procédure, elle, est déjà lue en liste par [`parse_page`]). Le code est
/// complet, clé de contrôle comprise, dans la première cellule de la carte
/// « Type de produit principal CPV ».
///
/// Cette carte n'existe que sur les fiches de MAPA — une demande de devis n'en a
/// aucune. Une telle fiche est lue et muette : bloc vide, la ligne sort de la
/// file d'enrichissement.
fn champs_fiche(html: &str) -> Result<DetailAnnonce> {
    static CPV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)Type de produit principal CPV.*?<div class='col-sm-2'>\s*(\d{8}(?:-\d)?)")
            .unwrap()
    });
    if html.contains("Renseigner votre profil visiteur") {
        return Err(SourceError::Invalid(
            "aji fiche : servie sans session anonyme".to_string(),
        ));
    }
    let champs = super::fiche::champs(html);
    Ok(DetailAnnonce {
        contenu: super::fiche::contenu(
            &champs,
            super::fiche::premier(&champs, &["description", "objet de la consultation"]),
        ),
        zones: Vec::new(),
        cpv: CPV.captures(html).map(|c| c[1].to_string()),
        procedure: None,
        acheteur: None,
    })
}

pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    tokio::time::sleep(POLITESSE).await;
    let client = super::client();
    send_text(
        client.get(format!("{BASE}/mapa/guest/anonymous")),
        "aji session anonyme",
    )
    .await?;
    let html = send_text(client.get(url_avis), "aji fiche").await?;
    champs_fiche(&html)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_annonces::zones::codes;

    const CARTE: &str = r#"<a href="/mapa/marche/168805/show" style="color:black;">

            <div class='card mb-2'>
                <div class='card-header bg-primary text-white'>
                    <i>PAJI/26/04354</i> - <b>traiteur - séjour nucourt</b><span
                        class="badge badge-primary float-right"><b>Publié le 05/08/2026</b></span>
                </div>
                <div class='card-body'>
                    <div class='row'>
                        <div class='col-sm-8'>
                             S <span class="product_mapa">Alimentation</span> : <span class="typeproduit">Achat de repas</span><br />                        </div>
                        <div class='col-sm-4'>
                            <div class="small">Collège Louis Aragon<br>Montigny-lès-Cormeilles</div>
                            <div class="small">
                                                                                                            Val-d&#039;Oise (95)
                                                                                                </div>
                        </div>
                    </div>
                </div>
                <div class="card-footer">
                    <div class="float-left"><i>Demande de devis</i></div>
                    <div class="float-right">Echéance : <b>le 29/09/2026 à 12h00 <i>( Heure de Paris
                                )</i></b>
                    </div>
                    <div class="clearfix"></div>
                </div>

            </div>
        </a>"#;

    #[test]
    fn une_carte_donne_tous_ses_champs() {
        let r = &parse_page(CARTE)[0];
        assert_eq!(r.source_uid, "aji:168805");
        assert_eq!(r.reference.as_deref(), Some("PAJI/26/04354"));
        assert_eq!(r.objet, "traiteur - séjour nucourt");
        assert_eq!(r.acheteur.as_deref(), Some("Collège Louis Aragon"));
        assert_eq!(codes(&r.zones), ["95"]);
        assert_eq!(r.nature, Some(NatureMarche::Services));
        assert_eq!(r.procedure.as_deref(), Some("Demande de devis"));
        assert_eq!(
            r.date_publication,
            Some(lj_core::dates::parse_date_fr("05/08/2026").unwrap())
        );
        assert_eq!(
            r.echeance.map(|t| t.to_string()).as_deref(),
            Some("2026-09-29T10:00:00Z")
        );
        assert_eq!(
            r.url_avis.as_deref(),
            Some("https://mapa.aji-france.com/mapa/marche/168805/show")
        );
        assert_eq!(
            r.url_depot.as_deref(),
            Some("https://mapa.aji-france.com/mapa/marche/168805/show?tab_show=offer")
        );
    }

    #[test]
    fn un_marche_alloti_prend_la_nature_de_son_premier_lot() {
        let carte = r#"<a href="/mapa/marche/167917/show" style="color:black;">
            <div class='card mb-2'>
                <div class='card-header bg-primary text-white'>
                    <i>PAJI/26/03607</i> - <b>Achat d&#039;équipements professionnels pour les élèves</b><span
                        class="badge badge-primary float-right"><b>Publié le 10/07/2026</b></span>
                </div>
                <div class='card-body'>
                    <div class='row'>
                        <div class='col-sm-8'>
                             F <span class="product_mapa">Entretien, réparations, matériels et travaux</span> : <span class="typeproduit">Outillage</span><br /> F <span class="product_mapa">Textile</span> : <span class="typeproduit">Vêtements de travail</span><br /> F <span class="product_mapa">Textile</span> : <span class="typeproduit">Chaussures de travail</span><br /> F <span class="product_mapa">Sécurité</span> : <span class="typeproduit">Equipement de protection individuelle</span><br />                        </div>
                        <div class='col-sm-4'>
                            <div class="small">Lycée professionnel Jean Perrin<br>Saint-André</div>
                            <div class="small">
                                                                                                            Réunion (974)
                                                                                                </div>
                        </div>
                    </div>
                </div>
                <div class="card-footer">
                    <div class="float-left"><i>MAPA</i></div>
                    <div class="float-right">Echéance : <b>le 11/08/2026 à 23h59 <i>( Heure de Paris
                                )</i></b>
                    </div>
                    <div class="clearfix"></div>
                </div>
            </div>
        </a>"#;
        let r = &parse_page(carte)[0];
        assert_eq!(r.nature, Some(NatureMarche::Fournitures));
        assert_eq!(codes(&r.zones), ["974"]);
        assert_eq!(
            r.acheteur.as_deref(),
            Some("Lycée professionnel Jean Perrin")
        );
        assert_eq!(
            r.objet,
            "Achat d'équipements professionnels pour les élèves"
        );
        assert_eq!(r.procedure.as_deref(), Some("MAPA"));
    }

    #[test]
    fn la_pagination_donne_le_total_sans_fabriquer_de_ligne() {
        let nav = r#"<li class="page-item">
                        <a class="page-link" href="/mapa/marche/?page=4">4</a>
                    </li>
                    <li class="page-item">
                        <a class="page-link" rel="next" href="/mapa/marche/?page=2">Suivant&nbsp;&raquo;</a>
                    </li>
                    <li class="page-item">
                        <span class="badge badge-info"><b>1000</b></span> consultations
                    </li>"#;
        assert_eq!(total_declare(nav), Some(1000));
        assert!(parse_page(nav).is_empty());
        assert_eq!(
            total_declare("<a href='/mapa/marche/'>Liste des consultations</a>"),
            None
        );
    }

    #[test]
    fn la_fiche_d_un_mapa_donne_le_cpv() {
        let html = r#"<div class='card mb-2'>
            <div class='card-header'>
                Type de produit principal CPV :
            </div>
            <div class='card-body'>
                                    <div class='row'>
                        <div class='col-sm-2'>
                            63515000-2                        </div>
                        <div class='col-sm-10'>
                            Services de voyages                        </div>
                    </div>
            </div>
        </div>"#;
        assert_eq!(
            champs_fiche(html).unwrap(),
            DetailAnnonce {
                contenu: ContenuAnnonce::default(),
                zones: Vec::new(),
                cpv: Some("63515000-2".to_string()),
                procedure: None,
                acheteur: None,
            }
        );
    }

    #[test]
    fn une_demande_de_devis_est_lue_et_muette() {
        let html = r#"<h1>Détail de la demande de devis</h1>
            <div class='card mb-2'>
                <div class='card-header'>Contact</div>
                <div class='card-body'><table class="table"><tbody>
                    <tr><th>Téléphone</th><td>0102030405</td></tr>
                    <tr><th>Email</th><td>intendance.0123456X@ac-exemple.test</td></tr>
                </tbody></table></div>
            </div>"#;
        assert_eq!(champs_fiche(html).unwrap(), DetailAnnonce::default());
    }

    #[test]
    fn une_page_de_profil_visiteur_est_a_retenter() {
        let html = r#"<div class="page-header">
            <h1>Renseigner votre profil visiteur</h1>
            Ce profil temporaire <b>(facultatif, gratuit et sans engagement)</b> nous permettra
            de vous tenir informé de l'évolution des marchés que vous avez consultés.
            </div>
            <a href="/mapa/guest/anonymous" class="btn btn-warning float-right">
                Continuer la visite anonymement.</a>"#;
        assert!(champs_fiche(html).is_err());
    }
}
