use std::sync::LazyLock;
use std::time::Duration;

use jiff::Timestamp;
use regex::Regex;

use lj_annonces::zones::zones_departements;
use lj_annonces::{
    cles_jointure_depuis_urls, departement_from_cp, echeance_ts, AnnonceBrute, ContenuAnnonce,
};
use lj_core::dates::parse_date_fr;

use super::{client, dedup_source_uid, fiche, send_text, txt, DetailAnnonce, MoteurRun};
use lj_sources::error::Result;

/// The apex and never `www.`: the subdomain redirect drops the query string, so
/// `?page=2` lands as `marches-publicspage=2` and answers 404.
const BASE: &str = "https://lacentraledesmarches.com";
const LISTE: &str = "https://lacentraledesmarches.com/marches-publics";

const POLITESSE: Duration = Duration::from_secs(1);

/// Hard stop behind the date window below, past the pages the whole stock fills.
const MAX_PAGES: u64 = 900;

/// Days of publication the daily run walks: it only needs what appeared since
/// the last one, and this window is its tolerance to a missed run. Deeper, the
/// listing republishes BOAMP and JOUE, which we hold at the source.
pub const FENETRE_JOURS: i32 = 7;

/// Failed pages in a row before dropping the engine. One lost page is a hole of
/// 25 consultations, replayed the next night; a streak is a host that closed,
/// and insisting costs 600 requests for nothing.
const ECHECS_SUITE: u32 = 5;

/// A card with no readable closing date, an award notice most of the time,
/// never enters the open window of the corpus.
fn ouverte(r: &AnnonceBrute, maintenant: Timestamp) -> bool {
    r.echeance.is_some_and(|e| e > maintenant)
}

fn champ(re: &Regex, bloc: &str) -> Option<String> {
    re.captures(bloc)
        .map(|c| txt(&c[1]).trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse_page(html: &str) -> Vec<AnnonceBrute> {
    const CARTE: &str = r#"id="annonce_"#;
    static TITRE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<h2 class="annonceTitle">(.*?)</h2>"#).expect("regex titre valide")
    });
    static LIEU: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<span class="color-blue">(.*?)</span>"#).expect("regex lieu valide")
    });
    static ACHETEUR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)Acheteur\s*:\s*(.*?)</div>").expect("regex acheteur"));
    static CLOTURE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Date de clôture estimée\s*:\s*(\d{2}/\d{2}/\d{4})").expect("regex clôture")
    });
    static PUBLICATION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Date de publication\s*:\s*(\d{2}/\d{2}/\d{4})").expect("regex publication")
    });
    static REFERENCE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)Référence\s*:\s*(.*?)</div>").expect("regex référence"));
    // A republished card names the original notice and its identifier, in the
    // very space of our `source_uid`: `boamp:26-88571`, `ted:629157-2026`. An
    // exact join, with no detail page to read.
    static ORIGINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r">(BOAMP|JOUE)</a>\s*-\s*([0-9A-Za-z][0-9A-Za-z\-]*)").expect("regex origine")
    });
    static CP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b\d{5}\b").expect("regex code postal valide"));

    let mut out = Vec::new();
    for bloc in html.split(CARTE).skip(1) {
        let id = bloc.split('"').next().unwrap_or_default();
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Some(objet) = champ(&TITRE, bloc) else {
            continue;
        };
        let cles = ORIGINE
            .captures(bloc)
            .map(|c| {
                let canal = if &c[1] == "BOAMP" { "boamp" } else { "ted" };
                vec![format!("{canal}:{}", &c[2])]
            })
            .unwrap_or_default();
        out.push(AnnonceBrute {
            contenu: ContenuAnnonce::default(),
            descripteurs: Vec::new(),
            moteur: "centraledesmarches",
            source_uid: format!("centraledesmarches:{id}"),
            cles_jointure: cles,
            reference: champ(&REFERENCE, bloc).map(|r| r.chars().take(120).collect()),
            acheteur: champ(&ACHETEUR, bloc),
            siren: None,
            objet: objet.chars().take(400).collect(),
            nature: None,
            cpv: None,
            // The label names the département after the postcode, but only the
            // postcode tells 2A from 2B.
            zones: zones_departements(
                champ(&LIEU, bloc)
                    .as_deref()
                    .and_then(|l| CP.find(l).map(|m| m.as_str().to_string()))
                    .as_deref()
                    .and_then(departement_from_cp),
            ),
            date_publication: champ(&PUBLICATION, bloc).as_deref().and_then(parse_date_fr),
            // The engine gives the closing date as estimated: it bounds the open
            // window, the original channel rules on the hour.
            echeance: champ(&CLOTURE, bloc)
                .as_deref()
                .and_then(parse_date_fr)
                .map(|d| echeance_ts(d, None)),
            url_avis: Some(format!("{BASE}/marches/{id}")),
            url_rc: None,
            url_dce: None,
            url_depot: None,
            procedure: None,
        });
    }
    out
}

