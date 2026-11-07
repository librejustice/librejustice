//! Recensement des profils d'acheteurs : à quel moteur appartient un hôte ?
//!
//! Les moteurs multi-tenants (Atexo, Safetender) n'ont pas de point d'entrée
//! national : il faut la liste de leurs instances. Deux sources, indépendantes
//! l'une de l'autre : les URLs citées par les avis (l'avis pointe le profil qui
//! porte le DCE), et les déclarations de profil d'acheteur sur data.gouv, que
//! l'arrêté du 22 mars 2019 impose. Chaque hôte est ensuite classé : liste
//! `HORS_BOAMP` et domaine connu d'abord, empreinte HTML ensuite, sonde API
//! Atexo en dernier. Sortie : la table `profil_acheteur`.

use std::collections::HashMap;
use std::sync::LazyLock;

use futures::StreamExt;
use regex::Regex;
use serde::Deserialize;

use super::{send_text, send_text_once};
use lj_sources::error::{Result, SourceError};

const HOTES_PARALLELES: usize = 48;

/// Un hôte recensé, avec le moteur identifié et son poids en avis BOAMP.
#[derive(Debug)]
pub struct Profil {
    pub host: String,
    pub moteur: Option<&'static str>,
    pub avis_boamp: i32,
}

/// Moteurs reconnaissables au seul domaine (un point d'entrée national).
const PAR_DOMAINE: [(&str, &str); 18] = [
    ("aws", r"(^|\.)(marches-publics\.info|aws-achat\.info)$"),
    ("achatpublic", r"(^|\.)achatpublic\.com$"),
    ("atline", r"(^|\.)marches-securises\.fr$"),
    // `centreofficielles.com` est une marque blanche Dematis (le portail sert
    // `centremarchespublics.fr`), derrière Cloudflare, qui ferme la porte au
    // client HTTP : le domaine tranche là où l'empreinte ne peut pas.
    (
        "dematis",
        r"(^|\.)(e-marchespublics\.com|centreofficielles\.com)$",
    ),
    (
        "synapse",
        r"(^|\.)(synapse-entreprises\.com|synapse-consult\.com|synapse-ouest\.(com|fr))$",
    ),
    ("klekoon", r"(^|\.)klekoon\.com$"),
    ("safetender", r"(^|\.)safetender\.com$"),
    ("xmarches", r"(^|\.)xmarches\.fr$"),
    ("aji", r"(^|\.)aji-france\.com$"),
    (
        "modula",
        r"(^|\.)(proxilegales\.fr|achatspublicscorse\.com|marches-demat\.com|marchespublics\.pro|antilles-legales\.fr|laprovencemarchespublics\.com)$",
    ),
    (
        "centraledesmarches",
        r"(^|\.)(centraledesmarches\.com|medialex\.fr)$",
    ),
    // Engines identified but not connected: censused to measure the coverage
    // residue, not harvested.
    ("agysoft", r"(^|\.)agysoft\.marches-publics\.info$"),
    ("telerecours", r"(^|\.)telerecours\.fr$"),
    ("eurolegales", r"(^|\.)marchespublics-eurolegales\.com$"),
    // Les éditeurs des entités adjudicatrices, que TED découvre et que le BOAMP
    // ne cite pas. Chacun sert quelques tenants, sous sa marque ou sous le
    // domaine de l'acheteur — d'où les hôtes littéraux. Jaggaer (ex-BravoSolution)
    // rend ses consultations ouvertes à une session invitée, sans compte.
    (
        "jaggaer",
        r"(^|\.)bravosolution\.com$|^e-sourcing\.extra\.laposte\.fr$",
    ),
    ("ivalua", r"(^|\.)ivalua\.app$"),
    (
        "oalia",
        r"(^|\.)oalia\.com$|^fournisseurs\.buyrte\.cloud-rte-france\.com$",
    ),
    // EU-Supply sert une marque par acheteur (`?B=…`) et le connecteur `synapse`
    // les énumère toutes : un hôte EU-Supply nu relève donc du même connecteur.
    ("synapse", r"(^|\.)eu-supply\.com$"),
];

