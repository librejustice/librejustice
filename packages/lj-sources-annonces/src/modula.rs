use std::sync::LazyLock;
use std::time::Duration;

use futures::StreamExt;
use regex::Regex;

use lj_annonces::{echeance_ts, parse_heure, AnnonceBrute, ContenuAnnonce};
use lj_core::dates::parse_date_fr;

use super::{apres_balise, dedup_source_uid, txt, MoteurRun};
use lj_sources::error::{Result, SourceError};

/// Les portails, en hôte canonique (sans `www.`) : c'est la forme que porte le
/// `source_uid`, donc la clé que le BOAMP donne en citant le DCE anonyme.
const PORTAILS: [&str; 6] = [
    "proxilegales.fr",
    "achatspublicscorse.com",
    "marches-demat.com",
    "marchespublics.pro",
    "antilles-legales.fr",
    "laprovencemarchespublics.com",
];

const PAR_PAGE: u64 = 200;
/// Pagination guard, far above the largest filtered listing of a portal.
const MAX_PAGES: u64 = 40;

fn listing_url(host: &str, page: u64) -> String {
    format!(
        "https://www.{host}/publisher_portail/public/annonce/afficherAnnonces.jsp\
         ?type_avis=tout&tri=marche.date_dern_modif%20desc&filtreType=marche.ALL_REF\
         &sSEOperatorValue=AND&bLaunchSearch=true&sIsAnnonceDemat=demat\
         &se_iMaxElementPerPage={PAR_PAGE}&se_bDisplayMapAnnounce=false&numPage={page}"
    )
}

/// La page anonyme du dossier de consultation — celle que l'arrêté du 14 avril
/// 2017 oblige à servir sans compte, et la seule URL stable du produit (le PDF
/// de l'avis est signé par un jeton de rendu). C'est mot pour mot la forme sous
/// laquelle les avis BOAMP citent le profil.
fn url_dce(host: &str, id: &str) -> String {
    format!(
        "https://www.{host}/publisher_portail/public/annonce/\
         consulterDCEHorsIdentification.jsp?iIdAffaire={id}"
    )
}

/// Décodage **ISO-8859-1**, l'encodage que le produit impose
/// (`Content-Type: text/html;charset=ISO-8859-1` ; `Accept-Charset: utf-8` n'y
/// change rien). Le client du workspace est bâti sans le décodeur de charset de
/// reqwest : `send_text` rendrait un `U+FFFD` par octet accentué. Les 256 octets
/// du latin-1 sont exactement les points de code U+0000..U+00FF, donc le mapping
/// direct **est** le décodage (même frontière que `ariane.rs`).
fn latin1(octets: &[u8]) -> String {
    octets.iter().map(|&o| o as char).collect()
}

/// Charge une page de listing. Mêmes reprises que `send_text` — 3 essais,
/// backoff ×2 sur 429/5xx — mais sur les octets, que `latin1` décode.
async fn charge(client: &wreq::Client, url: String, quoi: &str) -> Result<String> {
    const ESSAIS: u32 = 3;
    let mut derniere = String::new();
    for essai in 0..ESSAIS {
        if essai > 0 {
            tokio::time::sleep(Duration::from_millis(500 << essai)).await;
        }
        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => match r.bytes().await {
                Ok(b) => return Ok(latin1(&b)),
                Err(e) => derniere = format!("lecture du corps: {e}"),
            },
            Ok(r) if r.status().as_u16() == 429 || r.status().is_server_error() => {
                derniere = format!("HTTP {}", r.status());
            }
            Ok(r) => return Err(SourceError::Invalid(format!("{quoi}: HTTP {}", r.status()))),
            Err(e) => derniere = e.to_string(),
        }
    }
    Err(SourceError::Invalid(format!(
        "{quoi}: {derniere} après {ESSAIS} essais"
    )))
}

