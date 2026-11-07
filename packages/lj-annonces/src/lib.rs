//! The public-procurement fond of the notice pot, pure: deadlines, key folding,
//! multi-channel reconciliation, and the classifiers of a market by CPV
//! division, trade and zone.
//!
//! The connectors (`lj-sources-annonces`) produce one [`AnnonceBrute`] per
//! publication channel (BOAMP, a buyer-profile engine). The same market appears
//! on several channels and merges into one canonical row. Its uid is minted at
//! the group's first sighting and never recomputed; the keys, exact joins then
//! (folded buyer, reference) then a fallback on (folded buyer, deadline,
//! truncated folded object), only bring the channels together.

pub mod cpv;
pub mod metiers;
pub mod zones;

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};

use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use jiff::Timestamp;

use lj_core::dates::parse_date_fr;

use crate::zones::Zone;

/// The trigrams of a title in the shape `pg_trgm` gives them: lowercased, cut
/// on what is not alphanumeric, each word padded with two blanks before and one
/// after. Holding that shape is what makes a threshold measured in SQL hold here.
fn trigrammes(s: &str) -> HashSet<[char; 3]> {
    let mut set = HashSet::new();
    let mut mot: Vec<char> = Vec::new();
    for brut in s.split(|c: char| !c.is_alphanumeric()) {
        if brut.is_empty() {
            continue;
        }
        mot.clear();
        mot.extend([' ', ' ']);
        mot.extend(brut.chars().flat_map(char::to_lowercase));
        mot.push(' ');
        for f in mot.windows(3) {
            set.insert([f[0], f[1], f[2]]);
        }
    }
    set
}

/// Jaccard sur les trigrammes, la `similarity()` de `pg_trgm`.
pub fn similarite_titres(a: &str, b: &str) -> f64 {
    let (a, b) = (trigrammes(a), trigrammes(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let communs = a.intersection(&b).count();
    communs as f64 / (a.len() + b.len() - communs) as f64
}

// ── Vocabulaires ────────────────────────────────────────────────────────────

/// Nature de marché : le vocabulaire commun aux formulaires des moteurs et au
/// BOAMP, seedé en `facet_value`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NatureMarche {
    Travaux,
    Services,
    Fournitures,
    Autres,
}

impl NatureMarche {
    pub fn facet_uid(self) -> &'static str {
        match self {
            NatureMarche::Travaux => "nature_marche:TRAVAUX",
            NatureMarche::Services => "nature_marche:SERVICES",
            NatureMarche::Fournitures => "nature_marche:FOURNITURES",
            NatureMarche::Autres => "nature_marche:AUTRES",
        }
    }

    /// Reconnaît le libellé source (« Travaux », « Marché de services »,
    /// « FOURNITURES »…). `None` = libellé hors vocabulaire.
    pub fn parse(s: &str) -> Option<Self> {
        let p = lj_fold::fold(s);
        if p.contains("travaux") {
            Some(NatureMarche::Travaux)
        } else if p.contains("service") {
            Some(NatureMarche::Services)
        } else if p.contains("fourniture") {
            Some(NatureMarche::Fournitures)
        } else if p.contains("autre") {
            Some(NatureMarche::Autres)
        } else {
            None
        }
    }
}

/// Divisions CPV (règlement CE 213/2008) — le grain de facette de `/annonces`,
/// seedé en `facet_value`. Le code complet reste dans `details`.
const CPV_DIVISIONS: [&str; 45] = [
    "03", "09", "14", "15", "16", "18", "19", "22", "24", "30", "31", "32", "33", "34", "35", "37",
    "38", "39", "41", "42", "43", "44", "45", "48", "50", "51", "55", "60", "63", "64", "65", "66",
    "70", "71", "72", "73", "75", "76", "77", "79", "80", "85", "90", "92", "98",
];

/// `45233140-2` → `cpv:45`. `None` si le code ne porte pas une division CPV
/// connue.
pub fn cpv_division_uid(code: &str) -> Option<String> {
    let division = code.get(..2)?;
    CPV_DIVISIONS
        .contains(&division)
        .then(|| format!("cpv:{division}"))
}

/// Département depuis un code postal : `33000` → `33`, `97300` → `973`,
/// Corse par plage postale (`20000-20199` → `2A`, au-delà → `2B`).
pub fn departement_from_cp(cp: &str) -> Option<String> {
    if cp.len() != 5 || !cp.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let code = match &cp[..2] {
        // 980xx is Monaco; the rest of 98 is the Pacific collectivities.
        "98" if &cp[..3] == "980" => "98".to_string(),
        "97" | "98" => cp[..3].to_string(),
        "20" => if cp < "20200" { "2A" } else { "2B" }.to_string(),
        _ => cp[..2].to_string(),
    };
    au_referentiel(code)
}

/// Département a postal address names inside free text, the five digits a
/// commune follows. Nothing else in a notice reads that way: the PO box of
/// « BP 30107 88204 REMIREMONT » is followed by digits, the BOAMP number of
/// « idweb:26-65991 » is preceded by a hyphen, and both come first in the page.
pub fn departement_dans_texte(texte: &str) -> Option<String> {
    static CP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?m)(?:^|[^-\d./])(\d{5})\s+\p{Lu}").expect("regex code postal valide")
    });
    CP.captures_iter(texte)
        .find_map(|c| departement_from_cp(&c[1]))
}

fn au_referentiel(code: String) -> Option<String> {
    lj_norm::referential_labels::departement_label(&code)
        .is_some()
        .then_some(code)
}

/// Code département tel que servi par un moteur (`1`, `2A`, `971`, `(33)`) →
/// forme canonique à deux chiffres (trois en outre-mer), lettres Corse en
/// majuscules.
pub fn normalise_departement(s: &str) -> Option<String> {
    let t: String = s
        .trim()
        .trim_matches(|c| c == '(' || c == ')')
        .to_uppercase();
    // Overseas comes coded ISO 3166 on the state platform, and New Caledonia
    // splits into its three provinces, which share the one département code.
    let t = t.split_once('.').map_or(t.as_str(), |(pays, _)| pays);
    let code = match t {
        // Registries still serve Corsica in its pre-1976 numbering.
        "201" => "2A".to_string(),
        "202" => "2B".to_string(),
        "GP" => "971".to_string(),
        "MQ" => "972".to_string(),
        "GF" => "973".to_string(),
        "RE" => "974".to_string(),
        "PM" => "975".to_string(),
        "YT" => "976".to_string(),
        "BL" => "977".to_string(),
        "MF" => "978".to_string(),
        "TF" => "984".to_string(),
        "WF" => "986".to_string(),
        "PF" => "987".to_string(),
        "NC" => "988".to_string(),
        d if d.len() == 1 && d.bytes().all(|b| b.is_ascii_digit()) => format!("0{d}"),
        d => d.to_string(),
    };
    au_referentiel(code)
}

// ── Dates françaises des moteurs ────────────────────────────────────────────

/// Heure limite : `17h`, `17h00`, `17:00`, `17 : 00` — `None` = illisible.
pub fn parse_heure(s: &str) -> Option<Time> {
    let t = s.trim().to_lowercase();
    let (h, m) = match t.split_once(['h', ':']) {
        Some((h, m)) => (h, m),
        None => return None,
    };
    let h = h.trim().parse::<i8>().ok()?;
    let m = m.trim();
    let m = if m.is_empty() {
        0
    } else {
        m.parse::<i8>().ok()?
    };
    Time::new(h, m, 0, 0).ok()
}

/// Échéance en instant réel : date (+ heure) Europe/Paris. Sans heure,
/// l'échéance tombe à la fin du jour civil (23:59:59).
pub fn echeance_ts(date: Date, heure: Option<Time>) -> Timestamp {
    let heure = heure.unwrap_or_else(|| Time::new(23, 59, 59, 0).unwrap());
    let tz = TimeZone::get("Europe/Paris").expect("tzdb-bundle-always embarque Europe/Paris");
    date.at(heure.hour(), heure.minute(), heure.second(), 0)
        .to_zoned(tz)
        .expect("date civile valide par construction")
        .timestamp()
}

// ── Réconciliation multi-canal ──────────────────────────────────────────────

/// Timestamp ISO 8601 avec offset (`2026-09-12T17:00:00+02:00`) — Atexo,
/// BOAMP. `None` = illisible.
pub fn parse_ts_iso(s: &str) -> Option<Timestamp> {
    s.trim().parse().ok()
}

/// Époque en millisecondes → instant.
pub fn ts_from_epoch_ms(ms: i64) -> Option<Timestamp> {
    Timestamp::from_millisecond(ms).ok()
}

/// Clés de jointure directes extraites des URLs d'un avis BOAMP : l'avis cite le
/// profil d'acheteur qui porte la consultation, et l'identifiant qu'on y lit est
/// exactement le `source_uid` que le connecteur du moteur produit.
pub fn cles_jointure_depuis_urls<'a>(urls: impl Iterator<Item = &'a str>) -> Vec<String> {
    use std::sync::LazyLock;
    // Les formes sous lesquelles une plateforme MPE/Atexo cite une consultation :
    // le lien profond `/entreprise/consultation/<id>` et les pages du routeur
    // `?page=Entreprise.<vue>&…&id=<id>`. Un seul espace d'identifiants par tenant
    // (PLACE, maximilien, megalis…), donc une seule clé `atexo:<host>:<id>` — le
    // `source_uid` du connecteur Atexo. Une recherche par mot-clé ne porte pas
    // d'`id` et ne fonde rien. Deux groupes de capture : hôte sans `www.`, id.
    static MULTI: LazyLock<[(regex::Regex, &'static str); 3]> = LazyLock::new(|| {
        [
            (
                regex::Regex::new(r"https?://(?:www\.)?([^/]+)/entreprise/consultation/(\d+)")
                    .unwrap(),
                "atexo",
            ),
            (
                regex::Regex::new(
                    r"https?://(?:www\.)?([^/?]+)[^\s]*[?&]page=Entreprise\.Entreprise(?:AdvancedSearch|DetailsConsultation)[^\s]*[?&]id=(\d+)",
                )
                .unwrap(),
                "atexo",
            ),
            // Portails Modula-Demat : le DCE anonyme et le displayRC de Centrale des
            // marchés, canal qu'aucun connecteur ne sert.
            (
                regex::Regex::new(
                    r"https?://(?:www\.)?([^/\s]+)/(?:[^/\s]+/)?publisher_(?:portail|plateforme)/public/(?:annonce/consulterDCEHorsIdentification|displayRC)\.jsp\?iIdAffaire=(\d+)",
                )
                .unwrap(),
                "modula",
            ),
        ]
    });
    static MOTIFS: LazyLock<[(regex::Regex, &'static str); 13]> = LazyLock::new(|| {
        [
            // AWS écrit le paramètre dans la casse de son point d'entrée : `IDM` derrière
            // `fuseaction=demat.termes`, `Idm` derrière `dematent.login` — même espace
            // d'identifiants.
            (regex::Regex::new(r"[?&](?i:idm)=(\d+)").unwrap(), "aws"),
            (
                regex::Regex::new(r"/Annonces/((?:MPI|DL)-pub-[A-Za-z0-9_\-]+)").unwrap(),
                "aws:pub",
            ),
            (
                regex::Regex::new(r"PCSLID=([A-Za-z0-9_\-]+)").unwrap(),
                "achatpublic",
            ),
            (
                regex::Regex::new(r"cle_dce=([A-Za-z0-9_\-]+)").unwrap(),
                "atline",
            ),
            (
                regex::Regex::new(r"e-marchespublics\.com/appel-offre/[a-z0-9\-]+/[a-z0-9\-]+/[a-z0-9\-]+/(\d+)/\d+").unwrap(),
                "dematis",
            ),
            // Forme courte, sans slugs : celle que portent les URLs eForms du BOAMP
            // (`cbc:URI`).
            (
                regex::Regex::new(r"e-marchespublics\.com/appel-offre/(\d+)").unwrap(),
                "dematis",
            ),
            (regex::Regex::new(r"[?&]PID=(\d+)").unwrap(), "synapse"),
            (
                regex::Regex::new(r"xmarches\.fr/entreprise/detailConsultation\.php\?key=(\d+)")
                    .unwrap(),
                "xmarches",
            ),
            (
                regex::Regex::new(r"mapa\.aji-france\.com/mapa/marche/(\d+)/show").unwrap(),
                "aji",
            ),
            // Klekoon, ancré sur l'hôte : forme courte, forme à slug, et la forme
            // `dematernet/….asp` du stock historique — même espace d'identifiants.
            (
                regex::Regex::new(r"klekoon\.com/detail-consultation/(\d+)").unwrap(),
                "klekoon",
            ),
            (
                regex::Regex::new(r"klekoon\.com[^\s]*[?&]consultation_ID=(\d+)").unwrap(),
                "klekoon",
            ),
            // Centrale des marchés has two identifier spaces, and its own pages
            // never name the first: the `demat` shortcut bounces by HTTP to
            // `lacentraledesmarches.com/marches/<other id>`, which is what the
            // connector enumerates. Hence two prefixes, or a seven-digit
            // shortcut would join a six-digit consultation the day the ranges
            // meet.
            (
                regex::Regex::new(r"demat\.centraledesmarches\.com/(\d+)").unwrap(),
                "centraledesmarches:demat",
            ),
            (
                regex::Regex::new(r"lacentraledesmarches\.com/marches(?:-publics/[^/\s]+)?/(\d+)")
                    .unwrap(),
                "centraledesmarches",
            ),
        ]
    });
    let mut cles = Vec::new();
    for url in urls {
        for (re, moteur) in MULTI.iter() {
            if let Some(c) = re.captures(url) {
                cles.push(format!("{moteur}:{}:{}", &c[1], &c[2]));
            }
        }
        for (re, prefixe) in MOTIFS.iter() {
            if let Some(c) = re.captures(url) {
                cles.push(format!("{prefixe}:{}", &c[1]));
            }
        }
    }
    cles.sort();
    cles.dedup();
    cles
}

