//! TED — la colonne européenne du fond marchés.
//!
//! Publications Office API v3, public and keyless, served in eForms. Live stock:
//! French buyer and receipt deadline not passed. Three fields carry the buyer
//! profile, `document-url-lot` (the DCE), `submission-url-lot` (the submission)
//! and `buyer-profile`: a URL carrying the file's identifier gives the join key
//! to the engine's row, otherwise it gives the tenant.
//!
//! Ce que TED ajoute au BOAMP : un avis publié au JOUE dont l'acheteur a omis
//! le BOAMP, et les reports d'échéance, qu'un rectificatif TED date alors que la
//! ligne BOAMP reste figée.

use jiff::civil::Date;
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{json, Value};

use lj_annonces::zones::{departement_depuis_nuts3, zones_departements, Zone};
use lj_annonces::{
    cles_jointure_depuis_urls, departement_from_cp, embellir_titre, nettoyer_acheteur,
    parse_ts_iso, AnnonceBrute, ContenuAnnonce, LotAnnonce, LotAttribue, MontantEstime,
    NatureMarche,
};
use lj_core::dates::parse_date_fr;

use super::MoteurRun;
use lj_sources::error::{Result, SourceError};

const API: &str = "https://api.ted.europa.eu/v3/notices/search";
/// Plafond servi par l'API sur `limit`.
const PAR_PAGE: usize = 250;
/// Guard, far above the open FR stock: past it, the query has changed meaning.
const PAGES_MAX: usize = 200;

/// Acheteur français, date limite non passée. Le filtre serveur évite de
/// paginer tout l'historique — un avis clos n'apprend plus rien à la moisson.
const REQUETE: &str =
    "(organisation-country-buyer IN (FRA)) AND (deadline-receipt-request>=today())";

/// Where a notice says it applies, read in order until one answers: the NUTS
/// subdivision of the place of performance, the buyer's own NUTS, then its post
/// code. The post code of the place of performance is not asked for:
/// it is never served where the subdivision is silent. The `-lot` variants
/// carry one value per lot, which is where a market spanning several
/// départements shows itself.
const CHAMPS_ZONE: [&str; 6] = [
    "place-of-performance-subdiv-proc",
    "place-of-performance-subdiv-lot",
    "place-of-performance-other-proc",
    "place-of-performance-other-lot",
    "organisation-country-sub-buyer",
    "organisation-post-code-buyer",
];

const CHAMPS: [&str; 21] = [
    "publication-number",
    "notice-title",
    "publication-date",
    "deadline-receipt-request",
    "buyer-name",
    "buyer-profile",
    "document-url-lot",
    "document-url-part",
    "submission-url-lot",
    "internal-identifier-proc",
    "contract-nature",
    "procedure-type",
    "classification-cpv",
    "organisation-identifier-buyer",
    "description-proc",
    "title-lot",
    "description-lot",
    "main-classification-lot",
    "award-criterion-name-lot",
    "estimated-value-proc",
    "estimated-value-cur-proc",
];

/// Les champs qui portent une URL de profil d'acheteur, dans l'ordre de
/// confiance : le lien profond du dossier avant la porte du tenant.
const CHAMPS_URL: [&str; 4] = [
    "document-url-lot",
    "document-url-part",
    "submission-url-lot",
    "buyer-profile",
];

#[derive(Deserialize)]
struct Reponse {
    #[serde(default)]
    notices: Vec<Value>,
    #[serde(rename = "totalNoticeCount", default)]
    total: u64,
}

async fn page(client: &wreq::Client, n: usize, champs: &[&str]) -> Result<Reponse> {
    let corps = json!({
        "query": REQUETE,
        "fields": champs,
        "limit": PAR_PAGE,
        "page": n,
    });
    let body = super::send_text(client.post(API).json(&corps), "ted search").await?;
    serde_json::from_str(&body)
        .map_err(|e| SourceError::Invalid(format!("ted: réponse illisible: {e}")))
}