/// Walks the listing page by page. It sorts by publication descending, so the
/// walk stops on the first page that crosses the window, and the pagination
/// keeps going long past the open stock (2 open cards of 25 on page 600).
pub async fn enumerate(jours: i32) -> Result<MoteurRun> {
    let client = client();
    let maintenant = Timestamp::now();
    let depuis = jiff::Zoned::now()
        .date()
        .saturating_sub(jiff::Span::new().days(jours));
    let mut rows: Vec<AnnonceBrute> = Vec::new();
    let mut echecs = 0;
    let mut suite = 0;
    for p in 1..=MAX_PAGES {
        if p > 1 {
            tokio::time::sleep(POLITESSE).await;
        }
        let url = format!("{LISTE}?page={p}");
        let html = match send_text(client.get(&url), &format!("centraledesmarches page {p}")).await
        {
            Ok(h) => h,
            Err(e) => {
                tracing::debug!(url = %url, error = %e, "centraledesmarches: page failed");
                echecs += 1;
                suite += 1;
                if suite >= ECHECS_SUITE {
                    break;
                }
                continue;
            }
        };
        suite = 0;
        let lignes = parse_page(&html);
        if lignes.is_empty() {
            break;
        }
        let franchie = lignes
            .iter()
            .filter_map(|r| r.date_publication)
            .min()
            .is_some_and(|d| d < depuis);
        rows.extend(lignes.into_iter().filter(|r| ouverte(r, maintenant)));
        if franchie {
            break;
        }
    }

    Ok(MoteurRun {
        // The counter served atop the listing counts award and closed notices
        // too: against the 90 % floor it would call the engine degraded.
        annonce: None,
        echecs,
        hotes: Vec::new(),
        rows: dedup_source_uid(rows),
    })
}

/// The keys a consultation URL yields, for the join with the notices citing it
/// as their deposit.
pub fn cles_de(url: &str) -> Vec<String> {
    cles_jointure_depuis_urls(std::iter::once(url))
}

const DESCRIPTIONS: [&str; 6] = [
    "description succincte du marche",
    "objet du marche",
    "objet",
    "objet complet",
    "caracteristiques principales",
    "intitule du marche",
];

fn champs_fiche(html: &str) -> DetailAnnonce {
    let champs = fiche::champs(html);
    DetailAnnonce {
        // A card that lists its lots first puts a lot CPV ahead of the market's,
        // so the explicitly principal label is read before the positional rule.
        cpv: champs
            .iter()
            .find(|c| c.cle == "code cpv principal")
            .or_else(|| {
                champs
                    .iter()
                    .take_while(|c| !fiche::est_lot(&c.cle))
                    .find(|c| c.cle.starts_with("code cpv") || c.cle == "objet principal")
            })
            .and_then(|c| fiche::cpv_de(&c.valeur)),
        procedure: champs
            .iter()
            .find(|c| c.cle == "type de procedure" || c.cle == "mode de passation")
            .map(|c| c.valeur.trim_end_matches('.').to_string())
            .filter(|p| !p.is_empty()),
        acheteur: None,
        zones: Vec::new(),
        contenu: fiche::contenu(&champs, fiche::premier(&champs, &DESCRIPTIONS)),
    }
}

/// The card mirrors the original notice in full: for the consultations it is the
/// only channel of, nothing else carries description, allotment or criteria.
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    tokio::time::sleep(POLITESSE).await;
    let html = send_text(client().get(url_avis), "centraledesmarches fiche").await?;
    Ok(champs_fiche(&html))
}

