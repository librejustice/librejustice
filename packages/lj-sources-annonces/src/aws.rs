use std::sync::LazyLock;

use futures::StreamExt;
use regex::Regex;

use jiff::civil::Date;

use lj_annonces::zones::zones_departements;
use lj_annonces::{
    departement_from_cp, echeance_ts, nettoyer_acheteur, normalise_departement, parse_heure,
    AnnonceBrute, ContenuAnnonce, LotAnnonce, LotAttribue, NatureMarche,
};
use lj_core::dates::parse_date_fr;

use super::{dedup_source_uid, fiche, send_text, txt, DetailAnnonce, MoteurRun};
use lj_sources::error::{Result, SourceError};

const BASE: &str = "https://www.marches-publics.info";
const NATURES: [(&str, NatureMarche); 3] = [
    ("T", NatureMarche::Travaux),
    ("S", NatureMarche::Services),
    ("F", NatureMarche::Fournitures),
];
const MAX_PAGES: u64 = 60;
const CELLULES_PARALLELES: usize = 12;
/// The form's procedures (`<select name="IDP">`), the only axis that bites beyond
/// nature and department. They leave out the notices declaring no procedure, which
/// no value catches.
const PROCEDURES: [&str; 9] = ["PA", "AOO", "AOE", "MN", "CO", "CR", "DC", "AU", "DSP"];

/// Ce que `lister` rend : la page servie, ou le refus au-delà du plafond.
enum Cellule {
    Liste {
        annonce: u64,
        rows: Vec<AnnonceBrute>,
    },
    Plafond {
        annonce: u64,
    },
}

async fn session() -> Result<wreq::Client> {
    let client = super::client();
    send_text(
        client
            .get(format!("{BASE}/Annonces/rechercher"))
            .header("Referer", format!("{BASE}/Annonces/rechercher")),
        "aws rechercher",
    )
    .await?;
    Ok(client)
}

/// Départements offerts par le formulaire (`<select name="IDR">`).
async fn departements() -> Result<Vec<String>> {
    let page = send_text(
        super::client().get(format!("{BASE}/Annonces/rechercher")),
        "aws form",
    )
    .await?;
    static SELECT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<select[^>]*name="IDR".*?</select>"#).unwrap());
    static OPT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<option[^>]*value="([^"]*)""#).unwrap());
    static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,3}|2A|2B)$").unwrap());
    let select = SELECT
        .find(&page)
        .ok_or_else(|| SourceError::Invalid("aws: select IDR absent du formulaire".into()))?;
    Ok(OPT
        .captures_iter(select.as_str())
        .map(|c| c[1].to_string())
        .filter(|v| CODE.is_match(v))
        .collect())
}

/// A fresh session per page, the search context being global server-side. `IDE`
/// picks the fund: `EC` the open consultations, `AAA` the awards.
async fn page_html(ide: &str, dept: &str, nature: &str, idp: &str, page: u64) -> Result<String> {
    let client = session().await?;
    let mut url = format!("{BASE}/Annonces/lister");
    if page > 1 {
        url = format!("{url}?pager_{}={page}", nature.to_lowercase());
    }
    send_text(
        client.post(url).form(&[
            ("IDE", ide),
            ("IDN", nature),
            ("IDR", dept),
            ("IDP", idp),
            ("dateParution", ""),
            ("Rechercher", "Rechercher"),
        ]),
        &format!("aws lister {ide} {dept}/{nature}/{idp} p{page}"),
    )
    .await
}

async fn page_query(dept: &str, nature: &str, idp: &str, page: u64) -> Result<Cellule> {
    let page = page_html("EC", dept, nature, idp, page).await?;
    Ok(lecture(&page, dept, nature))
}

/// The refusal is recognised by its own count, served in place of the list.
fn plafond(plat: &str) -> Option<u64> {
    static PLAFOND: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)Votre recherche trouve\s*(\d[\d\s]*)\s*résultats?").unwrap()
    });
    PLAFOND
        .captures(plat)
        .and_then(|c| c[1].replace(char::is_whitespace, "").parse().ok())
}

fn lecture(page: &str, dept: &str, nature: &str) -> Cellule {
    static TOTAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)(\d[\d\s]*)\s*Avis\s+correspond").unwrap());
    let plat = txt(page);
    match plafond(&plat) {
        Some(annonce) => Cellule::Plafond { annonce },
        None => Cellule::Liste {
            annonce: TOTAL
                .captures(&plat)
                .and_then(|c| c[1].replace(char::is_whitespace, "").parse().ok())
                .unwrap_or(0),
            rows: parse_rows(page, dept, nature),
        },
    }
}