/// Portails hors du périmètre BOAMP, avec leur moteur. Le recensement dérive des
/// URLs citées par les avis BOAMP : un territoire dont la commande publique relève
/// d'un autre régime n'y publie pas, et son portail n'entrera jamais dans le
/// comptage. La Nouvelle-Calédonie a le sien (compétence propre du Congrès), et
/// `portail.marchespublics.nc` est une instance Atexo MPE standard. Ces hôtes sont
/// injectés au recensement à poids nul et leur moteur est pris ici plutôt que
/// sondé.
const HORS_BOAMP: [(&str, &str); 2] = [
    ("portail.marchespublics.nc", "atexo"),
    ("teariari.gov.pf", "safetender"),
];

/// Empreintes HTML, quand le domaine ne dit rien (marques blanches).
const EMPREINTES: [(&str, &str); 11] = [
    (
        "atexo",
        r"Entreprise\.EntrepriseAdvancedSearch|AtexoMpe|MPE_",
    ),
    ("dematis", r"e-marchespublics|Dematis"),
    // Avant `agysoft` : les portails Modula citent « marco agysoft » dans leur meta
    // keywords (texte SEO), et `find` prend la première empreinte.
    ("modula", r"publisher_portail|Modula D"),
    ("agysoft", r"marcoweb|Agysoft"),
    ("atline", r"marches-securises|Atline"),
    ("safetender", r"safetender|Local Trust"),
    ("klekoon", r"klekoon"),
    (
        "synapse",
        r"synapse-entreprises|synapse-consult|synapse-ouest|eu-supply",
    ),
    ("achatpublic", r"achatpublic\.com"),
    ("aws", r"marches-publics\.info|aws-achat"),
    // Ivalua sert la page de connexion sous le domaine de l'acheteur
    // (`pha2.edf.com`, `pha2.enedis.fr`) et n'y laisse que son nom.
    ("ivalua", r"ivalua"),
];

fn par_domaine(host: &str) -> Option<&'static str> {
    static RES: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
        PAR_DOMAINE
            .iter()
            .map(|(m, p)| (*m, Regex::new(p).expect("motif de domaine valide")))
            .collect()
    });
    RES.iter()
        .find(|(_, re)| re.is_match(host))
        .map(|(m, _)| *m)
}

fn empreinte(html: &str) -> Option<&'static str> {
    static RES: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
        EMPREINTES
            .iter()
            .map(|(m, p)| {
                (
                    *m,
                    Regex::new(&format!("(?i){p}")).expect("motif d'empreinte valide"),
                )
            })
            .collect()
    });
    RES.iter()
        .find(|(_, re)| re.is_match(html))
        .map(|(m, _)| *m)
}

/// Sonde l'API Atexo : `hydra:member` présent ⇒ instance MPE. Dernier recours,
/// après l'empreinte HTML (deux requêtes de plus par hôte).
async fn est_atexo(client: &wreq::Client, host: &str) -> bool {
    for h in [host.to_string(), format!("www.{host}")] {
        let sonde = send_text_once(
            client
                .get(format!("https://{h}/api/v2/consultations"))
                .header("Accept", "application/ld+json")
                .query(&[("itemsPerPage", "1")]),
        )
        .await;
        if sonde.is_some_and(|(_, body)| body.contains("hydra:member")) {
            return true;
        }
    }
    false
}

/// The engine a host belongs to, read on its name then on the page it served,
/// for a caller that already holds the page.
pub fn moteur_de_page(host: &str, html: &str) -> Option<&'static str> {
    par_domaine(host).or_else(|| empreinte(html))
}