#[cfg(test)]
mod tests {
    use lj_annonces::zones::codes;
    use lj_annonces::{LotAnnonce, MontantEstime};

    use super::*;

    /// A real card for a notice published in the press: no original notice, an
    /// insert reference, and the publisher's logo.
    const CARTE_PRESSE: &str = r#"id="annonce_610799">
	<div class="card-header annonce-header">
		<div class="row">
			<div class="col"><h2 class="annonceTitle">Accord-cadre de prestations d'entretien des réseaux d'assainissement</h2></div>
		</div>
	</div>
	<div class="card-body">
		<div class="font-weight-500" style="color:#17B495">
			<i class="fas fa-map-marker-alt"></i>&nbsp;&nbsp;<span class="color-blue">A proximité de Vitré cedex 35506 Ille Et Vilaine</span>
		</div>
		<div class="font-weight-500" style="color:#ef8d00">
		<i class="far fa-clock"></i>&nbsp;&nbsp;Reste 22 jours - Date de clôture estimée : 06/10/2026&nbsp;&nbsp;
		</div>
		<div class="font-weight-500">
			<i class="fas fa-building"></i>&nbsp;&nbsp;Acheteur : Mairie de Vitré</div>
		<div class="rounded p-3 mt-3">
			<div>Type d'avis : Avis de marché</div>
			<div>Type de marché : National</div>
			<div>Date de publication : 14/09/2026</div>
			<div>
				<div>Référence : 74718679</div>
					<div>
					Publié dans :
					<div>OUEST-FRANCE (14/09/2026)</div>
					</div>
				</div>
		</div>
	</div>
</div>"#;

    /// A real card for a BOAMP republication: the original notice identifier
    /// takes the place of the reference.
    const CARTE_BOAMP: &str = r#"id="annonce_612288">
	<div class="card-header annonce-header">
		<div class="row">
			<div class="col"><h2 class="annonceTitle">Restructuration et extension du centre d'incendie et de secours de Luçon</h2></div>
		</div>
	</div>
	<div class="card-body">
		<div class="font-weight-500" style="color:#17B495">
			<i class="fas fa-map-marker-alt"></i>&nbsp;&nbsp;<span class="color-blue">A proximité de Chasnais 85400 Vendée</span>
		</div>
		<div class="font-weight-500" style="color:#ef8d00">
		<i class="far fa-clock"></i>&nbsp;&nbsp;Reste 30 jours - Date de clôture estimée : 14/10/2026&nbsp;&nbsp;
		</div>
		<div class="font-weight-500">
			<i class="fas fa-building"></i>&nbsp;&nbsp;Acheteur : SDIS 85</div>
		<div class="rounded p-3 mt-3">
			<div>Type d'avis : Avis de marché</div>
			<div>Type de marché : National</div>
			<div>Date de publication : 14/09/2026</div>
			<div>
				<div><a href="https://www.boamp.fr/pages/avis/?q=idweb:%2226-88571%22" target="_blank">BOAMP</a> - 26-88571</div>
					</div>
		</div>
	</div>
</div>"#;

    #[test]
    fn press_card_carries_buyer_zone_and_reference() {
        let r = &parse_page(CARTE_PRESSE)[0];
        assert_eq!(r.source_uid, "centraledesmarches:610799");
        assert_eq!(
            r.objet,
            "Accord-cadre de prestations d'entretien des réseaux d'assainissement"
        );
        assert_eq!(r.acheteur.as_deref(), Some("Mairie de Vitré"));
        assert_eq!(codes(&r.zones), ["35"]);
        assert_eq!(r.reference.as_deref(), Some("74718679"));
        assert_eq!(r.date_publication, Some(jiff::civil::date(2026, 9, 14)));
        assert_eq!(
            r.echeance,
            Some(echeance_ts(jiff::civil::date(2026, 10, 6), None))
        );
        assert_eq!(
            r.url_avis.as_deref(),
            Some("https://lacentraledesmarches.com/marches/610799")
        );
        assert!(
            r.cles_jointure.is_empty(),
            "a press notice has no original notice to cite"
        );
    }