/// One lot of a market, as the notice publishes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct LotAnnonce {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nom: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// CPV of the lot itself, finer than the market's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpv: Option<String>,
}

/// Estimated value of the market, as the notice declares it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct MontantEstime {
    pub valeur: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub devise: Option<String>,
}

/// What a notice says about the market beyond its title: the closed list every
/// channel answers.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ContenuAnnonce {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lots: Vec<LotAnnonce>,
    /// Award criteria, weight in brackets when the notice gives one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub criteres: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub montant_estime: Option<MontantEstime>,
}

impl ContenuAnnonce {
    pub fn est_vide(&self) -> bool {
        self.description.is_none()
            && self.lots.is_empty()
            && self.criteres.is_empty()
            && self.montant_estime.is_none()
    }

    /// Field by field and not channel by channel: the BOAMP gives the
    /// description where a platform gives the criteria, and both enter.
    fn fondre<'a>(mut sources: impl Iterator<Item = &'a ContenuAnnonce> + Clone) -> ContenuAnnonce {
        ContenuAnnonce {
            description: sources.clone().find_map(|c| c.description.clone()),
            lots: sources
                .clone()
                .map(|c| &c.lots)
                .find(|l| !l.is_empty())
                .cloned()
                .unwrap_or_default(),
            criteres: sources
                .clone()
                .map(|c| &c.criteres)
                .find(|c| !c.is_empty())
                .cloned()
                .unwrap_or_default(),
            montant_estime: sources.find_map(|c| c.montant_estime.clone()),
        }
    }
}

/// Une consultation telle que vue par un seul canal de publication.
#[derive(Debug, Clone)]
pub struct AnnonceBrute {
    pub moteur: &'static str,
    /// `boamp:24-…`, `aws:1857736`, `atexo:<host>:<id>`…
    pub source_uid: String,
    /// Clés de jointure directes vers d'autres canaux (pour un avis BOAMP : les
    /// `source_uid` des moteurs lus dans ses URLs). Le `source_uid` du canal
    /// lui-même est toujours implicitement une clé.
    pub cles_jointure: Vec<String>,
    pub reference: Option<String>,
    pub acheteur: Option<String>,
    pub siren: Option<String>,
    pub objet: String,
    pub nature: Option<NatureMarche>,
    /// Code CPV principal complet (`45233140` ou `45233140-2`).
    pub cpv: Option<String>,
    /// Everywhere the market applies, as this channel publishes it. Empty means
    /// the channel said nothing, never that the market has no place: the two
    /// are told apart by [`Zone::FranceEntiere`].
    pub zones: Vec<Zone>,
    pub date_publication: Option<Date>,
    pub echeance: Option<Timestamp>,
    pub url_avis: Option<String>,
    pub url_rc: Option<String>,
    pub url_dce: Option<String>,
    /// Plateforme de dépôt déclarée par l'avis (hôte du recensement
    /// `profil_acheteur`), quand le canal n'offre pas de lien profond.
    pub url_depot: Option<String>,
    pub procedure: Option<String>,
    pub contenu: ContenuAnnonce,
    /// Thematic classification the channel publishes, in its own vocabulary:
    /// the BOAMP `descripteur_libelle`, 100 values on 2026.
    pub descripteurs: Vec<String>,
}

/// Trust order for the canonical fields, and the order in which a notice's
/// channels get their detail page read.
pub const PRIORITE: [&str; 17] = [
    "boamp",
    // TED est aussi structuré que le BOAMP (eForms) mais il abrège l'objet à
    // 200 caractères : il passe derrière, devant les scrapers.
    "ted",
    "atexo",
    "safetender",
    "aws",
    "achatpublic",
    "atline",
    "dematis",
    "synapse",
    "aji",
    "xmarches",
    "modula",
    "klekoon",
    "provincenord",
    // An aggregator, behind every platform it republishes: its card gives a
    // buyer and an estimated closing date where the source gives the contract.
    "centraledesmarches",
    // La presse ferme la marche : titre de composition, acheteur en capitales,
    // aucun champ structuré. Elle apporte l'avis qu'aucun autre canal n'a, pas
    // la meilleure version de celui que tous ont.
    "nicematin",
    "presse",
];

/// Engines whose notice page adds nothing to what the listing gave. Picking one
/// of them to enrich a row spends the visit and leaves the hole: [`PRIORITE`]
/// ranks by trust in the fields, not by what a page can still say. The
/// aggregator is not one of them: its card mirrors the notice in full, and 593
/// open markets have no other channel.
pub const SANS_FICHE: [&str; 6] = [
    "boamp",
    "ted",
    "presse",
    "provincenord",
    "nicematin",
    "decp",
];

fn priorite(moteur: &str) -> usize {
    PRIORITE
        .iter()
        .position(|m| *m == moteur)
        .unwrap_or(PRIORITE.len())
}

/// La ligne canonique prête pour la table `annonce` (le jsonb `details` est
/// assemblé côté ingest, à la frontière d'écriture).
#[derive(Debug)]
pub struct AnnonceCanonique {
    pub uid: String,
    pub famille_uid: Option<&'static str>,
    pub titre: String,
    pub acheteur_libelle: String,
    pub zones: Vec<Zone>,
    pub cpv: Option<String>,
    pub cpv_uid: Option<String>,
    pub date_publication: Date,
    pub echeance: Option<Timestamp>,
    pub url_action: String,
    pub url_depot: Option<String>,
    pub reference: Option<String>,
    pub procedure: Option<String>,
    pub contenu: ContenuAnnonce,
    pub descripteurs: Vec<String>,
    pub sirens: Vec<String>,
    pub sources: Vec<AnnonceBrute>,
}

fn cle_primaire(b: &AnnonceBrute) -> Option<String> {
    let (acheteur, reference) = (b.acheteur.as_deref()?, b.reference.as_deref()?);
    let (a, r) = (lj_fold::fold(acheteur), lj_fold::fold(reference));
    // Une « référence » égale à l'acheteur est un champ mal extrait, pas une
    // référence : elle fondrait toutes ses consultations en une ligne.
    (!a.is_empty() && !r.is_empty() && r != a).then(|| format!("ref|{a}|{r}"))
}

/// Clé de repli, la plus faible des trois. Un acheteur vide ne fonde aucune
/// identité.
fn cle_repli(b: &AnnonceBrute) -> Option<String> {
    let acheteur = lj_fold::fold(b.acheteur.as_deref()?);
    if acheteur.is_empty() {
        return None;
    }
    let jour = b
        .echeance
        .map(|ts| {
            ts.to_zoned(TimeZone::get("Europe/Paris").expect("tzdb embarquée"))
                .date()
                .to_string()
        })
        .unwrap_or_default();
    let objet: String = lj_fold::fold(&b.objet).chars().take(80).collect();
    Some(format!("obj|{acheteur}|{jour}|{objet}"))
}

/// Une fusion décidée contre le stock : la ligne `ancien` est le même marché
/// que `nouveau`. Le store re-parente ses canaux et ses arêtes, pose la
/// redirection (`annonce_redirect`, 301 sur `/marche/…`) et supprime la ligne
/// absorbée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fusion {
    pub ancien: String,
    pub nouveau: String,
}

/// Le produit d'un run de réconciliation : les lignes canoniques du batch et
/// les fusions découvertes contre le stock.
#[derive(Debug)]
pub struct Canonicalisation {
    pub annonces: Vec<AnnonceCanonique>,
    pub fusions: Vec<Fusion>,
}

/// Union-find sur les indices de canaux. La racine d'un groupe est toujours
/// son plus petit indice — le premier canal dans l'ordre de tri, qui devient
/// le canal fondateur quand le groupe frappe un uid neuf.
struct Groupes {
    parent: Vec<usize>,
    /// Moteurs présents dans chaque groupe, portés à la racine : le garde-fou
    /// des clés molles.
    moteurs: Vec<HashSet<&'static str>>,
}

impl Groupes {
    fn new(rows: &[AnnonceBrute]) -> Self {
        Groupes {
            parent: (0..rows.len()).collect(),
            moteurs: rows.iter().map(|r| HashSet::from([r.moteur])).collect(),
        }
    }

    fn racine(&mut self, i: usize) -> usize {
        if self.parent[i] != i {
            let r = self.racine(self.parent[i]);
            self.parent[i] = r;
        }
        self.parent[i]
    }

    /// Union inconditionnelle (clé exacte). La plus petite racine gagne.
    fn unir(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.racine(a), self.racine(b));
        if ra == rb {
            return;
        }
        let (bas, haut) = (ra.min(rb), ra.max(rb));
        self.parent[haut] = bas;
        let m = std::mem::take(&mut self.moteurs[haut]);
        self.moteurs[bas].extend(m);
    }

    /// Union par clé molle, refusée si les deux groupes partagent un moteur :
    /// sur une plateforme donnée, deux identifiants de consultation sont deux
    /// consultations.
    fn unir_si_moteurs_disjoints(&mut self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.racine(a), self.racine(b));
        if ra == rb {
            return true;
        }
        if !self.moteurs[ra].is_disjoint(&self.moteurs[rb]) {
            return false;
        }
        self.unir(ra, rb);
        true
    }
}

/// Accounts a platform keeps to demonstrate a consultation, folded.
const ACHETEURS_DE_TEST: [&str; 13] = [
    "organisme de test",
    "ote - organisme de test",
    "organisme de formation",
    "organisme de parametrage de reference",
    "commune de test",
    "mairie de test",
    "acheteur public demo",
    "test apc",
    "ztest",
    "supervision achatpublic",
    "sba_dematis",
    "klekoon compte profil d'acheteur",
    "atline services",
];

static MENTION_DE_TEST: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"cette consultation sert a mesurer des temps de reponse|\bconsultation de test\b|reponse electronique de test|test (pour la )?reponse electronique|appel d'offres de test\b|(^|[^a-z0-9])test_|_test([^a-z0-9]|$)",
    )
    .expect("regex mention de test valide")
});

/// A platform's own demonstration consultation, which its buyer account or its
/// text names as such.
pub fn consultation_de_test(acheteur: &str, textes: &[&str]) -> bool {
    let acheteur = lj_fold::fold(acheteur);
    ACHETEURS_DE_TEST.iter().any(|a| acheteur.starts_with(a))
        || textes
            .iter()
            .any(|t| MENTION_DE_TEST.is_match(&lj_fold::fold(t)))
}

