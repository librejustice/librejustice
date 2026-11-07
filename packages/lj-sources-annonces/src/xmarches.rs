use std::sync::LazyLock;

use futures::StreamExt;
use regex::Regex;

use lj_annonces::{echeance_ts, parse_heure, AnnonceBrute, ContenuAnnonce};
use lj_core::dates::parse_date_fr;

use super::{
    apres_balise, dedup_source_uid, fiche, send_bytes, send_text, txt, DetailAnnonce, MoteurRun,
};
use lj_sources::error::Result;

const BASE: &str = "https://www.xmarches.fr/entreprise";
const MAX_PAGES: u64 = 80;
const FICHES_PARALLELES: usize = 10;
/// Rereads of the list at most: the listing settles within a few, the rest is
/// margin.
const PASSES_MAX: u32 = 6;

fn parse_page(html: &str) -> Vec<AnnonceBrute> {
    static KEY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"detailConsultation\.php\?key=(\d+)").unwrap());
    // The row's first link is the buyer's reference (`2026-01`); the object is the heading.
    static TITRE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<h5[^>]*>(.*?)</h5>").unwrap());
    let mut out = Vec::new();
    for bloc in html.split("<tr").skip(1).map(apres_balise) {
        let Some(k) = KEY.captures(bloc).map(|c| c[1].to_string()) else {
            continue;
        };
        let objet = TITRE
            .captures(bloc)
            .map(|c| txt(&c[1]))
            .unwrap_or_else(|| txt(bloc))
            .chars()
            .take(400)
            .collect::<String>();
        out.push(AnnonceBrute {
            contenu: ContenuAnnonce::default(),
            descripteurs: Vec::new(),
            moteur: "xmarches",
            source_uid: format!("xmarches:{k}"),
            cles_jointure: Vec::new(),
            reference: None,
            acheteur: None,
            siren: None,
            objet,
            nature: None,
            cpv: None,
            zones: Vec::new(),
            date_publication: None,
            echeance: None,
            url_avis: Some(format!("{BASE}/detailConsultation.php?key={k}")),
            url_rc: None,
            url_dce: Some(format!("{BASE}/detailConsultation.php?key={k}")),
            url_depot: None,
            procedure: None,
        });
    }
    out
}

/// Acheteur d'une fiche : le libellé placé juste avant « Vérifier les
/// pré-requis ». Il faut couper après la dernière date ou heure qui le
/// précède : la fiche peut porter une phrase de report (« Ancienne date
/// limite : 19/10/2028 à 21:00:00 ») entre l'échéance et l'acheteur, et un
/// motif ancré sur la première date avale tout.
fn acheteur(flat: &str) -> Option<String> {
    static DATE_OU_HEURE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\d{1,2}[hH:]\d{2}(?::\d{2})?|\d{2}/\d{2}/\d{2,4}").unwrap());
    let fin = flat.find("Vérifier les pré-requis")?;
    let avant = &flat[..fin];
    let debut = DATE_OU_HEURE
        .find_iter(avant)
        .last()
        .map(|m| m.end())
        .unwrap_or(0);
    let nom = avant[debut..].trim().trim_start_matches('.').trim();
    (nom.len() >= 4 && nom.chars().count() <= 160).then(|| nom.to_string())
}

/// Complète une ligne par sa fiche : référence, acheteur, dates.
async fn completer_par_fiche(client: &wreq::Client, mut row: AnnonceBrute) -> AnnonceBrute {
    static REF: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Réf\s*:\s*(.*?)\s*Date de publication").unwrap());
    static PUB: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Date de publication\s*:\s*(\d{2}/\d{2}/\d{4})").unwrap());
    static LIM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"Date limite de remise des plis\s*:\s*(\d{2}/\d{2}/\d{4})(?:\s*à\s*([\dhH:]+))?",
        )
        .unwrap()
    });
    let url = row.url_avis.clone().unwrap_or_default();
    let html = match send_text(client.get(&url), "xmarches fiche").await {
        Ok(h) => h,
        // Fiche en échec : la ligne de liste reste servie sans dates plutôt que
        // d'être perdue.
        Err(e) => {
            tracing::debug!(url = %url, error = %e, "xmarches: record failed");
            return row;
        }
    };
    let flat = txt(&html);
    row.reference = REF
        .captures(&flat)
        .map(|c| c[1].chars().take(120).collect::<String>())
        .filter(|r| !r.is_empty());
    row.acheteur = acheteur(&flat);
    row.date_publication = PUB.captures(&flat).and_then(|c| parse_date_fr(&c[1]));
    row.echeance = LIM.captures(&flat).and_then(|c| {
        let d = parse_date_fr(&c[1])?;
        Some(echeance_ts(
            d,
            c.get(2).and_then(|h| parse_heure(h.as_str())),
        ))
    });
    row
}