async fn classer(client: &wreq::Client, host: String, avis_boamp: i32) -> Profil {
    let profil = |moteur| Profil {
        host: host.clone(),
        moteur,
        avis_boamp,
    };
    if let Some(m) = HORS_BOAMP.iter().find(|(h, _)| *h == host).map(|(_, m)| *m) {
        return profil(Some(m));
    }
    if let Some(m) = par_domaine(&host) {
        return profil(Some(m));
    }
    // La page d'accueil sert deux verdicts d'un coup : l'URL finale (un hôte
    // qui redirige vers son moteur) et l'empreinte HTML.
    // A profile still served in the clear answers 200 with an empty body over
    // TLS, so the plain scheme comes after the two TLS forms and not instead:
    // `mp74.fr` and `sarthe-marchespublics.fr` are AWS tenants that no TLS
    // probe could ever fingerprint.
    let cibles = [
        format!("https://{host}/"),
        format!("https://www.{host}/"),
        format!("http://{host}/"),
        format!("http://www.{host}/"),
    ];
    let mut accueil = None;
    for cible in cibles {
        let Some((url_finale, html)) = send_text_once(client.get(cible)).await else {
            continue;
        };
        // L'hôte d'arrivée, pas l'URL : les motifs de [`PAR_DOMAINE`] sont
        // ancrés sur un nom d'hôte et ne reconnaissent rien dans une URL entière.
        let arrivee = hote_de(&url_finale).unwrap_or_default();
        if let Some(m) = par_domaine(&arrivee).or_else(|| empreinte(&html)) {
            return profil(Some(m));
        }
        if accueil.is_none() {
            accueil = Some((url_finale, html));
        }
    }
    if est_atexo(client, &host).await {
        return profil(Some("atexo"));
    }
    if let Some(m) = moteur_par_redirection(&host).await {
        return profil(Some(m));
    }
    let Some((url, html)) = accueil else {
        return profil(None);
    };
    profil(moteur_derriere_le_site(client, &url, &html).await)
}

/// Redirection hops read, a profile's chain counting two or three.
const SAUTS: usize = 5;

/// The hosts a redirection chain crosses, whatever the landing page is worth:
/// `deauville-marches.fr` bounces onto an eu-supply login that loops, and a
/// probe reading only the arrival throws away the one verdict of the trip. The
/// clear scheme first, the redirection to the platform being posed there.
async fn moteur_par_redirection(host: &str) -> Option<&'static str> {
    static CLIENT: LazyLock<wreq::Client> = LazyLock::new(|| {
        super::avec_sortie(wreq::Client::builder())
            .emulation(super::NAVIGATEUR)
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(8))
            .tls_cert_verification(false)
            .tls_verify_hostname(false)
            .redirect(wreq::redirect::Policy::none())
            .build()
            .expect("wreq client build")
    });
    for depart in [format!("http://{host}/"), format!("https://{host}/")] {
        let Ok(mut url) = url::Url::parse(&depart) else {
            continue;
        };
        for _ in 0..SAUTS {
            let Ok(reponse) = CLIENT.get(url.as_str()).send().await else {
                break;
            };
            let Some(saut) = reponse
                .headers()
                .get(wreq::header::LOCATION)
                .and_then(|l| l.to_str().ok())
                .and_then(|l| url.join(l).ok())
            else {
                break;
            };
            url = saut;
            if let Some(m) = url
                .host_str()
                .map(canon_host)
                .as_deref()
                .and_then(par_domaine)
            {
                return Some(m);
            }
        }
    }
    None
}

/// Links followed from a buyer's homepage: a menu entry, a procurement page,
/// sometimes one section per department.
const LIENS_SUIVIS: usize = 6;

static VOCABULAIRE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)march[eé]s?[\s_-]*publics|appels?[\s_-]*d.offres|consultations?|avis[\s_-]*de[\s_-]*march|achats?[\s_-]*publics",
    )
    .unwrap()
});

static ANCRE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<a\b[^>]*href=["']([^"'#]+)["'][^>]*>(.*?)</a>"#).unwrap());

fn liens(base: &str, html: &str) -> Vec<(String, String)> {
    let Ok(base) = url::Url::parse(base) else {
        return Vec::new();
    };
    ANCRE
        .captures_iter(html)
        .filter_map(|c| {
            let url = base.join(&c[1]).ok()?;
            (url.scheme() == "http" || url.scheme() == "https")
                .then(|| (url.to_string(), c[2].to_string()))
        })
        .collect()
}

/// The platform a buyer's own site leads to, one link deeper. Only the landing domain
/// rules, never the words of a page the buyer wrote, a mention being no proof of
/// where one deposits.
async fn moteur_derriere_le_site(
    client: &wreq::Client,
    base: &str,
    html: &str,
) -> Option<&'static str> {
    let moteur_lie = |page: &str, html: &str| {
        liens(page, html)
            .into_iter()
            .find_map(|(url, _)| par_domaine(&hote_de(&url)?))
    };
    if let Some(m) = moteur_lie(base, html) {
        return Some(m);
    }
    let rubriques: Vec<String> = liens(base, html)
        .into_iter()
        .filter(|(url, texte)| VOCABULAIRE.is_match(texte) || VOCABULAIRE.is_match(url))
        .map(|(url, _)| url)
        .take(LIENS_SUIVIS)
        .collect();
    for rubrique in rubriques {
        let Some((finale, page)) = send_text_once(client.get(&rubrique)).await else {
            continue;
        };
        if let Some(m) = hote_de(&finale).as_deref().and_then(par_domaine) {
            return Some(m);
        }
        if let Some(m) = moteur_lie(&finale, &page) {
            return Some(m);
        }
    }
    None
}