/// Valeur d'un champ du bloc, du libellé en gras jusqu'à la fin de cellule
/// (`</td>`) ou de ligne (`</tr>`) : l'objet vit dans la cellule voisine de son
/// libellé, les autres champs dans la même.
fn champ(bloc: &str, motif: &Regex) -> Option<String> {
    motif
        .captures(bloc)
        .map(|c| txt(&c[1]))
        .filter(|v| !v.is_empty())
}

/// Acheteur et référence, portés ensemble par l'en-tête du bloc :
/// « MAIRIE DE SAINT-OMER - 2026/22-25 (AAPC) ». On retire le type d'avis en
/// queue, puis on coupe au **premier** séparateur : quand un en-tête en porte
/// plusieurs, c'est la référence qui est composée (« MAIRIE DE HOULLE - 21 - 017
/// - HOU »), jamais l'acheteur.
fn entete(bloc: &str) -> (Option<String>, Option<String>) {
    static ENTETE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)class="post-title-alt"[^>]*>(.*?)</span>"#).expect("regex en-tête valide")
    });
    // Le type d'avis publié : AAPC (appel public à concurrence), AATR
    // (attribution)…
    static TYPE_AVIS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*\([A-Z]{3,6}\)\s*$").expect("regex type d'avis valide"));
    // Identifiant interne de l'éditeur, collé au nom de l'acheteur —
    // « MAIRIE DE ROUVROY (00180330) » : six ou huit chiffres selon le portail,
    // jamais les neuf d'un SIREN. Le seuil à cinq laisse passer un millésime
    // entre parenthèses, qui appartient au nom.
    static ID_EDITEUR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*\(\d{5,}\)\s*$").expect("regex id éditeur valide"));

    let Some(brut) = ENTETE.captures(bloc).map(|c| txt(&c[1])) else {
        return (None, None);
    };
    let titre = TYPE_AVIS.replace(&brut, "");
    let (acheteur, reference) = match titre.split_once(" - ") {
        Some((a, r)) => (a, Some(r.trim().to_string())),
        None => (titre.as_ref(), None),
    };
    let acheteur = ID_EDITEUR.replace(acheteur.trim(), "").trim().to_string();
    (
        (!acheteur.is_empty()).then_some(acheteur),
        reference.filter(|r| !r.is_empty()),
    )
}

/// Date française du produit — « 7 Aout 2026 à 11:30 heures ».
fn date_et_heure(valeur: &str) -> Option<(jiff::civil::Date, Option<jiff::civil::Time>)> {
    static DATE_HEURE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(\d{1,2}\s+[\p{L}.]+\s+\d{4})(?:\s*à\s*(\d{1,2}[h:]\d{2}))?")
            .expect("regex date française valide")
    });
    let c = DATE_HEURE.captures(valeur)?;
    let date = parse_date_fr(&c[1])?;
    Some((date, c.get(2).and_then(|h| parse_heure(h.as_str()))))
}