    #[test]
    fn republication_cites_its_original_notice() {
        let boamp = &parse_page(CARTE_BOAMP)[0];
        assert_eq!(boamp.cles_jointure, ["boamp:26-88571"]);
        assert_eq!(boamp.acheteur.as_deref(), Some("SDIS 85"));
        assert_eq!(codes(&boamp.zones), ["85"]);
        assert_eq!(boamp.reference, None);

        let joue = CARTE_BOAMP
            .replace("id=\"annonce_612288\"", "id=\"annonce_611895\"")
            .replace(
                r#"<a href="https://www.boamp.fr/pages/avis/?q=idweb:%2226-88571%22" target="_blank">BOAMP</a> - 26-88571"#,
                r#"<a href="https://ted.europa.eu/udl?uri=TED:NOTICE:629157-2026:TEXT:FR:HTML" target="_blank">JOUE</a> - 629157-2026"#,
            );
        assert_eq!(parse_page(&joue)[0].cles_jointure, ["ted:629157-2026"]);
    }

    #[test]
    fn card_without_closing_date_stays_out_of_the_open_window() {
        let maintenant = echeance_ts(jiff::civil::date(2026, 9, 14), None);
        let sans = CARTE_PRESSE.replace("Date de clôture estimée : 06/10/2026", "");
        let r = &parse_page(&sans)[0];
        assert_eq!(r.echeance, None);
        assert!(!ouverte(r, maintenant));
        assert!(parse_page(CARTE_PRESSE)
            .iter()
            .all(|r| ouverte(r, maintenant)));
    }