/// Ré-énumère la liste puis complète chaque ligne par sa fiche.
pub async fn enumerate() -> Result<MoteurRun> {
    let client = super::client();
    send_text(client.get(format!("{BASE}/")), "xmarches accueil").await?;
    let premiere = send_text(
        client.get(format!("{BASE}/liste-des-consultations,1.html")),
        "xmarches liste 1",
    )
    .await?;
    static TOTAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(\d[\d\s]{0,6})\s*résultats?").unwrap());
    let total: u64 = TOTAL
        .captures(&txt(&premiere))
        .and_then(|c| c[1].replace(char::is_whitespace, "").parse().ok())
        .unwrap_or(0);

    let mut rows: Vec<AnnonceBrute> = Vec::new();
    let mut vus = std::collections::HashSet::new();
    let mut absorber = |lot: Vec<AnnonceBrute>, rows: &mut Vec<AnnonceBrute>| {
        for r in lot {
            if vus.insert(r.source_uid.clone()) {
                rows.push(r);
            }
        }
    };
    absorber(parse_page(&premiere), &mut rows);
    for _ in 0..PASSES_MAX {
        let avant = rows.len();
        for p in 1..=MAX_PAGES {
            let html = send_text(
                client.get(format!("{BASE}/liste-des-consultations,{p}.html")),
                &format!("xmarches liste {p}"),
            )
            .await?;
            // Fin de liste : la page est vide. Une page qui n'apprend rien ne
            // l'annonce pas, le réordonnancement en produit au milieu du stock.
            let lot = parse_page(&html);
            if lot.is_empty() {
                break;
            }
            absorber(lot, &mut rows);
        }
        if rows.len() == avant || total > 0 && rows.len() as u64 >= total {
            break;
        }
    }

    let rows: Vec<AnnonceBrute> = futures::stream::iter(rows.into_iter().map(|r| {
        let client = &client;
        async move { completer_par_fiche(client, r).await }
    }))
    .buffer_unordered(FICHES_PARALLELES)
    .collect()
    .await;

    Ok(MoteurRun {
        annonce: (total > 0).then_some(total),
        echecs: 0,
        hotes: Vec::new(),
        rows: dedup_source_uid(rows),
    })
}

/// `Description de la consultation` is the heading of the whole panel, whose
/// first line is the title; the object is the field under it.
const DESCRIPTIONS: [&str; 2] = ["objet de la consultation", "description de la consultation"];

/// Ce qu'une fiche apprend : la procédure, sous le libellé « Mode de
/// passation » — texte nu derrière le `</strong>` du libellé (« Procédure
/// adaptée ouverte », « Appel d'offres ouvert », « Avis d'information »).
///
/// No CPV and no place: the page carries neither, and both are read from the
/// notice PDF it links to.
fn champs_fiche(html: &str) -> DetailAnnonce {
    static PASSATION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Mode de passation\s*:\s*</strong>([^<]+)").unwrap());
    let champs = fiche::champs(html);
    DetailAnnonce {
        contenu: fiche::contenu(&champs, fiche::premier(&champs, &DESCRIPTIONS)),
        zones: Vec::new(),
        cpv: None,
        procedure: PASSATION
            .captures(html)
            .map(|c| txt(&c[1]))
            .filter(|p| !p.is_empty()),
        acheteur: None,
    }
}