/// Les consultations **natives** d'une page de listing. Les deux natures de
/// relais et les avis d'attribution n'ont pas de DCE anonyme à servir : ils
/// tombent sur ce seul test.
fn parse_page(html: &str, host: &str) -> Vec<AnnonceBrute> {
    static DCE_ANONYME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"popupTelechargementDCEAnonyme\.jsp[^"']*?[?&]idMarche=(\d+)"#)
            .expect("regex DCE anonyme valide")
    });
    static OBJET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<strong>\s*Objet\s*:\s*</strong>(.*?)</tr>").expect("regex objet valide")
    });
    static PROCEDURE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<strong>\s*Type de procédure\s*:\s*</strong>(.*?)</td>")
            .expect("regex procédure valide")
    });
    static PUBLICATION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<strong>\s*Date de publication\s*:\s*</strong>(.*?)</td>")
            .expect("regex publication valide")
    });
    // Une procédure restreinte borne la remise des candidatures, pas des plis.
    static ECHEANCE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?s)<strong>\s*Date limite de reception des (?:plis|candidatures)\s*:\s*</strong>(.*?)</td>",
        )
        .expect("regex échéance valide")
    });

    let mut out = Vec::new();
    for bloc in html.split(r#"<div class="post""#).skip(1).map(apres_balise) {
        let Some(id) = DCE_ANONYME.captures(bloc).map(|c| c[1].to_string()) else {
            continue;
        };
        let (acheteur, reference) = entete(bloc);
        let url = url_dce(host, &id);
        out.push(AnnonceBrute {
            contenu: ContenuAnnonce::default(),
            descripteurs: Vec::new(),
            moteur: "modula",
            source_uid: format!("modula:{host}:{id}"),
            cles_jointure: Vec::new(),
            reference,
            acheteur,
            siren: None,
            objet: champ(bloc, &OBJET).unwrap_or_default(),
            nature: None,
            cpv: None,
            zones: Vec::new(),
            date_publication: champ(bloc, &PUBLICATION)
                .and_then(|v| date_et_heure(&v))
                .map(|(d, _)| d),
            echeance: champ(bloc, &ECHEANCE)
                .and_then(|v| date_et_heure(&v))
                .map(|(d, h)| echeance_ts(d, h)),
            url_avis: Some(url.clone()),
            url_rc: None,
            url_dce: Some(url),
            url_depot: None,
            procedure: champ(bloc, &PROCEDURE),
        });
    }
    out
}

/// Un portail, page par page. Une page sans aucun bloc d'annonce est la fin de
/// la liste : le produit ne publie son total qu'en français, dans une phrase.
async fn enumerate_portail(client: &wreq::Client, host: &str) -> Result<Vec<AnnonceBrute>> {
    let mut rows = Vec::new();
    for page in 0..MAX_PAGES {
        let html = charge(
            client,
            listing_url(host, page),
            &format!("modula {host} page {page}"),
        )
        .await?;
        if !html.contains(r#"<div class="post""#) {
            break;
        }
        rows.extend(parse_page(&html, host));
    }
    Ok(rows)
}

/// Ré-énumère les six portails. Un portail en échec est un échec compté, pas
/// la perte du run : ce sont six sites indépendants derrière une même
/// application.
pub async fn enumerate() -> Result<MoteurRun> {
    let client = super::client();
    let resultats: Vec<(&str, Result<Vec<AnnonceBrute>>)> =
        futures::stream::iter(PORTAILS.iter().map(|host| {
            let client = &client;
            async move { (*host, enumerate_portail(client, host).await) }
        }))
        .buffer_unordered(PORTAILS.len())
        .collect()
        .await;
    let mut run = MoteurRun {
        // Le total du produit compte les annonces légales, les avis
        // d'attribution et les relais des portails voisins — rien de
        // comparable au collecté.
        annonce: None,
        echecs: 0,
        hotes: Vec::new(),
        rows: Vec::new(),
    };
    for (host, res) in resultats {
        match res {
            Ok(rows) => {
                tracing::debug!(host, natifs = rows.len(), "modula: portal enumerated");
                run.rows.extend(rows);
            }
            Err(e) => {
                run.echecs += 1;
                tracing::warn!(host, error = %e, "modula: portal failed");
            }
        }
    }
    run.rows = dedup_source_uid(run.rows);
    Ok(run)
}

/// Notice PDF of a consultation, behind a download link the DCE page signs with
/// a render token: it is only valid for the session that read the page, so the
/// two fetches share one client.
fn lien_avis(host: &str, page: &str) -> Option<String> {
    static TELECHARGER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?:href='|window\.open\(")(/publisher_portail/DownloadFilePublisher[^'"]+)"#)
            .expect("regex téléchargement valide")
    });
    let brut = TELECHARGER.captures(page)?[1].replace("&amp;", "&");
    Some(format!("https://www.{host}{brut}"))
}

