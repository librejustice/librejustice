//! AWS-Achat's per-title portals, which the national portal does not aggregate.
//! Rows carry `moteur: "aws"` and its `source_uid` shape: same platform, same
//! reference space, so a notice seen on both sides merges.

use std::sync::LazyLock;

use futures::StreamExt;
use regex::Regex;

use lj_annonces::zones::zones_departements;
use lj_annonces::{
    departement_from_cp, echeance_ts, normalise_departement, parse_heure, AnnonceBrute,
    ContenuAnnonce, NatureMarche,
};
use lj_core::dates::parse_date_fr;

use super::{apres_balise, dedup_source_uid, send_text, txt, HoteVerdict, MoteurRun};
use lj_sources::error::Result;

/// Full base URLs: Sud Ouest serves in clear only, and the EBRA subdomains are
/// regional views of one `DL-pub-` space that dedup by `source_uid` glues back.
const BASES: [&str; 11] = [
    "https://ladepeche-marchespublics.fr",
    "https://www.midilibre-marchespublics.com",
    "http://www.pro-marchespublics.com",
    "http://www.sudouest-marchespublics.com",
    "https://ledauphine.marchespublics-eurolegales.com",
    "https://lestrepublicain.marchespublics-eurolegales.com",
    "https://lerepublicainlorrain.marchespublics-eurolegales.com",
    "https://leprogres.marchespublics-eurolegales.com",
    "https://dna.marchespublics-eurolegales.com",
    "https://lalsace.marchespublics-eurolegales.com",
    "https://vosgesmatin.marchespublics-eurolegales.com",
];

const NATURES: [(&str, NatureMarche); 3] = [
    ("T", NatureMarche::Travaux),
    ("S", NatureMarche::Services),
    ("F", NatureMarche::Fournitures),
];
const PROCEDURES: [&str; 9] = ["PA", "AOO", "AOE", "MN", "CO", "CR", "DC", "AU", "DSP"];
const HOSTS_IN_FLIGHT: usize = 4;

fn form_url(base: &str) -> String {
    format!("{base}/avis/index.cfm?fuseaction=mpAW.affRech&IDS=25")
}

/// The search lives in a server-side context: a POST arriving without the
/// cookie gets an error page, not an empty result.
async fn session(base: &str) -> Result<wreq::Client> {
    let client = super::client();
    send_text(
        client.get(form_url(base)),
        &format!("aws-tenants form {base}"),
    )
    .await?;
    Ok(client)
}

/// A title portal only offers the departments it is habilitated for.
fn departments(page: &str) -> Vec<String> {
    static SELECT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<select[^>]*name="IDR".*?</select>"#).unwrap());
    static OPT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<option[^>]*value="([^"]*)""#).unwrap());
    static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,3}|2A|2B)$").unwrap());
    let Some(select) = SELECT.find(page) else {
        return Vec::new();
    };
    OPT.captures_iter(select.as_str())
        .map(|c| c[1].to_string())
        .filter(|v| CODE.is_match(v))
        .collect()
}

enum Cell {
    List(Vec<AnnonceBrute>),
    Capped,
}

async fn query(
    client: &wreq::Client,
    base: &str,
    dept: &str,
    nature: &str,
    idp: &str,
) -> Result<Cell> {
    let body = send_text(
        client
            .post(format!("{base}/avis/index.cfm?fuseaction=mpAW.rechM"))
            .header("Referer", form_url(base))
            .form(&[
                ("IDE", "EC"),
                ("IDN", nature),
                ("IDP", idp),
                ("IDR", dept),
                ("listeCPV", ""),
                ("txtLibre", ""),
                ("txtLibreAcheteur", ""),
                ("txtLibreVille", ""),
                ("txtLibreRef", ""),
                ("txtLibreObjet", ""),
                ("dateParution", ""),
                ("dateExpiration", ""),
                ("dateExpirationPassee", ""),
                ("annee", "X"),
            ]),
        &format!("aws-tenants {base} {dept}/{nature}/{idp}"),
    )
    .await?;
    match body.contains("Veuillez pr") {
        true => Ok(Cell::Capped),
        false => Ok(Cell::List(parse_rows(&body, base, dept))),
    }
}