/// Hôtes cités qui ne servent pas eux-mêmes le profil qu'ils désignent : portails
/// dont la MPE vit sur un sous-domaine, et coquilles de saisie de l'acheteur. La
/// forme canonique est celle qui répond ; l'hôte cité en hérite le moteur au
/// recensement, et le jumelage — qui canonise le même hôte de dépôt — y retrouve le
/// canal sous lequel la moisson a écrit ses lignes. Correspondance littérale,
/// jamais approchée.
const ALIAS_HOTE: [(&str, &str); 72] = [
    // Portails dont la MPE Atexo vit sous `marches.<hôte>` ; l'avis cite tantôt
    // l'apex, tantôt la MPE.
    ("departement13.fr", "marches.departement13.fr"),
    // The apex is an Umbraco site and nothing on it redirects to the MPE.
    (
        "alsacemarchespublics.eu",
        "plateforme.alsacemarchespublics.eu",
    ),
    (
        "marchespublics.mairie-marseille.fr",
        "mpe.mairie-marseille.fr",
    ),
    ("loire-atlantique.fr", "marches.loire-atlantique.fr"),
    ("maximilien.fr", "marches.maximilien.fr"),
    ("maximilien.com", "marches.maximilien.fr"),
    ("megalis.bretagne.bzh", "marches.megalis.bretagne.bzh"),
    ("montpellier3m.fr", "marches.montpellier3m.fr"),
    ("rhone.fr", "marches.rhone.fr"),
    ("ternum-bfc.fr", "marches.ternum-bfc.fr"),
    // Megalis, sous son ancien domaine (mort) et sans le point.
    (
        "marches.megalisbretagne.bzh",
        "marches.megalis.bretagne.bzh",
    ),
    (
        "marches.megalisbretagne.org",
        "marches.megalis.bretagne.bzh",
    ),
    // PLACE, dont l'acronyme sert d'hôte autant que les coquilles du domaine.
    ("marchespublics.gouv.fr", "marches-publics.gouv.fr"),
    ("marches-public.gouv.fr", "marches-publics.gouv.fr"),
    ("marches-publics.gouv", "marches-publics.gouv.fr"),
    ("marches-publics.gouv.f", "marches-publics.gouv.fr"),
    ("marches-publics.gouv.ff", "marches-publics.gouv.fr"),
    ("marches-publics.gouv.fr.l", "marches-publics.gouv.fr"),
    ("marches-publics.gouv.fr.la", "marches-publics.gouv.fr"),
    ("marches-publics.gou.fr", "marches-publics.gouv.fr"),
    ("marches-piblics.gouv.fr", "marches-publics.gouv.fr"),
    ("marches-publis.gouv.fr", "marches-publics.gouv.fr"),
    ("marches.publics.gouv.fr", "marches-publics.gouv.fr"),
    ("marchespublic.gouv.fr", "marches-publics.gouv.fr"),
    ("marches_pubics.gouv.fr", "marches-publics.gouv.fr"),
    ("marhces-publics.gouv.fr", "marches-publics.gouv.fr"),
    ("place.gouv.fr", "marches-publics.gouv.fr"),
    ("ww.marchespublics.gouv.fr", "marches-publics.gouv.fr"),
    ("wwww.marches-publics.gouv.fr", "marches-publics.gouv.fr"),
    // Atline.
    ("marches.securises.fr", "marches-securises.fr"),
    ("marches-securises", "marches-securises.fr"),
    ("marches-securise.fr", "marches-securises.fr"),
    ("marche-securises.fr", "marches-securises.fr"),
    ("marches-securises.com", "marches-securises.fr"),
    ("marches-securises.f", "marches-securises.fr"),
    ("marches-securises.fr.la", "marches-securises.fr"),
    ("marches-seucrises.fr", "marches-securises.fr"),
    ("marches_securises.fr", "marches-securises.fr"),
    ("marchessecurises.fr", "marches-securises.fr"),
    ("marches-securisés.fr", "marches-securises.fr"),
    ("marches-sécurises.fr", "marches-securises.fr"),
    ("marchés-securisés.fr", "marches-securises.fr"),
    ("marchés-sécurises.fr", "marches-securises.fr"),
    ("marchés-sécurisés.fr", "marches-securises.fr"),
    ("ww.marches-securises.fr", "marches-securises.fr"),
    ("wwww.marches-securises.fr", "marches-securises.fr"),
    // AWS.
    ("marche-public.info", "marches-publics.info"),
    ("marches-public.info", "marches-publics.info"),
    ("marches-publics.info.fr", "marches-publics.info"),
    ("marchespublics.info", "marches-publics.info"),
    ("aws.fr", "marches-publics.info"),
    (
        "savoie-dechets.marchespublics.info",
        "savoie-dechets.marches-publics.info",
    ),
    // Dematis, dont le trait d'union du domaine saute.
    ("e-marchepublics.com", "e-marchespublics.com"),
    (
        "beaupreau-en-mauges.e-marchespublics",
        "beaupreau-en-mauges.e-marchespublics.com",
    ),
    (
        "caprovenceverte.emarchespublics.com",
        "caprovenceverte.e-marchespublics.com",
    ),
    ("catp.emarchespublics.com", "catp.e-marchespublics.com"),
    (
        "ch-angouleme.e.marchespublics.com",
        "ch-angouleme.e-marchespublics.com",
    ),
    (
        "cmaroccitanie.emarchespublics.com",
        "cmaroccitanie.e-marchespublics.com",
    ),
    (
        "hautsdeseinehabitat.emarchespublics.com",
        "hautsdeseinehabitat.e-marchespublics.com",
    ),
    (
        "mairiecarquefou.emarchespublics.com",
        "mairiecarquefou.e-marchespublics.com",
    ),
    ("mnhn.emarchespublics.com", "mnhn.e-marchespublics.com"),
    (
        "sivubordeauxmerignac.emarchespublics.com",
        "sivubordeauxmerignac.e-marchespublics.com",
    ),
    ("syseg.emarchespublics.com", "syseg.e-marchespublics.com"),
    (
        "ville-plaisir.e-marches-publics.com",
        "ville-plaisir.e-marchespublics.com",
    ),
    // Safetender et achatpublic, sur le nom de l'instance.
    (
        "commandepublique-nca-vdn-ccas.safetender",
        "commandepublique-nca-vdn-ccas.safetender.com",
    ),
    (
        "marchespublics-ght-rouergue.safetnder.com",
        "marchespublics-ght-rouergue.safetender.com",
    ),
    (
        "vichycommunaute.satender.com",
        "vichycommunaute.safetender.com",
    ),
    (
        "vichycommunaute.sefetender.com",
        "vichycommunaute.safetender.com",
    ),
    ("elegia.achatspublic.com", "elegia.achatpublic.com"),
    ("synpase-entreprises.com", "synapse-entreprises.com"),
    // Omnikles n'est pas un moteur : une vitrine WordPress dont la « salle des
    // marchés » est un tenant Safetender, que la moisson énumère déjà.
    (
        "consultations-asf.omnikles.com",
        "asf-consultations.safetender.com",
    ),
    (
        "consultations-escota.omnikles.com",
        "escota-consultations.safetender.com",
    ),
];