/// Ré-énumère les avis FR encore ouverts.
pub async fn enumerate() -> Result<MoteurRun> {
    let client = super::client();
    let plafond = plafond_annee();
    let mut rows = Vec::new();
    let mut annonce = 0;
    let champs = [CHAMPS.as_slice(), CHAMPS_ZONE.as_slice()].concat();
    for n in 1..=PAGES_MAX {
        let r = page(&client, n, &champs).await?;
        annonce = r.total;
        if r.notices.is_empty() {
            break;
        }
        rows.extend(r.notices.iter().filter_map(|n| depuis_notice(n, plafond)));
        if rows.len() as u64 >= annonce {
            break;
        }
    }
    Ok(MoteurRun {
        annonce: Some(annonce),
        echecs: 0,
        hotes: Vec::new(),
        rows: super::dedup_source_uid(rows),
    })
}

/// The hosts each open notice cites, the census's second witness beside the
/// BOAMP: the contracting entities of the special sectors (SNCF, RATP, EDF, RTE,
/// La Poste…) publish in the OJEU and on their own profile, bypassing the BOAMP.
///
/// Four fields suffice, which pages lighter than [`enumerate`].
pub async fn hotes_par_avis() -> Result<Vec<std::collections::HashSet<String>>> {
    let client = super::client();
    let mut par_avis: Vec<std::collections::HashSet<String>> = Vec::new();
    for n in 1..=PAGES_MAX {
        let r = page(&client, n, &CHAMPS_URL).await?;
        if r.notices.is_empty() {
            break;
        }
        par_avis.extend(r.notices.iter().map(|notice| {
            urls(notice)
                .iter()
                .filter_map(|u| super::discover::hote_de(u))
                .map(|h| super::discover::canon_host(&h))
                .collect()
        }));
        if par_avis.len() as u64 >= r.total {
            break;
        }
    }
    Ok(par_avis)
}

/// Un champ multilingue TED (`{"fra": ["…"]}`) : le français d'abord, sinon la
/// première langue servie.
fn texte(v: Option<&Value>) -> Option<String> {
    let premier = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => a.first().and_then(Value::as_str).map(str::to_string),
        _ => None,
    };
    match v? {
        Value::Object(o) => o.get("fra").or_else(|| o.values().next()).and_then(premier),
        autre => premier(autre),
    }
    .map(|s| super::desechapper(s.trim()))
    .filter(|s| !s.is_empty())
}

/// Premier élément d'un champ répété par lot (`["services", "services"]`).
fn premier(v: Option<&Value>) -> Option<&str> {
    match v? {
        Value::String(s) => Some(s.as_str()),
        Value::Array(a) => a.first().and_then(Value::as_str),
        _ => None,
    }
}

/// Horizon past which a deadline is a typing slip: a dynamic purchasing system
/// stays open for years, and ten let the real ones through but not the typos.
const HORIZON_ANS: i16 = 10;

/// La date limite la plus tardive parmi les lots : la consultation reste
/// ouverte tant qu'un de ses lots l'est. Une date aberrante est écartée plutôt
/// que ramenée à une borne — la ligne existe, elle n'est simplement pas dans la
/// fenêtre ouverte.
fn echeance(v: Option<&Value>, plafond: i16) -> Option<Timestamp> {
    let tardive = match v? {
        Value::String(s) => parse_ts_iso(s),
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_str)
            .filter_map(parse_ts_iso)
            .max(),
        _ => None,
    }?;
    let annee = tardive.to_zoned(jiff::tz::TimeZone::UTC).date().year();
    (annee <= plafond).then_some(tardive)
}

/// L'année au-delà de laquelle une échéance n'est plus crédible.
fn plafond_annee() -> i16 {
    Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::get("Europe/Paris").expect("tzdb embarquée"))
        .date()
        .year()
        + HORIZON_ANS
}

/// TED titles a notice France, then the CPV label, then the object, joined by a
/// long dash no object uses. The prefix goes: kept, it stops the row from merging
/// into its twin, whose title is the buyer's own.
fn objet_nu(titre: &str) -> &str {
    static PREFIXE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^France\s*[–—]\s*[^–—]+?\s*[–—]\s*").expect("préfixe TED valide")
    });
    match PREFIXE.find(titre) {
        Some(m) => &titre[m.end()..],
        None => titre,
    }
}

/// La nature du marché, en vocabulaire eForms.
fn nature(v: Option<&Value>) -> Option<NatureMarche> {
    match premier(v)? {
        "works" => Some(NatureMarche::Travaux),
        "supplies" => Some(NatureMarche::Fournitures),
        "services" => Some(NatureMarche::Services),
        _ => Some(NatureMarche::Autres),
    }
}