/// Fond les canaux du batch en lignes canoniques, contre le stock :
/// `existants` porte les affectations connues (`source_uid → annonce_uid`, lues
/// dans `annonce_source` pour les canaux du batch et leurs clés de jointure).
/// L'uid est frappé à la première rencontre d'un groupe —
/// `marche:<xxh3("src|<canal fondateur>")>` — et ne se recalcule jamais : un
/// groupe qui recouvre un uid existant l'adopte, un groupe qui en recouvre
/// plusieurs rend les fusions correspondantes, un uid revendiqué par deux
/// groupes reste au groupe qui en détient le plus de canaux, et si le perdant
/// re-frappe l'uid disputé les deux groupes se replient en une ligne.
/// Déterministe : les canaux sont triés (référencés d'abord, puis confiance du
/// moteur) avant regroupement. `aujourdhui` remplace une date de publication
/// inconnue. Les canaux sans objet et sans la moindre URL sont écartés.
pub fn canonicaliser(
    mut rows: Vec<AnnonceBrute>,
    existants: &HashMap<String, String>,
    aujourdhui: Date,
) -> Canonicalisation {
    rows.sort_by(|x, y| {
        (
            x.reference.is_none(),
            priorite(x.moteur),
            x.source_uid.as_str(),
        )
            .cmp(&(
                y.reference.is_none(),
                priorite(y.moteur),
                y.source_uid.as_str(),
            ))
    });
    let rows: Vec<AnnonceBrute> = rows
        .into_iter()
        .filter(|r| {
            !r.objet.trim().is_empty()
                && [&r.url_avis, &r.url_dce, &r.url_rc]
                    .into_iter()
                    .any(|u| u.is_some())
                && !consultation_de_test(
                    r.acheteur.as_deref().unwrap_or_default(),
                    &[
                        &r.objet,
                        r.contenu.description.as_deref().unwrap_or_default(),
                    ],
                )
        })
        .collect();

    let mut groupes = Groupes::new(&rows);
    let mut par_cle: HashMap<String, usize> = HashMap::new();
    // Clés exactes d'abord, décisives et transitives.
    for (i, row) in rows.iter().enumerate() {
        for cle in std::iter::once(row.source_uid.as_str())
            .chain(row.cles_jointure.iter().map(String::as_str))
        {
            match par_cle.entry(format!("src|{cle}")) {
                Entry::Occupied(e) => groupes.unir(i, *e.get()),
                Entry::Vacant(v) => {
                    v.insert(i);
                }
            }
        }
    }
    // Puis les clés molles, gardées par moteur. Une union refusée réassigne
    // la clé au dernier arrivé.
    for (i, row) in rows.iter().enumerate() {
        for cle in [cle_primaire(row), cle_repli(row)].into_iter().flatten() {
            match par_cle.entry(cle) {
                Entry::Vacant(v) => {
                    v.insert(i);
                }
                Entry::Occupied(mut e) => {
                    if !groupes.unir_si_moteurs_disjoints(i, *e.get()) {
                        e.insert(i);
                    }
                }
            }
        }
    }

    // Membres par racine. BTreeMap : l'ordre des groupes suit le tri des
    // canaux, donc la sortie est déterministe.
    let mut membres: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..rows.len() {
        let r = groupes.racine(i);
        membres.entry(r).or_default().push(i);
    }

    // Revendications : les uids déjà en base des canaux de chaque groupe — par
    // le source_uid propre et par les clés de jointure, qui ramènent aussi un
    // canal absent du batch.
    let mut revendications: BTreeMap<usize, HashMap<String, HashSet<&str>>> = BTreeMap::new();
    for (&racine, idx) in &membres {
        let claims = revendications.entry(racine).or_default();
        for &i in idx {
            for cle in std::iter::once(rows[i].source_uid.as_str())
                .chain(rows[i].cles_jointure.iter().map(String::as_str))
            {
                if let Some(uid) = existants.get(cle) {
                    claims.entry(uid.clone()).or_default().insert(cle);
                }
            }
        }
    }
    // Un uid revendiqué par plusieurs groupes (scission) reste au groupe qui
    // en détient le plus de canaux ; à égalité, au premier du tri.
    let mut gagnant: HashMap<String, (usize, usize)> = HashMap::new();
    for (&racine, claims) in &revendications {
        for (uid, cles) in claims {
            match gagnant.entry(uid.clone()) {
                Entry::Vacant(v) => {
                    v.insert((cles.len(), racine));
                }
                Entry::Occupied(mut o) => {
                    if cles.len() > o.get().0 {
                        o.insert((cles.len(), racine));
                    }
                }
            }
        }
    }

    // Élection de l'uid de chaque groupe : l'existant le plus porté (égalité
    // → le plus petit), sinon frappe depuis le canal fondateur. Les autres
    // existants recouverts fusionnent vers l'élu.
    let mut elections: Vec<(String, Vec<usize>)> = Vec::new();
    let mut fusions: Vec<Fusion> = Vec::new();
    for (racine, idx) in membres {
        let claims = revendications.remove(&racine).unwrap_or_default();
        let mut possedes: Vec<(String, usize)> = claims
            .into_iter()
            .filter(|(uid, _)| gagnant[uid].1 == racine)
            .map(|(uid, cles)| (uid, cles.len()))
            .collect();
        possedes.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
        let uid = match possedes.first() {
            Some((u, _)) => u.clone(),
            None => format!(
                "marche:{:016x}",
                xxhash_rust::xxh3::xxh3_64(format!("src|{}", rows[racine].source_uid).as_bytes())
            ),
        };
        fusions.extend(possedes.into_iter().skip(1).map(|(ancien, _)| Fusion {
            ancien,
            nouveau: uid.clone(),
        }));
        elections.push((uid, idx));
    }

    // Deux élections peuvent désigner le même uid : le perdant d'une scission
    // frappe depuis son fondateur, et quand ce fondateur avait frappé l'uid
    // disputé, il le re-frappe à l'identique. L'identité est alors la même :
    // on replie sur une seule ligne.
    let mut par_uid: HashMap<String, usize> = HashMap::new();
    let mut repliees: Vec<(String, Vec<usize>)> = Vec::new();
    for (uid, idx) in elections {
        if let Some(&p) = par_uid.get(&uid) {
            let fonte = &mut repliees[p].1;
            fonte.extend(idx);
            fonte.sort_unstable();
        } else {
            par_uid.insert(uid.clone(), repliees.len());
            repliees.push((uid, idx));
        }
    }

    let mut rows: Vec<Option<AnnonceBrute>> = rows.into_iter().map(Some).collect();
    let annonces = repliees
        .into_iter()
        .map(|(uid, idx)| {
            let sources: Vec<AnnonceBrute> = idx
                .into_iter()
                .map(|i| {
                    rows[i]
                        .take()
                        .expect("chaque canal appartient à un seul groupe")
                })
                .collect();
            fusionner(uid, sources, aujourdhui)
        })
        .collect();
    Canonicalisation { annonces, fusions }
}

fn fusionner(uid: String, sources: Vec<AnnonceBrute>, aujourdhui: Date) -> AnnonceCanonique {
    let premier = |f: fn(&AnnonceBrute) -> Option<String>| sources.iter().find_map(f);
    let acheteur_libelle = premier(|s| s.acheteur.clone())
        .map(|a| nettoyer_acheteur(&a))
        .unwrap_or_default();
    let cpv = premier(|s| s.cpv.clone());
    // « Où agir » : le lien profond d'un profil d'acheteur prime sur la page
    // BOAMP, qui ne fait que republier l'avis.
    let url_action = sources
        .iter()
        .filter(|s| s.moteur != "boamp")
        .chain(sources.iter().filter(|s| s.moteur == "boamp"))
        .find_map(|s| {
            s.url_avis
                .clone()
                .or_else(|| s.url_dce.clone())
                .or_else(|| s.url_rc.clone())
        })
        .expect("groupe non vide de canaux portant au moins une URL");
    let url_depot = premier(|s| s.url_depot.clone());
    let objets: Vec<&str> = sources
        .iter()
        .map(|s| s.objet.trim())
        .filter(|o| !o.is_empty())
        .collect();
    let titre = embellir_titre(
        objets
            .iter()
            .find(|o| nomme_un_objet(o))
            .or(objets.first())
            .expect("groupe filtré sur objet non vide"),
    );
    let date_publication = sources
        .iter()
        .filter_map(|s| s.date_publication)
        .min()
        .unwrap_or(aujourdhui);
    let declare = cpv.as_deref().and_then(cpv_division_uid);
    let cpv_uid = declare.or_else(|| crate::cpv::division_probable(&titre));
    let famille_uid = sources
        .iter()
        .find_map(|s| s.nature)
        .or_else(|| cpv_uid.as_deref().and_then(crate::cpv::nature_probable))
        .map(NatureMarche::facet_uid);
    AnnonceCanonique {
        uid,
        famille_uid,
        titre,
        acheteur_libelle,
        // Union, never the first channel that answers: two registries name two
        // different départements of the same market far more often than they
        // contradict each other.
        zones: {
            let mut v: Vec<Zone> = sources.iter().flat_map(|s| s.zones.clone()).collect();
            v.sort();
            v.dedup();
            v
        },
        cpv_uid,
        cpv,
        date_publication,
        echeance: sources
            .iter()
            .filter_map(|s| s.echeance)
            .filter(|e| echeance_plausible(*e, date_publication))
            .max(),
        url_action,
        url_depot,
        reference: premier(|s| s.reference.clone()),
        procedure: premier(|s| s.procedure.clone()),
        contenu: ContenuAnnonce::fondre(sources.iter().map(|s| &s.contenu)),
        descripteurs: {
            let mut v: Vec<String> = sources
                .iter()
                .flat_map(|s| s.descripteurs.clone())
                .collect();
            v.sort();
            v.dedup();
            v
        },
        sirens: {
            let mut v: Vec<String> = sources.iter().filter_map(|s| s.siren.clone()).collect();
            v.sort();
            v.dedup();
            v
        },
        sources,
    }
}

// ── Résultats de marché ─────────────────────────────────────────────────────

/// Un lot attribué, tel que déclaré par un avis de résultat de marché.
#[derive(Debug, Clone, PartialEq)]
pub struct LotAttribue {
    pub description: Option<String>,
    pub titulaire: String,
    /// SIREN, porté par les seuls avis eForms.
    pub siren: Option<String>,
    pub ville: Option<String>,
    pub montant: Option<f64>,
    pub devise: Option<String>,
    pub nb_offres: Option<i64>,
    pub date_attribution: Option<Date>,
}

/// Un avis de résultat de marché BOAMP, prêt à écrire : une ligne du fond
/// `attribution` par idweb.
#[derive(Debug)]
pub struct ResultatMarche {
    pub idweb: String,
    /// L'avis de marché d'origine, clé du rattachement à la ligne `marche`
    /// quand elle est en stock : le premier `annonce_lie` du jeu ODS, sinon
    /// l'`avisInitial` que le blob déclare.
    pub parent_idweb: Option<String>,
    pub acheteur: Option<String>,
    pub siren_acheteur: Option<String>,
    pub objet: String,
    pub nature: Option<NatureMarche>,
    pub cpv: Option<String>,
    pub zones: Vec<Zone>,
    pub date_parution: Date,
    pub url_avis: String,
    pub contenu: ContenuAnnonce,
    pub descripteurs: Vec<String>,
    pub attribution: Attribution,
}

/// Ce qu'un avis de résultat dit de l'attribution.
#[derive(Debug, Default)]
pub struct Attribution {
    pub lots: Vec<LotAttribue>,
    /// Le paragraphe qui nomme l'attributaire, quand la source ne publie que
    /// ça — les avis simplifiés (MAPA sous seuil, `FNSimple` moderne) n'ont pas
    /// d'autre forme. Servi tel quel.
    pub texte: Option<String>,
    /// `idWeb` de l'avis initial déclaré dans le blob (format `MAPA`), quand
    /// le jeu ODS ne donne pas d'`annonce_lie`.
    pub avis_initial: Option<String>,
}

/// L'uid du marché qu'un avis de résultat désigne : l'idweb de la
/// **consultation** quand l'avis la déclare, sinon le sien. Deux résultats d'un
/// même marché tombent ainsi sur la même ligne, et la consultation ingérée plus
/// tard s'y raccroche par son `source_uid`.
pub fn marche_uid_de_resultat(idweb: &str, parent_idweb: Option<&str>) -> String {
    let cle = format!("boamp|{}", parent_idweb.unwrap_or(idweb));
    format!("marche:{:016x}", xxhash_rust::xxh3::xxh3_64(cle.as_bytes()))
}