fn parse_rows(page: &str, dept: &str, nature: &str) -> Vec<AnnonceBrute> {
    // La balise ouvrante entière : un match qui démarre à `class=` laisse un `<div `
    // orphelin dans l'objet après le `replace`.
    static REF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)<div[^>]*class="[^"]*ref-acheteur[^"]*"[^>]*>(.*?)</div>"#).unwrap()
    });
    static BOX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)id='titre_box'>(.*?)</div>\s*</div>").unwrap());
    static IDM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"IDM=(\d+)").unwrap());
    static AVIS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"href="(/Annonces/((?:MPI|DL)-pub-[^".]+)\.htm)""#).unwrap());
    static PUB: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Publié le\s*([\d/]{6,10})").unwrap());
    static LIM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"Date limite\s*:?\s*le\s*([\d/]{6,10})(?:\s*à\s*([\dh:]+))?").unwrap()
    });
    static CP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\((\d{5})\)").unwrap());

    let nature = NATURES
        .iter()
        .find(|(code, _)| *code == nature)
        .map(|(_, n)| *n);
    let mut out = Vec::new();
    for block in page.split(r#"<h2 class="h2-avis">"#).skip(1) {
        let b: String = block.chars().take(9_000).collect();
        let acheteur_brut = txt(b.split("</h2>").next().unwrap_or(""));
        let (mut reference, mut objet) = (None, String::new());
        if let Some(cap) = BOX.captures(&b) {
            let mut inner = cap[1].to_string();
            if let Some(r) = REF.captures(&inner) {
                reference = Some(
                    txt(&r[1])
                        .trim_start_matches("[réf.")
                        .trim_end_matches(']')
                        .trim()
                        .to_string(),
                );
                inner = inner.replace(&r[0], " ");
            }
            objet = txt(&inner);
        }
        let idm = IDM.captures(&b).map(|c| c[1].to_string());
        let avis = AVIS.captures(&b);
        let source_uid = match (&idm, &avis) {
            (Some(idm), _) => format!("aws:{idm}"),
            (None, Some(a)) => format!("aws:pub:{}", &a[2]),
            (None, None) => continue,
        };
        let flat = txt(&b);
        let echeance = LIM.captures(&flat).and_then(|c| {
            let date = parse_date_fr(&c[1])?;
            Some(echeance_ts(
                date,
                c.get(2).and_then(|h| parse_heure(h.as_str())),
            ))
        });
        let cp = CP.captures(&acheteur_brut).map(|c| c[1].to_string());
        out.push(AnnonceBrute {
            contenu: ContenuAnnonce::default(),
            descripteurs: Vec::new(),
            moteur: "aws",
            source_uid,
            cles_jointure: Vec::new(),
            reference: reference.filter(|r| !r.is_empty()),
            acheteur: Some(CP.replace(&acheteur_brut, "").trim().to_string())
                .filter(|a| !a.is_empty()),
            siren: None,
            objet,
            nature,
            cpv: None,
            zones: zones_departements(
                normalise_departement(dept).or_else(|| cp.as_deref().and_then(departement_from_cp)),
            ),
            date_publication: PUB.captures(&flat).and_then(|c| parse_date_fr(&c[1])),
            echeance,
            url_avis: avis.map(|a| format!("{BASE}{}", &a[1])),
            url_rc: idm
                .as_ref()
                .filter(|_| b.contains("fuseaction=dce.dRC"))
                .map(|idm| format!("{BASE}/mpiaws/index.cfm?fuseaction=dce.dRC&IDM={idm}")),
            url_dce: idm
                .as_ref()
                .filter(|_| b.contains("fuseaction=dematEnt.login"))
                .map(|idm| {
                    format!("{BASE}/mpiaws/index.cfm?fuseaction=dematEnt.login&type=DCE&IDM={idm}")
                }),
            url_depot: None,
            procedure: None,
        });
    }
    out
}

/// The consultations the portal returns for a market reference. `txtLibreRef`
/// is the form's own reference field, and the national portal answers for every
/// tenant, so the search needs no host. Its index and its enumerable list do
/// not cover the same set: a consultation missing from the second comes out of
/// the first.
pub async fn chercher(reference: &str) -> Result<Vec<AnnonceBrute>> {
    let client = session().await?;
    let corps = send_text(
        client.post(format!("{BASE}/Annonces/lister")).form(&[
            ("IDE", "EC"),
            ("txtLibreRef", reference),
            ("Rechercher", "Rechercher"),
        ]),
        &format!("aws recherche {reference}"),
    )
    .await?;
    Ok(parse_rows(&corps, "", ""))
}