/// A capped cell is recut by nature, then by procedure. A refusal surviving
/// both is dropped: a missing cell beats a loop with no axis left to cut on.
async fn cell(client: &wreq::Client, base: &str, dept: &str) -> Vec<AnnonceBrute> {
    if let Ok(Cell::List(rows)) = query(client, base, dept, "X", "X").await {
        return rows;
    }
    let mut out = Vec::new();
    for (nature, _) in NATURES {
        match query(client, base, dept, nature, "X").await {
            Ok(Cell::List(rows)) => out.extend(rows),
            Ok(Cell::Capped) => {
                for idp in PROCEDURES {
                    if let Ok(Cell::List(rows)) = query(client, base, dept, nature, idp).await {
                        out.extend(rows);
                    }
                }
            }
            Err(e) => tracing::warn!(base, dept, nature, error = %e, "aws-tenants: cell lost"),
        }
    }
    out
}

/// The listing groups by nature under a subheading, the only place it is
/// written: the row itself does not carry it.
fn parse_rows(page: &str, base: &str, dept: &str) -> Vec<AnnonceBrute> {
    static PUB: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"/Publications/([A-Za-z]+-pub-\d+)\.htm"#).unwrap());
    static ACHETEUR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<b>(.*?)</b>\s*\((\d{5})\)").unwrap());
    static OBJET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)\]<br\s*/?>(.*?)</td>").unwrap());
    static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r">(\d{2}/\d{2}/\d{2})<").unwrap());
    static HEURE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{1,2}h\d{2})").unwrap());

    let mut out = Vec::new();
    for section in page.split(r#"<p class="AW_Titre""#).skip(1) {
        let entete = txt(apres_balise(section).split("</p>").next().unwrap_or(""));
        let nature = match entete.trim().to_uppercase().as_str() {
            "TRAVAUX" => Some(NatureMarche::Travaux),
            "SERVICES" => Some(NatureMarche::Services),
            "FOURNITURES" => Some(NatureMarche::Fournitures),
            _ => None,
        };
        // The row splits on nothing: its `<tr>` wraps a nested table whose own
        // `<tr>` carries the link. The two dates sit before that link, the
        // buyer and the object after it.
        let morceaux: Vec<&str> = section.split("/Publications/").collect();
        for (i, bloc) in morceaux.iter().enumerate().skip(1) {
            let Some(reference) = PUB
                .captures(&format!("/Publications/{bloc}"))
                .map(|c| c[1].to_string())
            else {
                continue;
            };
            let avant = morceaux[i - 1];
            let acheteur = ACHETEUR.captures(bloc);
            let cp = acheteur.as_ref().map(|c| c[2].to_string());
            let mut dates: Vec<_> = DATE
                .captures_iter(avant)
                .filter_map(|c| parse_date_fr(&c[1]))
                .collect();
            dates = dates.split_off(dates.len().saturating_sub(2));
            out.push(AnnonceBrute {
                contenu: ContenuAnnonce::default(),
                descripteurs: Vec::new(),
                moteur: "aws",
                source_uid: format!("aws:pub:{reference}"),
                cles_jointure: Vec::new(),
                // The row's bracketed reference is the publication's own, not
                // the buyer's: posing it would shadow the real one.
                reference: None,
                acheteur: acheteur.map(|c| txt(&c[1]).trim().to_string()),
                siren: None,
                objet: OBJET.captures(bloc).map(|c| txt(&c[1])).unwrap_or_default(),
                nature,
                cpv: None,
                zones: zones_departements(
                    normalise_departement(dept)
                        .or_else(|| cp.as_deref().and_then(departement_from_cp)),
                ),
                date_publication: dates.first().copied(),
                echeance: dates.get(1).map(|d| {
                    echeance_ts(*d, HEURE.captures(avant).and_then(|c| parse_heure(&c[1])))
                }),
                url_avis: Some(format!("{base}/Publications/{reference}.htm")),
                url_rc: None,
                url_dce: None,
                url_depot: None,
                procedure: None,
            });
        }
    }
    out
}