/// The notice is the standard form rendered to PDF, and the rendering keeps
/// three things the shared reader cannot use: a bold run carrying a section
/// heading and the label under it at once, a word cut at the end of a line, and
/// a line break inside a sentence. Only the break between two fields survives.
fn deplie(html: &str) -> String {
    static CESURE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\p{L})-<br\s*/?>\s*(\p{Ll})").expect("regex césure valide"));
    static GRAS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<b>(.*?)</b>").expect("regex gras valide"));
    static SAUT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").expect("regex saut valide"));
    // The renderer spaces a justified line with `&#160;`, whose `;` would pass
    // for the end of a sentence in the fold below.
    static INSECABLE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)&(?:nbsp|#160|#xa0);").expect("regex espace insécable valide")
    });
    let espace = INSECABLE.replace_all(html, " ");
    let recolle = CESURE.replace_all(&espace, "$1$2");
    let etiquettes = GRAS.replace_all(&recolle, |c: &regex::Captures| {
        SAUT.split(&c[1])
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| format!("<b>{l}</b>"))
            .collect::<Vec<_>>()
            .join("<br/>")
    });
    let mut lignes: Vec<String> = Vec::new();
    for morceau in SAUT
        .split(&etiquettes)
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        // A line wrapped by the renderer opens in lowercase, where a line the
        // form itself opened starts on a label or a capital.
        let suite = morceau.starts_with(char::is_lowercase)
            && lignes
                .last()
                .is_some_and(|p| !p.ends_with(['>', '.', ':', ';', '!', '?']));
        match suite {
            true => {
                let precedente = lignes.last_mut().expect("garde ci-dessus");
                precedente.push(' ');
                precedente.push_str(morceau);
            }
            false => lignes.push(morceau.to_string()),
        }
    }
    lignes.join("<br/>")
}

/// What a consultation adds off the listing: the place, which the product
/// publishes nowhere else, and everything the notice form carries that its
/// listing row does not.
pub async fn detail(url_avis: &str) -> Result<super::DetailAnnonce> {
    let host = url_avis
        .strip_prefix("https://www.")
        .and_then(|r| r.split_once('/'))
        .map(|(h, _)| h)
        .ok_or_else(|| SourceError::Invalid(format!("modula: URL d'avis illisible {url_avis}")))?;
    let client = super::client();
    let page = charge(&client, url_avis.to_string(), "modula fiche DCE").await?;
    let Some(url) = lien_avis(host, &page) else {
        return Ok(super::DetailAnnonce::default());
    };
    let octets = super::send_bytes(client.get(&url), "modula avis").await?;
    tokio::task::spawn_blocking(move || lire_avis(&octets))
        .await
        .map_err(|e| SourceError::Invalid(format!("modula avis: {e}")))?
}

