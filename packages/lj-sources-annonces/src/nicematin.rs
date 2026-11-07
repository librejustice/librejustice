//! The legal-notice desk of the Nice-Matin group, which serves its own archive
//! rather than renting a portal. A press channel: it names the buyer profile
//! where the notice sits, it does not open it.

use std::sync::LazyLock;

use futures::StreamExt;
use jiff::civil::Date;
use regex::Regex;

use lj_annonces::zones::zones_departements;
use lj_annonces::{
    cles_jointure_depuis_urls, echeance_ts, nettoyer_acheteur, parse_heure, AnnonceBrute,
    ContenuAnnonce, LotAnnonce,
};
use lj_core::dates::parse_date_fr;

use super::{fiche, send_text, txt, HoteVerdict, MoteurRun};
use lj_sources::error::Result;

const BASE: &str = "https://consultation.legales-nicematin.fr";
/// The `Marchés publics` box of the category tree; the nine others are company
/// life, public enquiries and auctions.
const CATEGORIE: &str = "1598";
/// The four departments the group is habilitated for.
const DEPARTEMENTS: [&str; 4] = ["06", "13", "83", "84"];
/// A notice open today was published within this window: the search offers no
/// deadline filter, so the publication date is the only handle on the stock.
const FENETRE_JOURS: i32 = 120;
/// Ten notices a page, well past what the window holds.
const PAGES_MAX: usize = 60;
const FICHES_EN_VOL: usize = 4;

fn recherche_url() -> String {
    format!("{BASE}/consultation.php/")
}

fn page_url(n: usize) -> String {
    format!("{BASE}/consultation.php/recherche/resultats/{n}.html")
}

fn fiche_url(id: &str) -> String {
    format!("{BASE}/consultation.php/resultats/annonce/{id}.html")
}

/// The search keeps its criteria in the session, so the pages that follow are
/// plain GETs. `release_date=custom` is the only value that takes a window.
fn criteres(aujourdhui: Date) -> Vec<(String, String)> {
    let debut = aujourdhui.saturating_sub(jiff::Span::new().days(FENETRE_JOURS));
    let fr = |d: Date| format!("{:02}/{:02}/{}", d.day(), d.month(), d.year());
    let mut f = vec![
        ("search[keyword]".to_string(), String::new()),
        ("search[release_date]".to_string(), "custom".to_string()),
        ("search[release_date_period][from]".to_string(), fr(debut)),
        (
            "search[release_date_period][to]".to_string(),
            fr(aujourdhui),
        ),
        ("search[categories][]".to_string(), CATEGORIE.to_string()),
    ];
    for d in DEPARTEMENTS {
        f.push(("search[departements][]".to_string(), d.to_string()));
    }
    f
}

/// The pager never writes its length: the highest number it offers is the last
/// page.
fn pages(page: &str) -> usize {
    static PAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"resultats/(\d+)\.html").unwrap());
    PAGE.captures_iter(page)
        .filter_map(|c| c[1].parse().ok())
        .max()
        .unwrap_or(1)
}

struct Ligne {
    id: String,
    date_publication: Option<Date>,
    acteur: String,
    departement: String,
}

fn parse_lignes(page: &str) -> Vec<Ligne> {
    static LIGNE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<tr class="line\d+">(.*?)</tr>"#).unwrap());
    static ID: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"annonce/([A-Za-z0-9_-]+)\.html").unwrap());
    static DATE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<td class="release-date">(.*?)</td>"#).unwrap());
    static REF: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<td class="internalRef">(.*?)</td>"#).unwrap());
    static DEPT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<td class="departement">(.*?)</td>"#).unwrap());

    LIGNE
        .captures_iter(page)
        .filter_map(|l| {
            let bloc = &l[1];
            Some(Ligne {
                id: ID.captures(bloc)?[1].to_string(),
                date_publication: DATE.captures(bloc).and_then(|c| parse_date_fr(&txt(&c[1]))),
                acteur: REF.captures(bloc).map(|c| txt(&c[1])).unwrap_or_default(),
                departement: DEPT
                    .captures(bloc)
                    .map(|c| txt(&c[1]).trim().to_string())
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// The deadline sits in running prose, under a dozen wordings and three date
/// forms. Without one the row cannot enter the open window, so it is dropped.
fn echeance(corps: &str) -> Option<jiff::Timestamp> {
    static LIMITE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)(?:date limite|limite de r[ée]ception|remise des (?:offres|plis)|d[ée]p[ôo]t des (?:offres|plis|candidatures)|avant le)[^:.\n]{0,70}?:?\s*(?:le\s+)?((?:\d{1,2}/\d{1,2}/\d{2,4})|(?:\d{1,2}(?:er)?\s+[a-zA-Zéûà]+\s+\d{4}))(?:\s*(?:à|a)\s*(\d{1,2}\s*[hH:]\s*\d{0,2}))?",
        )
        .unwrap()
    });
    let c = LIMITE.captures(corps)?;
    let d = parse_date_fr(&c[1])?;
    Some(echeance_ts(
        d,
        c.get(2).and_then(|h| parse_heure(h.as_str())),
    ))
}