/// Toutes les pages d'une cellule servie (département × nature × procédure).
async fn pages(dept: &str, nature: &str, idp: &str) -> Result<Cellule> {
    let (annonce, mut rows) = match page_query(dept, nature, idp, 1).await? {
        Cellule::Plafond { annonce } => return Ok(Cellule::Plafond { annonce }),
        Cellule::Liste { annonce, rows } => (annonce, rows),
    };
    let mut vus: std::collections::HashSet<String> =
        rows.iter().map(|r| r.source_uid.clone()).collect();
    let mut page = 1;
    while (vus.len() as u64) < annonce && page < MAX_PAGES && !rows.is_empty() {
        page += 1;
        let Cellule::Liste { rows: suite, .. } = page_query(dept, nature, idp, page).await? else {
            break;
        };
        let avant = vus.len();
        for r in suite {
            if vus.insert(r.source_uid.clone()) {
                rows.push(r);
            }
        }
        if vus.len() == avant {
            break;
        }
    }
    Ok(Cellule::Liste { annonce, rows })
}

/// Une cellule, redécoupée par procédure quand le moteur refuse de la lister.
/// Le déclaré retenu reste celui du refus, pour que le résidu se voie au ratio.
async fn enumerate_cell(dept: String, nature: &'static str) -> super::HoteRun {
    let plafond = match pages(&dept, nature, "X").await? {
        Cellule::Liste { annonce, rows } => return Ok((annonce, rows)),
        Cellule::Plafond { annonce } => annonce,
    };
    let mut rows: Vec<AnnonceBrute> = Vec::new();
    let mut vus = std::collections::HashSet::new();
    for idp in PROCEDURES {
        match pages(&dept, nature, idp).await? {
            Cellule::Liste { rows: lot, .. } => {
                for r in lot {
                    if vus.insert(r.source_uid.clone()) {
                        rows.push(r);
                    }
                }
            }
            Cellule::Plafond { annonce } => {
                return Err(SourceError::Invalid(format!(
                    "aws: {dept}/{nature}/{idp} pèse {annonce} avis, plus rien ne le redécoupe"
                )));
            }
        }
    }
    Ok((plafond, rows))
}

/// Ré-énumère le stock actif France entière, cellule par cellule.
pub async fn enumerate() -> Result<MoteurRun> {
    let departements = departements().await?;
    let cellules: Vec<(String, &'static str)> = departements
        .iter()
        .flat_map(|d| NATURES.iter().map(|(n, _)| (d.clone(), *n)))
        .collect();
    let resultats: Vec<super::HoteRun> =
        futures::stream::iter(cellules.into_iter().map(|(d, n)| enumerate_cell(d, n)))
            .buffer_unordered(CELLULES_PARALLELES)
            .collect()
            .await;
    let mut run = MoteurRun {
        annonce: Some(0),
        echecs: 0,
        hotes: Vec::new(),
        rows: Vec::new(),
    };
    for res in resultats {
        match res {
            Ok((annonce, rows)) => {
                // Le total par cellule double-compte les avis multi-départements ; le
                // dédoublonnage par source_uid ci-dessous ramène les lignes, pas le déclaré.
                run.annonce = run.annonce.map(|a| a + annonce);
                run.rows.extend(rows);
            }
            Err(e) => {
                run.echecs += 1;
                tracing::warn!(error = %e, "aws: cell failed, stock kept");
            }
        }
    }
    let avant = run.rows.len() as u64;
    run.rows = dedup_source_uid(run.rows);
    // Le déclaré compte des doublons multi-départements : on le rabat du même
    // facteur que les lignes pour garder un ratio de sanity check honnête.
    if avant > 0 {
        run.annonce = run.annonce.map(|a| a * run.rows.len() as u64 / avant);
    }
    Ok(run)
}

/// Page de détail d'un avis : le CPV et la procédure, absents de la ligne de
/// listing. Même carte pour les deux formes d'URL, `MPI-pub-…` comme `DL-pub-…` ;
/// pas de session, la page est servie en GET nu.
pub async fn detail(url_avis: &str) -> Result<DetailAnnonce> {
    let page = send_text(
        super::client().get(url_avis),
        &format!("aws détail {url_avis}"),
    )
    .await?;
    Ok(parse_detail(&page))
}

/// The table layout labels the description `Description`, the press one
/// `Objet du marché` then `Objet`.
const DESCRIPTIONS: [&str; 3] = ["description", "objet du marche", "objet"];

/// The allotment hangs in a table whose cells carry no label: the number, the
/// heading then the lot's own `Description :`, then the lot CPV, published here alone.
fn lots_tableau(html: &str) -> Vec<LotAnnonce> {
    static TABLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<b>\s*Lots\s*</b>.*?</table>").unwrap());
    static RANGEE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<tr>(.*?)</tr>").unwrap());
    static CELLULE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<td[^>]*>(.*?)</td>").unwrap());
    static NUMERO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^n°\s*(\S+)$").unwrap());

    let Some(table) = TABLE.find(html) else {
        return Vec::new();
    };
    RANGEE
        .captures_iter(table.as_str())
        .filter_map(|r| {
            let cellules: Vec<&str> = CELLULE
                .captures_iter(r.get(1)?.as_str())
                .filter_map(|c| c.get(1).map(|m| m.as_str()))
                .collect();
            let numero = NUMERO.captures(txt(cellules.first()?).trim())?[1].to_string();
            let lignes = fiche::lignes(cellules.get(1)?);
            let nom = lignes.first()?.clone();
            Some(LotAnnonce {
                description: lignes
                    .iter()
                    .find_map(|l| l.strip_prefix("Description :"))
                    .map(|d| d.trim().to_string())
                    .filter(|d| !d.is_empty() && *d != nom),
                nom: Some(format!("Lot {numero} : {nom}")),
                cpv: cellules.last().and_then(|c| fiche::cpv_de(&txt(c))),
            })
        })
        .collect()
}

/// The AAPC weighs its criteria the other way round from every other channel:
/// `60 % : Prix des prestations`, one a line under a preamble in prose.
fn criteres_ponderes(champs: &[fiche::Champ]) -> Vec<String> {
    static PONDERE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^([0-9]+(?:[.,][0-9]+)?\s*%)\s*:\s*(\S.*?)\s*[.;]?$").unwrap()
    });
    champs
        .iter()
        .filter(|c| c.cle.contains("criteres d'attrib"))
        .flat_map(|c| fiche::lignes(&c.bloc))
        .filter_map(|l| {
            PONDERE
                .captures(&l)
                .map(|m| format!("{} ({})", &m[2], &m[1]))
        })
        .collect()
}