/// Mots courants (≤ 5 lettres) des objets de consultation, tels que les
/// sources en capitales les écrivent — sans accents. En casse de phrase ils
/// redescendent en minuscules ; tout autre jeton court reste en capitales,
/// réputé sigle.
const MOTS_COURANTS: &[&str] = &[
    "a", "achat", "afin", "aide", "aides", "ainsi", "aire", "aires", "an", "annee", "ans", "appel",
    "apres", "arbre", "au", "audit", "aux", "avant", "avec", "avis", "bail", "beton", "bois",
    "bon", "bons", "cadre", "ce", "ces", "cet", "chez", "cours", "d", "dans", "de", "des", "deux",
    "dont", "du", "eau", "eaux", "ecole", "en", "entre", "et", "etude", "gare", "gares", "gaz",
    "halle", "hors", "il", "ile", "iles", "jeu", "jeux", "l", "la", "le", "les", "leur", "local",
    "lot", "lots", "lycee", "mais", "meme", "mise", "mur", "murs", "n", "ne", "noel", "non",
    "objet", "offre", "ou", "par", "parc", "parcs", "pas", "place", "plan", "pont", "ponts",
    "port", "ports", "pose", "pour", "prix", "quai", "quais", "regie", "bas", "basse", "est",
    "grand", "haut", "haute", "hauts", "neuf", "neufs", "neuve", "nord", "ouest", "petit", "sud",
    "vert", "verte", "verts", "vieux", "route", "rue", "rues", "sa", "salle", "sans", "se",
    "selon", "ses", "site", "sites", "sol", "sols", "son", "sous", "stade", "suite", "sur", "taxe",
    "taxes", "toit", "tous", "tout", "trois", "type", "un", "une", "vers", "via", "ville", "voie",
    "voies", "voirie", "y",
];

const ELISIONS: [&str; 13] = [
    "l", "d", "n", "s", "c", "j", "m", "t", "qu", "jusqu", "lorsqu", "puisqu", "quelqu",
];
const APRES_ELISION: &str = "aeiouyàâäéèêëîïôöùûüh";

/// A source that lost its encoding publishes `d?offres`. Only the elision, most of
/// the occurrences, comes back: `march?s` lost a letter too, and which one is a guess.
pub fn reparer_elisions(texte: &str) -> String {
    if !texte.contains('?') {
        return texte.to_string();
    }
    let lettres: Vec<char> = texte.chars().collect();
    let mut sortie = String::with_capacity(texte.len());
    for (i, c) in lettres.iter().enumerate() {
        if *c != '?' {
            sortie.push(*c);
            continue;
        }
        let mut debut = i;
        while debut > 0 && lettres[debut - 1].is_alphabetic() {
            debut -= 1;
        }
        let mot = lettres[debut..i].iter().collect::<String>().to_lowercase();
        let suit = lettres
            .get(i + 1)
            .map(|c| c.to_lowercase().next().unwrap_or(' '))
            .unwrap_or(' ');
        let elide = ELISIONS.contains(&mot.as_str()) && APRES_ELISION.contains(suit);
        sortie.push(if elide { '\'' } else { '?' });
    }
    sortie
}

/// Past twenty years, the span of the fund's longest qualification systems, a
/// deadline's year is a typo (`2924`, `7017`).
pub const ECHEANCE_ANS_MAX: i16 = 20;

fn echeance_plausible(echeance: Timestamp, publication: Date) -> bool {
    echeance.to_zoned(TimeZone::UTC).year() - publication.year() <= ECHEANCE_ANS_MAX
}

/// A buyer's reference (`2026-05`, `APS2026`, `-.`) files a market and names nothing.
fn nomme_un_objet(objet: &str) -> bool {
    objet.split_whitespace().any(|mot| {
        let mot = mot.trim_matches(|c: char| !c.is_alphanumeric());
        mot.chars().filter(|c| c.is_alphabetic()).count() >= 4
            && !mot.chars().any(|c| c.is_ascii_digit())
    })
}

/// Les blancs d'un objet se replient en espaces simples, puis un objet publié
/// tout en capitales passe en casse de phrase : les segments alphabétiques
/// courts (≤ 5 lettres) hors mots courants gardent leurs capitales — sigles
/// (CCAS, SIVU), codes (RN1, CEA/DIF) et noms propres courts — et le premier
/// caractère alphabétique prend la majuscule. Les capitales des sources sont
/// dépourvues d'accents : on ne les restitue pas. Un titre en casse mixte est
/// déjà en forme, sa casse ne bouge pas.
pub fn embellir_titre(titre: &str) -> String {
    let titre = reparer_elisions(titre);
    let titre = titre.split_whitespace().collect::<Vec<_>>().join(" ");
    let lettres = titre.chars().filter(|c| c.is_alphabetic()).count();
    let capitales = titre.chars().filter(|c| c.is_uppercase()).count();
    if lettres < 15 || (capitales as f64) < lettres as f64 * 0.8 {
        return titre;
    }
    let mut phrase = String::with_capacity(titre.len());
    let mut segment = String::new();
    for c in titre.chars().chain(std::iter::once(' ')) {
        if c.is_alphabetic() {
            segment.push(c);
            continue;
        }
        if !segment.is_empty() {
            let minuscules = segment.to_lowercase();
            if MOTS_COURANTS.contains(&minuscules.as_str()) || segment.chars().count() > 5 {
                phrase.push_str(&minuscules);
            } else {
                phrase.push_str(&segment);
            }
            segment.clear();
        }
        phrase.push(c);
    }
    phrase.pop();
    let mut vu_premiere = false;
    phrase
        .chars()
        .map(|c| {
            if !vu_premiere && c.is_alphabetic() {
                vu_premiere = true;
                c.to_uppercase()
                    .next()
                    .expect("majuscule d'un alphabétique")
            } else {
                c
            }
        })
        .collect()
}

/// Le libellé d'un acheteur s'arrête où commencent ses coordonnées : les
/// sources (le `nomacheteur` BOAMP surtout) empilent sous le nom l'adresse
/// postale, le téléphone, le courriel ou le SIRET, ligne à ligne. On garde les
/// lignes de nom — la première toujours — et on les replie en une seule, blancs
/// simples. La casse ne bouge pas.
pub fn nettoyer_acheteur(libelle: &str) -> String {
    let nom = libelle
        .lines()
        .enumerate()
        .take_while(|(i, l)| *i == 0 || !ligne_de_coordonnees(l))
        .flat_map(|(_, l)| l.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ");
    sans_departement(&nom).to_string()
}

/// Le BOAMP suffixe le nom de l'acheteur du département qui l'héberge —
/// « Trivalis (85) », « Ville de Koungou (976) » —, là où sa plateforme de dépôt
/// publie « Trivalis ». C'est une annotation de l'annuaire, pas une part de la
/// dénomination, et le département vit déjà dans son propre champ.
///
/// Seul un code de département part : deux ou trois chiffres, ou la Corse
/// (`2A`/`2B`). Une parenthèse finale de lettres est un sigle du nom
/// (« Métropole Européenne de Lille (MEL) ») et reste.
fn sans_departement(nom: &str) -> &str {
    let coupe = nom.trim_end();
    let Some(ouvrante) = coupe.strip_suffix(')').and_then(|s| s.rfind('(')) else {
        return nom;
    };
    let code = &coupe[ouvrante + 1..coupe.len() - 1];
    let departement = matches!(code.len(), 2 | 3)
        && (code.bytes().all(|b| b.is_ascii_digit()) || matches!(code, "2A" | "2B"));
    if departement {
        coupe[..ouvrante].trim_end()
    } else {
        nom
    }
}

/// Une ligne de coordonnées : numéro de voie ou code postal (chiffre en tête),
/// courriel (`@`), boîte postale (`BP`/`B.P.`/`CS` + chiffres), téléphone/fax
/// ou SIRET. Les préfixes exigent un chiffre derrière : « Télécom Paris »,
/// « CSAPA » restent des noms.
fn ligne_de_coordonnees(ligne: &str) -> bool {
    let l = ligne.trim();
    if l.starts_with(|c: char| c.is_ascii_digit()) || l.contains('@') {
        return true;
    }
    let bas = l.to_lowercase();
    ["bp", "b.p", "cs", "tel", "tél", "fax", "siret"]
        .iter()
        .any(|p| {
            bas.strip_prefix(p).is_some_and(|reste| {
                reste
                    .trim_start_matches([' ', '.', ':'])
                    .starts_with(|c: char| c.is_ascii_digit())
            })
        })
}

/// L'attribution déclarée par un avis de résultat. Le BOAMP en diffuse quatre
/// formes : eForms `ContractAwardNotice` (titulaires avec SIREN/SIRET), `MAPA`
/// (titulaire et montant structurés, plus l'idWeb de l'avis initial),
/// `ATTRIBUTION.DECISION` par lot (avis anciens), et le `FNSimple` moderne qui
/// ne publie qu'un paragraphe. Un avis clos sans attributaire (`clos-nw`) rend
/// une attribution vide.
pub fn parse_attribution(donnees: &serde_json::Value) -> Attribution {
    let lots = eforms_lots(donnees);
    if !lots.is_empty() {
        return Attribution {
            lots,
            ..Attribution::default()
        };
    }
    let mapa = chemin(donnees, &["MAPA", "attribution"]);
    Attribution {
        lots: mapa.map(mapa_lots).unwrap_or_else(|| fns_lots(donnees)),
        texte: chemin(donnees, &["FNSimple", "attribution", "attributionMarche"])
            .and_then(feuille_str),
        avis_initial: mapa
            .and_then(|m| chemin(m, &["avisInitial", "idWeb"]))
            .and_then(feuille_str),
    }
}

/// Format `MAPA` : `attribution.resultat.attribue` porte le titulaire et le
/// montant. Une décision sans `attribue` ne rend rien.
fn mapa_lots(attribution: &serde_json::Value) -> Vec<LotAttribue> {
    let description = chemin(attribution, &["descriptionReduite", "objet"]).and_then(feuille_str);
    liste(chemin(attribution, &["attribution", "resultat"]))
        .into_iter()
        .flat_map(|r| liste(r.get("attribue")))
        .filter_map(|a| {
            let titulaire = a.get("titulaire")?;
            let nom = ["PersonneMorale", "PersonnePhysique"]
                .iter()
                .find_map(|k| titulaire.get(k).and_then(feuille_str))?;
            let montant = a.get("montant");
            Some(LotAttribue {
                description: description.clone(),
                titulaire: nom,
                siren: None,
                ville: chemin(titulaire, &["adr", "ville"]).and_then(feuille_str),
                montant: montant.and_then(|m| m.get("valeur")).and_then(feuille_f64),
                devise: montant.and_then(devise_de),
                nb_offres: None,
                date_attribution: None,
            })
        })
        .collect()
}

/// Valeur d'un XML→JSON : scalaire nu, ou enveloppé (`{"#text": …, "@…": …}`).
fn feuille(v: &serde_json::Value) -> Option<&serde_json::Value> {
    match v {
        serde_json::Value::Object(o) => o.get("#text"),
        _ => Some(v),
    }
}

fn feuille_str(v: &serde_json::Value) -> Option<String> {
    let f = feuille(v)?;
    if let Some(s) = f.as_str() {
        let s = s.trim();
        return (!s.is_empty()).then(|| s.to_string());
    }
    f.as_i64().map(|n| n.to_string())
}

fn feuille_f64(v: &serde_json::Value) -> Option<f64> {
    let f = feuille(v)?;
    f.as_f64().or_else(|| {
        f.as_str()?
            .replace([' ', '\u{a0}'], "")
            .replace(',', ".")
            .parse()
            .ok()
    })
}

fn feuille_i64(v: &serde_json::Value) -> Option<i64> {
    let f = feuille(v)?;
    f.as_i64().or_else(|| f.as_str()?.trim().parse().ok())
}

/// Un nœud répété arrive en tableau, l'unique en objet — les deux se lisent
/// pareil.
fn liste(v: Option<&serde_json::Value>) -> Vec<&serde_json::Value> {
    match v {
        Some(serde_json::Value::Array(a)) => a.iter().collect(),
        Some(o) => vec![o],
        None => Vec::new(),
    }
}

fn chemin<'a>(v: &'a serde_json::Value, cles: &[&str]) -> Option<&'a serde_json::Value> {
    cles.iter().try_fold(v, |v, k| v.get(k))
}

fn devise_de(v: &serde_json::Value) -> Option<String> {
    ["@currencyID", "@DEVISE", "@devise"]
        .iter()
        .find_map(|k| v.get(k))?
        .as_str()
        .map(str::to_string)
}