    #[test]
    fn two_cards_of_a_page_read_apart() {
        let page = format!("{CARTE_PRESSE}\n{CARTE_BOAMP}");
        let rows = parse_page(&page);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].acheteur.as_deref(), Some("Mairie de Vitré"));
        assert_eq!(rows[1].acheteur.as_deref(), Some("SDIS 85"));
    }

    /// A press layout: the criteria are a preamble then one bullet a line, and
    /// the engine's own sector sentence sits beside the mirrored notice.
    const FICHE_PRESSE: &str = r#"<div class="details-cpv p-3">
		Secteurs d'activité
		<div class="mt-3">
			Service de gestion du réseau d'assainissement.</div>
	</div>
	<div class="tab-pane">
	<strong>Objet du marché : </strong>travaux AEP - Programme 2026.<br>
	<strong>Type de procédure : </strong>procédure adaptée ouverte.<br>
	<strong>Objet principal : </strong>75200000-8.<br>
	<strong>Montant maximum annuel : </strong>150 000 euros HT sur 7 ans.<br>
	<strong>Critères d'attribution : </strong>offre économiquement la plus avantageuse appréciée en fonction des critères énoncés ci-dessous avec leur pondération :<br>
	- prix des prestations : 30 %,<br>
	- valeur technique : 65 %,<br>
	- performance environnementale : 5 %.<br>
	Le détail des sous-critères est donné à l'article 8.2 du RC.<br>
	<strong>Délai minimum de validité des offres :</strong> 3 mois.<br>
	</div>"#;

    /// An allotted notice: each lot is followed by its own CPV, and the criteria
    /// block is repeated identically under every one of them.
    const FICHE_LOTS: &str = r#"<div class="tab-pane">
	<strong>Code CPV : </strong>Principal : 45262210-6<br>
	<strong>Lot n°1 : </strong>LOT 01 FONDATIONS PROFONDES.<br>
	<strong>Code CPV : </strong>Principal : 45262210-6<br>
	<strong>Critères d'attribution : </strong>: Critère de qualité 1 : Capacité technique et professionnelle,Pondération : 25, cf règlement de consultation.<br>
	<strong>Lot n°2 : </strong>LOT 02 GROS OEUVRE.<br>
	<strong>Code CPV : </strong>Principal : 45223220-4<br>
	<strong>Critères d'attribution : </strong>: Critère de qualité 1 : Capacité technique et professionnelle,Pondération : 25, cf règlement de consultation.<br>
	</div>"#;

    #[test]
    fn press_notice_yields_its_object_amount_and_weighted_criteria() {
        let d = champs_fiche(FICHE_PRESSE);
        assert_eq!(
            d.contenu.description.as_deref(),
            Some("travaux AEP - Programme 2026.")
        );
        assert_eq!(d.cpv.as_deref(), Some("75200000-8"));
        assert_eq!(d.procedure.as_deref(), Some("procédure adaptée ouverte"));
        assert_eq!(
            d.contenu.montant_estime,
            Some(MontantEstime {
                valeur: 150_000.0,
                devise: Some("EUR".to_string()),
            })
        );
        assert_eq!(
            d.contenu.criteres,
            [
                "prix des prestations (30 %)",
                "valeur technique (65 %)",
                "performance environnementale (5 %)",
            ]
        );
        assert!(d.contenu.lots.is_empty(), "l'avis n'est pas alloti");
    }

    #[test]
    fn each_lot_takes_the_cpv_that_follows_it() {
        let d = champs_fiche(FICHE_LOTS);
        assert_eq!(
            d.contenu.lots,
            [
                LotAnnonce {
                    nom: Some("LOT 01 FONDATIONS PROFONDES".to_string()),
                    description: None,
                    cpv: Some("45262210-6".to_string()),
                },
                LotAnnonce {
                    nom: Some("LOT 02 GROS OEUVRE".to_string()),
                    description: None,
                    cpv: Some("45223220-4".to_string()),
                },
            ]
        );
        assert_eq!(
            d.cpv.as_deref(),
            Some("45262210-6"),
            "le CPV du marché est celui qui précède le premier lot"
        );
    }

    /// A press layout hangs the whole allotment under one label, and repeats a
    /// nameless summary of it beside: the named prose is the one to read.
    const FICHE_LOTS_EN_PROSE: &str = r#"<div class="tab-pane">
	<strong>Lot 1</strong><br>
	<strong>Lot 2</strong><br>
	<strong>5. N° et désignation des lots : </strong><br>
	Lot 01 : aménagements extérieurs<br>
	Lot 02 : couverture tuiles.<br>
	Possibilité de soumissionner pour plusieurs lots.<br>
	<strong>6. Critères d'attribution : </strong>l'offre économiquement la plus avantageuse sera appréciée en fonction des critères pondérés énoncés ci-dessous :<br>
	- prix des prestations : sur 50 %,<br>
	- valeur technique : sur 50 %.<br>
	</div>"#;

    #[test]
    fn named_lots_in_prose_beat_the_nameless_summary() {
        let d = champs_fiche(FICHE_LOTS_EN_PROSE);
        assert_eq!(
            d.contenu
                .lots
                .iter()
                .filter_map(|l| l.nom.as_deref())
                .collect::<Vec<_>>(),
            [
                "Lot 01 : aménagements extérieurs",
                "Lot 02 : couverture tuiles"
            ]
        );
        assert_eq!(
            d.contenu.criteres,
            ["prix des prestations (50 %)", "valeur technique (50 %)"]
        );
    }

    #[test]
    fn a_criteria_block_repeated_under_every_lot_counts_once() {
        assert_eq!(
            champs_fiche(FICHE_LOTS).contenu.criteres,
            ["Capacité technique et professionnelle (25)"]
        );
    }

    /// The sector sentence sits outside the notice, in the engine's own summary:
    /// it is the label of the CPV code the row already carries, not a description
    /// of the market.
    #[test]
    fn la_phrase_de_secteur_n_est_pas_une_description() {
        let sans_objet = FICHE_PRESSE.replace("Objet du marché", "Lieu d'exécution");
        assert_eq!(champs_fiche(&sans_objet).contenu.description, None);
        assert_eq!(champs_fiche(FICHE_LOTS).contenu.description, None);
    }

    #[test]
    fn consultation_url_yields_its_key() {
        assert_eq!(
            cles_de("https://lacentraledesmarches.com/marches/574929"),
            ["centraledesmarches:574929"]
        );
        assert_eq!(
            cles_de(
                "https://lacentraledesmarches.com/marches-publics/Vitre-cedex-Mairie-de-Vitre-Accord-cadre/610799"
            ),
            ["centraledesmarches:610799"]
        );
    }
}