/// A buyer files its notice as the rendered form, or as the Word file it wrote
/// it in, as some do. The form is read from its bold, the Word
/// text has no use for one: there a label closes on a colon.
///
/// Poppler, not the in-process extractor: pdf-extract panics on an invalid
/// content stream instead of erroring, which would kill the run.
fn lire_avis(octets: &[u8]) -> Result<super::DetailAnnonce> {
    if lj_sources::docx::is_zip_container(octets) {
        let texte = lj_sources::docx::extract_docx_text(octets)?;
        return Ok(super::DetailAnnonce {
            zones: lj_annonces::zones::zones_departements(lj_annonces::departement_dans_texte(
                &texte,
            )),
            ..super::formulaire::detail(&texte)
        });
    }
    let rendu = lj_sources::pdf::pdftohtml_extract(octets)?;
    let champs = super::fiche::champs(&deplie(&rendu));
    Ok(super::DetailAnnonce {
        cpv: super::fiche::premier(&champs, &["code cpv principal"])
            .as_deref()
            .and_then(super::fiche::cpv_de),
        procedure: super::fiche::premier(&champs, &["type de procedure"]),
        contenu: super::fiche::contenu(
            &champs,
            super::fiche::premier(
                &champs,
                &[
                    "description succinte du marche",
                    "description succincte du marche",
                    "objet du marche",
                    "intitule du marche",
                ],
            ),
        ),
        zones: lj_annonces::zones::zones_departements(lj_annonces::departement_dans_texte(
            &super::txt(&rendu),
        )),
        ..super::DetailAnnonce::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bloc natif réel : boutons de dépôt, DCE anonyme, dates en toutes lettres.
    const BLOC_NATIF: &str = r#"<div class="post" style="display:inline-block;width:100%;">
	<div class="post-title post-title-annonce">
		<table class="fullWidth"><tr><td>
			<span class="post-title-alt"><strong>MAIRIE DE SAINT-OMER - 2026/22-25</strong>&nbsp;(AAPC)</strong></span>
		</td></tr></table>
	</div>
	<style>#mps { height:17px; }</style>
	<div class="post-block">
	    <table cellpadding="0" cellspacing="0">
	        <tr>
	            <td class="right" style="vertical-align:top;"><strong>Objet : </strong></td>
	            <td style="text-align:left;">
	            	<div style="display:inline-block; vertical-align: bottom">
	            PRESTATION DE SERVICES DE CONSERVATION RESTAURATION DES OEUVRES DU SERVICE DES MUSEES ET DU PATRIMOINE DE SAINT-OMER - PROGRAMME 2027-2030
	             	</div>
	             </td>
	        </tr>
	        <tr>
	        	<td colspan="2" style="padding-top:4px;"><strong>Type de procédure : </strong>Procédure adaptée ouverte</td>
	        </tr>
	    </table>
	</div>
	<div class="post-block" style="float:left;">
		<table>
			<tr><td><strong>Date de publication :</strong><br>7 Aout 2026 à 11:30 heures</td></tr>
			<tr><td><strong>Date limite de reception des plis :</strong><br>16 Octobre 2026 à 12:00 heures</td></tr>
			<tr><td style="text-align:left"><div><strong>Temps restant :</strong><span class="rouge">
				<div class="enligne" id="joursTimer_36083">68</div><div class="enligne"> jours</div>
			</span></div></td></tr>
		</table>
	</div>
	<div style="float:right;" align=right>
		<div class="post-button" onClick="javascript:OuvrirPopup('/publisher_portail/PublicationPdfGeneric;jsessionid=0000AAAA?iIdMarche=36083&iIdPublicationType=1&id=AAPC_36083&sts=0000000000000000',800, 600,'menubar=no')">Visualiser l'avis</div>
		<div class="post-button" onClick='javascript:window.name="mainWindow";OuvrirPopup("/publisher_portail/public/annonce/popupTelechargementDCEAnonyme.jsp;jsessionid=0000AAAA?idMarche=36083",800,315,"menubar=no")'>Télécharger le DCE</div>
		<div class="post-button repondre" onClick='Redirect("/publisher_portail/public/inscription/inscriptionOrganisation.jsp")'>Répondre à la consultation</div>
	</div>
</div>"#;

    /// Bloc de relais réseau réel : la consultation est native sur un autre portail,
    /// et le seul bouton renvoie à la recherche du portail source.
    const BLOC_RELAIS_RESEAU: &str = r#"<div class="post" style="display:inline-block;width:100%;">
	<div class="post-title post-title-annonce">
		<table class="fullWidth"><tr><td>
			<span class="post-title-alt"><strong>CENTRE HOSPITALIER UNIVERSITAIRE ROUEN (96096498) - 21293721</strong>&nbsp;(AAPC)</strong></span>
		</td></tr></table>
	</div>
	<div class="post-block" style="float:left; width:95%">
		<table style="display:inline-block; width:60%">
			<tr><td><strong>Objet : </strong>CHU de Rouen - APPEL A MANIFESTATION D&#039;INTERET POUR LA MISE EN PLACE D&#039;UN ESPACE LIBRAIRIE TEMPORAIRE </td></tr>
			<tr><td><strong>Date de publication :</strong><br>8 Aout 2026 à 00:00 heures</td></tr>
			<tr><td><table><tr>
				<td style="vertical-align: middle;">Publié sur le site :  </td>
				<td><img src="/include/images/rossel/logo_import.png;jsessionid=0000BBBB" alt="n.c." style="width:70px;" /></td>
			</tr></table></td></tr>
		</table>
		<table style="display:inline-grid; text-align:right; width:16%" align="right">
			<tr><td>
				<button class="post-button" onclick="alert(sMessageRedirection);window.open('https://www.proxilegales.fr/publisher_portail/public/annonce/afficherAnnonces.jsp?filtreType=marche.ALL_REF&filtre=21293721&raisonSociale=CENTRE+HOSPITALIER+UNIVERSITAIRE+ROUEN+%2896096498%29&bLaunchSearch=true&se_iMaxElementPerPage=5')" style="width:138px;">Accéder au marché sur le site source</button>
			</td></tr>
		</table>
	</div>
</div>"#;

    /// Bloc de relais presse réel : l'annonce légale republiée. Identifiant haché,
    /// aucun bouton de dépôt.
    const BLOC_RELAIS_PRESSE: &str = r#"<div class="post" style="display:inline-block;width:100%;">
	<div class="post-title post-title-annonce">
		<table class="fullWidth"><tr><td>
			<span class="post-title-alt"><strong>MAIRIE DE WIMILLE (96042163) - 21276834</strong>&nbsp;(AAPC)</strong></span>
		</td></tr></table>
	</div>
	<div class="post-block">
	    <table cellpadding="0" cellspacing="0">
	        <tr>
	            <td class="right"><strong>Objet : </strong></td>
	            <td style="text-align:left;">
	            	<div style="display:inline-block; vertical-align: bottom">
	            Accord cadre travaux d&#039;entretien et petites r&eacute;parations des toitures
	             	</div>
	             </td>
	        </tr>
	        <tr>
	        	<td colspan="2"><strong>Type de procédure : </strong>Procedure Adaptee Ouverte (sup&eacute;rieur &agrave; 90 K&euro;)</td>
	        </tr>
	    </table>
	</div>
	<div class="post-block" style="float:left;">
		<table>
			<tr><td><strong>Date de publication :</strong><br>31 Juillet 2026 à 00:00 heures</td></tr>
			<tr><td><strong>Date limite :</strong><br>21 Septembre 2026 à 12:00 heures</td></tr>
			<tr><td style="text-align:left"><table><tr><td><strong>Publié sur :</strong></td>
				<td><img src="/templatesClients/rossel/images/logo_annonces.png;jsessionid=0000AAAA?v=2.0" width="70px"></td>
			</tr></table></td></tr>
		</table>
	</div>
	<div style="float:right;" align=right>
		<div class="post-button" onClick='javascript:window.name="mainWindow";OuvrirPopup("/publisher_portail/public/annonce/displayPetiteAnnonceDetail.jsp;jsessionid=0000AAAA?iIdMarche=1d4ed465be393a66",800,600,"menubar=no")'>Visualiser l'annonce</div>
	</div>
</div>"#;

    #[test]
    fn seule_la_consultation_native_est_ingeree() {
        let page = format!("{BLOC_RELAIS_PRESSE}{BLOC_NATIF}{BLOC_RELAIS_RESEAU}");
        let rows = parse_page(&page, "proxilegales.fr");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source_uid, "modula:proxilegales.fr:36083");
    }

    #[test]
    fn un_relais_reseau_ne_rend_rien() {
        assert!(parse_page(BLOC_RELAIS_RESEAU, "marches-demat.com").is_empty());
    }

    #[test]
    fn un_relais_presse_ne_rend_rien() {
        assert!(parse_page(BLOC_RELAIS_PRESSE, "proxilegales.fr").is_empty());
    }

    #[test]
    fn un_avis_d_attribution_ne_rend_rien() {
        let aatr = BLOC_NATIF
            .replace("popupTelechargementDCEAnonyme.jsp", "DownloadFilePublisher")
            .replace("(AAPC)", "(AATR)");
        assert!(parse_page(&aatr, "proxilegales.fr").is_empty());
    }

    #[test]
    fn le_bloc_natif_rend_tous_ses_champs() {
        let rows = parse_page(BLOC_NATIF, "proxilegales.fr");
        let r = &rows[0];
        assert_eq!(r.moteur, "modula");
        assert_eq!(r.acheteur.as_deref(), Some("MAIRIE DE SAINT-OMER"));
        assert_eq!(r.reference.as_deref(), Some("2026/22-25"));
        assert_eq!(
            r.objet,
            "PRESTATION DE SERVICES DE CONSERVATION RESTAURATION DES OEUVRES DU SERVICE \
             DES MUSEES ET DU PATRIMOINE DE SAINT-OMER - PROGRAMME 2027-2030"
        );
        assert_eq!(r.procedure.as_deref(), Some("Procédure adaptée ouverte"));
        assert_eq!(
            r.date_publication.map(|d| d.to_string()).as_deref(),
            Some("2026-08-07")
        );
        // 16 octobre 2026 à 12:00, Europe/Paris.
        assert_eq!(
            r.echeance.map(|t| t.to_string()).as_deref(),
            Some("2026-10-16T10:00:00Z")
        );
        assert_eq!(
            r.url_avis.as_deref(),
            Some(
                "https://www.proxilegales.fr/publisher_portail/public/annonce/\
                 consulterDCEHorsIdentification.jsp?iIdAffaire=36083"
            )
        );
        assert_eq!(r.url_dce, r.url_avis);
    }

    #[test]
    fn l_identifiant_est_local_au_portail() {
        let a = parse_page(BLOC_NATIF, "proxilegales.fr");
        let b = parse_page(BLOC_NATIF, "marchespublics.pro");
        assert_ne!(a[0].source_uid, b[0].source_uid);
    }

    #[test]
    fn l_entete_coupe_au_premier_separateur() {
        let entete_de = |t: &str| entete(&format!(r#"<span class="post-title-alt">{t}</span>"#));
        assert_eq!(
            entete_de("<strong>MAIRIE DE HOULLE - 21 - 017 - HOU</strong>&nbsp;(AAPC)"),
            (
                Some("MAIRIE DE HOULLE".to_string()),
                Some("21 - 017 - HOU".to_string())
            )
        );
        assert_eq!(
            entete_de(
                "<strong>MAIRIE DE ROUVROY (00180330) - M62724_2026_004</strong>&nbsp;(AAPC)"
            ),
            (
                Some("MAIRIE DE ROUVROY".to_string()),
                Some("M62724_2026_004".to_string())
            )
        );
        assert_eq!(
            entete_de("<strong>SM3A - SM3A - 2026.010</strong>&nbsp;(AAPC)"),
            (
                Some("SM3A".to_string()),
                Some("SM3A - 2026.010".to_string())
            )
        );
    }

    #[test]
    fn le_listing_se_lit_en_latin1() {
        assert_eq!(
            latin1(b"Type de proc\xe9dure : 16 Octobre 2026 \xe0 12:00 heures"),
            "Type de procédure : 16 Octobre 2026 à 12:00 heures"
        );
    }

    #[test]
    fn l_echeance_des_candidatures_est_lue_comme_celle_des_plis() {
        let restreinte = BLOC_NATIF.replace(
            "Date limite de reception des plis",
            "Date limite de reception des candidatures",
        );
        let rows = parse_page(&restreinte, "proxilegales.fr");
        assert_eq!(
            rows[0].echeance.map(|t| t.to_string()).as_deref(),
            Some("2026-10-16T10:00:00Z")
        );
    }

    /// Real « Avis liés au marché » block. A consultation that published no
    /// notice keeps the block and loses both of its buttons, and then nothing is
    /// left to read.
    #[test]
    fn le_lien_de_l_avis_sort_du_bouton_telecharger() {
        let bloc = r#"<div class="enLigne"><strong>Avis de marché : </strong></div>
	<div class="enLigne pointer" onclick='javascript:OuvrirPopup("/publisher_portail/PublicationPdfGeneric?iIdMarche=35791",800,650)'>Visualiser</div>
	<div style="float:right"><a class="enLigne download_label" href='/publisher_portail/DownloadFilePublisher;jsessionid=24F0?iId=35791&amp;iIdTypeObjet=77&amp;sts=bc46'>Télécharger</a></div>"#;
        assert_eq!(
            lien_avis("proxilegales.fr", bloc).as_deref(),
            Some(
                "https://www.proxilegales.fr/publisher_portail/\
                 DownloadFilePublisher;jsessionid=24F0?iId=35791&iIdTypeObjet=77&sts=bc46"
            )
        );

        let sans_avis = r#"<div class="enLigne"><strong>Avis de marché : </strong></div>"#;
        assert_eq!(lien_avis("proxilegales.fr", sans_avis), None);

        // Half the portals wrap the same link in the popup opener instead.
        let popup = r#"<div class="enLigne pointer" onClick='javascript:window.open("/publisher_portail/DownloadFilePublisher;jsessionid=110B?sAction=view&iId=6929&iIdTypeObjet=14&sts=7885")' >Visualiser</div>"#;
        assert_eq!(
            lien_avis("achatspublicscorse.com", popup).as_deref(),
            Some(
                "https://www.achatspublicscorse.com/publisher_portail/\
                 DownloadFilePublisher;jsessionid=110B?sAction=view&iId=6929&iIdTypeObjet=14&sts=7885"
            )
        );
    }

    /// Real excerpt of the notice form as poppler renders it: a bold run holding
    /// a section heading and a label, a word cut across two lines, a sentence
    /// wrapped, and the justification spaces the renderer writes as entities.
    const FORMULAIRE: &str = r#"<b>Section 3 : Procédure<br/>Type de procédure<br/></b>Procédure adaptée ouverte<br/><b>Critères d'attribution<br/></b>Valeur&#160;&#160;technique&#160;&#160;(qualité&#160;&#160;de&#160;&#160;service):&#160;&#160;60<br/>points<br/>Prix : 40 points<br/><b>Section 4 : Identification du marché<br/>Intitulé du marché<br/></b>Réservation de titres<br/><b>Code CPV principal<br/></b>63512000-1<br/><b>Description succinte du marché<br/></b>Prestations&#160;comprenant&#160;la&#160;réservation&#160;de&#160;titres&#160;de&#160;trans-<br/>port terrestres, ainsi que des<br/>services associés.<br/><b>Section 5 : Lots<br/>Marché alloti :&#160;</b>Oui<br/><b>Description du lot<br/></b>Gros&#160;oeuvre&#160;-&#160;Revêtements&#160;de<br/>sol / Faïence.<br/><b>Code CPV principal<br/></b>45262522-6<br/>"#;

    #[test]
    fn le_formulaire_rendu_rend_les_quatre_champs() {
        let champs = super::super::fiche::champs(&deplie(FORMULAIRE));
        assert_eq!(
            super::super::fiche::premier(&champs, &["code cpv principal"]).as_deref(),
            Some("63512000-1")
        );
        assert_eq!(
            super::super::fiche::premier(&champs, &["type de procedure"]).as_deref(),
            Some("Procédure adaptée ouverte")
        );
        let contenu = super::super::fiche::contenu(
            &champs,
            super::super::fiche::premier(&champs, &["description succinte du marche"]),
        );
        assert_eq!(
            contenu.description.as_deref(),
            Some(
                "Prestations comprenant la réservation de titres de transport terrestres, \
                 ainsi que des services associés."
            )
        );
        assert_eq!(
            contenu.criteres,
            [
                "Valeur technique (qualité de service) (60 points)",
                "Prix (40 points)"
            ]
        );
        assert_eq!(
            contenu.lots,
            [lj_annonces::LotAnnonce {
                nom: Some("Gros oeuvre - Revêtements de sol / Faïence".to_string()),
                description: None,
                cpv: Some("45262522-6".to_string()),
            }]
        );
    }
}