/// La carte d'avis est une table d'étiquettes en gras : `Mode` porte le libellé
/// de procédure, `Code CPV principal` le code (8 chiffres ici, la clé de
/// contrôle `-N` restant admise). Un MAPA court se publie sans aucune ligne
/// CPV — la page est lue, `cpv` reste `None`.
fn parse_detail(page: &str) -> DetailAnnonce {
    static CPV: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)Code CPV principal\s*(\d{8}(?:-\d)?)").unwrap());
    // Le libellé se lit dans la cellule qui suit l'étiquette : à plat, rien ne
    // marque où il s'arrête (l'étiquette suivante le prolongerait).
    static MODE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?is)<b>\s*Mode\s*</b>\s*</td>\s*<td[^>]*>(.*?)</td>").unwrap()
    });
    let champs = fiche::champs(page);
    let mut contenu = fiche::contenu(&champs, fiche::premier(&champs, &DESCRIPTIONS));
    let tableau = lots_tableau(page);
    if !tableau.is_empty() {
        contenu.lots = tableau;
    }
    if contenu.criteres.is_empty() {
        contenu.criteres = criteres_ponderes(&champs);
    }
    DetailAnnonce {
        contenu,
        zones: Vec::new(),
        cpv: CPV
            .captures(&txt(page))
            .map(|c| c[1].to_string())
            .or_else(|| {
                champs
                    .iter()
                    .find(|c| c.cle == "classification cpv")
                    .and_then(|c| fiche::cpv_de(&c.texte()))
            }),
        procedure: MODE
            .captures(page)
            .map(|c| txt(&c[1]))
            .filter(|p| !p.is_empty())
            .or_else(|| {
                champs
                    .iter()
                    .find(|c| c.cle == "procedure" || c.cle == "type de procedure")
                    .map(|c| c.valeur.trim_end_matches('.').trim().to_string())
                    .filter(|p| !p.is_empty())
            }),
        acheteur: None,
    }
}

/// A row of the award listing. The notice page is not read here: it costs a
/// request each, and only the rows that resolve to a market are worth it.
#[derive(Debug)]
pub struct AttributionListee {
    pub publication: String,
    pub reference: String,
    pub acheteur: String,
    pub url_avis: String,
    pub date_publication: Option<Date>,
}