/// `cbc:CompanyID` eForms → SIREN : un SIRET (14 chiffres) se tronque, un
/// SIREN (9) passe tel quel, tout le reste (TVA étrangère…) est écarté.
fn siren_de_company_id(id: &str) -> Option<String> {
    let d = id.replace(' ', "");
    if !d.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match d.len() {
        9 => Some(d),
        14 => Some(d[..9].to_string()),
        _ => None,
    }
}

/// L'offre retenue d'un lot eForms, telle que `efac:LotTender` la déclare.
struct Offre {
    montant: Option<f64>,
    devise: Option<String>,
    /// `TPA-…` : le soumissionnaire, qui mène à l'organisation titulaire.
    partie: Option<String>,
}

fn eforms_lots(donnees: &serde_json::Value) -> Vec<LotAttribue> {
    let Some(can) = chemin(donnees, &["EFORMS", "ContractAwardNotice"]) else {
        return Vec::new();
    };
    let Some(ext) = chemin(
        can,
        &[
            "ext:UBLExtensions",
            "ext:UBLExtension",
            "ext:ExtensionContent",
            "efext:EformsExtension",
        ],
    ) else {
        return Vec::new();
    };
    // Répertoire des organisations : `ORG-…` → (dénomination, SIREN, ville).
    let orgs: HashMap<String, (String, Option<String>, Option<String>)> =
        liste(chemin(ext, &["efac:Organizations", "efac:Organization"]))
            .into_iter()
            .filter_map(|org| {
                let company = org.get("efac:Company")?;
                let id = chemin(company, &["cac:PartyIdentification", "cbc:ID"])
                    .and_then(feuille_str)?;
                let nom = chemin(company, &["cac:PartyName", "cbc:Name"]).and_then(feuille_str)?;
                let siren = chemin(company, &["cac:PartyLegalEntity", "cbc:CompanyID"])
                    .and_then(feuille_str)
                    .as_deref()
                    .and_then(siren_de_company_id);
                let ville =
                    chemin(company, &["cac:PostalAddress", "cbc:CityName"]).and_then(feuille_str);
                Some((id, (nom, siren, ville)))
            })
            .collect();
    // Intitulés de lots : `LOT-…` → nom du lot.
    let intitules: HashMap<String, String> = liste(can.get("cac:ProcurementProjectLot"))
        .into_iter()
        .filter_map(|lot| {
            Some((
                lot.get("cbc:ID").and_then(feuille_str)?,
                chemin(lot, &["cac:ProcurementProject", "cbc:Name"]).and_then(feuille_str)?,
            ))
        })
        .collect();
    let Some(res) = ext.get("efac:NoticeResult") else {
        return Vec::new();
    };
    // Soumissionnaires : `TPA-…` → `ORG-…`.
    let parties: HashMap<String, String> = liste(res.get("efac:TenderingParty"))
        .into_iter()
        .filter_map(|p| {
            Some((
                p.get("cbc:ID").and_then(feuille_str)?,
                chemin(p, &["efac:Tenderer", "cbc:ID"]).and_then(feuille_str)?,
            ))
        })
        .collect();
    // Offres : `TEN-…` → l'offre retenue.
    let offres: HashMap<String, Offre> = liste(res.get("efac:LotTender"))
        .into_iter()
        .filter_map(|t| {
            let id = t.get("cbc:ID").and_then(feuille_str)?;
            let paye = chemin(t, &["cac:LegalMonetaryTotal", "cbc:PayableAmount"]);
            Some((
                id,
                Offre {
                    montant: paye.and_then(feuille_f64),
                    devise: paye.and_then(devise_de),
                    partie: chemin(t, &["efac:TenderingParty", "cbc:ID"]).and_then(feuille_str),
                },
            ))
        })
        .collect();
    let mut lots = Vec::new();
    for lr in liste(res.get("efac:LotResult")) {
        // Seul `selec-w` (gagnant retenu) porte une attribution.
        if lr
            .get("cbc:TenderResultCode")
            .and_then(feuille_str)
            .as_deref()
            != Some("selec-w")
        {
            continue;
        }
        let nb_offres = liste(lr.get("efac:ReceivedSubmissionsStatistics"))
            .into_iter()
            .find_map(|s| {
                (s.get("efbc:StatisticsCode")
                    .and_then(feuille_str)
                    .as_deref()
                    == Some("tenders"))
                .then(|| s.get("efbc:StatisticsNumeric").and_then(feuille_i64))
                .flatten()
            });
        let description = chemin(lr, &["efac:TenderLot", "cbc:ID"])
            .and_then(feuille_str)
            .and_then(|id| intitules.get(&id).cloned());
        for ten in liste(lr.get("efac:LotTender")) {
            let Some(offre) = ten
                .get("cbc:ID")
                .and_then(feuille_str)
                .and_then(|id| offres.get(&id))
            else {
                continue;
            };
            let Some((nom, siren, ville)) = offre
                .partie
                .as_ref()
                .and_then(|t| parties.get(t))
                .and_then(|o| orgs.get(o))
            else {
                continue;
            };
            lots.push(LotAttribue {
                description: description.clone(),
                titulaire: nom.clone(),
                siren: siren.clone(),
                ville: ville.clone(),
                montant: offre.montant,
                devise: offre.devise.clone(),
                nb_offres,
                date_attribution: None,
            });
        }
    }
    lots
}