/// `Objet du marché : …` labelled, else the `OBJET DE LA CONSULTATION …`
/// heading. The listing only carries the kind of notice, which says nothing
/// about the market.
fn objet(corps: &str) -> Option<String> {
    static ETIQUETTE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)objet\b[^:\n]{0,40}:\s*(\S.{15,400})").unwrap());
    static TITRE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)objet\b(?:\s+de\s+la\s+(?:consultation|prestation))?\s+(\S.{15,400})")
            .unwrap()
    });
    let brut = ETIQUETTE
        .captures(corps)
        .or_else(|| TITRE.captures(corps))?[1]
        .to_string();
    Some(premiere_phrase(&brut))
}

/// `L. 2122-1-1` and `art. 3` end no sentence: a full stop counts only behind a
/// word of three letters or more.
fn premiere_phrase(t: &str) -> String {
    for (i, _) in t.match_indices(". ") {
        if t[..i]
            .split_whitespace()
            .last()
            .unwrap_or("")
            .chars()
            .count()
            >= 3
        {
            return t[..i].trim().to_string();
        }
    }
    t.trim().to_string()
}

/// The ad labels its allotment nowhere: the lots follow `Forme du marché` as
/// bare lines, inside the block that label opens.
fn lots(champs: &[fiche::Champ]) -> Vec<LotAnnonce> {
    static LIGNE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^(Lot\s*n?[°o]?\s*\d+\s*[-:.]\s*\S.*)$").unwrap());
    champs
        .iter()
        .filter(|c| c.cle.starts_with("forme d"))
        .flat_map(|c| fiche::lignes(&c.bloc))
        .filter_map(|l| LIGNE.captures(&l).map(|m| m[1].trim().to_string()))
        .map(|nom| LotAnnonce {
            nom: Some(nom),
            description: None,
            cpv: None,
        })
        .collect()
}

/// The ad is read whole at enumeration, so its content costs no second visit
/// and `detail()` has nothing left to fetch. Its allotment is read from the one
/// place an ad states it: a sale of three plots also numbers its lots, and its
/// visiting hours would enter as three of them.
fn contenu(page: &str) -> ContenuAnnonce {
    let champs = fiche::champs(page);
    let mut contenu = fiche::contenu(&champs, fiche::premier(&champs, &["objet"]));
    contenu.lots = lots(&champs);
    contenu
}

fn liens(corps: &str) -> Vec<String> {
    static URL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?:https?://|www\.)[^\s,;)"'<>]+"#).unwrap());
    URL.find_iter(corps)
        .map(|m| {
            let u = m.as_str().trim_end_matches(['.', ')', ',']);
            match u.starts_with("www.") {
                true => format!("https://{u}"),
                false => u.to_string(),
            }
        })
        .collect()
}