/// The award listing keys its rows on `id="entity"` and puts the publication
/// date before the buyer's `h2`, where the open listing puts it after.
fn parse_attributions(page: &str) -> Vec<AttributionListee> {
    static AVIS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"href="(/Annonces/((?:MPI|DL)-pub-[^".]+)\.htm)""#).unwrap());
    static ACHETEUR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<h2 class="h2-avis">(.*?)</h2>"#).unwrap());
    static REFERENCE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[r[ée]f\.\s*([^\]]+)\]").unwrap());
    static PUB: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Publié le\s*([\d/]{6,10})").unwrap());
    static CP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\((\d{5})\)").unwrap());

    let mut out = Vec::new();
    for bloc in page.split(r#"id="entity""#).skip(1) {
        let plat = txt(bloc);
        let (Some(avis), Some(reference)) = (
            AVIS.captures(bloc),
            REFERENCE
                .captures(&plat)
                .map(|c| c[1].trim().to_string())
                .filter(|r| !r.is_empty()),
        ) else {
            continue;
        };
        let acheteur = ACHETEUR
            .captures(bloc)
            .map(|c| txt(&c[1]))
            .unwrap_or_default();
        out.push(AttributionListee {
            publication: avis[2].to_string(),
            reference,
            acheteur: CP.replace(&acheteur, "").trim().to_string(),
            url_avis: format!("{BASE}{}", &avis[1]),
            date_publication: PUB.captures(&plat).and_then(|c| parse_date_fr(&c[1])),
        });
    }
    out
}

/// What closes a winner's address: the next field, or the notes that follow it.
const FIN_TITULAIRE: [&str; 8] = [
    "Le titulaire est",
    "Sous-traitance",
    "Renseignements complémentaires",
    "Envoi le",
    "Publication aux supports",
    "Nombre d'offres",
    "Montant HT",
    "Date d'attribution",
];

fn tronque<'a>(s: &'a str, fins: &[&str]) -> &'a str {
    fins.iter()
        .filter_map(|f| s.find(f))
        .min()
        .map_or(s, |i| &s[..i])
}

/// `Date d'attribution` opens the fields of a lot: before it sits the lot's own
/// heading, after it the fields, then the winner and its address run together. A
/// lot repeats `Marché n°` once per co-titulaire, each preceded by its amount.
fn lots_attribues(bloc: &str, numerote: bool) -> Vec<LotAttribue> {
    static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*:\s*([\d/]{6,10})").unwrap());
    static CHAMPS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"Nombre d'offres[^:\d]{0,120}:\s*\d+|Montant HT\b[^€]{0,80}€|max\s*:\s*[\d\s\u{a0},.]*€",
        )
        .unwrap()
    });
    static OFFRES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Nombre d'offres reçues\s*:\s*(\d+)").unwrap());
    // `Montant HT de X à Y` and `Montant HT min : X max : Y` sit beside the
    // plain form: the figure kept is the last one before the euro sign.
    static MONTANT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Montant HT\b[^€]{0,80}?([\d][\d\s\u{a0},.]*)\s*€").unwrap());
    // The market number carries spaces (`0 003 26 003 lot 1`) as often as not:
    // only the dash that follows it separates it from the winner.
    static NUMERO: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)^\s*:\s*.{0,60}?\s+-\s+").unwrap());

    let Some((titre, apres)) = bloc.split_once("Date d'attribution") else {
        return Vec::new();
    };
    let commun = |brut: &str, montant: Option<f64>| {
        let (titulaire, ville) = titulaire_et_ville(nettoyer_acheteur(brut).trim());
        (!titulaire.is_empty()).then(|| LotAttribue {
            description: numerote.then(|| format!("Lot {}", titre.trim())),
            titulaire,
            siren: None,
            ville,
            montant,
            devise: montant.map(|_| "EUR".to_string()),
            nb_offres: OFFRES.captures(apres).and_then(|c| c[1].parse().ok()),
            date_attribution: DATE.captures(apres).and_then(|c| parse_date_fr(&c[1])),
        })
    };

    let segments: Vec<&str> = apres.split("Marché n°").collect();
    if segments.len() > 1 {
        return segments
            .windows(2)
            .filter_map(|p| {
                let m = NUMERO.find(p[1])?;
                let montant = MONTANT
                    .captures_iter(p[0])
                    .last()
                    .and_then(|c| montant_ht(&c[1]));
                commun(tronque(&p[1][m.end()..], &FIN_TITULAIRE), montant)
            })
            .collect();
    }
    let sans_date = DATE.replace(apres, " ");
    let sans_champs = CHAMPS.replace_all(&sans_date, " ");
    commun(
        tronque(&sans_champs, &FIN_TITULAIRE[..5]),
        MONTANT.captures(apres).and_then(|c| montant_ht(&c[1])),
    )
    .into_iter()
    .collect()
}

/// `SYSTHERMIC 19 RUE D'APOLLO 31240 L'UNION`: the post code is the only
/// separator the line offers between the winner, its street and its town.
fn titulaire_et_ville(brut: &str) -> (String, Option<String>) {
    static ADRESSE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)^(.*?)\s+\d{5}\s+(.+)$").unwrap());
    static NUMERO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+\d+\s+\S").unwrap());
    let Some(c) = ADRESSE.captures(brut) else {
        return (brut.to_string(), None);
    };
    let avant = &c[1];
    let nom = match NUMERO.find(avant) {
        Some(m) => &avant[..m.start()],
        None => avant,
    };
    (nom.trim().to_string(), Some(c[2].trim().to_string()))
}