fn fns_lots(donnees: &serde_json::Value) -> Vec<LotAttribue> {
    let attribution = donnees
        .get("ATTRIBUTION")
        .or_else(|| chemin(donnees, &["AVIS", "ATTRIBUTION"]));
    let mut lots = Vec::new();
    for d in liste(attribution.and_then(|a| a.get("DECISION"))) {
        let rens = d.get("RENSEIGNEMENT");
        let montant = rens.and_then(|r| r.get("MONTANT"));
        for t in liste(d.get("TITULAIRE")) {
            let Some(nom) = t.get("DENOMINATION").and_then(feuille_str) else {
                continue;
            };
            lots.push(LotAttribue {
                description: d.get("DESCRIPTION").and_then(feuille_str),
                titulaire: nom,
                siren: None,
                ville: t.get("VILLE").and_then(feuille_str),
                montant: montant.and_then(feuille_f64),
                devise: montant.and_then(devise_de),
                nb_offres: rens
                    .and_then(|r| r.get("NB_OFFRE_RECU"))
                    .and_then(feuille_i64),
                date_attribution: rens
                    .and_then(|r| r.get("DATE_ATTRIBUTION"))
                    .and_then(feuille_str)
                    .as_deref()
                    .and_then(parse_date_fr),
            });
        }
    }
    lots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_demonstration_consultation_is_told_from_a_market_about_tests() {
        for (acheteur, titre) in [
            ("Organisme de test pour les entreprises", "AOO18122025"),
            ("ORGANISME DE FORMATION CDG47", "Consultation technique"),
            ("zTest_Commune_de_Worldline_2", "PLTF-144216-SAD-Chap"),
            ("ACHETEUR PUBLIC DÉMO", "Marché alloti"),
            (
                "La Commune de Soultz-les-Bains",
                "Cette consultation sert à mesurer des temps de réponse de la plateforme",
            ),
            (
                "Ville de Lyon",
                "Familiarisez-vous avec la procédure DUME grâce à cette consultation de test !",
            ),
            ("COMMUNE DE X", "Test réponse électronique 3 lots"),
            (
                "La Commune de Soultz-les-Bains",
                "Consultation_Test_Sans_Signature",
            ),
            ("COMMUNE DE GOURDON", "TEST_DUME"),
        ] {
            assert!(
                consultation_de_test(acheteur, &[titre]),
                "{acheteur} : {titre}"
            );
        }
        for (acheteur, titre) in [
            (
                "IFP Energies nouvelles",
                "Fourniture de Tests TOEIC et prestations associées",
            ),
            (
                "CA Amiens Metropole",
                "Prestations de tests psychotechniques",
            ),
            (
                "Troyes Champagne Metropole",
                "Travaux de construction d'un espace-TEST agricole en maraichage",
            ),
            ("TEST SA", "Réalisation de l'enquête globale transport"),
            ("Commune d'Ax", "Consultation de tests d'étanchéité à l'air"),
        ] {
            assert!(
                !consultation_de_test(acheteur, &[titre]),
                "{acheteur} : {titre}"
            );
        }
    }

    /// Values computed by Postgres: the SQL near-title threshold holds only if both agree.
    #[test]
    fn la_similarite_des_titres_est_celle_de_postgres() {
        for (a, b, attendu) in [
            (
                "Accord cadre de diagnostics amiante",
                "Accord-cadre de diagnostics amiante",
                1.0,
            ),
            (
                "Maîtrise d'oeuvre pour la rénovation",
                "Maîtrise d'œuvre pour la rénovation",
                0.825,
            ),
            (
                "PA 2026-15 : Maintenance préventive des ascenseurs",
                "Maintenance préventive des ascenseurs",
                0.791_667,
            ),
            (
                "Entretien des espaces verts",
                "Fourniture de repas en liaison froide",
                0.087_719,
            ),
        ] {
            let vu = similarite_titres(a, b);
            assert!(
                (vu - attendu).abs() < 1e-6,
                "similarite_titres({a:?}, {b:?}) = {vu}, attendu {attendu}"
            );
        }
        assert_eq!(similarite_titres("", "Travaux de voirie"), 0.0);
    }

    fn brute(moteur: &'static str, source_uid: &str) -> AnnonceBrute {
        AnnonceBrute {
            contenu: ContenuAnnonce::default(),
            descripteurs: Vec::new(),
            moteur,
            source_uid: source_uid.to_string(),
            cles_jointure: Vec::new(),
            reference: None,
            acheteur: None,
            siren: None,
            objet: String::new(),
            nature: None,
            cpv: None,
            zones: Vec::new(),
            date_publication: None,
            echeance: None,
            url_avis: None,
            url_rc: None,
            url_dce: None,
            url_depot: None,
            procedure: None,
        }
    }

    #[test]
    fn embellir_titre_met_les_capitales_en_casse_de_phrase() {
        assert_eq!(
            embellir_titre(
                "MARCHE PUBLIC DE TRAVAUX POUR REALISATION EN BETON DE LA RUE SINTI DOI"
            ),
            "Marche public de travaux pour realisation en beton de la rue SINTI DOI"
        );
        assert_eq!(
            embellir_titre("CREATION D'UN BASSIN DE NATATION"),
            "Creation d'un bassin de natation"
        );
        assert_eq!(
            embellir_titre("RN1 - REPARATION DU PONT DE LA GABARRE POUR LE CEA/DIF"),
            "RN1 - reparation du pont de la gabarre pour le CEA/DIF"
        );
        assert_eq!(
            embellir_titre("Fourniture, livraison et mise en place de contenants à déchets"),
            "Fourniture, livraison et mise en place de contenants à déchets"
        );
        assert_eq!(embellir_titre("CCAS 2026"), "CCAS 2026");
        assert_eq!(
            embellir_titre("ENTRETIEN DES ESPACES VERTS DE LA VILLE"),
            "Entretien des espaces verts de la ville"
        );
        assert_eq!(
            embellir_titre("Marché alloti pour la fourniture d'électricité\npour le groupement"),
            "Marché alloti pour la fourniture d'électricité pour le groupement"
        );
    }

    /// A lost encoding gives back `?` for two different characters, and the
    /// letter is unrecoverable where the apostrophe is not.
    #[test]
    fn seule_l_apostrophe_elidee_se_recupere() {
        assert_eq!(
            reparer_elisions("Le présent appel d?offres a pour objet l?assurance"),
            "Le présent appel d'offres a pour objet l'assurance"
        );
        assert_eq!(
            reparer_elisions("AMENAGEMENT DE L?ALLEE"),
            "AMENAGEMENT DE L'ALLEE"
        );
        assert_eq!(reparer_elisions("qu?il en soit"), "qu'il en soit");
        assert_eq!(reparer_elisions("march?s publics"), "march?s publics");
        assert_eq!(
            reparer_elisions("Travaux d?assainissement ? Etanchéité"),
            "Travaux d'assainissement ? Etanchéité"
        );
        assert_eq!(reparer_elisions("Quoi ?"), "Quoi ?");
        assert_eq!(reparer_elisions("n?j"), "n?j");
        assert_eq!(
            embellir_titre("MARCHE PUBLIC D?ASSURANCE DES BATIMENTS COMMUNAUX"),
            "Marche public d'assurance des batiments communaux"
        );
    }

    #[test]
    fn le_libelle_acheteur_s_arrete_aux_coordonnees() {
        assert_eq!(
            nettoyer_acheteur(
                "Caisse d'allocations familiales de Loire Atlantique\n22 rue de Malville\n44937 Nantes cedex 9"
            ),
            "Caisse d'allocations familiales de Loire Atlantique"
        );
        assert_eq!(
            nettoyer_acheteur("Etablissement du Service d'Infrastructure de la \nDéfense de Metz"),
            "Etablissement du Service d'Infrastructure de la Défense de Metz"
        );
        assert_eq!(
            nettoyer_acheteur(
                "Département de Maine-et-Loire\nHôtel du Département\nDIFAJE-SCPA\ncs94104\n49941 Angers cedex 9"
            ),
            "Département de Maine-et-Loire Hôtel du Département DIFAJE-SCPA"
        );
        assert_eq!(
            nettoyer_acheteur(
                "Service technique : Mme Durand\nTél : 01 02 03 04 05\nCourriel : a@ville.fr"
            ),
            "Service technique : Mme Durand"
        );
        assert_eq!(
            nettoyer_acheteur("Télécom Paris\nCSAPA du Sud-Ouest"),
            "Télécom Paris CSAPA du Sud-Ouest"
        );
        assert_eq!(
            nettoyer_acheteur("UNIVERSITE  DE LILLE"),
            "UNIVERSITE DE LILLE"
        );
        assert_eq!(
            nettoyer_acheteur("3F Grand Est\n67000 Strasbourg"),
            "3F Grand Est"
        );
    }

    #[test]
    fn le_departement_suffixe_par_le_boamp_ne_fait_pas_partie_du_nom() {
        assert_eq!(nettoyer_acheteur("Trivalis (85)"), "Trivalis");
        assert_eq!(
            nettoyer_acheteur("Angers Loire Metropole (49)"),
            "Angers Loire Metropole"
        );
        assert_eq!(
            nettoyer_acheteur("Ville de Koungou (976)"),
            "Ville de Koungou"
        );
        assert_eq!(nettoyer_acheteur("Ville d'Ajaccio (2A)"), "Ville d'Ajaccio");
        assert_eq!(
            nettoyer_acheteur("Métropole Européenne de Lille (MEL)"),
            "Métropole Européenne de Lille (MEL)"
        );
        assert_eq!(
            nettoyer_acheteur("Voies navigables de France (VNF)"),
            "Voies navigables de France (VNF)"
        );
        assert_eq!(
            nettoyer_acheteur("Ville de Paris (75) direction des achats"),
            "Ville de Paris (75) direction des achats"
        );
    }

    #[test]
    fn cle_de_repli_tronque_en_caracteres() {
        let mut b = brute("atline", "atline:1");
        b.acheteur = Some("Ville de Nîmes".into());
        b.objet = format!("{} – lot 2 de travaux", "é".repeat(40));
        b.url_avis = Some("https://a/1".into());
        let out = canonicaliser(vec![b], &HashMap::new(), Date::new(2026, 8, 4).unwrap());
        assert_eq!(out.annonces.len(), 1);
    }

    #[test]
    fn heures_limites() {
        let h = |s| parse_heure(s).map(|t| t.to_string());
        assert_eq!(h("17h00"), Some("17:00:00".into()));
        assert_eq!(h("17:30"), Some("17:30:00".into()));
        assert_eq!(h("12 : 00"), Some("12:00:00".into()));
        assert_eq!(h("17h"), Some("17:00:00".into()));
        assert_eq!(h("99h00"), None);
    }

    #[test]
    fn echeance_paris_ete_hiver() {
        let ete = echeance_ts(Date::new(2026, 7, 15).unwrap(), parse_heure("12h00"));
        assert_eq!(ete.to_string(), "2026-07-15T10:00:00Z");
        let hiver = echeance_ts(Date::new(2026, 12, 15).unwrap(), parse_heure("12h00"));
        assert_eq!(hiver.to_string(), "2026-12-15T11:00:00Z");
        let sans_heure = echeance_ts(Date::new(2026, 7, 15).unwrap(), None);
        assert_eq!(sans_heure.to_string(), "2026-07-15T21:59:59Z");
    }

    #[test]
    fn departements() {
        assert_eq!(departement_from_cp("33000").as_deref(), Some("33"));
        assert_eq!(departement_from_cp("97300").as_deref(), Some("973"));
        assert_eq!(departement_from_cp("20000").as_deref(), Some("2A"));
        assert_eq!(departement_from_cp("20600").as_deref(), Some("2B"));
        assert_eq!(departement_from_cp("3300"), None);
        assert_eq!(normalise_departement("1").as_deref(), Some("01"));
        assert_eq!(normalise_departement("2b").as_deref(), Some("2B"));
        assert_eq!(normalise_departement("(33)").as_deref(), Some("33"));
        assert_eq!(normalise_departement("971").as_deref(), Some("971"));
        assert_eq!(normalise_departement("X"), None);
    }

    #[test]
    fn seul_un_code_du_referentiel_sort_de_la_normalisation() {
        assert_eq!(normalise_departement("201").as_deref(), Some("2A"));
        assert_eq!(normalise_departement("202").as_deref(), Some("2B"));
        assert_eq!(normalise_departement("98").as_deref(), Some("98"));
        assert_eq!(normalise_departement("99").as_deref(), Some("99"));
        for hors in ["20", "970", "000", "42424"] {
            assert_eq!(normalise_departement(hors), None, "{hors}");
        }
        assert_eq!(departement_from_cp("98000").as_deref(), Some("98"));
        assert_eq!(departement_from_cp("98600").as_deref(), Some("986"));
    }

    /// L'outre-mer arrive codé ISO 3166 sur la plateforme de l'État, et la
    /// Nouvelle-Calédonie y suffixe sa province.
    #[test]
    fn un_code_iso_d_outre_mer_rend_son_departement() {
        assert_eq!(normalise_departement("PF").as_deref(), Some("987"));
        assert_eq!(normalise_departement("NC").as_deref(), Some("988"));
        assert_eq!(normalise_departement("NC.SU").as_deref(), Some("988"));
        assert_eq!(normalise_departement("nc.no").as_deref(), Some("988"));
        assert_eq!(normalise_departement("GF").as_deref(), Some("973"));
        assert_eq!(normalise_departement("WF").as_deref(), Some("986"));
        assert_eq!(normalise_departement("RE").as_deref(), Some("974"));
        assert_eq!(
            normalise_departement("FR"),
            None,
            "la France entière n'est pas un département"
        );
    }

    #[test]
    fn cpv_au_grain_division() {
        assert_eq!(cpv_division_uid("45233140-2").as_deref(), Some("cpv:45"));
        assert_eq!(cpv_division_uid("45233140").as_deref(), Some("cpv:45"));
        assert_eq!(cpv_division_uid("99999999"), None);
        assert_eq!(cpv_division_uid("4"), None);
    }

    #[test]
    fn natures() {
        assert_eq!(NatureMarche::parse("Travaux"), Some(NatureMarche::Travaux));
        assert_eq!(
            NatureMarche::parse("Marché de services"),
            Some(NatureMarche::Services)
        );
        assert_eq!(
            NatureMarche::parse("FOURNITURES"),
            Some(NatureMarche::Fournitures)
        );
        assert_eq!(NatureMarche::parse("concession"), None);
    }

    #[test]
    fn reconciliation_par_reference() {
        let mut a = brute("aws", "aws:1");
        a.acheteur = Some("Commune de Cayenne".into());
        a.reference = Some("2026-VOIRIE-01".into());
        a.objet = "Réfection de voirie".into();
        a.url_avis = Some("https://aws/1".into());
        let mut b = brute("boamp", "boamp:24-1");
        b.acheteur = Some("COMMUNE DE CAYENNE".into());
        b.reference = Some("2026-voirie-01".into());
        b.objet = "Réfection de la voirie communale".into();
        b.url_avis = Some("https://boamp/24-1".into());
        b.siren = Some("219730123".into());

        let out = canonicaliser(vec![a, b], &HashMap::new(), Date::new(2026, 8, 4).unwrap());
        assert_eq!(out.annonces.len(), 1);
        assert!(out.fusions.is_empty());
        let c = &out.annonces[0];
        assert_eq!(c.sources.len(), 2);
        assert_eq!(c.titre, "Réfection de la voirie communale");
        assert_eq!(c.url_action, "https://aws/1");
        assert_eq!(c.sirens, vec!["219730123".to_string()]);
        assert!(c.uid.starts_with("marche:"));
    }

    #[test]
    fn reconciliation_repli_sans_reference() {
        let ech = Some(echeance_ts(Date::new(2026, 9, 1).unwrap(), None));
        let mut a = brute("atline", "atline:x");
        a.acheteur = Some("Département du Rhône".into());
        a.objet = "Fourniture de repas scolaires".into();
        a.echeance = ech;
        a.url_avis = Some("https://atline/x".into());
        let mut b = brute("xmarches", "xmarches:y");
        b.acheteur = Some("DEPARTEMENT DU RHONE".into());
        b.objet = "Fourniture de repas scolaires".into();
        b.echeance = ech;
        b.url_avis = Some("https://xmarches/y".into());
        let mut c = brute("atline", "atline:z");
        c.acheteur = Some("Département du Rhône".into());
        c.objet = "Entretien des espaces verts".into();
        c.echeance = ech;
        c.url_avis = Some("https://atline/z".into());

        let out = canonicaliser(
            vec![a, b, c],
            &HashMap::new(),
            Date::new(2026, 8, 4).unwrap(),
        );
        assert_eq!(out.annonces.len(), 2);
        assert_eq!(out.annonces.iter().map(|g| g.sources.len()).max(), Some(2));
    }

    #[test]
    fn le_repli_ne_fond_pas_deux_consultations_d_un_meme_moteur() {
        let ech = Some(echeance_ts(Date::new(2026, 8, 25).unwrap(), None));
        let lot = |n: u32| {
            let mut b = brute("atexo", &format!("atexo:demat-ampa.fr:60906{n}"));
            b.acheteur = Some("Communauté de communes Maremne Adour Côte-Sud".into());
            b.objet = "Achat de denrées alimentaires pour le Pôle Culinaire".into();
            b.echeance = ech;
            b.url_avis = Some(format!("https://demat-ampa.fr/c/{n}"));
            b
        };
        let out = canonicaliser(
            vec![lot(1), lot(2), lot(3), lot(4), lot(5)],
            &HashMap::new(),
            Date::new(2026, 8, 4).unwrap(),
        );
        assert_eq!(out.annonces.len(), 5);
        let uids: HashSet<&str> = out.annonces.iter().map(|c| c.uid.as_str()).collect();
        assert_eq!(uids.len(), 5);
        let mut avis = brute("boamp", "boamp:26-65490");
        avis.acheteur = Some("CC Maremne Adour Côte-Sud".into());
        avis.objet = "Achat de denrées alimentaires pour le Pôle Culinaire".into();
        avis.echeance = ech;
        avis.url_avis = Some("https://boamp.fr/a/1".into());
        avis.cles_jointure = vec!["atexo:demat-ampa.fr:609063".into()];
        let out = canonicaliser(
            vec![lot(1), lot(2), lot(3), lot(4), lot(5), avis],
            &HashMap::new(),
            Date::new(2026, 8, 4).unwrap(),
        );
        assert_eq!(out.annonces.len(), 5);
        assert_eq!(out.annonces.iter().map(|g| g.sources.len()).max(), Some(2));
    }

    #[test]
    fn sans_acheteur_aucun_regroupement_par_repli() {
        let ech = Some(echeance_ts(Date::new(2122, 12, 31).unwrap(), None));
        let sonde = |hote: &str| {
            let mut b = brute("atexo", &format!("atexo:{hote}:1"));
            b.acheteur = None;
            b.objet = "Maintenance des installations de chauffage".into();
            b.echeance = ech;
            b.url_avis = Some(format!("https://{hote}/c/1"));
            b
        };
        let out = canonicaliser(
            vec![sonde("a.fr"), sonde("b.fr"), sonde("c.fr")],
            &HashMap::new(),
            Date::new(2026, 8, 4).unwrap(),
        );
        assert_eq!(out.annonces.len(), 3);
        let mut vide = sonde("d.fr");
        vide.acheteur = Some("  -  ".into());
        let mut vide2 = sonde("e.fr");
        vide2.acheteur = Some("-".into());
        assert_eq!(
            canonicaliser(
                vec![vide, vide2],
                &HashMap::new(),
                Date::new(2026, 8, 4).unwrap()
            )
            .annonces
            .len(),
            2
        );
    }

    #[test]
    fn uid_stable_quel_que_soit_l_ordre_des_canaux() {
        let ech = Some(echeance_ts(Date::new(2026, 9, 1).unwrap(), None));
        let mut sans_ref = brute("dematis", "dematis:1");
        sans_ref.acheteur = Some("Ville de Vichy".into());
        sans_ref.objet = "Rénovation du gymnase".into();
        sans_ref.echeance = ech;
        sans_ref.url_avis = Some("https://d/1".into());
        let mut avec_ref = brute("aws", "aws:2");
        avec_ref.acheteur = Some("Ville de Vichy".into());
        avec_ref.reference = Some("GYM-26".into());
        avec_ref.objet = "Rénovation du gymnase".into();
        avec_ref.echeance = ech;
        avec_ref.url_avis = Some("https://a/2".into());

        let un = canonicaliser(
            vec![sans_ref.clone(), avec_ref.clone()],
            &HashMap::new(),
            Date::new(2026, 8, 4).unwrap(),
        );
        let deux = canonicaliser(
            vec![avec_ref, sans_ref],
            &HashMap::new(),
            Date::new(2026, 8, 4).unwrap(),
        );
        assert_eq!(un.annonces.len(), 1);
        assert_eq!(un.annonces[0].uid, deux.annonces[0].uid);
    }

    #[test]
    fn le_perdant_d_une_scission_ne_refrappe_pas_l_uid_perdu() {
        let uid_fondateur = format!(
            "marche:{:016x}",
            xxhash_rust::xxh3::xxh3_64("src|boamp:26-1".as_bytes())
        );
        let mut fondateur = brute("boamp", "boamp:26-1");
        fondateur.acheteur = Some("Ville d'Angers".into());
        fondateur.objet = "Entretien des chaudières des bâtiments communaux".into();
        fondateur.url_avis = Some("https://boamp.fr/a/26-1".into());
        let mut rectif = brute("boamp", "boamp:26-2");
        rectif.acheteur = Some("Ville d'Angers".into());
        rectif.objet = "Entretien des chaudières — lot 2 relancé".into();
        rectif.url_avis = Some("https://boamp.fr/a/26-2".into());
        rectif.cles_jointure = vec!["aws:111".into(), "aws:222".into()];
        let existants: HashMap<String, String> = [
            ("boamp:26-1".to_string(), uid_fondateur.clone()),
            ("aws:111".to_string(), uid_fondateur.clone()),
            ("aws:222".to_string(), uid_fondateur.clone()),
        ]
        .into();
        let out = canonicaliser(
            vec![fondateur, rectif],
            &existants,
            Date::new(2026, 8, 8).unwrap(),
        );
        assert_eq!(out.annonces.len(), 1);
        assert_eq!(out.annonces[0].uid, uid_fondateur);
        assert_eq!(out.annonces[0].sources.len(), 2);
        assert!(out.fusions.is_empty());
    }

    #[test]
    fn jointure_directe_par_url() {
        let cles = cles_jointure_depuis_urls(
            [
                "https://www.marches-publics.gouv.fr/entreprise/consultation/2940332?orgAcronyme=x7c",
                "https://www.marches-publics.info/mpiaws/index.cfm?fuseaction=dce.dRC&IDM=1857736",
                "https://www.dives-sur-mer.fr",
            ]
            .into_iter(),
        );
        assert_eq!(
            cles,
            vec![
                "atexo:marches-publics.gouv.fr:2940332".to_string(),
                "aws:1857736".to_string(),
            ]
        );
        assert_eq!(
            cles_jointure_depuis_urls(
                [
                    "https://www.e-marchespublics.com/appel-offre/guyane/cayenne/travaux/1176995/2",
                    "https://www.e-marchespublics.com/appel-offre/1176995",
                ]
                .into_iter()
            ),
            vec!["dematis:1176995".to_string()]
        );
        assert_eq!(
            cles_jointure_depuis_urls(
                ["https://mapa.aji-france.com/mapa/marche/168153/show"].into_iter()
            ),
            vec!["aji:168153".to_string()]
        );
        assert_eq!(
            cles_jointure_depuis_urls(
                [
                    "https://www.klekoon.com/detail-consultation/93290/1",
                    "https://www.klekoon.com/appels-offres/avis/travaux-marquises?consultation_ID=93337&page=1&dce=1#detail_marche",
                    "https://www.klekoon.com/dematernet/formulaire_3_Detail_Consultation.asp?consultation_ID=52969",
                ]
                .into_iter()
            ),
            vec![
                "klekoon:52969".to_string(),
                "klekoon:93290".to_string(),
                "klekoon:93337".to_string(),
            ]
        );
        assert_eq!(
            cles_jointure_depuis_urls(
                [
                    "https://www.proxilegales.fr/publisher_portail/public/annonce/consulterDCEHorsIdentification.jsp?iIdAffaire=36083",
                    "https://demat.marches.medialex.fr/medialex/publisher_portail/public/displayRC.jsp?iIdAffaire=21290",
                ]
                .into_iter()
            ),
            vec![
                "modula:demat.marches.medialex.fr:21290".to_string(),
                "modula:proxilegales.fr:36083".to_string(),
            ]
        );
        assert_eq!(
            cles_jointure_depuis_urls(
                [
                    "https://demat.centraledesmarches.com/7085737",
                    "https://demat.centraledesmarches.com",
                    "https://lacentraledesmarches.com/marches/574929",
                    "https://lacentraledesmarches.com/marches-publics/Vitre-Mairie-Accord-cadre/610799",
                    "https://centraledesmarches.com/publisher_plateforme/public/annonce/consulterDCEHorsIdentification.jsp?iIdAffaire=812345",
                ]
                .into_iter()
            ),
            vec![
                "centraledesmarches:574929".to_string(),
                "centraledesmarches:610799".to_string(),
                // The shortcut lives in a space of its own, and joining it to a
                // consultation would be a guess: it only resolves by HTTP.
                "centraledesmarches:demat:7085737".to_string(),
                "modula:centraledesmarches.com:812345".to_string(),
            ]
        );

        let mut boamp = brute("boamp", "boamp:26-1");
        boamp.acheteur = Some("Commune de Dives-sur-Mer".into());
        boamp.objet = "Restauration de l'église".into();
        boamp.url_avis = Some("https://www.boamp.fr/avis/26-1".into());
        boamp.cles_jointure = vec!["atexo:marches-publics.gouv.fr:2940332".into()];
        let mut atexo = brute("atexo", "atexo:marches-publics.gouv.fr:2940332");
        atexo.objet = "Restauration de l'église Saint-Machin — lot 2".into();
        atexo.url_avis =
            Some("https://marches-publics.gouv.fr/entreprise/consultation/2940332".into());

        let out = canonicaliser(
            vec![boamp, atexo],
            &HashMap::new(),
            Date::new(2026, 8, 4).unwrap(),
        );
        assert_eq!(out.annonces.len(), 1);
        assert_eq!(out.annonces[0].sources.len(), 2);
    }

    #[test]
    fn a_deadline_whose_year_is_a_typo_closes_nothing() {
        let avis = |moteur: &'static str, uid: &str, annee: i16| {
            let mut b = brute(moteur, uid);
            b.acheteur = Some("Commune de Gray".into());
            b.objet = "Travaux de mise en sécurité de la crête".into();
            b.url_avis = Some(format!("https://{moteur}.fr/avis/1"));
            b.date_publication = Some(Date::new(2024, 6, 17).unwrap());
            b.echeance = Some(echeance_ts(Date::new(annee, 7, 19).unwrap(), None));
            b
        };
        let echeance = |sources| {
            canonicaliser(sources, &HashMap::new(), Date::new(2026, 9, 23).unwrap()).annonces[0]
                .echeance
        };
        assert_eq!(echeance(vec![avis("boamp", "boamp:24-1", 2924)]), None);
        let mut boamp = avis("boamp", "boamp:24-1", 2924);
        boamp.cles_jointure = vec!["atexo:h.fr:1".into()];
        assert_eq!(
            echeance(vec![boamp, avis("atexo", "atexo:h.fr:1", 2024)]),
            Some(echeance_ts(Date::new(2024, 7, 19).unwrap(), None))
        );
        assert!(echeance(vec![avis("achatpublic", "achatpublic:1", 2044)]).is_some());
    }

    #[test]
    fn a_reference_leaves_the_title_to_the_channel_that_names_the_object() {
        let mut boamp = brute("boamp", "boamp:26-87503");
        boamp.acheteur = Some("Mairie de Pont-Saint-Esprit".into());
        boamp.objet = "2026-05".into();
        boamp.url_avis = Some("https://www.boamp.fr/avis/26-87503".into());
        boamp.cles_jointure = vec!["xmarches:41370".into()];
        let mut xmarches = brute("xmarches", "xmarches:41370");
        xmarches.objet = "Fournitures de bureau et fournitures scolaires".into();
        xmarches.url_avis = Some("https://www.xmarches.fr/entreprise/c/41370".into());
        let titre = |sources| {
            canonicaliser(sources, &HashMap::new(), Date::new(2026, 9, 23).unwrap()).annonces[0]
                .titre
                .clone()
        };
        assert_eq!(
            titre(vec![boamp.clone(), xmarches]),
            "Fournitures de bureau et fournitures scolaires"
        );
        assert_eq!(titre(vec![boamp]), "2026-05");
        for reference in ["APS2026", "aaj-cb-2026-8", "-.", "SIEP2026-2", "MCO REM"] {
            assert!(!nomme_un_objet(reference), "{reference}");
        }
        for objet in ["CCAS 2026", "Frugt og grønt", "Réfection de l'église"] {
            assert!(nomme_un_objet(objet), "{objet}");
        }
    }

    #[test]
    fn jointure_par_url_formes_de_depot() {
        let cles = |u: &str| cles_jointure_depuis_urls([u].into_iter());
        assert_eq!(
            cles("https://www.marches-publics.info/mpiaws/index.cfm?fuseaction=dematent.login&type=Dce&Idm=1843425"),
            ["aws:1843425"]
        );
        assert_eq!(
            cles("https://dl.aws-achat.info//avis/index.cfm?fuseaction=dematent.login&type=Dce&idm=76829"),
            ["aws:76829"]
        );
        assert_eq!(
            cles("https://www.marches-publics.gouv.fr/?page=Entreprise.EntrepriseAdvancedSearch&AllCons&id=3049944&orgAcronyme=d4t"),
            ["atexo:marches-publics.gouv.fr:3049944"]
        );
        assert_eq!(
            cles("https://marches.megalis.bretagne.bzh/?page=Entreprise.EntrepriseAdvancedSearch&AllCons&id=234743&orgAcronyme=a4j"),
            ["atexo:marches.megalis.bretagne.bzh:234743"]
        );
        assert_eq!(
            cles("https://mpe.mairie-marseille.fr/index.php?page=Entreprise.EntrepriseDetailsConsultation&id=526127&orgAcronyme=a2z"),
            ["atexo:mpe.mairie-marseille.fr:526127"]
        );
        assert_eq!(
            cles("https://demat-ampa.fr/index.php?page=Entreprise.EntrepriseDetailsConsultation&id=609283&orgAcronyme=MAIRIE_33_102"),
            ["atexo:demat-ampa.fr:609283"]
        );
        assert!(cles("https://www.marches-publics.gouv.fr/?page=Entreprise.EntrepriseAdvancedSearch&searchAnnCons&keyWord=APHPSU&categorie=0&localisations=").is_empty());
        assert_eq!(
            cles("https://www.xmarches.fr/entreprise/detailConsultation.php?key=41170"),
            ["xmarches:41170"]
        );
        assert!(
            cles("https://profil.exemple.fr/entreprise/detailConsultation.php?key=41170")
                .is_empty()
        );
        assert!(cles("https://demat.exemple.fr/consultation?key=41170").is_empty());
    }

    #[test]
    fn canaux_inaffichables_ecartes() {
        let mut sans_objet = brute("synapse", "synapse:1");
        sans_objet.url_avis = Some("https://s/1".into());
        let mut sans_url = brute("synapse", "synapse:2");
        sans_url.objet = "Un objet".into();
        assert!(canonicaliser(
            vec![sans_objet, sans_url],
            &HashMap::new(),
            Date::new(2026, 8, 4).unwrap()
        )
        .annonces
        .is_empty());
    }

    #[test]
    fn une_reference_egale_a_l_acheteur_ne_fonde_rien() {
        let mut a = brute("aws", "aws:1");
        a.acheteur = Some("Ville de Colmar".into());
        a.reference = Some("VILLE DE COLMAR".into());
        a.objet = "Montage des illuminations de Noël".into();
        a.url_avis = Some("https://a/1".into());
        let mut b = brute("dematis", "dematis:2");
        b.acheteur = Some("Ville de Colmar".into());
        b.reference = Some("Ville de Colmar".into());
        b.objet = "Parking du parc des expositions".into();
        b.url_avis = Some("https://d/2".into());
        let out = canonicaliser(vec![a, b], &HashMap::new(), Date::new(2026, 8, 7).unwrap());
        assert_eq!(out.annonces.len(), 2);
    }

    #[test]
    fn la_cle_primaire_ne_fond_pas_deux_lots_d_un_meme_moteur() {
        let lot = |n: u32, objet: &str| {
            let mut b = brute("aws", &format!("aws:185003{n}"));
            b.acheteur = Some("Commune d'Exemple".into());
            b.reference = Some("2026 35".into());
            b.objet = objet.into();
            b.url_avis = Some(format!("https://aws/{n}"));
            b
        };
        let out = canonicaliser(
            vec![lot(1, "Lot 1 : gros œuvre"), lot(2, "Lot 2 : charpente")],
            &HashMap::new(),
            Date::new(2026, 8, 7).unwrap(),
        );
        assert_eq!(out.annonces.len(), 2);
        let mut c = brute("dematis", "dematis:9");
        c.acheteur = Some("Commune d'Exemple".into());
        c.reference = Some("2026 35".into());
        c.objet = "Lot 1 : gros œuvre".into();
        c.url_avis = Some("https://d/9".into());
        let out = canonicaliser(
            vec![lot(1, "Lot 1 : gros œuvre"), c],
            &HashMap::new(),
            Date::new(2026, 8, 7).unwrap(),
        );
        assert_eq!(out.annonces.len(), 1);
    }

    #[test]
    fn l_uid_existant_est_adopte() {
        let mut a = brute("aws", "aws:2");
        a.acheteur = Some("Ville de Vichy".into());
        a.reference = Some("GYM-26".into());
        a.objet = "Rénovation du gymnase".into();
        a.url_avis = Some("https://a/2".into());
        let existants =
            HashMap::from([("aws:2".to_string(), "marche:00000000000000aa".to_string())]);
        let out = canonicaliser(vec![a], &existants, Date::new(2026, 8, 7).unwrap());
        assert_eq!(out.annonces[0].uid, "marche:00000000000000aa");
        assert!(out.fusions.is_empty());
    }

    #[test]
    fn deux_uids_recouverts_fusionnent() {
        let mut avis = brute("boamp", "boamp:26-1");
        avis.acheteur = Some("Commune de Grand-Santi".into());
        avis.objet = "Construction de l'école".into();
        avis.url_avis = Some("https://boamp/26-1".into());
        avis.cles_jointure = vec!["dematis:1176995".into()];
        let mut profil = brute("dematis", "dematis:1176995");
        profil.acheteur = Some("Commune de Grand-Santi".into());
        profil.objet = "École — gros œuvre".into();
        profil.url_avis = Some("https://d/1176995".into());
        let existants = HashMap::from([
            (
                "boamp:26-1".to_string(),
                "marche:00000000000000aa".to_string(),
            ),
            (
                "dematis:1176995".to_string(),
                "marche:00000000000000bb".to_string(),
            ),
        ]);
        let out = canonicaliser(
            vec![avis, profil],
            &existants,
            Date::new(2026, 8, 7).unwrap(),
        );
        assert_eq!(out.annonces.len(), 1);
        assert_eq!(out.annonces[0].uid, "marche:00000000000000aa");
        assert_eq!(
            out.fusions,
            vec![Fusion {
                ancien: "marche:00000000000000bb".to_string(),
                nouveau: "marche:00000000000000aa".to_string(),
            }]
        );
    }

    #[test]
    fn la_jointure_adopte_un_canal_absent_du_batch() {
        let mut avis = brute("boamp", "boamp:26-2");
        avis.acheteur = Some("Commune de Cayenne".into());
        avis.objet = "Réfection de la piste".into();
        avis.url_avis = Some("https://boamp/26-2".into());
        avis.cles_jointure = vec!["atexo:h.fr:5".into()];
        let existants = HashMap::from([(
            "atexo:h.fr:5".to_string(),
            "marche:00000000000000cc".to_string(),
        )]);
        let out = canonicaliser(vec![avis], &existants, Date::new(2026, 8, 7).unwrap());
        assert_eq!(out.annonces[0].uid, "marche:00000000000000cc");
        assert!(out.fusions.is_empty());
    }

    #[test]
    fn une_scission_laisse_l_uid_au_plus_gros_fragment() {
        let ech = Some(echeance_ts(Date::new(2026, 9, 1).unwrap(), None));
        let lot = |m: &'static str, id: &str, objet: &str| {
            let mut b = brute(m, id);
            b.acheteur = Some("Commune d'Exemple".into());
            b.objet = objet.into();
            b.echeance = ech;
            b.url_avis = Some(format!("https://x/{id}"));
            b
        };
        let existants: HashMap<String, String> = [
            ("aws:1", "marche:00000000000000dd"),
            ("dematis:2", "marche:00000000000000dd"),
            ("atline:3", "marche:00000000000000dd"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let out = canonicaliser(
            vec![
                lot("aws", "aws:1", "Fourniture de repas"),
                lot("dematis", "dematis:2", "Fourniture de repas"),
                lot("atline", "atline:3", "Entretien des espaces verts"),
            ],
            &existants,
            Date::new(2026, 8, 7).unwrap(),
        );
        assert_eq!(out.annonces.len(), 2);
        assert!(out.fusions.is_empty());
        let duo = out.annonces.iter().find(|a| a.sources.len() == 2).unwrap();
        let solo = out.annonces.iter().find(|a| a.sources.len() == 1).unwrap();
        assert_eq!(duo.uid, "marche:00000000000000dd");
        assert_ne!(solo.uid, "marche:00000000000000dd");
    }

    #[test]
    fn attribution_eforms_resout_le_gagnant() {
        let donnees = serde_json::json!({"EFORMS": {"ContractAwardNotice": {
            "ext:UBLExtensions": {"ext:UBLExtension": {"ext:ExtensionContent": {"efext:EformsExtension": {
                "efac:Organizations": {"efac:Organization": [
                    {"efac:Company": {
                        "cac:PartyIdentification": {"cbc:ID": {"@schemeName": "organization", "#text": "ORG-0001"}},
                        "cac:PartyName": {"cbc:Name": {"@languageID": "FRA", "#text": "SAEM VENDEE - Vendée Globe"}},
                        "cac:PartyLegalEntity": {"cbc:CompanyID": "45151864100035"}}},
                    {"efac:Company": {
                        "cac:PartyIdentification": {"cbc:ID": {"@schemeName": "organization", "#text": "ORG-0004"}},
                        "cac:PartyName": {"cbc:Name": {"@languageID": "FRA", "#text": "NEFSEA"}},
                        "cac:PartyLegalEntity": {"cbc:CompanyID": "438776080"},
                        "cac:PostalAddress": {"cbc:CityName": "Nantes"}}}]},
                "efac:NoticeResult": {
                    "cbc:TotalAmount": {"@currencyID": "EUR", "#text": "3500000"},
                    "efac:LotResult": {
                        "cbc:TenderResultCode": {"@listName": "winner-selection-status", "#text": "selec-w"},
                        "efac:LotTender": {"cbc:ID": "TEN-0001"},
                        "efac:ReceivedSubmissionsStatistics": {
                            "efbc:StatisticsCode": {"@listName": "received-submission-type", "#text": "tenders"},
                            "efbc:StatisticsNumeric": 1},
                        "efac:TenderLot": {"cbc:ID": "LOT-0001"}},
                    "efac:LotTender": {
                        "cbc:ID": {"@schemeName": "tender", "#text": "TEN-0001"},
                        "cac:LegalMonetaryTotal": {"cbc:PayableAmount": {"@currencyID": "EUR", "#text": "3500000"}},
                        "efac:TenderingParty": {"cbc:ID": "TPA-0001"}},
                    "efac:TenderingParty": {
                        "cbc:ID": {"@schemeName": "tendering-party", "#text": "TPA-0001"},
                        "efac:Tenderer": {"cbc:ID": "ORG-0004"}}}}}}},
            "cac:ProcurementProjectLot": {
                "cbc:ID": {"@schemeName": "Lot", "#text": "LOT-0001"},
                "cac:ProcurementProject": {"cbc:Name": "Production audiovisuelle"}}}}});
        let lots = parse_attribution(&donnees).lots;
        assert_eq!(lots.len(), 1);
        let lot = &lots[0];
        assert_eq!(lot.titulaire, "NEFSEA");
        assert_eq!(lot.siren.as_deref(), Some("438776080"));
        assert_eq!(lot.ville.as_deref(), Some("Nantes"));
        assert_eq!(lot.montant, Some(3_500_000.0));
        assert_eq!(lot.devise.as_deref(), Some("EUR"));
        assert_eq!(lot.nb_offres, Some(1));
        assert_eq!(lot.description.as_deref(), Some("Production audiovisuelle"));
    }

    #[test]
    fn attribution_fnsimple_par_lots() {
        let donnees = serde_json::json!({
        "IDENTITE": {"DENOMINATION": "BEAUNE COTE SUD"},
        "ATTRIBUTION": {"DECISION": [
            {"NUM_LOT": 1,
             "DESCRIPTION": "Travaux de création du terrain synthétique",
             "TITULAIRE": {"DENOMINATION": "Groupement Id Verde - Pennequin", "CP": 90600, "VILLE": "Grandvillars"},
             "RENSEIGNEMENT": {"DATE_ATTRIBUTION": "2018-12-14", "NB_OFFRE_RECU": 4,
                               "MONTANT": {"@DEVISE": "EUR", "#text": 555864}}},
            {"NUM_LOT": 2, "DESCRIPTION": "Lot déclaré infructueux"},
        ]}});
        let lots = parse_attribution(&donnees).lots;
        assert_eq!(lots.len(), 1);
        let lot = &lots[0];
        assert_eq!(lot.titulaire, "Groupement Id Verde - Pennequin");
        assert_eq!(lot.siren, None);
        assert_eq!(lot.montant, Some(555_864.0));
        assert_eq!(lot.nb_offres, Some(4));
        assert_eq!(
            lot.date_attribution.map(|d| d.to_string()).as_deref(),
            Some("2018-12-14")
        );
        assert!(parse_attribution(&serde_json::json!({})).lots.is_empty());
    }

    #[test]
    fn l_avis_simplifie_ne_donne_qu_un_texte() {
        let donnees = serde_json::json!({"FNSimple": {"attribution": {
            "natureMarche": {"intitule": "Banquet annuel des Séniors"},
            "attributionMarche": "L'attributaire de l'accord-cadre est la société LECOINTE TRAITEUR (5 rue Berthelot - 76150 MAROMME) pour un montant maximum de 60 000 € HT."}}});
        let a = parse_attribution(&donnees);
        assert!(a.lots.is_empty());
        assert!(a.texte.expect("prose").starts_with("L'attributaire"));
        assert_eq!(a.avis_initial, None);
    }

    #[test]
    fn siren_depuis_company_id() {
        assert_eq!(
            siren_de_company_id("45151864100035").as_deref(),
            Some("451518641")
        );
        assert_eq!(
            siren_de_company_id("438776080").as_deref(),
            Some("438776080")
        );
        assert_eq!(
            siren_de_company_id("21 140 225 000 016").as_deref(),
            Some("211402250")
        );
        assert_eq!(siren_de_company_id("123456"), None);
        assert_eq!(siren_de_company_id("FR12345678901"), None);
    }

    #[test]
    fn deux_resultats_d_un_meme_marche_tombent_sur_la_meme_ligne() {
        let a = marche_uid_de_resultat("26-78331", Some("26-8202"));
        let b = marche_uid_de_resultat("26-91004", Some("26-8202"));
        assert_eq!(a, b);
        assert!(a.starts_with("marche:"));
        assert_ne!(
            marche_uid_de_resultat("26-78331", None),
            marche_uid_de_resultat("26-91004", None)
        );
    }
}