/// Downloadable piece of a card, behind a `data-url` the page turns into a
/// download on click. Only a document names a postal address, and the notice is
/// far lighter than a consultation rulebook, which some cards leave as the only
/// one on offer.
fn lien_piece(html: &str, quoi: &str) -> Option<String> {
    let motif = Regex::new(&format!(r#"data-url="([^"]*telequoi={quoi}[^"]*)""#)).ok()?;
    let brut = motif.captures(html)?[1].replace("&amp;", "&");
    Some(format!("{BASE}/{}", brut.trim_start_matches("./")))
}

/// What the notice PDF adds: the buyer's postal address, and the CPV that no
/// page of the engine carries.
fn champs_avis(texte: &str) -> (Vec<lj_annonces::zones::Zone>, Option<String>) {
    static CPV: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)CPV.{0,60}?\b(\d{8}(?:-\d)?)\b").unwrap());
    (
        lj_annonces::zones::zones_departements(lj_annonces::departement_dans_texte(texte)),
        CPV.captures(texte).map(|c| c[1].to_string()),
    )
}

/// Card of a notice: the procedure, read without a session, then the notice PDF
/// it links to. An unreadable PDF still leaves the procedure posed, which takes
/// the line out of the queue for good, so a later field costs a catch-up pass.
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    let client = super::client();
    let html = send_text(client.get(url_avis), "xmarches détail").await?;
    let mut d = champs_fiche(&html);
    for quoi in ["AVIS", "RC"] {
        let Some(url) = lien_piece(&html, quoi) else {
            continue;
        };
        // `pdftotext`, not the in-process extractor: pdf-extract panics on an
        // invalid content stream instead of erroring, which would kill the run.
        let texte = match send_bytes(client.get(&url), "xmarches pièce").await {
            Ok(b) => tokio::task::spawn_blocking(move || lj_sources::pdf::pdftotext_extract(&b))
                .await
                .map_err(|e| {
                    lj_sources::error::SourceError::Invalid(format!("xmarches pièce: {e}"))
                })
                .and_then(|r| r),
            Err(e) => Err(e),
        };
        match texte {
            Ok(texte) => {
                let (zones, cpv) = champs_avis(&texte);
                d.cpv = d.cpv.or(cpv);
                if !zones.is_empty() {
                    d.zones = zones;
                    break;
                }
            }
            Err(e) => tracing::debug!(url = %url, error = %e, "xmarches: attachment unreadable"),
        }
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acheteur_apres_la_derniere_date() {
        let flat = "Détail de la consultation Consultation de Test Réf : TEST \
             Date de publication : 01/10/2018 Date limite de remise des plis : \
             14/10/2027 à 14h52 La date limite a fait l'objet d'une modification \
             le 28/12/23 à 09:15. Ancienne date limite : 19/10/2028 à 21:00:00 \
             Commune de TestVille Vérifier les pré-requis à la réponse électronique";
        assert_eq!(acheteur(flat).as_deref(), Some("Commune de TestVille"));
    }

    #[test]
    fn acheteur_juste_apres_l_echeance() {
        let flat = "Date limite de remise des plis : 30/09/2026 à 17h00 \
             Ville de Reims Vérifier les pré-requis";
        assert_eq!(acheteur(flat).as_deref(), Some("Ville de Reims"));
    }

    #[test]
    fn sans_repere_pas_d_acheteur() {
        assert_eq!(acheteur("Se connecter Mot de passe oublié ?"), None);
    }

    #[test]
    fn procedure_dune_fiche_sans_cpv() {
        let html = r#"<li><strong>Objet de la consultation : </strong>
                        La présente procédure a pour objet l'Assistance à Maîtrise
                        d'Ouvrage.</li>
                    <li>
                        <strong>Mode de passation : </strong>Appel d'offres ouvert                    </li>
                    <li>
                        <strong>Type de marché : </strong>Services                    </li>"#;
        let d = champs_fiche(html);
        assert_eq!(d.cpv, None);
        assert_eq!(d.procedure.as_deref(), Some("Appel d'offres ouvert"));
        assert_eq!(
            d.contenu.description.as_deref(),
            Some("La présente procédure a pour objet l'Assistance à Maîtrise d'Ouvrage.")
        );
    }

    /// Real notice 41539, reduced to the list it hangs its fields on. The
    /// `<li>` closes no value, so the object read beside its label runs to the
    /// end of the list and swallows the three labels that follow it.
    const FICHE_LISTE: &str = r#"<ul class="detail">
                    <li><strong>Titre : </strong>Accord-cadre de fourniture et acheminement d'électricité</li>
                    <li><strong>Objet de la consultation : </strong>
                        Ces prestations comprennent :<br />• la fourniture d'électricité pour les points de livraison<br />• la mission de responsable d'équilibre.</li>
                    <li><strong>Mode de passation : </strong>Procédure adaptée ouverte                    </li>
                    <li><strong>Lot(s) : </strong>
                        <ul class='disc'><li>Lot n°1 : Liste des points de livraison distribués par Enedis (variante autorisée)<div class="text italic padding-left"></div></li><li>Lot n°2 : Liste des points de livraison non distribués par Enedis (variante autorisée)<div class="text italic padding-left"></div></li></ul>                    </li>
                    <li><strong>Critères de jugement des offres : </strong><ul><li>Se reporter au règlement de la consultation.</li></ul>                    </li>
                    </ul>"#;

    #[test]
    fn the_description_stops_at_the_next_label_and_the_lots_are_named() {
        let d = champs_fiche(FICHE_LISTE);
        assert_eq!(
            d.contenu.description.as_deref(),
            Some(
                "Ces prestations comprennent : • la fourniture d'électricité pour les points de \
                 livraison • la mission de responsable d'équilibre."
            )
        );
        assert_eq!(
            d.contenu
                .lots
                .iter()
                .filter_map(|l| l.nom.as_deref())
                .collect::<Vec<_>>(),
            [
                "Liste des points de livraison distribués par Enedis (variante autorisée)",
                "Liste des points de livraison non distribués par Enedis (variante autorisée)",
            ]
        );
        assert!(
            d.contenu.criteres.is_empty(),
            "a referral to the rulebook names no criterion"
        );
    }

    /// Real text of notice 41308, reduced. The first five-digit code that falls
    /// in the referential is the buyer's: neither the year, nor the time, nor
    /// the eight-digit CPV slips in ahead of it.
    #[test]
    fn l_avis_pdf_donne_le_departement_et_le_cpv() {
        let texte = "Date limite de réception des offres :\n\
             Mercredi 16 septembre 2026 à 12h00\n\
             MARCHÉ PUBLIC DE TRAVAUX\n2026CTX09\n\
             Identification de l'organisme qui passe le marché :\n\
             Commune de Contrexéville - 75 rue Gaston Thomson - BP26 - 88140 CONTREXÉVILLE Cedex\n\
             Code CPV principal :\n-  45223800-4 : Assemblage et montage de structures préfabriquées\n";
        let (zones, cpv) = champs_avis(texte);
        assert_eq!(lj_annonces::zones::codes(&zones), ["88"]);
        assert_eq!(cpv.as_deref(), Some("45223800-4"));

        let (zones, cpv) = champs_avis("Nature : Travaux\nDétail : Travaux d'infrastructures\n");
        assert!(zones.is_empty());
        assert_eq!(cpv, None);

        let bp = "L'ACHETEUR Ville de REMIREMONT - 1 place Christian Poncelet - \
             BP 30107 88204 REMIREMONT Cedex Pôle technique";
        assert_eq!(lj_annonces::zones::codes(&champs_avis(bp).0), ["88"]);

        let boamp = "cliquez sur https://www.boamp.fr/pages/avis/?q=idweb:26-65991 \
             Département(s) de publication : 54 Annonce n° 26-65991 Travaux\n\
             Ville : HARAUCOURT Code postal : 54110 Groupement de commandes : Non";
        assert_eq!(lj_annonces::zones::codes(&champs_avis(boamp).0), ["54"]);
    }

    #[test]
    fn le_lien_de_l_avis_sort_du_data_url() {
        let html = r##"<li><a class="color-theme" data-url="./redirect.php?dl&amp;key=41308&amp;us=&amp;id=0&amp;prov=XMAR&amp;telequoi=AVIS&amp;rectifid=" href="#">Télécharger cet avis (863.12 Ko)</a></li>"##;
        assert_eq!(
            lien_piece(html, "AVIS").as_deref(),
            Some(
                "https://www.xmarches.fr/entreprise/redirect.php\
                 ?dl&key=41308&us=&id=0&prov=XMAR&telequoi=AVIS&rectifid="
            )
        );
        assert_eq!(lien_piece("<p>Se connecter</p>", "AVIS"), None);
    }

    #[test]
    fn fiche_muette_rend_un_bloc_vide() {
        assert_eq!(
            champs_fiche("Se connecter Mot de passe oublié ?"),
            DetailAnnonce::default()
        );
    }

    #[test]
    fn residu_de_balise_ecarte() {
        let html = r#"<tr class="ligne" id="l1"><td>
             <a href="./detailConsultation.php?key=42">Travaux de voirie</a></td></tr>"#;
        let rows = parse_page(html);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].objet, "Travaux de voirie");
        assert_eq!(rows[0].source_uid, "xmarches:42");
    }

    #[test]
    fn the_title_is_the_heading_and_not_the_reference() {
        let html = r#"<tr class="trAnnonce"><td class="td_ref">
             <a title="Accéder à la consultation Amélioration énergétique"  href="./detailConsultation.php?key=40942">
             2026-01 </a></td><td class="td_objet_consult">
             <a title="Accéder à la consultation Amélioration énergétique"  href="./detailConsultation.php?key=40942">
             <div class="color-black"><h5 class="color-theme text big normal">Amélioration énergétique
             de l'ancienne école maternelle</h5><span>Travaux de construction</span></div></a></td></tr>"#;
        assert_eq!(
            parse_page(html)[0].objet,
            "Amélioration énergétique de l'ancienne école maternelle"
        );
    }
}