/// Hôte canonique : minuscules, sans `www.` ni point final, réécrit vers
/// l'hôte qui sert le profil quand [`ALIAS_HOTE`] en nomme un.
pub fn canon_host(h: &str) -> String {
    let h = h.trim().trim_end_matches('.').to_lowercase();
    let h = h.trim_start_matches("www.");
    ALIAS_HOTE
        .iter()
        .find(|(cite, _)| *cite == h)
        .map_or_else(|| h.to_string(), |(_, sert)| (*sert).to_string())
}

/// Classe une liste d'hôtes pondérée par le nombre d'avis BOAMP qui les
/// citent. Les hôtes sont canonisés et fusionnés avant sondage.
pub async fn recenser(poids: HashMap<String, i32>) -> Result<Vec<Profil>> {
    let mut fusion: HashMap<String, i32> = HashMap::new();
    for (h, n) in poids {
        *fusion.entry(canon_host(&h)).or_insert(0) += n;
    }
    let client = super::client_sonde();
    Ok(futures::stream::iter(fusion.into_iter().map(|(host, n)| {
        let client = &client;
        async move { classer(client, host, n).await }
    }))
    .buffer_unordered(HOTES_PARALLELES)
    .collect()
    .await)
}

/// Le résidu de couverture : hôtes qui ne portent ni profil d'acheteur ni DCE
/// (juridictions, Légifrance, europa.eu…).
static BRUIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(w3\.org|europa\.eu|boamp\.fr|legifrance|tribunal-administratif|juradm|conseil-etat|service-public|economie\.gouv\.fr|data\.gouv\.fr|inpi\.fr)",
    )
    .unwrap()
});