/// Le SIREN de l'acheteur : `organisation-identifier-buyer` porte le SIRET,
/// parfois suivi d'identifiants internes qui n'en sont pas.
/// Every string a field carries, however it nests them: a plain value, one per
/// lot, or a language map.
fn tous(v: Option<&Value>) -> Vec<&str> {
    fn aplatir<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
        match v {
            Value::String(s) => out.push(s.as_str()),
            Value::Array(a) => a.iter().for_each(|x| aplatir(x, out)),
            Value::Object(o) => o.values().for_each(|x| aplatir(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Some(v) = v {
        aplatir(v, &mut out);
    }
    out
}

fn zones(n: &Value) -> Vec<Zone> {
    let depuis_nuts = |cles: &[&str]| {
        zones_departements(
            cles.iter()
                .flat_map(|c| tous(n.get(*c)))
                .filter_map(departement_depuis_nuts3),
        )
    };
    let mut z = depuis_nuts(&[
        "place-of-performance-subdiv-proc",
        "place-of-performance-subdiv-lot",
    ]);
    if z.is_empty() {
        z = depuis_nuts(&["organisation-country-sub-buyer"]);
    }
    if z.is_empty() {
        z = zones_departements(
            tous(n.get("organisation-post-code-buyer"))
                .into_iter()
                .filter_map(departement_from_cp),
        );
    }
    // A place-less market is a statement, not a gap: `anyw-cou` stops at the
    // country, `anyw` and `anyw-eea` reach past it.
    for marqueur in [
        "place-of-performance-other-proc",
        "place-of-performance-other-lot",
    ]
    .iter()
    .flat_map(|c| tous(n.get(*c)))
    {
        match marqueur {
            "anyw-cou" => z.push(Zone::FranceEntiere),
            "anyw" | "anyw-eea" => z.push(Zone::UnionEuropeenne),
            _ => {}
        }
    }
    z.sort();
    z.dedup();
    z
}

fn siren(v: Option<&Value>) -> Option<String> {
    let candidats: Vec<&str> = match v? {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
        _ => return None,
    };
    candidats
        .into_iter()
        .map(|s| s.replace(' ', ""))
        .find(|s| s.len() == 14 && s.chars().all(|c| c.is_ascii_digit()))
        .map(|s| s[..9].to_string())
}

/// TED serves some of its buyer URLs with the query separator HTML-escaped.
fn desechappe(u: &str) -> String {
    u.replace("&amp;", "&")
}

fn urls(n: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for champ in CHAMPS_URL {
        match n.get(champ) {
            Some(Value::String(s)) => out.push(desechappe(s)),
            Some(Value::Array(a)) => {
                out.extend(a.iter().filter_map(Value::as_str).map(desechappe));
            }
            _ => {}
        }
    }
    // Ordre de confiance conservé, doublons ôtés : un même lien revient sur
    // chaque lot.
    let mut vus = std::collections::HashSet::new();
    out.retain(|u| vus.insert(u.clone()));
    out
}

/// A dossier URL designates a consultation. A bare platform root designates
/// none and already lives in `url_depot`: `buyer-profile`, last of the four
/// fields.
fn precise(u: &str) -> bool {
    u.split_once("://").is_some_and(|(_, reste)| {
        reste
            .split_once('/')
            .is_some_and(|(_, chemin)| !chemin.is_empty())
    })
}

/// L'avis sur le site TED, en français : l'URL de référence de la ligne.
fn url_avis(numero: &str) -> String {
    format!("https://ted.europa.eu/fr/notice/-/detail/{numero}")
}

/// The French entries of a per-lot multilingual field, in lot order.
fn par_lot(v: Option<&Value>) -> Vec<String> {
    let Some(Value::Object(o)) = v else {
        return Vec::new();
    };
    let Some(Value::Array(a)) = o.get("fra").or_else(|| o.values().next()) else {
        return Vec::new();
    };
    a.iter()
        .map(|x| x.as_str().unwrap_or_default().trim().to_string())
        .collect()
}

/// Lots read off three arrays TED serves in the same order, which nothing in
/// the payload states. Arrays of unequal length, a notice without a French
/// version, yield no lot rather than a mismatched one.
fn lots(n: &Value) -> Vec<LotAnnonce> {
    let noms = par_lot(n.get("title-lot"));
    let descriptions = par_lot(n.get("description-lot"));
    let cpvs: Vec<String> = match n.get("main-classification-lot") {
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| x.as_str().unwrap_or_default().to_string())
            .collect(),
        _ => Vec::new(),
    };
    if noms.len() != descriptions.len() || noms.len() != cpvs.len() {
        return Vec::new();
    }
    noms.into_iter()
        .zip(descriptions)
        .zip(cpvs)
        .map(|((nom, description), cpv)| LotAnnonce {
            nom: Some(nom).filter(|s| !s.is_empty()),
            description: Some(description).filter(|s| !s.is_empty()),
            cpv: Some(cpv).filter(|s| !s.is_empty()),
        })
        .filter(|l| l != &LotAnnonce::default())
        .collect()
}

/// The four fields, all served by the search: TED has no notice page to read.
fn contenu(n: &Value) -> ContenuAnnonce {
    let mut criteres = par_lot(n.get("award-criterion-name-lot"));
    criteres.retain(|c| !c.is_empty());
    let mut vus = std::collections::HashSet::new();
    criteres.retain(|c| vus.insert(c.clone()));
    ContenuAnnonce {
        description: texte(n.get("description-proc")),
        lots: lots(n),
        criteres,
        montant_estime: texte(n.get("estimated-value-proc"))
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| *v > 0.0)
            .map(|valeur| MontantEstime {
                valeur,
                devise: texte(n.get("estimated-value-cur-proc")),
            }),
    }
}

fn depuis_notice(n: &Value, plafond: i16) -> Option<AnnonceBrute> {
    let numero = n.get("publication-number")?.as_str()?;
    let objet = texte(n.get("notice-title")).map(|t| embellir_titre(objet_nu(&t)))?;
    let urls = urls(n);
    let date_publication: Option<Date> = texte(n.get("publication-date"))
        .as_deref()
        .and_then(parse_date_fr);
    Some(AnnonceBrute {
        contenu: contenu(n),
        descripteurs: Vec::new(),
        moteur: "ted",
        source_uid: format!("ted:{numero}"),
        cles_jointure: cles_jointure_depuis_urls(urls.iter().map(String::as_str)),
        reference: texte(n.get("internal-identifier-proc")),
        acheteur: texte(n.get("buyer-name")).map(|a| nettoyer_acheteur(&a)),
        siren: siren(n.get("organisation-identifier-buyer")),
        objet,
        nature: nature(n.get("contract-nature")),
        cpv: premier(n.get("classification-cpv")).map(str::to_string),
        zones: zones(n),
        date_publication,
        echeance: echeance(n.get("deadline-receipt-request"), plafond),
        url_avis: Some(url_avis(numero)),
        url_rc: None,
        url_dce: urls.iter().find(|u| precise(u)).cloned(),
        // Le tenant, quand l'avis le nomme : c'est lui que le recensement
        // classe et que le jumelage lit.
        url_depot: texte(n.get("buyer-profile")),
        procedure: texte(n.get("procedure-type")),
    })
}

/// Every notice of one procedure carries this UUID: it is the only handle from
/// an award back to the consultation it closes.
fn procedure(n: &Value) -> Option<String> {
    texte(n.get("procedure-identifier"))
}

/// Procedures resolved in batches: the query string is a literal list, and
/// hundreds of UUIDs in one `IN` make too long a line.
const PROCEDURES_PAR_LOT: usize = 50;

/// The consultation notices of each procedure, `ted:<numéro>` as the store
/// keys them. Only the `cn-` shapes: a result attaches to a consultation.
async fn consultations_par_procedure(
    client: &wreq::Client,
    procedures: &[String],
) -> Result<std::collections::HashMap<String, Vec<String>>> {
    let mut out: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for tranche in procedures.chunks(PROCEDURES_PAR_LOT) {
        let corps = json!({
            "query": format!("(procedure-identifier IN ({}))", tranche.join(", ")),
            "fields": ["publication-number", "procedure-identifier", "notice-type"],
            "limit": PAR_PAGE,
            "page": 1,
        });
        let body = super::send_text(client.post(API).json(&corps), "ted procedures").await?;
        let r: Reponse = serde_json::from_str(&body)
            .map_err(|e| SourceError::Invalid(format!("ted procedures: reponse illisible: {e}")))?;
        for n in &r.notices {
            let (Some(p), Some(numero), Some(genre)) = (
                procedure(n),
                n.get("publication-number").and_then(Value::as_str),
                n.get("notice-type").and_then(Value::as_str),
            ) else {
                continue;
            };
            if genre.starts_with("cn-") {
                out.entry(p).or_default().push(format!("ted:{numero}"));
            }
        }
    }
    Ok(out)
}

/// Every eForms shape an award takes.
const TYPES_RESULTAT: &str = "can-standard, can-social, can-tran, can-modif, can-desg";

const CHAMPS_RESULTAT: [&str; 7] = [
    "publication-number",
    "procedure-identifier",
    "publication-date",
    "notice-title",
    "winner-name",
    "contract-conclusion-date",
    "total-value",
];

/// What an award notice says about the contract it closes, keyed by the
/// procedure it belongs to.
#[derive(Debug)]
pub struct ResultatTed {
    pub procedure: String,
    /// `ted:<number>` of every consultation notice of the procedure, which is
    /// how the store keys them. Empty when none is published.
    pub cles_consultation: Vec<String>,
    pub publication: String,
    pub url_avis: String,
    pub date_publication: Option<Date>,
    pub lots: Vec<LotAttribue>,
}

/// One winner per lot, in three parallel arrays TED serves in lot order. A
/// notice naming no winner is a result all the same: the event is the fact.
fn lots_attribues(n: &Value) -> Vec<LotAttribue> {
    let liste = |v: Option<&Value>| -> Vec<String> {
        match v {
            Some(Value::Object(o)) => o
                .get("fra")
                .or_else(|| o.values().next())
                .map(valeurs)
                .unwrap_or_default(),
            autre => autre.map(valeurs).unwrap_or_default(),
        }
    };
    let titulaires = liste(n.get("winner-name"));
    let dates = liste(n.get("contract-conclusion-date"));
    let montant = n.get("total-value").and_then(Value::as_f64);
    titulaires
        .iter()
        .enumerate()
        .map(|(i, t)| LotAttribue {
            description: None,
            titulaire: nettoyer_acheteur(t),
            // `organisation-identifier-tenderer` is a TED internal key
            // (`1821478-1-2-1`), never a SIREN: posing it would be a false edge.
            siren: None,
            ville: None,
            // The total covers the whole notice, so it is posed once, on the
            // single-lot case where it is unambiguous.
            montant: match titulaires.len() {
                1 => montant,
                _ => None,
            },
            devise: montant.map(|_| "EUR".to_string()),
            nb_offres: None,
            date_attribution: dates.get(i).and_then(|d| parse_date_fr(d)),
        })
        .collect()
}

fn valeurs(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => s.clone(),
                autre => autre.to_string(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn depuis_resultat(n: &Value) -> Option<ResultatTed> {
    let numero = n.get("publication-number")?.as_str()?;
    Some(ResultatTed {
        procedure: procedure(n)?,
        cles_consultation: Vec::new(),
        publication: numero.to_string(),
        url_avis: url_avis(numero),
        date_publication: texte(n.get("publication-date"))
            .as_deref()
            .and_then(parse_date_fr),
        lots: lots_attribues(n),
    })
}

/// The French award notices published over the last `jours` days. The window
/// replaces the deadline filter, which an award notice does not carry.
pub async fn resultats(jours: u32) -> Result<Vec<ResultatTed>> {
    let client = super::client();
    let requete = format!(
        "(organisation-country-buyer IN (FRA)) AND (notice-type IN ({TYPES_RESULTAT})) \
         AND (publication-date>=today(-{jours}))"
    );
    let mut out = Vec::new();
    for n in 1..=PAGES_MAX {
        let corps = json!({
            "query": requete,
            "fields": CHAMPS_RESULTAT,
            "limit": PAR_PAGE,
            "page": n,
        });
        let body = super::send_text(client.post(API).json(&corps), "ted resultats").await?;
        let r: Reponse = serde_json::from_str(&body)
            .map_err(|e| SourceError::Invalid(format!("ted resultats: reponse illisible: {e}")))?;
        if r.notices.is_empty() {
            break;
        }
        out.extend(r.notices.iter().filter_map(depuis_resultat));
        if out.len() as u64 >= r.total {
            break;
        }
    }
    let procedures: Vec<String> = out
        .iter()
        .map(|r| r.procedure.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let par_procedure = consultations_par_procedure(&client, &procedures).await?;
    for r in &mut out {
        r.cles_consultation = par_procedure.get(&r.procedure).cloned().unwrap_or_default();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{depuis_notice, depuis_resultat, objet_nu, precise};
    use lj_annonces::zones::codes;
    use lj_annonces::NatureMarche;

    /// Plafond fixe : le test ne dépend pas de l'année où il tourne.
    const PLAFOND: i16 = 2036;

    /// Real shape TED serves: the three lot arrays run in the same
    /// order, and the criteria repeat lot after lot.
    #[test]
    fn un_avis_ted_rend_les_quatre_champs() {
        let n = serde_json::json!({
            "publication-number": "53812-2024",
            "notice-title": {"fra": "Acquisition de véhicules"},
            "description-proc": {"fra": "Un système d'acquisition dynamique pour l'achat de véhicules neufs ou d'occasion."},
            "title-lot": {"fra": ["Petite citadine propre", "Fourgon frigorifique"]},
            "description-lot": {"fra": ["quantités estimées : 2 unités par an", ""]},
            "main-classification-lot": ["34144700", "34144000"],
            "award-criterion-name-lot": {"fra": ["Prix", "Valeur technique", "Prix", "Valeur technique"]},
            "estimated-value-proc": "249590110",
            "estimated-value-cur-proc": "EUR",
            "buyer-profile": ["https://marchespublics.auvergnerhonealpes.eu/"]
        });
        let r = depuis_notice(&n, PLAFOND).expect("avis lisible");
        assert_eq!(
            r.contenu.description.as_deref(),
            Some(
                "Un système d'acquisition dynamique pour l'achat de véhicules neufs ou d'occasion."
            )
        );
        assert_eq!(r.contenu.lots.len(), 2);
        assert_eq!(
            r.contenu.lots[0].nom.as_deref(),
            Some("Petite citadine propre")
        );
        assert_eq!(r.contenu.lots[0].cpv.as_deref(), Some("34144700"));
        assert_eq!(r.contenu.lots[1].description, None);
        assert_eq!(r.contenu.criteres, ["Prix", "Valeur technique"]);
        let m = r.contenu.montant_estime.expect("montant lu");
        assert_eq!(
            (m.valeur, m.devise.as_deref()),
            (249_590_110.0, Some("EUR"))
        );
    }

    /// A notice with no French version leaves its lot arrays out of step: no lot
    /// rather than a wrong one.
    #[test]
    fn des_tableaux_de_lot_desalignes_ne_rendent_aucun_lot() {
        let n = serde_json::json!({
            "publication-number": "2-2026",
            "notice-title": {"fra": "Transport scolaire"},
            "main-classification-lot": ["60112000"],
        });
        let r = depuis_notice(&n, PLAFOND).expect("avis lisible");
        assert!(r.contenu.lots.is_empty());
        assert!(r.contenu.est_vide());
    }

    #[test]
    fn la_racine_nue_d_une_plateforme_n_est_pas_un_dossier() {
        assert!(!precise("https://www.marches-securises.fr"));
        assert!(!precise("http://achat.maregionsud.fr/"));
        assert!(precise(
            "https://www.achatpublic.com/sdm/ent/gen/ent_detail.do?PCSLID=CSL_2026_c8i7xqiSxN"
        ));
        assert!(precise(
            "https://www.marches-publics.info/?fuseaction=dematEnt"
        ));
    }

    /// TED prefixes the object with the country and the CPV label: the prefix
    /// goes, the object's short dashes stay.
    #[test]
    fn le_prefixe_pays_et_cpv_de_ted_ne_fait_pas_partie_de_l_objet() {
        assert_eq!(
            objet_nu("France – Véhicules à moteur – Achat de véhicules - lot 2"),
            "Achat de véhicules - lot 2"
        );
        assert_eq!(
            objet_nu("France – Services d'architecture – Réfection – tranche ferme"),
            "Réfection – tranche ferme"
        );
        // Un titre qui ne suit pas le gabarit n'est pas amputé.
        assert_eq!(objet_nu("Fourniture de repas"), "Fourniture de repas");
    }

    /// A deadline past the ceiling is a typing slip: it is dropped rather than
    /// clamped, so the row is not held open for centuries.
    #[test]
    fn une_echeance_aberrante_est_ecartee_pas_ramenee() {
        let n = serde_json::json!({
            "publication-number": "1-2026",
            "notice-title": "France – Transport – AT 01 transport scolaire",
            "deadline-receipt-request": ["2924-03-18T12:00:00+02:00"],
            "buyer-profile": ["https://marchespublics.auvergnerhonealpes.eu/"]
        });
        let r = depuis_notice(&n, PLAFOND).expect("avis lisible");
        assert_eq!(r.echeance, None);
        assert_eq!(r.objet, "AT 01 transport scolaire");
    }

    /// Real shape TED serves: multilingual fields, values repeated per lot, URLs
    /// in an array. The PLACE URL carries the file's identifier, the join key to
    /// the Atexo row.
    #[test]
    fn un_avis_ted_rend_sa_cle_de_jointure_et_son_echeance_la_plus_tardive() {
        let n = serde_json::json!({
            "publication-number": "49554-2024",
            "notice-title": {"fra": ["France – Produits pharmaceutiques – Fourniture de produits de contraste"]},
            "publication-date": "2024-01-24Z",
            "deadline-receipt-request": [
                "2026-09-14T12:00:00+02:00",
                "2026-10-02T12:00:00+02:00"
            ],
            "buyer-name": {"fra": ["CHU DE BORDEAUX (33)"]},
            "buyer-profile": ["https://www.marches-publics.gouv.fr/?page=Entreprise.EntrepriseAdvancedSearch&AllCons&id=2461241&orgAcronyme=x7c"],
            "document-url-lot": ["https://www.marches-publics.gouv.fr/?page=Entreprise.EntrepriseAdvancedSearch&AllCons&id=2461241&orgAcronyme=x7c"],
            "internal-identifier-proc": "24S0012",
            "contract-nature": ["supplies", "supplies"],
            "procedure-type": "open",
            "classification-cpv": ["33696000", "33696000"],
            "place-of-performance-subdiv-proc": ["FRI12"],
            "organisation-identifier-buyer": ["26330011200017", "1776461"]
        });
        let r = depuis_notice(&n, PLAFOND).expect("avis lisible");
        assert_eq!(r.source_uid, "ted:49554-2024");
        assert_eq!(
            r.cles_jointure,
            vec!["atexo:marches-publics.gouv.fr:2461241".to_string()]
        );
        assert_eq!(r.acheteur.as_deref(), Some("CHU DE BORDEAUX"));
        assert_eq!(r.siren.as_deref(), Some("263300112"));
        assert_eq!(r.reference.as_deref(), Some("24S0012"));
        assert_eq!(r.nature, Some(NatureMarche::Fournitures));
        assert_eq!(r.cpv.as_deref(), Some("33696000"));
        assert_eq!(codes(&r.zones), ["33"]);
        assert_eq!(
            r.echeance.map(|t| t.to_string()),
            Some("2026-10-02T10:00:00Z".to_string())
        );
        assert_eq!(
            r.url_avis.as_deref(),
            Some("https://ted.europa.eu/fr/notice/-/detail/49554-2024")
        );
    }

    /// The three things a TED notice says about a place, on one notice: a NUTS
    /// per lot, the fallback on the buyer, and the place-less scope.
    #[test]
    fn un_avis_multi_lots_porte_toutes_ses_zones_et_sa_portee() {
        let n = serde_json::json!({
            "publication-number": "1-2026",
            "notice-title": {"fra": ["Accord-cadre de travaux"]},
            "place-of-performance-subdiv-lot": [["FRY30"], ["FR101", "FRM01"]],
            "place-of-performance-other-lot": [["anyw-cou"]],
            "organisation-country-sub-buyer": ["FRE11"],
        });
        let r = depuis_notice(&n, PLAFOND).expect("avis lisible");
        assert_eq!(codes(&r.zones), ["2A", "75", "973", "fr"]);
    }

    #[test]
    fn sans_lieu_d_execution_le_nuts_de_l_acheteur_repond_puis_son_code_postal() {
        let acheteur = serde_json::json!({
            "publication-number": "2-2026",
            "notice-title": {"fra": ["Fournitures"]},
            "organisation-country-sub-buyer": ["FRE11"],
            "organisation-post-code-buyer": ["59000"],
        });
        assert_eq!(
            codes(&depuis_notice(&acheteur, PLAFOND).unwrap().zones),
            ["59"]
        );

        let cp_seul = serde_json::json!({
            "publication-number": "3-2026",
            "notice-title": {"fra": ["Fournitures"]},
            "organisation-post-code-buyer": ["97300"],
        });
        assert_eq!(
            codes(&depuis_notice(&cp_seul, PLAFOND).unwrap().zones),
            ["973"]
        );

        let etranger = serde_json::json!({
            "publication-number": "4-2026",
            "notice-title": {"fra": ["Fournitures"]},
            "place-of-performance-subdiv-proc": ["CH025"],
        });
        assert!(depuis_notice(&etranger, PLAFOND).unwrap().zones.is_empty());
    }

    #[test]
    fn l_url_de_dossier_arrive_desechappee() {
        let n = serde_json::json!({
            "publication-number": "104233-2025",
            "notice-title": {"fra": ["Travaux de voirie"]},
            "document-url-lot": ["https://www.marches-publics.info/mpiaws/index.cfm?fuseaction=dematEnt.login&amp;type=DCE&amp;IDM=1849888"]
        });
        let r = depuis_notice(&n, PLAFOND).expect("avis lisible");
        assert_eq!(
            r.url_dce.as_deref(),
            Some("https://www.marches-publics.info/mpiaws/index.cfm?fuseaction=dematEnt.login&type=DCE&IDM=1849888")
        );
    }

    /// Un avis sans objet ne fait pas une ligne : rien à afficher, rien à
    /// apparier. La réconciliation l'écarterait de toute façon.
    #[test]
    fn un_avis_sans_objet_est_ignore() {
        let n = serde_json::json!({"publication-number": "1-2026"});
        assert!(depuis_notice(&n, PLAFOND).is_none());
    }
    /// One `can-standard` as the API serves it: winners, dates and the total
    /// come as parallel arrays in lot order.
    #[test]
    fn un_avis_d_attribution_rend_un_lot_par_titulaire() {
        let n: serde_json::Value = serde_json::json!({
            "publication-number": "616271-2026",
            "procedure-identifier": "ef902aa0-f6ee-455b-a1a6-a529ec03150b",
            "publication-date": "2026-08-17+02:00",
            "winner-name": {"fra": ["IDVERDE", "REGIE DE QUARTIER"]},
            "contract-conclusion-date": ["2026-07-16+02:00", "2026-07-16+02:00"],
            "total-value": 1080447.96
        });
        let r = depuis_resultat(&n).expect("avis d'attribution lisible");
        assert_eq!(r.publication, "616271-2026");
        assert_eq!(r.procedure, "ef902aa0-f6ee-455b-a1a6-a529ec03150b");
        assert_eq!(r.lots.len(), 2);
        assert_eq!(r.lots[0].titulaire, "IDVERDE");
        assert_eq!(
            r.lots[0].date_attribution,
            Some(jiff::civil::date(2026, 7, 16))
        );
        // The total covers the whole notice: posing it per lot would multiply
        // the amount by their number.
        assert_eq!(r.lots[0].montant, None);
    }

    #[test]
    fn un_avis_a_lot_unique_porte_le_montant_total() {
        let n: serde_json::Value = serde_json::json!({
            "publication-number": "616291-2026",
            "procedure-identifier": "b83f7a2f-ab68-46d9-9553-1ae44a97cad8",
            "winner-name": {"fra": ["AMC2 Architectes"]},
            "total-value": 301208.0
        });
        let r = depuis_resultat(&n).expect("avis d'attribution lisible");
        assert_eq!(r.lots.len(), 1);
        assert_eq!(r.lots[0].montant, Some(301208.0));
        assert_eq!(r.lots[0].devise.as_deref(), Some("EUR"));
    }

    /// Without a procedure identifier the notice attaches to nothing, so it is
    /// dropped rather than posed on some row.
    #[test]
    fn un_avis_sans_procedure_est_ecarte() {
        let n: serde_json::Value = serde_json::json!({
            "publication-number": "1-2026",
            "winner-name": {"fra": ["X"]}
        });
        assert!(depuis_resultat(&n).is_none());
    }
}