fn montant_ht(s: &str) -> Option<f64> {
    s.replace(char::is_whitespace, "")
        .replace('\u{a0}', "")
        .replace(',', ".")
        .parse()
        .ok()
}

/// The lots of an award notice. Empty when the page awards nothing: `IDE=AAA`
/// also serves the markets declared without outcome.
fn parse_lots(plat: &str) -> Vec<LotAttribue> {
    let Some(section) = plat.split("ATTRIBUTION DU MARCH").nth(1) else {
        return Vec::new();
    };
    match section.contains(" Lot ") {
        true => section
            .split(" Lot ")
            .skip(1)
            .flat_map(|b| lots_attribues(b, true))
            .collect(),
        false => lots_attribues(section, false),
    }
}

async fn attributions_cellule(
    dept: &str,
    nature: &str,
    idp: &str,
) -> Result<Option<Vec<AttributionListee>>> {
    let mut out: Vec<AttributionListee> = Vec::new();
    let mut vus = std::collections::HashSet::new();
    for page in 1..=MAX_PAGES {
        let corps = page_html("AAA", dept, nature, idp, page).await?;
        if plafond(&txt(&corps)).is_some() {
            return Ok(None);
        }
        let avant = vus.len();
        for a in parse_attributions(&corps) {
            if vus.insert(a.publication.clone()) {
                out.push(a);
            }
        }
        if vus.len() == avant {
            break;
        }
    }
    Ok(Some(out))
}

/// A cell the engine refuses to list is recut by procedure. Unlike the open
/// path, a cell still refused is dropped rather than fatal: an award is posed on
/// a market that stands without it.
async fn attributions_departement(dept: String) -> Vec<AttributionListee> {
    let mut brut: Vec<AttributionListee> = Vec::new();
    for (nature, _) in NATURES {
        match attributions_cellule(&dept, nature, "X").await {
            Ok(Some(lot)) => {
                brut.extend(lot);
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, dept, nature, "aws: awards cell failed");
                continue;
            }
        }
        for idp in PROCEDURES {
            match attributions_cellule(&dept, nature, idp).await {
                Ok(Some(lot)) => brut.extend(lot),
                Ok(None) => tracing::warn!(
                    dept,
                    nature,
                    idp,
                    "aws: awards at the ceiling, nothing splits further"
                ),
                Err(e) => {
                    tracing::warn!(error = %e, dept, nature, idp, "aws: awards cell failed")
                }
            }
        }
    }
    let mut vus = std::collections::HashSet::new();
    brut.retain(|a| vus.insert(a.publication.clone()));
    brut
}

/// The award notices the national portal serves, département by département.
/// The portal sets its window, which no form parameter bounds.
pub async fn attributions() -> Result<Vec<AttributionListee>> {
    let departements = departements().await?;
    let lots: Vec<Vec<AttributionListee>> =
        futures::stream::iter(departements.into_iter().map(attributions_departement))
            .buffer_unordered(CELLULES_PARALLELES)
            .collect()
            .await;
    let mut vus = std::collections::HashSet::new();
    Ok(lots
        .into_iter()
        .flatten()
        .filter(|a| vus.insert(a.publication.clone()))
        .collect())
}