/// The engines actually harvested, the denominator of coverage. The others
/// (`agysoft`, `eurolegales`, `telerecours`) are identified and not connected:
/// their notices count as uncovered.
pub const MOTEURS_CONNECTES: [&str; 12] = [
    "atexo",
    "aws",
    "safetender",
    "dematis",
    "achatpublic",
    "atline",
    "synapse",
    "xmarches",
    "aji",
    "klekoon",
    "modula",
    "centraledesmarches",
];

// ── Deuxième source : ce que les acheteurs déclarent ────────────────────────

const DATAGOUV: &str = "https://www.data.gouv.fr/api/1/datasets/";
/// Le tag sous lequel une déclaration de profil d'acheteur est publiée.
const TAG_PROFIL: &str = "profil-acheteur";
/// Guard: past two pages, the tag has changed meaning.
const PAGES_DATAGOUV: usize = 5;
const PAR_PAGE_DATAGOUV: usize = 100;

#[derive(Deserialize)]
struct PageJeux {
    #[serde(default)]
    data: Vec<Jeu>,
}

#[derive(Deserialize)]
struct Jeu {
    #[serde(default)]
    resources: Vec<Ressource>,
}

#[derive(Deserialize)]
struct Ressource {
    #[serde(default)]
    format: Option<String>,
    url: String,
}