async fn host_run(base: &'static str) -> (HoteVerdict, Vec<AnnonceBrute>) {
    let host = base.split("://").nth(1).unwrap_or(base).to_string();
    let Ok(client) = session(base).await else {
        return (
            HoteVerdict {
                host,
                joignable: false,
            },
            Vec::new(),
        );
    };
    let Ok(form) = send_text(client.get(form_url(base)), "aws-tenants IDR").await else {
        return (
            HoteVerdict {
                host,
                joignable: false,
            },
            Vec::new(),
        );
    };
    let mut rows = Vec::new();
    for dept in departments(&form) {
        rows.extend(cell(&client, base, &dept).await);
    }
    (
        HoteVerdict {
            host,
            joignable: true,
        },
        rows,
    )
}

/// Re-enumerates the live stock of every title portal.
pub async fn enumerate() -> Result<MoteurRun> {
    let resultats: Vec<(HoteVerdict, Vec<AnnonceBrute>)> =
        futures::stream::iter(BASES.into_iter().map(host_run))
            .buffer_unordered(HOSTS_IN_FLIGHT)
            .collect()
            .await;
    let mut run = MoteurRun {
        // The header counts notices where the list renders consultations, which
        // would fall under the 90 % floor.
        annonce: None,
        echecs: 0,
        hotes: Vec::new(),
        rows: Vec::new(),
    };
    for (verdict, rows) in resultats {
        run.hotes.push(verdict);
        run.rows.extend(rows);
    }
    run.rows = dedup_source_uid(run.rows);
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// From `mpAW.rechM` on `ladepeche-marchespublics.fr`, department 11.
    const PAGE: &str = r#"<h3>Votre recherche trouve 4 avis.</h3>
 <p class="AW_Titre" style="margin-bottom:5px;">
  FOURNITURES
 </p>
 <table class="AW_Table">
  <tr>
   <td width="60" class="AW_Table_Entete">Publi&eacute;</td>
  </tr>
  <tr>
   <td align="center" class="AW_Table_Ligne0" valign="top">04/09/26</td>
   <td class="AW_Table_Ligne0" valign="top" align="center">16/10/26<br /> &agrave; 10h00</td>
   <td class="AW_Table_Ligne0" valign="top">
    <table id='cokpit'>
     <tr><td><a href="/Publications/LDM-pub-2026247003.htm" title="Consulter l'avis">Avis</a></td></tr>
    </table>
    <b>COMMUNE DE PEXIORA</b> (11150)
    <br />
    [r&eacute;f. LDM-pub-2026247003]<br />
    Travaux de fourniture, pose et maintenance du dispositif de videoprotection
   </td>
  </tr>
 </table>"#;

    #[test]
    fn a_row_gives_the_buyer_the_object_and_both_dates() {
        let rows = parse_rows(PAGE, "https://ladepeche-marchespublics.fr", "11");
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.source_uid, "aws:pub:LDM-pub-2026247003");
        assert_eq!(r.acheteur.as_deref(), Some("COMMUNE DE PEXIORA"));
        assert_eq!(
            r.objet,
            "Travaux de fourniture, pose et maintenance du dispositif de videoprotection"
        );
        assert_eq!(r.nature, Some(NatureMarche::Fournitures));
        assert_eq!(
            r.url_avis.as_deref(),
            Some("https://ladepeche-marchespublics.fr/Publications/LDM-pub-2026247003.htm")
        );
        assert!(r.date_publication.is_some() && r.echeance.is_some());
    }

    /// The header row carries no publication link: reading it as a notice would
    /// yield one empty annonce per section.
    #[test]
    fn the_table_header_is_not_a_row() {
        assert!(parse_rows(
            r#"<p class="AW_Titre">SERVICES</p><tr><td class="AW_Table_Entete">Publié</td></tr>"#,
            "https://x.fr",
            "11"
        )
        .is_empty());
    }

    #[test]
    fn the_department_select_keeps_only_codes() {
        let form = r#"<select name="IDR"><option value="X">par région</option>
            <option value="1,38,69">AUVERGNE-RHÔNE-ALPES</option>
            <option value="31">31 - HAUTE-GARONNE</option>
            <option value="2A">2A - CORSE DU SUD</option></select>"#;
        assert_eq!(departments(form), vec!["31", "2A"]);
    }
}