async fn fiche(client: &wreq::Client, l: Ligne) -> Option<AnnonceBrute> {
    let url = fiche_url(&l.id);
    let page = send_text(client.get(&url), &format!("nicematin fiche {}", l.id))
        .await
        .ok()?;
    let corps = txt(&page);
    let echeance = echeance(&corps)?;
    let urls = liens(&corps);
    let acheteur = nettoyer_acheteur(l.acteur.split(" - ").next().unwrap_or(&l.acteur));
    Some(AnnonceBrute {
        contenu: contenu(&page),
        descripteurs: Vec::new(),
        moteur: "nicematin",
        source_uid: format!("nicematin:{}", l.id),
        cles_jointure: cles_jointure_depuis_urls(urls.iter().map(String::as_str)),
        reference: None,
        acheteur: Some(acheteur).filter(|a| !a.is_empty()),
        siren: None,
        objet: objet(&corps).unwrap_or(l.acteur),
        nature: None,
        cpv: None,
        zones: zones_departements([&l.departement]),
        date_publication: l.date_publication,
        echeance: Some(echeance),
        url_avis: Some(url),
        url_rc: None,
        url_dce: None,
        url_depot: urls.into_iter().next(),
        procedure: None,
    })
}

/// Re-enumerates the market notices published over the window.
pub async fn enumerate() -> Result<MoteurRun> {
    let client = super::client();
    let host = BASE.trim_start_matches("https://").to_string();
    let aujourdhui = jiff::Zoned::now().date();
    let premiere = match send_text(
        client.post(recherche_url()).form(&criteres(aujourdhui)),
        "nicematin recherche",
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "nicematin: search unreachable");
            return Ok(MoteurRun {
                annonce: None,
                echecs: 0,
                hotes: vec![HoteVerdict {
                    host,
                    joignable: false,
                }],
                rows: Vec::new(),
            });
        }
    };
    let mut lignes = parse_lignes(&premiere);
    let mut echecs = 0;
    for n in 2..=pages(&premiere).min(PAGES_MAX) {
        match send_text(client.get(page_url(n)), &format!("nicematin page {n}")).await {
            Ok(p) => lignes.extend(parse_lignes(&p)),
            Err(e) => {
                tracing::warn!(page = n, error = %e, "nicematin: page lost");
                echecs += 1;
            }
        }
    }
    let vues = lignes.len();
    let rows: Vec<AnnonceBrute> = futures::stream::iter(lignes)
        .map(|l| fiche(&client, l))
        .buffer_unordered(FICHES_EN_VOL)
        .filter_map(|r| async move { r })
        .collect()
        .await;
    tracing::info!(vues, retenues = rows.len(), "nicematin: records read");
    Ok(MoteurRun {
        // The listing counts every legal notice of the category, award and
        // cancellation notices included; only those with a deadline are rows.
        annonce: None,
        echecs,
        hotes: vec![HoteVerdict {
            host,
            joignable: true,
        }],
        rows: super::dedup_source_uid(rows),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTE: &str = r#"
<a href="/consultation.php/recherche/resultats/2.html">2</a>
<a href="/consultation.php/recherche/resultats/14.html">14</a>
<tr class="line0">
  <td class="release-date">14/09/26</td>
  <td class="title"><a href="/consultation.php/resultats/annonce/NM24131.html">PUBLICITE RESUMEE SUPPLEMENTAIRE</a></td>
  <td class="internalRef"><a href="/consultation.php/resultats/annonce/NM24131.html">MAIRIE DE LA SEYNE-SUR-MER - CARBURANTS</a></td>
  <td class="departement">83</td>
  <td class="edition">Var-Matin (Support papier)</td>
</tr>"#;

    #[test]
    fn a_listing_row_gives_the_id_the_buyer_the_date_and_the_department() {
        let lignes = parse_lignes(LISTE);
        assert_eq!(lignes.len(), 1);
        assert_eq!(lignes[0].id, "NM24131");
        assert_eq!(lignes[0].acteur, "MAIRIE DE LA SEYNE-SUR-MER - CARBURANTS");
        assert_eq!(lignes[0].departement, "83");
        assert_eq!(
            lignes[0].date_publication,
            Some(jiff::civil::date(2026, 9, 14))
        );
    }

    #[test]
    fn the_last_numbered_link_is_the_page_count() {
        assert_eq!(pages(LISTE), 14);
        assert_eq!(pages("<p>rien</p>"), 1);
    }

    /// The three wordings and the three date forms met on the archive.
    #[test]
    fn the_deadline_is_read_through_its_wordings() {
        let cas = [
            "Remise des offres : 30/09/26 à 12h00 au plus tard",
            "Date limite de remise des offres : 6 novembre 2026 à 16h30",
            "Date limite de dépôt des candidatures le 29/09/2026",
        ];
        for c in cas {
            assert!(echeance(c).is_some(), "échéance non lue dans « {c} »");
        }
        assert!(echeance("Avis d'annulation de la consultation.").is_none());
    }

    #[test]
    fn the_object_is_read_from_its_heading_and_stops_at_its_sentence() {
        assert_eq!(
            objet("POUVOIR ADJUDICATEUR Mairie. OBJET DE LA CONSULTATION La présente consultation concerne la fourniture de carburants. DUREE DU MARCHE Le marché prendra effet")
                .as_deref(),
            Some("La présente consultation concerne la fourniture de carburants")
        );
    }

    #[test]
    fn a_labelled_object_wins_over_the_heading() {
        assert_eq!(
            objet("Objet du marché : Marché public de service portant sur la gestion de la piscine. Lot unique.")
                .as_deref(),
            Some("Marché public de service portant sur la gestion de la piscine")
        );
    }

    /// The stop behind `L.` ends no sentence: the word before it is too short.
    #[test]
    fn an_abbreviation_does_not_end_the_sentence() {
        assert_eq!(
            premiere_phrase(
                "En application de l'article L. 2122-1-1 du CGPPP, la Métropole organise. Ensuite"
            ),
            "En application de l'article L. 2122-1-1 du CGPPP, la Métropole organise"
        );
    }

    /// A bare domain is the usual form of the buyer profile in a press ad, and
    /// it is a door only once it is an URL.
    #[test]
    fn a_bare_domain_becomes_an_url() {
        assert_eq!(
            liens("Adresse du profil d'acheteur : www.marches-securises.fr."),
            vec!["https://www.marches-securises.fr"]
        );
    }

    /// Real ad NM24162, the labelled form: the lots are bare lines under the
    /// form of the procedure, and the criteria name none.
    const ANNONCE: &str = r#"<div class="texte">
AVIS DE PUBLICITE<br/>
MAIRIE DE LORGUES<br/>
R&eacute;f&eacute;rence acheteur : 2026_09_MAPAS<br/>
Objet : ENTRETIEN DES RESEAUX HUMIDES DE LA VILLE DE LORGUES<br/>
Proc&eacute;dure : Proc&eacute;dure adapt&eacute;e<br/>
Forme de la proc&eacute;dure : Prestation divis&eacute;e en lots : oui<br/>
Lot N° 01 - Entretien du r&eacute;seau pluvial<br/>
Lot N° 02 - Entretien des r&eacute;seaux d'assainissement, poste de rel&egrave;vement<br/>
Crit&egrave;res d'attribution : Offre &eacute;conomiquement la plus avantageuse appr&eacute;ci&eacute;e en fonction des crit&egrave;res &eacute;nonc&eacute;s dans le cahier des charges.<br/>
Remise des offres : 12/10/26 &agrave; 09h00 au plus tard.<br/>
</div>"#;

    #[test]
    fn the_lots_are_the_bare_lines_under_the_form_of_the_procedure() {
        let c = contenu(ANNONCE);
        assert_eq!(
            c.description.as_deref(),
            Some("ENTRETIEN DES RESEAUX HUMIDES DE LA VILLE DE LORGUES")
        );
        assert_eq!(
            c.lots
                .iter()
                .filter_map(|l| l.nom.as_deref())
                .collect::<Vec<_>>(),
            [
                "Lot N° 01 - Entretien du réseau pluvial",
                "Lot N° 02 - Entretien des réseaux d'assainissement, poste de relèvement",
            ]
        );
        assert!(
            c.criteres.is_empty(),
            "a referral to the specifications names no criterion"
        );
    }

    /// A sale of three plots numbers its lots too, and its visiting hours would
    /// enter as three of them.
    #[test]
    fn an_ad_that_states_no_form_of_procedure_yields_no_lot() {
        let vente = r#"<div class="texte">
VENTE AMIABLE<br/>
La Ville de Tende a d&eacute;cid&eacute; la mise en vente de 3 lots.<br/>
Une visite est pr&eacute;vue le lundi 21 Septembre 2026 :<br/>
Lot 3 : de 9h30 &agrave; 11h30 (Granile)<br/>
Lot 2 : de 13h30 &agrave; 15 h00 (Sainte Catherine)<br/>
</div>"#;
        assert!(contenu(vente).lots.is_empty());
    }
}