pub async fn attribution_lots(url_avis: &str) -> Result<Vec<LotAttribue>> {
    let page = send_text(
        super::client().get(url_avis),
        &format!("aws attribution {url_avis}"),
    )
    .await?;
    Ok(parse_lots(&txt(&page)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l_objet_ne_garde_rien_de_la_div_reference() {
        let page = r#"<h2 class="h2-avis">Ville de Colmar (68000)</h2>
<a href="index.cfm?fuseaction=marches.certif&IDM=1848353">certificat</a>
<div id='titre_box'><div class="ref-acheteur">[réf. 26AC01]</div><div>Montage et démontage des illuminations</div></div>
Publié le 07/08/2026 Date limite : le 09/09/2026 à 10h00"#;
        let rows = parse_rows(page, "68", "2");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].objet, "Montage et démontage des illuminations");
        assert_eq!(rows[0].reference.as_deref(), Some("26AC01"));
        assert_eq!(rows[0].source_uid, "aws:1848353");
    }

    /// Le refus compte lui aussi ses avis : le lire comme un total ferait passer
    /// la cellule pour servie et vide.
    #[test]
    fn le_refus_de_lister_ne_se_lit_pas_comme_un_total() {
        let refus = r#"<h2>Votre recherche trouve 115 résultats.
            Veuillez préciser votre recherche à l&#039;aide des critères suivants :</h2>"#;
        assert!(matches!(
            lecture(refus, "75", "S"),
            Cellule::Plafond { annonce: 115 }
        ));
        let servie = r#"<h2 id="nbResultAvis"><strong>
                6
            </strong>
            Avis correspondent à votre recherche</h2>"#;
        assert!(matches!(
            lecture(servie, "68", "T"),
            Cellule::Liste { annonce: 6, .. }
        ));
    }

    /// Extrait de `MPI-pub-20261911503.htm` : les deux lignes de la carte.
    #[test]
    fn la_carte_d_avis_donne_le_cpv_principal_et_le_mode() {
        let page = r#"<tr>
    <td class="AW_TableM_Bloc1_Clair" valign="top"><b>Mode</b></td>
    <td class="AW_TableM_Bloc1_Clair">
    Procédure adaptée ouverte
    </td>
</tr>
<tr>
    <td class="AW_TableM_Bloc1_Clair" valign="top"><b>Code CPV principal</b></td>
    <td class="AW_TableM_Bloc1_Clair"><strong>45233140</strong> - Travaux routiers</td>
</tr>"#;
        assert_eq!(
            parse_detail(page),
            DetailAnnonce {
                contenu: ContenuAnnonce::default(),
                zones: Vec::new(),
                cpv: Some("45233140".into()),
                procedure: Some("Procédure adaptée ouverte".into()),
                acheteur: None,
            }
        );
    }

    /// Un MAPA court se publie sans ligne CPV : page lue, pas d'échec.
    #[test]
    fn une_carte_sans_ligne_cpv_garde_la_procedure() {
        let page = r#"<tr>
    <td class="AW_TableM_Bloc1_Clair" valign="top"><b>Mode</b></td>
    <td class="AW_TableM_Bloc1_Clair">
    Procédure ouverte
    </td>
</tr>"#;
        assert_eq!(
            parse_detail(page),
            DetailAnnonce {
                contenu: ContenuAnnonce::default(),
                zones: Vec::new(),
                cpv: None,
                procedure: Some("Procédure ouverte".into()),
                acheteur: None,
            }
        );
    }

    #[test]
    fn an_award_row_gives_its_reference_its_buyer_and_its_date() {
        let page = r#"<div id="entity">
<div>Publié le 18/06/26 | Résultat : Attribué</div>
<h2 class="h2-avis">Toulouse Métropole (31100)</h2>
<div id='titre_box'><div class="ref-acheteur">[réf. 26MC0013AOO]</div></div>
<a href="/Annonces/MPI-pub-20261691200.htm">Consulter</a></div>"#;
        let a = parse_attributions(page);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].publication, "MPI-pub-20261691200");
        assert_eq!(a[0].reference, "26MC0013AOO");
        assert_eq!(a[0].acheteur, "Toulouse Métropole");
        assert_eq!(a[0].date_publication, Some(jiff::civil::date(2026, 6, 18)));
    }

    #[test]
    fn un_avis_a_lots_rend_un_titulaire_par_lot() {
        let plat = "ATTRIBUTION DU MARCHÉ Valeur totale du marché (hors TVA) Valeur : 4170000 € \
Lot 5C : Chauffage Ventilation Climatisation Territoire Centre Date d'attribution : 11/06/26 \
Nombre d'offres reçues : 8 Montant HT : 1 230 000,00 € \
Marché n° : 2026M0197 - SYSTHERMIC 19 RUE D'APOLLO 31240 L'UNION Le titulaire est une PME : NON \
Lot 5E : Ventilation Territoire Est Date d'attribution : 11/06/26 \
Nombre d'offres reçues : 8 Montant HT : 690 000,00 € \
Marché n° : 2026M0193 - SOCOCLIM 3 CHE DE VIGNALIS 31130 FLOURENS Le titulaire est une PME : NON";
        let lots = parse_lots(plat);
        assert_eq!(lots.len(), 2);
        assert_eq!(lots[0].titulaire, "SYSTHERMIC");
        assert_eq!(lots[0].ville.as_deref(), Some("L'UNION"));
        assert_eq!(lots[0].montant, Some(1_230_000.0));
        assert_eq!(lots[0].nb_offres, Some(8));
        assert_eq!(
            lots[0].description.as_deref(),
            Some("Lot 5C : Chauffage Ventilation Climatisation Territoire Centre")
        );
        assert_eq!(
            lots[0].date_attribution,
            Some(jiff::civil::date(2026, 6, 11))
        );
        assert_eq!(lots[1].titulaire, "SOCOCLIM");
    }

    /// The winner is also given without a market number and without an amount,
    /// right after the date alone: an award that keying on `Marché n°` misses.
    #[test]
    fn un_avis_sans_lot_ni_numero_rend_quand_meme_son_titulaire() {
        let lots = parse_lots(
            "ATTRIBUTION DU MARCHÉ Date d'attribution : 17/06/26 \
STTL - Société Toulousaine de travaux et Locations 15 chemin des Pierres 31150 BRUGUIERES \
Envoi le 18/06/26 à la publication",
        );
        assert_eq!(lots.len(), 1);
        assert_eq!(
            lots[0].titulaire,
            "STTL - Société Toulousaine de travaux et Locations"
        );
        assert_eq!(lots[0].ville.as_deref(), Some("BRUGUIERES"));
        assert_eq!(lots[0].description, None);
    }

    /// A lot can be awarded to several holders: each has its own `Marché n°`
    /// line preceded by its own amount, and the non-EU tender label runs seventy
    /// characters before its colon.
    #[test]
    fn un_lot_a_deux_titulaires_rend_deux_lignes_et_deux_montants() {
        let plat = "ATTRIBUTION DU MARCHÉ Valeur totale du marché (hors TVA) Valeur : 180000 € \
Lot 01 : SECTEUR LONS LE SAUNIER Date d'attribution : 23/07/26 Nombre d'offres reçues : 2 \
Nombre d'offres reçues de la part de PME : 0 \
Nombre d'offres reçues de la part de soumissionnaires d'autres États membres de l'UE : 0 \
Montant HT : 60 000,00 € Marché n° : 2026074A - Claire Martin Psychologue 12 PL DU MARCHE 39000 LONS-LE-SAUNIER \
Le titulaire est une PME : NON Sous-traitance : oui. Part de la sous-traitance Inconnue. \
Montant HT : 42 000,00 € Marché n° : 2026074A - Paul Durand 8 RTE DES CHAMPS 01240 LENT \
Le titulaire est une PME : NON";
        let lots = parse_lots(plat);
        assert_eq!(lots.len(), 2);
        assert_eq!(lots[0].titulaire, "Claire Martin Psychologue");
        assert_eq!(lots[0].montant, Some(60_000.0));
        assert_eq!(lots[1].titulaire, "Paul Durand");
        assert_eq!(lots[1].ville.as_deref(), Some("LENT"));
        assert_eq!(lots[1].montant, Some(42_000.0));
        assert_eq!(lots[1].nb_offres, Some(2));
        assert_eq!(
            lots[1].description.as_deref(),
            Some("Lot 01 : SECTEUR LONS LE SAUNIER")
        );
    }

    /// The amount is also given as a range, with no colon: the ceiling is kept,
    /// and the label must not pass for the winner's name.
    #[test]
    fn un_montant_en_fourchette_rend_son_plafond() {
        let lots = parse_lots(
            "ATTRIBUTION DU MARCHÉ Lot 1 : Lot n°1 : Date d'attribution : 04/05/26 \
Nombre d'offres reçues : 1 Montant HT de 56 000,00 à 80 000,00 € \
GIP Auvergne 43 BD FRANCOIS MITTERRAND 63000 CLERMONT-FERRAND",
        );
        assert_eq!(lots.len(), 1);
        assert_eq!(lots[0].titulaire, "GIP Auvergne");
        assert_eq!(lots[0].ville.as_deref(), Some("CLERMONT-FERRAND"));
        assert_eq!(lots[0].montant, Some(80_000.0));
    }

    /// The market number carries spaces as often as not: only the dash that
    /// follows it separates it from the winner.
    #[test]
    fn un_numero_de_marche_a_espaces_ne_deborde_pas_sur_le_titulaire() {
        let lots = parse_lots(
            "ATTRIBUTION DU MARCHÉ Lot 1 : TRANSPORTS Date d'attribution : 25/06/26 \
Nombre d'offres reçues : 1 Montant HT : 100 000,00 € \
Marché n° : 0 003 26 003 lot 1 - RAPIDES DU VAL DE LOIRE 11 AV AMPERE 45800 SAINT-JEAN-DE-BRAYE \
Sous-traitance : non.",
        );
        assert_eq!(lots.len(), 1);
        assert_eq!(lots[0].titulaire, "RAPIDES DU VAL DE LOIRE");
        assert_eq!(lots[0].ville.as_deref(), Some("SAINT-JEAN-DE-BRAYE"));
    }

    /// `IDE=AAA` also serves the markets declared without outcome, which award
    /// nothing.
    #[test]
    fn un_marche_sans_suite_ne_rend_aucun_lot() {
        assert!(parse_lots("MARCHE SANS SUITE Objet du marché Travaux").is_empty());
    }
}