/// Nom de colonne réduit à ses lettres minuscules : les déclarations écrivent
/// `urlProfilAcheteur`, `URL_PROFIL_ACHETEUR` ou `url profil acheteur`.
fn cle_colonne(entete: &str) -> String {
    entete
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Les URLs de profil déclarées par un fichier au schéma `etalab/schema-decp-dpa`
/// (`siretAcheteur;urlProfilAcheteur;urlDCAT;coordonnees`). Un fichier hors
/// schéma ne déclare rien : un jeu mal classé sous le tag ne doit pas inventer
/// de tenants.
fn urls_declarees(csv: &str) -> Vec<String> {
    let csv = csv.trim_start_matches('\u{feff}');
    let Some(entete) = csv.lines().next() else {
        return Vec::new();
    };
    // Le schéma impose le point-virgule ; des déclarations sortent d'un tableur
    // qui a mis la virgule.
    let sep = if entete.matches(';').count() >= entete.matches(',').count() {
        b';'
    } else {
        b','
    };
    let mut lecteur = csv::ReaderBuilder::new()
        .delimiter(sep)
        .flexible(true)
        .from_reader(csv.as_bytes());
    let Ok(entetes) = lecteur.headers().cloned() else {
        return Vec::new();
    };
    let colonne = |nom: &str| entetes.iter().position(|e| cle_colonne(e) == nom);
    let Some(profil) = colonne("urlprofilacheteur") else {
        return Vec::new();
    };
    let dcat = colonne("urldcat");
    lecteur
        .records()
        .filter_map(std::result::Result::ok)
        .flat_map(|r| {
            [Some(profil), dcat]
                .into_iter()
                .flatten()
                .filter_map(|i| r.get(i))
                .map(str::trim)
                .filter(|s| s.starts_with("http"))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Les hôtes des profils d'acheteurs déclarés sur data.gouv. La liste n'est ni
/// consolidée ni exhaustive — elle ne mesure donc aucune couverture ; elle
/// nomme des tenants qu'aucun avis ne cite, et qu'on moissonnerait sinon
/// jamais.
pub async fn profils_declares() -> Result<Vec<String>> {
    let client = super::client();
    let mut jeux: Vec<Jeu> = Vec::new();
    for n in 1..=PAGES_DATAGOUV {
        let body = send_text(
            client.get(DATAGOUV).query(&[
                ("tag", TAG_PROFIL),
                ("page_size", &PAR_PAGE_DATAGOUV.to_string()),
                ("page", &n.to_string()),
            ]),
            "data.gouv datasets",
        )
        .await?;
        let page: PageJeux = serde_json::from_str(&body)
            .map_err(|e| SourceError::Invalid(format!("data.gouv: réponse illisible: {e}")))?;
        let recu = page.data.len();
        jeux.extend(page.data);
        // data.gouv rend 404 au-delà de la dernière page, jamais une page vide :
        // une page incomplète est la dernière, et un appel de plus est une erreur.
        if recu < PAR_PAGE_DATAGOUV {
            break;
        }
    }
    let mut hotes = Vec::new();
    for r in jeux.iter().flat_map(|j| &j.resources).filter(|r| {
        r.format
            .as_deref()
            .is_some_and(|f| f.eq_ignore_ascii_case("csv"))
    }) {
        // Une déclaration illisible (404, tableur renommé) se compte comme
        // absente : elle ne fait pas échouer le recensement.
        match send_text(client.get(&r.url), "data.gouv resource").await {
            Ok(csv) => hotes.extend(urls_declarees(&csv)),
            Err(e) => tracing::debug!(url = %r.url, erreur = %e, "declared profile unreadable"),
        }
    }
    let hotes: Vec<String> = hotes
        .iter()
        .filter_map(|u| hote_de(u))
        .map(|h| canon_host(&h))
        .filter(|h| !BRUIT.is_match(h))
        .collect();
    Ok(hotes)
}

/// L'hôte d'une URL, sans parseur : les URLs des déclarations sont saisies à la
/// main et un tiers ne passe pas `Url::parse`.
pub fn hote_de(url: &str) -> Option<String> {
    let hote = url
        .split_once("://")?
        .1
        .split(['/', '?', '#'])
        .next()?
        .split(':')
        .next()?
        .trim_end_matches(',');
    (hote.contains('.')).then(|| hote.to_lowercase())
}

/// Couverture mesurée sur un témoin : avis examinés, avis dont au moins un hôte
/// cité relève d'un moteur connecté.
#[derive(Debug)]
pub struct Temoin {
    pub canal: &'static str,
    pub avis: usize,
    pub couverts: usize,
}

/// Résultat du recensement : les profils classés et la couverture mesurée.
#[derive(Debug)]
pub struct Recensement {
    pub profils: Vec<Profil>,
    /// Un témoin par canal : leur somme est la couverture du fond, leur écart
    /// dit lequel voit un gisement que l'autre ignore.
    pub temoins: Vec<Temoin>,
    /// Hôtes venus des seules déclarations data.gouv : des tenants qu'aucun avis
    /// ne cite.
    pub declares_seuls: usize,
}

/// Recense les profils que citent les avis de marché actifs, et mesure la
/// couverture **par avis** : un avis est couvert dès qu'une de ses URLs porte un
/// moteur connecté.
///
/// Deux témoins d'avis, parce qu'un seul se mesurerait sur son propre reflet :
/// le BOAMP, qui pondère (il porte le gros du national et sert de file d'attente
/// au sondage), et TED, à poids nul, qui apporte les entités adjudicatrices
/// absentes du BOAMP. Plus une source de tenants sans avis : les déclarations
/// data.gouv, à poids nul elles aussi.
pub async fn recenser_profils() -> Result<Recensement> {
    let boamp = super::boamp::hotes_par_avis().await?;
    // TED est un appoint du recensement, pas sa colonne vertébrale : son échec
    // ne doit pas emporter la passe.
    let ted = super::ted::hotes_par_avis().await.unwrap_or_else(|e| {
        tracing::error!(erreur = %e, "census: TED witness failed");
        Vec::new()
    });
    let mut poids: HashMap<String, i32> = HashMap::new();
    for hotes in &boamp {
        for h in hotes.iter().filter(|h| !BRUIT.is_match(h)) {
            *poids.entry(h.clone()).or_insert(0) += 1;
        }
    }
    // Les hôtes sans poids n'entrent qu'après les pondérés : un hôte déjà cité
    // par le BOAMP garde le poids que ses avis lui donnent.
    let mut sans_poids = |hotes: Vec<String>| {
        let mut inedits = 0;
        for h in hotes.into_iter().filter(|h| !BRUIT.is_match(h)) {
            if let std::collections::hash_map::Entry::Vacant(e) = poids.entry(h) {
                e.insert(0);
                inedits += 1;
            }
        }
        inedits
    };
    sans_poids(HORS_BOAMP.iter().map(|(h, _)| (*h).to_string()).collect());
    sans_poids(ted.iter().flatten().cloned().collect());
    let declares_seuls = match profils_declares().await {
        Ok(hotes) => sans_poids(hotes),
        // Le canal déclaratif est un appoint : son échec ne coûte que les
        // tenants qu'il aurait nommés.
        Err(e) => {
            tracing::error!(erreur = %e, "census: data.gouv declarations failed");
            0
        }
    };
    let profils = recenser(poids).await?;
    let connectes: std::collections::HashSet<&str> = profils
        .iter()
        .filter(|p| p.moteur.is_some_and(|m| MOTEURS_CONNECTES.contains(&m)))
        .map(|p| p.host.as_str())
        .collect();
    let couverts = |avis: &[std::collections::HashSet<String>]| {
        avis.iter()
            .filter(|hotes| {
                hotes
                    .iter()
                    .any(|h| connectes.contains(canon_host(h).as_str()))
            })
            .count()
    };
    let temoins = vec![
        Temoin {
            canal: "boamp",
            avis: boamp.len(),
            couverts: couverts(&boamp),
        },
        Temoin {
            canal: "ted",
            avis: ted.len(),
            couverts: couverts(&ted),
        },
    ];
    Ok(Recensement {
        temoins,
        declares_seuls,
        profils,
    })
}

#[cfg(test)]
mod tests {
    use super::{canon_host, hote_de, par_domaine, ALIAS_HOTE, HORS_BOAMP};

    /// La page d'accueil d'un profil mène souvent à son moteur : Aéroports de
    /// Paris arrive sur `spartacus-supplier.ivalua.app/page.aspx/…`. C'est
    /// l'hôte d'arrivée qui classe — les motifs sont ancrés sur un nom d'hôte
    /// et ne reconnaissent rien dans une URL entière.
    #[test]
    fn l_hote_d_arrivee_classe_le_profil_qui_redirige_vers_son_moteur() {
        let url = "https://spartacus-supplier.ivalua.app/page.aspx/fr/usr/login";
        assert_eq!(par_domaine(url), None);
        assert_eq!(par_domaine(&hote_de(url).unwrap()), Some("ivalua"));
    }

    /// L'hôte cité par l'avis n'est pas toujours celui qui sert le profil : coquille de
    /// saisie ou portail dont la MPE vit sur un sous-domaine. La canonicalisation rend
    /// l'hôte qui répond, celui que le recensement classe.
    #[test]
    fn l_hote_cite_est_reecrit_vers_celui_qui_sert_le_profil() {
        assert_eq!(
            canon_host("WWW.Marches.Securises.FR."),
            "marches-securises.fr"
        );
        assert_eq!(canon_host("place.gouv.fr"), "marches-publics.gouv.fr");
        assert_eq!(
            canon_host("megalis.bretagne.bzh"),
            "marches.megalis.bretagne.bzh"
        );
        assert_eq!(
            canon_host("www.marches-securises.fr"),
            "marches-securises.fr"
        );
    }

    /// Une entrée d'alias doit être atteignable et terminale : la clé se lit **après**
    /// minuscules et `www.` retiré, la cible n'est la clé d'aucune autre.
    #[test]
    fn chaque_alias_est_atteignable_et_terminal() {
        for (cite, sert) in ALIAS_HOTE {
            assert_eq!(canon_host(cite), sert, "alias « {cite} » inatteignable");
            assert_eq!(canon_host(sert), sert, "cible « {sert} » non terminale");
        }
    }

    /// `classer` reçoit l'hôte déjà canonisé : une entrée écrite `www.X` ou en
    /// capitales serait classée par la sonde plutôt que par son moteur déclaré.
    #[test]
    fn les_hotes_hors_boamp_sont_canoniques() {
        for (h, _) in HORS_BOAMP {
            assert_eq!(canon_host(h), h);
        }
    }
}
