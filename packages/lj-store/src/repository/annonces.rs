//! Fond marchés du pot Annonces : upserts, cycle de vie, rattachement au graphe
//! des entités, recensement des profils d'acheteurs.
//!
//! Écriture en masse par `unnest` : chaque run ré-énumère le stock complet, donc
//! l'écrasante majorité des lignes existent déjà. Aucun état de cycle de vie
//! n'est stampé : on enregistre le flux, la fenêtre « ouverte » se calcule.

use tokio_postgres::types::{Json, ToSql};

use lj_vocab::ZONE_SANS;

use crate::error::Result;
use crate::repository::DecisionRepository;

/// Une ligne canonique prête à écrire, avec ses canaux et ses arêtes.
#[derive(Debug)]
pub struct AnnonceWriteItem {
    pub uid: String,
    /// Fond of the pot, one of `lj_vocab::JOURNAL_FOND_*`.
    pub fond: String,
    pub famille_uid: Option<String>,
    pub titre: String,
    pub description: Option<String>,
    pub acteur_libelle: String,
    pub zones: Vec<String>,
    pub cpv_uid: Option<String>,
    pub date_publication: chrono::NaiveDate,
    pub echeance: Option<chrono::DateTime<chrono::Utc>>,
    pub url_action: String,
    pub details: serde_json::Value,
    pub metiers: Vec<String>,
    /// Arêtes vers le graphe par SIREN donné à la source (eForms BOAMP), avec
    /// leur rôle : `acheteur` sur une consultation, `titulaire` sur une
    /// attribution.
    pub aretes_siren: Vec<(String, String)>,
    pub sources: Vec<AnnonceSourceWriteItem>,
}

/// A row a source described, with the trades it carries now.
#[derive(Debug)]
pub struct AnnonceDecrite {
    pub uid: String,
    pub titre: String,
    pub descripteurs: Vec<String>,
    pub codes_des_lots: Vec<String>,
    pub metiers: Vec<String>,
}

/// Un canal de publication d'une annonce.
#[derive(Debug)]
pub struct AnnonceSourceWriteItem {
    pub source_uid: String,
    pub url_avis: Option<String>,
}

/// Une ligne sans canal à régulariser : ses champs d'identification et les
/// URLs de ses liens, qui désignent ses cibles possibles.
#[derive(Debug)]
pub struct AnnonceOrpheline {
    pub uid: String,
    pub titre: String,
    pub acteur_libelle: String,
    pub echeance: Option<chrono::DateTime<chrono::Utc>>,
    pub urls: Vec<String>,
}

/// Les champs d'identification d'une ligne, pour départager des cibles.
#[derive(Debug)]
pub struct AnnonceChamps {
    pub uid: String,
    pub titre: String,
    pub acteur_libelle: String,
    pub echeance: Option<chrono::DateTime<chrono::Utc>>,
    pub zones: Vec<String>,
    /// L'entité acheteur de la ligne, quand elle en porte une seule — deux
    /// arêtes `acheteur` ne désignent personne.
    pub acheteur_uid: Option<String>,
    /// `details.reference` — la référence que l'acheteur a donnée au marché.
    pub reference: Option<String>,
}

/// The single market a buyer reference designates, with the buyer to check it
/// against: the reference alone names another buyer's market often enough.
#[derive(Debug)]
pub struct MarcheParReference {
    pub reference: String,
    pub uid: String,
    pub acteur_libelle: String,
}

/// Une ligne ouverte que seul le BOAMP publie, avec la plateforme de dépôt que
/// son avis déclare et les URLs de ses liens : la matière de `annonces-jumeler`.
#[derive(Debug)]
pub struct AnnonceAJumeler {
    pub uid: String,
    pub titre: String,
    pub acteur_libelle: String,
    pub echeance: Option<chrono::DateTime<chrono::Utc>>,
    pub zones: Vec<String>,
    /// `details.depot` — l'URL de la plateforme, sans identifiant de dossier.
    pub depot: String,
    pub urls: Vec<String>,
    /// Voir [`AnnonceChamps::acheteur_uid`].
    pub acheteur_uid: Option<String>,
    /// Voir [`AnnonceChamps::reference`].
    pub reference: Option<String>,
}

/// Un canal de plateforme d'une annonce ouverte, avec les champs
/// d'identification de sa ligne.
#[derive(Debug)]
pub struct AnnonceCanalOuvert {
    pub source_uid: String,
    pub annonce: AnnonceChamps,
}

/// A closed market still waiting for its award, and what identifies its buyer:
/// the SIREN when a single edge names one, the label otherwise.
#[derive(Debug)]
pub struct MarcheClosSansResultat {
    pub uid: String,
    pub titre: String,
    pub acteur_libelle: String,
    pub echeance: chrono::DateTime<chrono::Utc>,
    pub acheteur_siren: Option<String>,
    pub cpv_uid: Option<String>,
}

/// L'attribution à poser sur un marché déjà en stock. L'avis de résultat n'a
/// pas de ligne à lui : il devient un canal du marché qu'il clôt, et ses
/// titulaires deviennent des arêtes de ce marché.
#[derive(Debug)]
pub struct ResultatPose {
    pub annonce_uid: String,
    pub resultat: serde_json::Value,
    pub source: AnnonceSourceWriteItem,
    pub aretes_siren: Vec<(String, String)>,
}

/// Une ligne de la file d'enrichissement : où aller chercher la page de détail
/// de cette annonce, et par quel moteur la lire.
#[derive(Debug)]
pub struct AnnonceAEnrichir {
    pub uid: String,
    pub moteur: String,
    pub url_avis: String,
}

/// What the enrichment walks. `NonVisitees` is the nightly queue and empties
/// itself; the others are catch-ups, revisiting pages read before their
/// extractor knew how to read a field. A catch-up empties too, on the `vu`
/// stamp the current parser leaves: the page that names no buyer is read once
/// more, never every week.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEnrichissement {
    NonVisitees,
    SansAcheteur,
    SansZone,
    SansContenu,
}

#[derive(Debug)]
pub struct DetailPose {
    pub annonce_uid: String,
    pub cpv_uid: Option<String>,
    pub detail: serde_json::Value,
    /// Read off the notice page, and only ever fills an empty column.
    pub description: Option<String>,
    /// Acheteur lu sur la fiche. Ne comble qu'un libellé vide.
    pub acheteur: Option<String>,
    /// Read off the notice page, and only ever fills an empty column.
    pub zones: Vec<String>,
}

/// What a re-read of a channel already in stock brings back, keyed by that
/// channel and not by the market: the reconciliation is not replayed.
#[derive(Debug)]
pub struct ContenuPose {
    pub source_uid: String,
    pub description: Option<String>,
    /// The `details` keys to graft, built by the ingest.
    pub contenu: serde_json::Value,
}

/// Un trou de moisson : des lignes ouvertes qu'un canal de publication a vues
/// et qu'aucune plateforme moissonnée ne porte, avec le dépôt qu'elles
/// déclarent.
#[derive(Debug)]
pub struct TrouCouverture {
    /// L'URL de la plateforme déclarée par l'avis — vide quand il n'en nomme
    /// aucune.
    pub depot: String,
    /// Les canaux qui les portent, séparés par des virgules (`boamp,ted`).
    pub canaux: String,
    pub lignes: i64,
}

/// Récapitulatif d'un run d'écriture.
#[derive(Debug, Default)]
pub struct AnnonceWriteStats {
    pub annonces: u64,
    pub canaux: u64,
    pub aretes: u64,
}

/// Taille d'une table, index et TOAST compris.
#[derive(Debug)]
pub struct TableSize {
    pub table: String,
    pub bytes: i64,
}

/// One verdict of the link probe, for one engine and one kind of link.
#[derive(Debug)]
pub struct SondeLiens {
    pub moteur: String,
    pub kind: String,
    /// Count per verdict name.
    pub verdicts: serde_json::Value,
    /// One probed URL per verdict name, to reopen by hand.
    pub exemples: serde_json::Value,
}

// ── Lecture : la fenêtre servie par `/annonces` ─────────────────────────────

/// Ordre de lecture. `Nouveautes` trie par première apparition dans nos canaux
/// (`min(annonce_source.first_seen_at)`) : les moteurs scrapés n'ont pas de
/// curseur de publication fiable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnonceOrdre {
    Echeance,
    Publication,
    Nouveautes,
}

/// Filtre d'une lecture du fond. Les uids sont complets
/// (`nature_marche:TRAVAUX`, `cpv:45`) — la traduction depuis les suffixes de
/// l'API se fait à la frontière HTTP.
#[derive(Debug, Clone)]
pub struct AnnonceFiltre {
    pub fond: String,
    /// Full-text over title and buyer, through `annonce_bm25`. Every term of
    /// the query must be present, in one field or the other.
    pub q: Option<String>,
    pub zones: Vec<String>,
    pub familles: Vec<String>,
    pub cpv: Vec<String>,
    pub metiers: Vec<String>,
    /// Fenêtre d'échéance : ne garde que `echeance <= now() + N jours`.
    pub echeance_jours: Option<i32>,
    /// Inclut les lignes closes (défaut : la fenêtre vivante seule).
    pub closes: bool,
    pub ordre: AnnonceOrdre,
    pub limit: i64,
    pub offset: i64,
}

impl AnnonceFiltre {
    pub fn nouveau(fond: &str) -> Self {
        Self {
            fond: fond.to_string(),
            q: None,
            zones: Vec::new(),
            familles: Vec::new(),
            cpv: Vec::new(),
            metiers: Vec::new(),
            echeance_jours: None,
            closes: false,
            ordre: AnnonceOrdre::Echeance,
            limit: 20,
            offset: 0,
        }
    }
}

/// Une annonce lue, libellés de référentiel résolus.
#[derive(Debug)]
pub struct AnnonceRow {
    pub uid: String,
    pub fond: String,
    pub famille_uid: Option<String>,
    pub famille_label: Option<String>,
    pub titre: String,
    pub acteur_libelle: String,
    pub zones: Vec<String>,
    pub cpv_uid: Option<String>,
    pub cpv_label: Option<String>,
    pub metiers: Vec<String>,
    pub date_publication: chrono::NaiveDate,
    pub echeance: Option<chrono::DateTime<chrono::Utc>>,
    pub url_action: String,
    pub details: serde_json::Value,
    pub description: Option<String>,
    /// Moteurs qui publient l'annonce, lus dans le préfixe des `source_uid`.
    pub canaux: Vec<String>,
    /// Première apparition dans nos canaux — le curseur des alertes.
    pub premiere_vue: Option<chrono::DateTime<chrono::Utc>>,
}

/// Un canal de publication tel que servi par la fiche.
#[derive(Debug)]
pub struct AnnonceCanalRow {
    pub source_uid: String,
    pub url_avis: Option<String>,
    pub first_seen_at: chrono::DateTime<chrono::Utc>,
}

/// Une arête vers le graphe des entités, dénomination résolue quand l'entité
/// existe encore : les référentiels se rechargent par remplacement de namespace,
/// donc l'arête peut pointer dans le vide.
#[derive(Debug)]
pub struct AnnonceEntiteRow {
    pub entity_uid: String,
    pub role: String,
    pub denomination: Option<String>,
}

/// Une annonce d'une entité, avec le rôle de l'arête et le total du rôle.
#[derive(Debug)]
pub struct EntityAnnonceRow {
    pub role: String,
    pub total_role: i64,
    pub annonce: AnnonceRow,
}

/// Les clés de `details` dont la moisson n'est pas la source : posées par un
/// autre flux (page de détail, avis d'attribution) qu'elle ne reverra jamais.
/// Re-greffées sur le jsonb du batch, elles survivent à la ré-énumération
/// nocturne ; léguées par une ligne absorbée, elles survivent à la fusion. Tout
/// le reste de `details` appartient au moteur, qui l'écrase.
const DETAILS_PRESERVES: &str = "ARRAY['detail', 'resultat']";

/// The keys a batch that leaves them empty does not erase. The BOAMP stops
/// diffusing a notice at its own deadline, which another channel may extend.
const DETAILS_DURABLES: &str = "ARRAY['descripteurs', 'lots', 'criteres', 'montant_estime', \
     'cpv', 'reference', 'procedure', 'depot']";

/// A row's lots, those of the notice it was harvested from and those of its page.
fn lots_de(ligne: &str) -> String {
    format!(
        "jsonb_array_elements(COALESCE({ligne}.details -> 'lots', '[]') \
         || COALESCE({ligne}.details #> '{{detail,lots}}', '[]'))"
    )
}

/// What a row's trades are folded from: a descriptor, or the code of a works lot.
fn metiers_declares(ligne: &str) -> String {
    format!(
        "({ligne}.details ? 'descripteurs' \
         OR EXISTS (SELECT 1 FROM {} l WHERE l ->> 'cpv' LIKE '{}%'))",
        lots_de(ligne),
        lj_vocab::DIVISION_TRAVAUX
    )
}

/// A code a source published, which the listing carries under `cpv` and the
/// notice page under `detail.cpv`.
fn code_cpv_publie(ligne: &str) -> String {
    format!("COALESCE({ligne}.details->>'cpv', {ligne}.details->'detail'->>'cpv') IS NOT NULL")
}

/// A description repeating the title carries nothing. Refused at write, so the
/// column means "adds something to the title".
fn description_utile(valeur: &str, titre: &str) -> String {
    format!("CASE WHEN lower(btrim({valeur})) = lower(btrim({titre})) THEN NULL ELSE {valeur} END")
}

/// `details` of a row seen again: what the batch proposes, the durable keys it
/// leaves empty taken from the stock, the preserved keys grafted on top. Also
/// the right-hand term of the write-once guard.
fn details_greffe() -> String {
    format!(
        "EXCLUDED.details \
         || COALESCE((SELECT jsonb_object_agg(k, annonce.details -> k) \
            FROM unnest({DETAILS_DURABLES}) AS k \
            WHERE jsonb_typeof(annonce.details -> k) <> 'null' \
              AND COALESCE(jsonb_typeof(EXCLUDED.details -> k), 'null') = 'null'), '{{}}'::jsonb) \
         || COALESCE((SELECT jsonb_object_agg(k, annonce.details -> k) \
            FROM unnest({DETAILS_PRESERVES}) AS k WHERE annonce.details ? k), '{{}}'::jsonb)"
    )
}

/// Ce qu'une ligne absorbée lègue à l'élue : les clés préservées que `source`
/// porte et que `cible` n'a pas. Au grain de la clé, et la cible gagne quand les
/// deux la portent — l'élue est la ligne vivante, celle que le batch vient
/// d'écrire.
///
/// Fragment `FROM … WHERE …` : il sert de corps à l'agrégat du legs et à la
/// garde `EXISTS` qui borne l'UPDATE aux lignes qui gagnent quelque chose —
/// `jsonb_object_agg` d'un ensemble vide rend NULL, et le `||` effacerait le
/// `details` de l'élue.
fn details_leguees(source: &str, cible: &str) -> String {
    format!(
        "FROM unnest({DETAILS_PRESERVES}) AS k \
         WHERE {source}.details ? k AND NOT {cible}.details ? k"
    )
}

/// Le prédicat « cette annonce est ouverte », dit une seule fois pour tout le
/// fond. Entièrement calculé — on enregistre le flux, on ne stampe aucun état :
///
/// - `echeance > NOW()` — le délai de dépôt court encore. Une annonce sans
///   échéance n'entre jamais dans la fenêtre : on ne sait pas borner son dépôt,
///   on ne prétend pas qu'il est ouvert.
/// - au moins un canal la porte — la réconciliation re-parente ses
///   `annonce_source` quand elle revoit un groupage. L'uid abandonné garde sa
///   page et son archive, mais plus aucun moteur ne publie ce point d'entrée.
fn ouverte(alias: &str) -> String {
    format!(
        "{alias}.echeance > NOW() \
         AND EXISTS (SELECT 1 FROM annonce_source s WHERE s.annonce_uid = {alias}.uid)"
    )
}

/// The "closes within N days" window. It carries its own lower bound rather
/// than taking [`ouverte`]'s: `closes` drops that one, and the window would
/// count the whole closed history. `jours` is a placeholder or a caller column.
fn fenetre_echeance(alias: &str, jours: &str) -> String {
    format!(
        "{alias}.echeance > NOW() \
         AND {alias}.echeance <= NOW() + make_interval(days => {jours})"
    )
}

/// L'entité acheteur d'une ligne, quand elle en porte une seule. L'arête vient
/// du SIRET déclaré par la source ou de la dénomination pliée résolue sans
/// ambiguïté (`relier_annonces_par_denomination`) ; deux arêtes `acheteur` sur la
/// même ligne ne désignent personne, le `HAVING` les efface.
fn acheteur_unique(alias: &str) -> String {
    format!(
        "(SELECT max(ae.entity_uid) FROM annonce_entity ae \
          WHERE ae.annonce_uid = {alias}.uid AND ae.role = 'acheteur' \
          HAVING count(*) = 1)"
    )
}

/// Colonnes projetées par toute lecture d'annonce, libellés joints. `canaux`
/// vient d'un LATERAL sur `annonce_source` : un aller-retour par page, pas par
/// ligne.
const ANNONCE_COLS: &str = "
    a.uid, a.fond, a.famille_uid, ff.label AS famille_label, a.titre,
    a.acteur_libelle, a.zones, a.cpv_uid, fc.label AS cpv_label, a.metiers,
    a.date_publication, a.echeance, a.url_action, a.details, a.description,
    COALESCE(c.canaux, ARRAY[]::text[]) AS canaux, c.premiere_vue";

const ANNONCE_JOINS: &str = "
    LEFT JOIN facet_value ff ON ff.uid = a.famille_uid
    LEFT JOIN facet_value fc ON fc.uid = a.cpv_uid
    LEFT JOIN LATERAL (
      SELECT array_agg(DISTINCT split_part(s.source_uid, ':', 1)) AS canaux,
             min(s.first_seen_at) AS premiere_vue
      FROM annonce_source s WHERE s.annonce_uid = a.uid
    ) c ON true";

fn annonce_from_row(row: &tokio_postgres::Row) -> AnnonceRow {
    AnnonceRow {
        uid: row.get("uid"),
        fond: row.get("fond"),
        famille_uid: row.get("famille_uid"),
        famille_label: row.get("famille_label"),
        titre: row.get("titre"),
        acteur_libelle: row.get("acteur_libelle"),
        zones: row.get("zones"),
        cpv_uid: row.get("cpv_uid"),
        cpv_label: row.get("cpv_label"),
        metiers: row.get("metiers"),
        date_publication: row.get("date_publication"),
        echeance: row.get("echeance"),
        url_action: row.get("url_action"),
        details: row.get("details"),
        description: row.get("description"),
        canaux: row.get("canaux"),
        premiere_vue: row.get("premiere_vue"),
    }
}

/// Axe de facette du fond.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnonceAxe {
    Zone,
    Famille,
    Cpv,
    Metier,
}

impl AnnonceAxe {
    pub fn nom(self) -> &'static str {
        match self {
            AnnonceAxe::Zone => "zone",
            AnnonceAxe::Famille => "famille",
            AnnonceAxe::Cpv => "cpv",
            AnnonceAxe::Metier => "metier",
        }
    }

    fn colonne(self) -> &'static str {
        match self {
            AnnonceAxe::Zone => "a.zones",
            AnnonceAxe::Famille => "a.famille_uid",
            AnnonceAxe::Cpv => "a.cpv_uid",
            AnnonceAxe::Metier => "a.metiers",
        }
    }

    /// The axes served as one facet, which therefore leave each other out of its
    /// counts: nature is the root level of the CPV tree, so selecting a division
    /// must not empty the natures the other divisions hang from.
    fn groupe(self) -> &'static [AnnonceAxe] {
        match self {
            AnnonceAxe::Famille | AnnonceAxe::Cpv => &[AnnonceAxe::Famille, AnnonceAxe::Cpv],
            AnnonceAxe::Zone => &[AnnonceAxe::Zone],
            AnnonceAxe::Metier => &[AnnonceAxe::Metier],
        }
    }
}

/// Accumulateur de paramètres : `$1` est toujours le fond, les filtres suivent
/// dans l'ordre où ils sont posés.
struct Filtre {
    /// `Send` requis : ces paramètres traversent un `await` dans un handler
    /// axum, dont la future doit rester `Send`.
    params: Vec<Box<dyn ToSql + Sync + Send>>,
    clauses: String,
}

impl Filtre {
    /// Construit les clauses `AND …` du filtre. `sauf` retire un axe du
    /// prédicat — c'est ce qui rend la multi-sélection d'une facette possible :
    /// son propre compte se calcule sans elle.
    fn new(f: &AnnonceFiltre, sauf: Option<AnnonceAxe>) -> Self {
        let mut me = Filtre {
            params: vec![Box::new(f.fond.clone())],
            clauses: String::new(),
        };
        if !f.closes {
            me.clauses.push_str(&format!(" AND {}", ouverte("a")));
        }
        if let Some(q) = f.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
            let ph = me.push(Box::new(q.to_string()));
            me.clauses.push_str(&format!(
                " AND a.uid @@@ paradedb.boolean(should => ARRAY[\
                   paradedb.match('titre', ${ph}, conjunction_mode => true), \
                   paradedb.match('acteur_libelle', ${ph}, conjunction_mode => true)])"
            ));
        }
        for (axe, valeurs) in [
            (AnnonceAxe::Zone, &f.zones),
            (AnnonceAxe::Famille, &f.familles),
            (AnnonceAxe::Cpv, &f.cpv),
            (AnnonceAxe::Metier, &f.metiers),
        ] {
            if valeurs.is_empty() || sauf.is_some_and(|s| s.groupe().contains(&axe)) {
                continue;
            }
            let colonne = axe.colonne();
            let ph = me.push(Box::new(valeurs.clone()));
            // A zone is a set, so the test is an overlap, and asking for the
            // unzoned rows asks for the empty set, which no overlap can match.
            me.clauses.push_str(&match axe {
                AnnonceAxe::Zone if valeurs.iter().any(|v| v == ZONE_SANS) => {
                    format!(" AND ({colonne} && ${ph}::text[] OR cardinality({colonne}) = 0)")
                }
                AnnonceAxe::Zone | AnnonceAxe::Metier => {
                    format!(" AND {colonne} && ${ph}::text[]")
                }
                _ => format!(" AND {colonne} = ANY(${ph}::text[])"),
            });
        }
        if let Some(jours) = f.echeance_jours {
            let ph = me.push(Box::new(jours));
            me.clauses.push_str(&format!(
                " AND {}",
                fenetre_echeance("a", &format!("${ph}"))
            ));
        }
        me
    }

    fn push(&mut self, value: Box<dyn ToSql + Sync + Send>) -> usize {
        self.params.push(value);
        self.params.len()
    }

    fn refs(&self) -> Vec<&(dyn ToSql + Sync)> {
        self.params
            .iter()
            .map(|b| b.as_ref() as &(dyn ToSql + Sync))
            .collect()
    }
}

/// Fragment `ORDER BY`. L'échéance croissante est l'ordre du produit ; elle
/// emprunte l'index `annonce_fond_echeance_idx`.
fn order_by(ordre: AnnonceOrdre) -> &'static str {
    match ordre {
        AnnonceOrdre::Echeance => "a.echeance ASC NULLS LAST, a.uid",
        AnnonceOrdre::Publication => "a.date_publication DESC, a.uid",
        AnnonceOrdre::Nouveautes => "c.premiere_vue DESC NULLS LAST, a.uid",
    }
}

/// `ORDER BY` fragment of the enrichment queue. The nightly queue empties itself
/// and is served by arrival; the two catch-ups walk a stock no run finishes, and
/// keep the deadline.
fn enrichment_order_by(file: FileEnrichissement) -> &'static str {
    match file {
        FileEnrichissement::NonVisitees => {
            "(SELECT min(v.first_seen_at) FROM annonce_source v \
              WHERE v.annonce_uid = a.uid) DESC"
        }
        FileEnrichissement::SansAcheteur
        | FileEnrichissement::SansZone
        | FileEnrichissement::SansContenu => "a.echeance",
    }
}

impl DecisionRepository<'_> {
    /// Upsert d'un lot d'annonces canoniques et de leurs canaux, write-once : une
    /// ligne revue identique n'est pas réécrite. `first_seen_at` ne bouge jamais
    /// (c'est le curseur des alertes). Les compteurs rendus ne comptent donc que le
    /// nouveau et le modifié.
    ///
    /// Le batch n'écrase pas ce dont il n'est pas la source : un `cpv_uid` absent
    /// laisse celui du stock, et les clés [`DETAILS_PRESERVES`] de `details` sont
    /// re-greffées.
    #[tracing::instrument(name = "db.upsert_annonces", skip(self, items), fields(db.system = "postgresql", n = items.len()))]
    pub async fn upsert_annonces(&self, items: &[AnnonceWriteItem]) -> Result<AnnonceWriteStats> {
        if items.is_empty() {
            return Ok(AnnonceWriteStats::default());
        }
        let uid: Vec<&str> = items.iter().map(|i| i.uid.as_str()).collect();
        let fond: Vec<&str> = items.iter().map(|i| i.fond.as_str()).collect();
        let famille: Vec<Option<&str>> = items.iter().map(|i| i.famille_uid.as_deref()).collect();
        let titre: Vec<&str> = items.iter().map(|i| i.titre.as_str()).collect();
        let acteur: Vec<&str> = items.iter().map(|i| i.acteur_libelle.as_str()).collect();
        let zones: Vec<Json<&Vec<String>>> = items.iter().map(|i| Json(&i.zones)).collect();
        let cpv: Vec<Option<&str>> = items.iter().map(|i| i.cpv_uid.as_deref()).collect();
        let pub_: Vec<chrono::NaiveDate> = items.iter().map(|i| i.date_publication).collect();
        let ech: Vec<Option<chrono::DateTime<chrono::Utc>>> =
            items.iter().map(|i| i.echeance).collect();
        let url: Vec<&str> = items.iter().map(|i| i.url_action.as_str()).collect();
        let details: Vec<Json<&serde_json::Value>> =
            items.iter().map(|i| Json(&i.details)).collect();
        let descr: Vec<Option<&str>> = items.iter().map(|i| i.description.as_deref()).collect();
        let metiers: Vec<Json<&Vec<String>>> = items.iter().map(|i| Json(&i.metiers)).collect();

        let greffe = details_greffe();
        // Une liste muette n'efface pas l'acheteur que la fiche a nommé : sur Atexo,
        // l'API ne le porte pas et la fiche oui.
        let acheteur = "COALESCE(NULLIF(EXCLUDED.acteur_libelle, ''), annonce.acteur_libelle)";
        // Same rule for the zones: on achatpublic only the notice page names the place
        // of execution, so a plain assignment gives the enrichment back an empty array.
        let zones_maj =
            "CASE WHEN EXCLUDED.zones = '{}' THEN annonce.zones ELSE EXCLUDED.zones END";
        let description = format!(
            "COALESCE({}, annonce.description)",
            description_utile("EXCLUDED.description", "EXCLUDED.titre")
        );
        // A division read off the title follows the title; one a source published stays,
        // even on a night when only a mute channel answers.
        let cpv_maj = format!(
            "CASE WHEN {} OR NOT {} \
             THEN COALESCE(EXCLUDED.cpv_uid, annonce.cpv_uid) \
             ELSE COALESCE(annonce.cpv_uid, EXCLUDED.cpv_uid) END",
            code_cpv_publie("EXCLUDED"),
            code_cpv_publie("annonce"),
        );
        // Same shape for the nature, which the weekly net also poses from the
        // division: a batch that carries none leaves the one already there.
        let famille_maj = "COALESCE(EXCLUDED.famille_uid, annonce.famille_uid)";
        // Same for the trades of a row that carries a declaration, which the caller folds
        // again on the merged row; elsewhere a title's reading replaces the reading.
        let metiers_maj = format!(
            "CASE WHEN cardinality(annonce.metiers) = 0 \
             OR cardinality(EXCLUDED.metiers) > 0 AND NOT {} \
             THEN EXCLUDED.metiers ELSE annonce.metiers END",
            metiers_declares("annonce")
        );
        let annonces = self
            .conn
            .execute(
                &format!(
                    "
                INSERT INTO annonce
                  (uid, fond, famille_uid, titre, acteur_libelle, zones, cpv_uid,
                   date_publication, echeance, url_action, details, description, metiers)
                SELECT u, fo, f, t, a, ARRAY(SELECT jsonb_array_elements_text(z)), c, p, e, l, j, d,
                       ARRAY(SELECT jsonb_array_elements_text(m))
                FROM unnest($1::text[], $2::text[], $3::text[], $4::text[], $5::text[],
                            $6::jsonb[], $7::text[], $8::date[], $9::timestamptz[],
                            $10::text[], $11::jsonb[], $12::text[], $13::jsonb[])
                     AS s(u, fo, f, t, a, z, c, p, e, l, j, d, m)
                ON CONFLICT (uid) DO UPDATE SET
                  famille_uid = {famille_maj},
                  titre = EXCLUDED.titre,
                  acteur_libelle = {acheteur},
                  zones = {zones_maj},
                  cpv_uid = {cpv_maj},
                  date_publication = LEAST(annonce.date_publication, EXCLUDED.date_publication),
                  echeance = EXCLUDED.echeance,
                  url_action = EXCLUDED.url_action,
                  details = {greffe},
                  description = {description},
                  metiers = {metiers_maj}
                WHERE (annonce.famille_uid, annonce.titre, annonce.acteur_libelle,
                       annonce.zones, annonce.cpv_uid, annonce.echeance,
                       annonce.url_action, annonce.details, annonce.description,
                       annonce.metiers)
                      IS DISTINCT FROM
                      ({famille_maj}, EXCLUDED.titre, {acheteur},
                       {zones_maj}, {cpv_maj},
                       EXCLUDED.echeance, EXCLUDED.url_action, {greffe}, {description},
                       {metiers_maj})
                   OR EXCLUDED.date_publication < annonce.date_publication
                "
                ),
                &[
                    &uid, &fond, &famille, &titre, &acteur, &zones, &cpv, &pub_, &ech, &url,
                    &details, &descr, &metiers,
                ],
            )
            .await?;

        let mut s_uid: Vec<&str> = Vec::new();
        let mut s_annonce: Vec<&str> = Vec::new();
        let mut s_url: Vec<Option<&str>> = Vec::new();
        for item in items {
            for src in &item.sources {
                s_uid.push(&src.source_uid);
                s_annonce.push(&item.uid);
                s_url.push(src.url_avis.as_deref());
            }
        }
        let canaux = self.ecrire_canaux(&s_uid, &s_annonce, &s_url).await?;

        let mut e_annonce: Vec<&str> = Vec::new();
        let mut e_entity: Vec<String> = Vec::new();
        let mut e_role: Vec<&str> = Vec::new();
        for item in items {
            for (siren, role) in &item.aretes_siren {
                e_annonce.push(&item.uid);
                e_entity.push(format!("siren:{siren}"));
                e_role.push(role);
            }
        }
        let aretes = self.ecrire_aretes(&e_annonce, &e_entity, &e_role).await?;

        Ok(AnnonceWriteStats {
            annonces,
            canaux,
            aretes,
        })
    }

    /// Rattache des `source_uid` à leur annonce. Partagé par la création d'une
    /// ligne et par la pose d'un résultat sur une ligne existante : un avis de
    /// résultat est un canal de plus du marché qu'il clôt.
    async fn ecrire_canaux(
        &self,
        source_uid: &[&str],
        annonce_uid: &[&str],
        url_avis: &[Option<&str>],
    ) -> Result<u64> {
        if source_uid.is_empty() {
            return Ok(0);
        }
        Ok(self
            .conn
            .execute(
                "
                INSERT INTO annonce_source
                  (source_uid, annonce_uid, url_avis, first_seen_at)
                SELECT s, a, u, NOW()
                FROM unnest($1::text[], $2::text[], $3::text[]) AS t(s, a, u)
                ON CONFLICT (source_uid) DO UPDATE SET
                  annonce_uid = EXCLUDED.annonce_uid,
                  url_avis = EXCLUDED.url_avis
                WHERE (annonce_source.annonce_uid, annonce_source.url_avis)
                      IS DISTINCT FROM (EXCLUDED.annonce_uid, EXCLUDED.url_avis)
                ",
                &[&source_uid, &annonce_uid, &url_avis],
            )
            .await?)
    }

    /// Arêtes par SIREN donné à la source (eForms BOAMP).
    async fn ecrire_aretes(
        &self,
        annonce_uid: &[&str],
        entity_uid: &[String],
        role: &[&str],
    ) -> Result<u64> {
        if annonce_uid.is_empty() {
            return Ok(0);
        }
        Ok(self
            .conn
            .execute(
                "
                INSERT INTO annonce_entity (annonce_uid, entity_uid, role)
                SELECT a, e, r
                FROM unnest($1::text[], $2::text[], $3::text[]) AS t(a, e, r)
                ON CONFLICT DO NOTHING
                ",
                &[&annonce_uid, &entity_uid, &role],
            )
            .await?)
    }

    /// Deletes rows with their channels, edges and the redirects that led to
    /// them. Transactional, like a merge.
    #[tracing::instrument(name = "db.purger_annonces", skip(self, uids), fields(db.system = "postgresql", n = uids.len()))]
    pub async fn purger_annonces(&self, uids: &[&str]) -> Result<u64> {
        if uids.is_empty() {
            return Ok(0);
        }
        self.conn.batch_execute("BEGIN").await?;
        let applique = async {
            for requete in [
                "DELETE FROM annonce_entity WHERE annonce_uid = ANY($1::text[])",
                "DELETE FROM annonce_source WHERE annonce_uid = ANY($1::text[])",
                "DELETE FROM annonce_redirect WHERE nouvel_uid = ANY($1::text[])",
            ] {
                self.conn.execute(requete, &[&uids]).await?;
            }
            self.conn
                .execute("DELETE FROM annonce WHERE uid = ANY($1::text[])", &[&uids])
                .await
        }
        .await;
        match applique {
            Ok(supprimees) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(supprimees)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e.into())
            }
        }
    }

    /// Résout des canaux vers leur annonce : `source_uid` → `annonce_uid`. Le
    /// rattachement d'un résultat de marché à sa consultation : le `annonce_lie`
    /// BOAMP est un `source_uid`, la ligne canonique est derrière.
    #[tracing::instrument(name = "db.annonces_par_source", skip(self, source_uids), fields(db.system = "postgresql", n = source_uids.len()))]
    pub async fn annonces_par_source(
        &self,
        source_uids: &[String],
    ) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, annonce_uid FROM annonce_source \
                 WHERE source_uid = ANY($1::text[])",
                &[&source_uids],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// La cible de redirection d'un uid absorbé. `None` = uid vivant ou inconnu.
    #[tracing::instrument(name = "db.redirection_annonce", skip(self), fields(db.system = "postgresql"))]
    pub async fn redirection_annonce(&self, uid: &str) -> Result<Option<String>> {
        let row = self
            .conn
            .query_opt(
                "SELECT nouvel_uid FROM annonce_redirect WHERE ancien_uid = $1",
                &[&uid],
            )
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    /// Applique des fusions `(ancien, nouveau)` décidées par la réconciliation :
    /// re-parente les canaux et les arêtes de l'absorbée, lui fait léguer ses clés
    /// [`DETAILS_PRESERVES`] à l'élue, pose la redirection — en re-pointant les
    /// chaînes qui visaient l'absorbée — et supprime la ligne. Transactionnel : une
    /// fusion à moitié appliquée referait une orpheline.
    #[tracing::instrument(name = "db.fusionner_annonces", skip(self, fusions), fields(db.system = "postgresql", n = fusions.len()))]
    pub async fn fusionner_annonces(&self, fusions: &[(String, String)]) -> Result<u64> {
        if fusions.is_empty() {
            return Ok(0);
        }
        let ancien: Vec<&str> = fusions.iter().map(|(a, _)| a.as_str()).collect();
        let nouveau: Vec<&str> = fusions.iter().map(|(_, n)| n.as_str()).collect();
        self.conn.batch_execute("BEGIN").await?;
        let applique = async {
            self.conn
                .execute(
                    "UPDATE annonce_source s SET annonce_uid = t.n \
                     FROM unnest($1::text[], $2::text[]) AS t(a, n) \
                     WHERE s.annonce_uid = t.a",
                    &[&ancien, &nouveau],
                )
                .await?;
            self.conn
                .execute(
                    "INSERT INTO annonce_entity (annonce_uid, entity_uid, role) \
                     SELECT t.n, e.entity_uid, e.role \
                     FROM annonce_entity e \
                     JOIN unnest($1::text[], $2::text[]) AS t(a, n) ON e.annonce_uid = t.a \
                     ON CONFLICT DO NOTHING",
                    &[&ancien, &nouveau],
                )
                .await?;
            self.conn
                .execute(
                    "DELETE FROM annonce_entity e \
                     USING unnest($1::text[]) AS t(a) WHERE e.annonce_uid = t.a",
                    &[&ancien],
                )
                .await?;
            let legs = details_leguees("a", "n");
            self.conn
                .execute(
                    &format!(
                        "UPDATE annonce n \
                         SET details = n.details \
                             || (SELECT jsonb_object_agg(k, a.details -> k) {legs}) \
                         FROM annonce a, unnest($1::text[], $2::text[]) AS t(a_uid, n_uid) \
                         WHERE n.uid = t.n_uid AND a.uid = t.a_uid \
                           AND EXISTS (SELECT 1 {legs})"
                    ),
                    &[&ancien, &nouveau],
                )
                .await?;
            self.conn
                .execute(
                    "UPDATE annonce_redirect r SET nouvel_uid = t.n \
                     FROM unnest($1::text[], $2::text[]) AS t(a, n) \
                     WHERE r.nouvel_uid = t.a",
                    &[&ancien, &nouveau],
                )
                .await?;
            self.conn
                .execute(
                    "INSERT INTO annonce_redirect (ancien_uid, nouvel_uid) \
                     SELECT t.a, t.n FROM unnest($1::text[], $2::text[]) AS t(a, n) \
                     ON CONFLICT (ancien_uid) DO UPDATE SET nouvel_uid = EXCLUDED.nouvel_uid",
                    &[&ancien, &nouveau],
                )
                .await?;
            self.conn
                .execute(
                    "DELETE FROM annonce WHERE uid = ANY($1::text[])",
                    &[&ancien],
                )
                .await
        }
        .await;
        match applique {
            Ok(supprimees) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(supprimees)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e.into())
            }
        }
    }

    /// Les lignes d'un fond sans plus aucun canal, avec les URLs de leurs
    /// `details.liens` : la matière de `annonces-regulariser`.
    #[tracing::instrument(name = "db.annonces_orphelines", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonces_orphelines(&self, fond: &str) -> Result<Vec<AnnonceOrpheline>> {
        let rows = self
            .conn
            .query(
                "SELECT a.uid, a.titre, a.acteur_libelle, a.echeance, \
                        COALESCE(array_agg(u.url) FILTER (WHERE u.url IS NOT NULL), '{}') \
                 FROM annonce a \
                 LEFT JOIN LATERAL ( \
                   SELECT unnest(ARRAY[el->>'avis', el->>'rc', el->>'dce']) AS url \
                   FROM jsonb_array_elements(a.details->'liens') el \
                 ) u ON true \
                 WHERE a.fond = $1 \
                   AND NOT EXISTS (SELECT 1 FROM annonce_source s WHERE s.annonce_uid = a.uid) \
                 GROUP BY a.uid",
                &[&fond],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceOrpheline {
                uid: r.get(0),
                titre: r.get(1),
                acteur_libelle: r.get(2),
                echeance: r.get(3),
                urls: r.get(4),
            })
            .collect())
    }

    /// Rows still without a trade once the fold has run: either no source
    /// described them, or their only descriptor names a contract form.
    #[tracing::instrument(name = "db.annonces_sans_metier", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonces_sans_metier(&self, limite: i64) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, titre FROM annonce \
                 WHERE fond = 'marche' AND cardinality(metiers) = 0 AND titre <> '' \
                 ORDER BY echeance DESC NULLS LAST LIMIT $1",
                &[&limite],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Rows carrying a descriptor or a works lot, among `parmi` when given.
    /// `lj_annonces::metiers::metiers_du_marche` holds the fold that decides.
    #[tracing::instrument(name = "db.annonces_a_metier", skip(self, parmi), fields(db.system = "postgresql"))]
    pub async fn annonces_a_metier(&self, parmi: Option<&[String]>) -> Result<Vec<AnnonceDecrite>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT uid, titre, \
                     ARRAY(SELECT jsonb_array_elements_text(details -> 'descripteurs')), \
                     ARRAY(SELECT l ->> 'cpv' FROM {} l WHERE jsonb_typeof(l -> 'cpv') = 'string'), \
                     metiers FROM annonce \
                     WHERE ($1::text[] IS NULL OR uid = ANY($1)) AND {}",
                    lots_de("annonce"),
                    metiers_declares("annonce")
                ),
                &[&parmi],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceDecrite {
                uid: r.get(0),
                titre: r.get(1),
                descripteurs: r.get(2),
                codes_des_lots: r.get(3),
                metiers: r.get(4),
            })
            .collect())
    }

    /// The trades folded on each row, over whatever it carried.
    #[tracing::instrument(name = "db.replier_metiers", skip(self, metiers), fields(db.system = "postgresql", n = metiers.len()))]
    pub async fn replier_metiers(&self, metiers: &[(String, Vec<String>)]) -> Result<u64> {
        if metiers.is_empty() {
            return Ok(0);
        }
        let uid: Vec<&str> = metiers.iter().map(|(u, _)| u.as_str()).collect();
        let m: Vec<Json<&Vec<String>>> = metiers.iter().map(|(_, m)| Json(m)).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE annonce a SET metiers = t.m \
                 FROM (SELECT uid, ARRAY(SELECT jsonb_array_elements_text(m)) AS m \
                       FROM unnest($1::text[], $2::jsonb[]) AS u(uid, m)) AS t \
                 WHERE a.uid = t.uid AND a.metiers IS DISTINCT FROM t.m",
                &[&uid, &m],
            )
            .await?)
    }

    #[tracing::instrument(name = "db.poser_metiers", skip(self, metiers), fields(db.system = "postgresql", n = metiers.len()))]
    pub async fn poser_metiers(&self, metiers: &[(String, Vec<String>)]) -> Result<u64> {
        if metiers.is_empty() {
            return Ok(0);
        }
        let uid: Vec<&str> = metiers.iter().map(|(u, _)| u.as_str()).collect();
        let m: Vec<Json<&Vec<String>>> = metiers.iter().map(|(_, m)| Json(m)).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE annonce a SET metiers = ARRAY(SELECT jsonb_array_elements_text(t.m)) \
                 FROM unnest($1::text[], $2::jsonb[]) AS t(uid, m) \
                 WHERE a.uid = t.uid AND cardinality(a.metiers) = 0",
                &[&uid, &m],
            )
            .await?)
    }

    /// The rows no harvest has zoned yet, with the two things a zone can be
    /// read from without leaving the table: the single code the previous schema
    /// kept, and the buyer label, which often names a région.
    #[tracing::instrument(name = "db.annonces_a_zoner", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonces_a_zoner(&self) -> Result<Vec<(String, Option<String>, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, departement, acteur_libelle FROM annonce \
                 WHERE cardinality(zones) = 0",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    #[tracing::instrument(name = "db.poser_zones", skip(self, zones), fields(db.system = "postgresql", n = zones.len()))]
    pub async fn poser_zones(&self, zones: &[(String, Vec<String>)]) -> Result<u64> {
        if zones.is_empty() {
            return Ok(0);
        }
        let uid: Vec<&str> = zones.iter().map(|(u, _)| u.as_str()).collect();
        let z: Vec<Json<&Vec<String>>> = zones.iter().map(|(_, z)| Json(z)).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE annonce a SET zones = ARRAY(SELECT jsonb_array_elements_text(t.z)) \
                 FROM unnest($1::text[], $2::jsonb[]) AS t(uid, z) \
                 WHERE a.uid = t.uid AND cardinality(a.zones) = 0",
                &[&uid, &z],
            )
            .await?)
    }

    /// Markets no source ever categorised, title included so the caller can
    /// infer. Ordered by closing date so a run bounded by `limite` serves what
    /// is still open first.
    #[tracing::instrument(name = "db.annonces_sans_cpv", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonces_sans_cpv(&self, limite: i64) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, titre FROM annonce \
                 WHERE fond = 'marche' AND cpv_uid IS NULL AND titre <> '' \
                 ORDER BY echeance DESC NULLS LAST LIMIT $1",
                &[&limite],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// The inferred division and its mark, in one write. The guard on
    /// `cpv_uid IS NULL` keeps a code a source published out of reach.
    #[tracing::instrument(name = "db.poser_cpv_deduit", skip(self, poses), fields(db.system = "postgresql", n = poses.len()))]
    pub async fn poser_cpv_deduit(&self, poses: &[(String, String)]) -> Result<u64> {
        if poses.is_empty() {
            return Ok(0);
        }
        let uid: Vec<&str> = poses.iter().map(|(u, _)| u.as_str()).collect();
        let div: Vec<&str> = poses.iter().map(|(_, d)| d.as_str()).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE annonce a \
                 SET cpv_uid = t.d \
                 FROM unnest($1::text[], $2::text[]) AS t(u, d) \
                 WHERE a.uid = t.u AND a.cpv_uid IS NULL",
                &[&uid, &div],
            )
            .await?)
    }

    /// Market nature read off the division for the rows no source typed. The
    /// mapping travels as two arrays, so the whole stock is one statement.
    #[tracing::instrument(name = "db.deduire_famille_depuis_cpv", skip(self, table), fields(db.system = "postgresql", n = table.len()))]
    pub async fn deduire_famille_depuis_cpv(&self, table: &[(&str, &str)]) -> Result<u64> {
        if table.is_empty() {
            return Ok(0);
        }
        let division: Vec<&str> = table.iter().map(|(d, _)| *d).collect();
        let nature: Vec<&str> = table.iter().map(|(_, n)| *n).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE annonce a \
                 SET famille_uid = t.n \
                 FROM unnest($1::text[], $2::text[]) AS t(d, n) \
                 WHERE a.fond = 'marche' AND a.famille_uid IS NULL \
                   AND a.cpv_uid = 'cpv:' || t.d",
                &[&division, &nature],
            )
            .await?)
    }

    /// Candidates of the elision and entity repairs, pre-filtered wide.
    /// `lj_annonces::reparer_elisions` and `lj_sources_annonces::desechapper`
    /// hold the rules that decide.
    #[tracing::instrument(name = "db.descriptions_a_reparer", skip(self), fields(db.system = "postgresql"))]
    pub async fn descriptions_a_reparer(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, description FROM annonce \
                 WHERE description ~ '[[:alpha:]][?][[:alpha:]]' \
                    OR description ~ '&(#x?[0-9a-fA-F]+|[a-zA-Z][a-zA-Z0-9]*);'",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    #[tracing::instrument(name = "db.corriger_descriptions", skip(self, corrections), fields(db.system = "postgresql", n = corrections.len()))]
    pub async fn corriger_descriptions(&self, corrections: &[(String, String)]) -> Result<u64> {
        if corrections.is_empty() {
            return Ok(0);
        }
        let uid: Vec<&str> = corrections.iter().map(|(u, _)| u.as_str()).collect();
        let texte: Vec<&str> = corrections.iter().map(|(_, d)| d.as_str()).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE annonce a SET description = t.description \
                 FROM unnest($1::text[], $2::text[]) AS t(uid, description) \
                 WHERE a.uid = t.uid",
                &[&uid, &texte],
            )
            .await?)
    }

    /// What the write guard now refuses, left in stock by the writes that
    /// predate it. Idempotent: nothing matches once it has run.
    #[tracing::instrument(name = "db.effacer_descriptions_copiees", skip(self), fields(db.system = "postgresql"))]
    pub async fn effacer_descriptions_copiees(&self) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE annonce SET description = NULL \
                 WHERE description IS NOT NULL \
                   AND lower(btrim(description)) = lower(btrim(titre))",
                &[],
            )
            .await?)
    }

    /// Same net for a deadline more than `ans_max` years after publication.
    #[tracing::instrument(name = "db.effacer_echeances_implausibles", skip(self), fields(db.system = "postgresql"))]
    pub async fn effacer_echeances_implausibles(&self, ans_max: i16) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE annonce SET echeance = NULL \
                 WHERE fond = 'marche' AND echeance IS NOT NULL \
                   AND extract(year FROM echeance AT TIME ZONE 'UTC') \
                     - extract(year FROM date_publication) > $1::int",
                &[&i32::from(ans_max)],
            )
            .await?)
    }

    /// Those rows are markets known by their outcome; the derived status reads
    /// from `details.resultat`, so the fond carries nothing of its own.
    #[tracing::instrument(name = "db.refondre_attributions", skip(self), fields(db.system = "postgresql"))]
    pub async fn refondre_attributions(&self) -> Result<u64> {
        Ok(self
            .conn
            .execute(
                "UPDATE annonce SET fond = 'marche' WHERE fond = 'attribution'",
                &[],
            )
            .await?)
    }

    /// Titre et libellé d'acheteur de tout le pot — assiette de la re-normalisation
    /// rétroactive de la régularisation : une règle de forme nouvelle s'applique au
    /// stock clos, que la moisson ne réécrit plus.
    #[tracing::instrument(name = "db.formes_annonces", skip(self), fields(db.system = "postgresql"))]
    pub async fn formes_annonces(&self) -> Result<Vec<(String, String, String)>> {
        let rows = self
            .conn
            .query("SELECT uid, titre, acteur_libelle FROM annonce", &[])
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    #[tracing::instrument(name = "db.corriger_formes", skip(self, corrections), fields(db.system = "postgresql", n = corrections.len()))]
    pub async fn corriger_formes(&self, corrections: &[(String, String, String)]) -> Result<u64> {
        if corrections.is_empty() {
            return Ok(0);
        }
        let uid: Vec<&str> = corrections.iter().map(|(u, _, _)| u.as_str()).collect();
        let titre: Vec<&str> = corrections.iter().map(|(_, t, _)| t.as_str()).collect();
        let acteur: Vec<&str> = corrections.iter().map(|(_, _, a)| a.as_str()).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE annonce a SET titre = t.titre, acteur_libelle = t.acteur \
                 FROM unnest($1::text[], $2::text[], $3::text[]) AS t(uid, titre, acteur) \
                 WHERE a.uid = t.uid",
                &[&uid, &titre, &acteur],
            )
            .await?)
    }

    /// Les blocs `details.resultat` du pot — assiette de la re-normalisation
    /// rétroactive des lots d'attribution.
    #[tracing::instrument(name = "db.resultats_annonces", skip(self), fields(db.system = "postgresql"))]
    pub async fn resultats_annonces(&self) -> Result<Vec<(String, serde_json::Value)>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, details->'resultat' FROM annonce WHERE details ? 'resultat'",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    #[tracing::instrument(name = "db.corriger_resultats", skip(self, corrections), fields(db.system = "postgresql", n = corrections.len()))]
    pub async fn corriger_resultats(
        &self,
        corrections: &[(String, serde_json::Value)],
    ) -> Result<u64> {
        if corrections.is_empty() {
            return Ok(0);
        }
        let uid: Vec<&str> = corrections.iter().map(|(u, _)| u.as_str()).collect();
        let resultat: Vec<&serde_json::Value> = corrections.iter().map(|(_, v)| v).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE annonce a SET details = jsonb_set(a.details, '{resultat}', t.resultat) \
                 FROM unnest($1::text[], $2::jsonb[]) AS t(uid, resultat) \
                 WHERE a.uid = t.uid",
                &[&uid, &resultat],
            )
            .await?)
    }

    /// Les champs d'identification d'un lot de lignes — pour départager les
    /// cibles d'une orpheline lors de la régularisation.
    #[tracing::instrument(name = "db.annonces_champs", skip(self, uids), fields(db.system = "postgresql", n = uids.len()))]
    pub async fn annonces_champs(&self, uids: &[String]) -> Result<Vec<AnnonceChamps>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT a.uid, a.titre, a.acteur_libelle, a.echeance, a.zones, {}, \
                            NULLIF(TRIM(a.details->>'reference'), '') \
                     FROM annonce a WHERE a.uid = ANY($1::text[])",
                    acheteur_unique("a")
                ),
                &[&uids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceChamps {
                uid: r.get(0),
                titre: r.get(1),
                acteur_libelle: r.get(2),
                echeance: r.get(3),
                zones: r.get(4),
                acheteur_uid: r.get(5),
                reference: r.get(6),
            })
            .collect())
    }

    /// The market a buyer reference designates, when it designates only one:
    /// `2026-01` runs over many rows and joins nothing. Markets already awarded
    /// drop out, their notice costing a request each.
    #[tracing::instrument(name = "db.marches_par_reference", skip(self, references), fields(db.system = "postgresql", n = references.len()))]
    pub async fn marches_par_reference(
        &self,
        references: &[String],
    ) -> Result<Vec<MarcheParReference>> {
        let rows = self
            .conn
            .query(
                "SELECT reference, uid, acteur_libelle FROM ( \
                   SELECT NULLIF(TRIM(a.details->>'reference'), '') AS reference, \
                          a.uid, a.acteur_libelle, \
                          count(*) OVER (PARTITION BY NULLIF(TRIM(a.details->>'reference'), '')) AS n, \
                          a.details ? 'resultat' AS attribue \
                   FROM annonce a \
                   WHERE a.fond = 'marche' \
                     AND NULLIF(TRIM(a.details->>'reference'), '') = ANY($1::text[]) \
                 ) t WHERE n = 1 AND NOT attribue",
                &[&references],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| MarcheParReference {
                reference: r.get(0),
                uid: r.get(1),
                acteur_libelle: r.get(2),
            })
            .collect())
    }

    /// The closed markets an award declaration could still close, over a window
    /// of deadlines. Whole rows, not a per-declaration lookup: matching every
    /// declaration one query at a time is a trigram index probe each. A result
    /// naming neither a notice nor a text is one this same channel wrote, and
    /// comes back in: no other channel writes that shape, and a rerun has to be
    /// able to repose it.
    #[tracing::instrument(name = "db.marches_clos_sans_resultat", skip(self), fields(db.system = "postgresql"))]
    pub async fn marches_clos_sans_resultat(
        &self,
        fond: &str,
        depuis: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<MarcheClosSansResultat>> {
        // `poser_resultats` is transactional: the second OR only catches up the old orphans.
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT a.uid, a.titre, a.acteur_libelle, a.echeance, a.cpv_uid, {} \
                     FROM annonce a \
                     WHERE a.fond = $1 AND a.echeance >= $2 AND a.echeance < NOW() \
                       AND (NOT (a.details ? 'resultat') \
                            OR (a.details->'resultat'->>'avis_url' IS NULL \
                                AND a.details->'resultat'->>'texte' IS NULL))",
                    acheteur_unique("a")
                ),
                &[&fond, &depuis],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| MarcheClosSansResultat {
                uid: r.get(0),
                titre: r.get(1),
                acteur_libelle: r.get(2),
                echeance: r.get(3),
                cpv_uid: r.get(4),
                acheteur_siren: r
                    .get::<_, Option<String>>(5)
                    .and_then(|u| u.strip_prefix("siren:").map(str::to_string)),
            })
            .collect())
    }

    /// The registry name of each SIREN, which is what turns a declaration that
    /// only carries a SIRET into something a buyer label can be compared to.
    #[tracing::instrument(name = "db.denominations_par_siren", skip(self, sirens), fields(db.system = "postgresql", n = sirens.len()))]
    pub async fn denominations_par_siren(
        &self,
        sirens: &[String],
    ) -> Result<Vec<(String, String)>> {
        let uids: Vec<String> = sirens.iter().map(|s| format!("siren:{s}")).collect();
        let rows = self
            .conn
            .query(
                "SELECT uid, denomination FROM entity WHERE uid = ANY($1::text[])",
                &[&uids],
            )
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let uid: String = r.get(0);
                Some((uid.strip_prefix("siren:")?.to_string(), r.get(1)))
            })
            .collect())
    }

    /// Résout des URLs de canal vers leur annonce (`url_avis` exact). Pas
    /// d'index sur la colonne : réservé au one-shot de régularisation.
    #[tracing::instrument(name = "db.annonces_par_url_avis", skip(self, urls), fields(db.system = "postgresql", n = urls.len()))]
    pub async fn annonces_par_url_avis(&self, urls: &[String]) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT url_avis, annonce_uid FROM annonce_source \
                 WHERE url_avis = ANY($1::text[])",
                &[&urls],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// The open rows of a fund that no platform carries: the harvest could not
    /// attach them, and only their platform twin can join them. One more
    /// publication channel, TED over a notice already on BOAMP, gives no door
    /// and does not take a row out of the queue. A declared deposit is not
    /// required, the buyer key having no use for it.
    #[tracing::instrument(name = "db.annonces_a_jumeler", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonces_a_jumeler(&self, fond: &str) -> Result<Vec<AnnonceAJumeler>> {
        let publication: Vec<String> = lj_vocab::CANAUX_PUBLICATION
            .iter()
            .map(|c| (*c).to_string())
            .collect();
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT a.uid, a.titre, a.acteur_libelle, a.echeance, a.zones, \
                        COALESCE(a.details->>'depot', ''), \
                        COALESCE(array_agg(u.url) FILTER (WHERE u.url IS NOT NULL), '{{}}'), \
                        {}, NULLIF(TRIM(a.details->>'reference'), '') \
                 FROM annonce a \
                 LEFT JOIN LATERAL ( \
                   SELECT unnest(ARRAY[el->>'avis', el->>'rc', el->>'dce']) AS url \
                   FROM jsonb_array_elements(a.details->'liens') el \
                 ) u ON true \
                 WHERE a.fond = $1 AND a.echeance > NOW() \
                   AND EXISTS (SELECT 1 FROM annonce_source s WHERE s.annonce_uid = a.uid) \
                   AND NOT EXISTS (SELECT 1 FROM annonce_source s \
                                   WHERE s.annonce_uid = a.uid \
                                     AND split_part(s.source_uid, ':', 1) <> ALL($2::text[])) \
                 GROUP BY a.uid",
                    acheteur_unique("a")
                ),
                &[&fond, &publication],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceAJumeler {
                uid: r.get(0),
                titre: r.get(1),
                acteur_libelle: r.get(2),
                echeance: r.get(3),
                zones: r.get(4),
                depot: r.get(5),
                urls: r.get(6),
                acheteur_uid: r.get(7),
                reference: r.get(8),
            })
            .collect())
    }

    /// Every platform channel of a fund's open rows, the twinning pool. A row
    /// with several channels yields one entry per channel: the channel, not the
    /// engine, designates the instance. A publication channel is not one of
    /// them, having no deep link to give, and a row that only publishes would
    /// otherwise be its own twin.
    #[tracing::instrument(name = "db.canaux_plateforme_ouverts", skip(self), fields(db.system = "postgresql"))]
    pub async fn canaux_plateforme_ouverts(&self, fond: &str) -> Result<Vec<AnnonceCanalOuvert>> {
        let publication: Vec<String> = lj_vocab::CANAUX_PUBLICATION
            .iter()
            .map(|c| (*c).to_string())
            .collect();
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT s.source_uid, a.uid, a.titre, a.acteur_libelle, a.echeance, \
                            a.zones, {}, \
                            NULLIF(TRIM(a.details->>'reference'), '') \
                     FROM annonce a JOIN annonce_source s ON s.annonce_uid = a.uid \
                     WHERE a.fond = $1 AND a.echeance > NOW() \
                       AND split_part(s.source_uid, ':', 1) <> ALL($2::text[])",
                    acheteur_unique("a")
                ),
                &[&fond, &publication],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceCanalOuvert {
                source_uid: r.get(0),
                annonce: AnnonceChamps {
                    uid: r.get(1),
                    titre: r.get(2),
                    acteur_libelle: r.get(3),
                    echeance: r.get(4),
                    zones: r.get(5),
                    acheteur_uid: r.get(6),
                    reference: r.get(7),
                },
            })
            .collect())
    }

    /// Les lignes ouvertes qu'aucun canal de plateforme ne porte : un canal de
    /// publication les a vues (BOAMP, TED, presse), la moisson des profils
    /// d'acheteurs non. C'est la mesure de complétude de la moisson, tenant par
    /// tenant — `plateformes` liste les moteurs effectivement moissonnés.
    #[tracing::instrument(name = "db.annonces_sans_plateforme", skip(self, plateformes), fields(db.system = "postgresql"))]
    pub async fn annonces_sans_plateforme(
        &self,
        fond: &str,
        plateformes: &[&str],
    ) -> Result<Vec<TrouCouverture>> {
        let plateformes: Vec<String> = plateformes.iter().map(|m| (*m).to_string()).collect();
        let rows = self
            .conn
            .query(
                "WITH ouvertes AS ( \
                   SELECT a.uid, NULLIF(a.details->>'depot', '') AS depot, \
                          array_agg(DISTINCT split_part(s.source_uid, ':', 1)) AS canaux \
                   FROM annonce a JOIN annonce_source s ON s.annonce_uid = a.uid \
                   WHERE a.fond = $1 AND a.echeance > NOW() \
                   GROUP BY a.uid, 2 \
                 ) \
                 SELECT COALESCE(depot, ''), array_to_string(canaux, ','), count(*)::bigint \
                 FROM ouvertes \
                 WHERE NOT (canaux && $2::text[]) \
                 GROUP BY 1, 2 \
                 ORDER BY 3 DESC",
                &[&fond, &plateformes],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| TrouCouverture {
                depot: r.get(0),
                canaux: r.get(1),
                lignes: r.get(2),
            })
            .collect())
    }

    /// Le recensement des profils d'acheteurs, hôte par hôte : à quel moteur
    /// appartient une plateforme. Les hôtes que la sonde n'a pas su classer
    /// n'en sont pas.
    #[tracing::instrument(name = "db.moteurs_par_hote", skip(self), fields(db.system = "postgresql"))]
    pub async fn moteurs_par_hote(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT host, moteur FROM profil_acheteur WHERE moteur IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Pose l'attribution sur la ligne du marché déjà en stock : le bloc
    /// `details.resultat` write-once, l'avis de résultat comme canal de plus, et les
    /// arêtes `titulaire`. La ligne garde son titre, son acheteur et son échéance de
    /// consultation — seul son état change.
    ///
    /// Transactional: the posed block takes the row out of every queue, so a
    /// `details.resultat` without its channel is an orphan no rerun reaches.
    #[tracing::instrument(name = "db.poser_resultats", skip(self, poses), fields(db.system = "postgresql", n = poses.len()))]
    pub async fn poser_resultats(&self, poses: &[ResultatPose]) -> Result<u64> {
        if poses.is_empty() {
            return Ok(0);
        }
        self.conn.batch_execute("BEGIN").await?;
        let applique = self.poser_resultats_tx(poses).await;
        match applique {
            Ok(poses_count) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(poses_count)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    async fn poser_resultats_tx(&self, poses: &[ResultatPose]) -> Result<u64> {
        let uid: Vec<&str> = poses.iter().map(|p| p.annonce_uid.as_str()).collect();
        let resultat: Vec<Json<&serde_json::Value>> =
            poses.iter().map(|p| Json(&p.resultat)).collect();
        let poses_count = self
            .conn
            .execute(
                "
                UPDATE annonce a
                SET details = jsonb_set(a.details, '{resultat}', t.j)
                FROM unnest($1::text[], $2::jsonb[]) AS t(u, j)
                WHERE a.uid = t.u AND a.details->'resultat' IS DISTINCT FROM t.j
                ",
                &[&uid, &resultat],
            )
            .await?;

        let s_uid: Vec<&str> = poses.iter().map(|p| p.source.source_uid.as_str()).collect();
        let s_url: Vec<Option<&str>> = poses.iter().map(|p| p.source.url_avis.as_deref()).collect();
        self.ecrire_canaux(&s_uid, &uid, &s_url).await?;

        let mut e_annonce: Vec<&str> = Vec::new();
        let mut e_entity: Vec<String> = Vec::new();
        let mut e_role: Vec<&str> = Vec::new();
        for p in poses {
            for (siren, role) in &p.aretes_siren {
                e_annonce.push(&p.annonce_uid);
                e_entity.push(format!("siren:{siren}"));
                e_role.push(role);
            }
        }
        self.ecrire_aretes(&e_annonce, &e_entity, &e_role).await?;

        Ok(poses_count)
    }

    /// La file d'enrichissement par page de détail, dérivée : rien n'est stampé,
    /// elle se calcule comme la fenêtre ouverte. Les annonces ouvertes du fond sans
    /// bloc `details.detail`, hors BOAMP — dont le flux porte déjà cpv et procédure.
    /// One row per notice: the highest-trust channel (`PRIORITE`) that carries a
    /// `url_avis` and whose page still has something to say, `sans_fiche` dropping
    /// the ones whose page repeats the listing. Freshest arrival first
    /// (`min(annonce_source.first_seen_at)`): by deadline a notice published today
    /// queues behind the whole stock. The catch-ups keep the deadline order.
    ///
    /// `sans_acheteur` échange le critère de file : les lignes sans libellé
    /// d'acheteur, visitées ou non. C'est le rattrapage des fiches lues avant que
    /// l'extracteur ne sache lire l'acheteur, que le bloc `detail` posé avait
    /// sorties de la file pour toujours.
    ///
    /// `SansZone` is the same catch-up on the place, and it alone keeps the BOAMP
    /// rows: a canonical row is the union of the channels one run saw, so a night
    /// that met only the platform left the registry's département behind.
    #[tracing::instrument(name = "db.annonces_a_enrichir", skip(self, priorite), fields(db.system = "postgresql"))]
    pub async fn annonces_a_enrichir(
        &self,
        fond: &str,
        priorite: &[&str],
        sans_fiche: &[&str],
        limite: i64,
        file: FileEnrichissement,
    ) -> Result<Vec<AnnonceAEnrichir>> {
        // A BOAMP row with no zone has lost the one its registry gave it, on a
        // run that saw only its other channel: the exclusion assumes the flux
        // answers, and for this column it no longer does.
        const HORS_BOAMP: &str = "AND NOT EXISTS (SELECT 1 FROM annonce_source b \
             WHERE b.annonce_uid = a.uid AND b.source_uid LIKE 'boamp:%')";
        let ordre = enrichment_order_by(file);
        // A catch-up leaves out what the current parser has already read, which
        // is what `vu` stamps: a row that answers nothing would come back every
        // week.
        let relu = match file {
            FileEnrichissement::NonVisitees => "",
            _ => "AND NOT (COALESCE(a.details -> 'detail', '{}'::jsonb) ? 'vu')",
        };
        let (file, hors_boamp) = match file {
            FileEnrichissement::NonVisitees => ("NOT a.details ? 'detail'", HORS_BOAMP),
            FileEnrichissement::SansAcheteur => ("a.acteur_libelle = ''", HORS_BOAMP),
            FileEnrichissement::SansZone => ("a.zones = '{}'", ""),
            FileEnrichissement::SansContenu => (
                "a.description IS NULL AND NOT (COALESCE(a.details -> 'detail', '{}'::jsonb) \
                 ?| ARRAY['lots', 'criteres', 'montant_estime'])",
                HORS_BOAMP,
            ),
        };
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT a.uid, split_part(s.source_uid, ':', 1) AS moteur, s.url_avis \
                 FROM annonce a \
                 JOIN LATERAL ( \
                   SELECT s.source_uid, s.url_avis FROM annonce_source s \
                   WHERE s.annonce_uid = a.uid AND s.url_avis IS NOT NULL \
                     AND split_part(s.source_uid, ':', 1) <> ALL($4::text[]) \
                   ORDER BY array_position($2::text[], split_part(s.source_uid, ':', 1)) \
                            NULLS LAST, s.source_uid \
                   LIMIT 1 \
                 ) s ON true \
                 WHERE a.fond = $1 AND a.echeance > NOW() \
                   AND {file} {relu} {hors_boamp} \
                 ORDER BY {ordre} \
                 LIMIT $3"
                ),
                &[&fond, &priorite, &limite, &sans_fiche],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceAEnrichir {
                uid: r.get(0),
                moteur: r.get(1),
                url_avis: r.get(2),
            })
            .collect())
    }

    /// Pose le bloc `details.detail` d'un lot d'annonces. Le bloc est écrit même
    /// vide : c'est le marqueur « déjà visité » qui sort la ligne de la file. Un
    /// `cpv_uid` absent laisse celui du stock.
    #[tracing::instrument(name = "db.poser_detail_annonces", skip(self, poses), fields(db.system = "postgresql", n = poses.len()))]
    pub async fn poser_detail_annonces(&self, poses: &[DetailPose]) -> Result<u64> {
        if poses.is_empty() {
            return Ok(0);
        }
        let uid: Vec<&str> = poses.iter().map(|p| p.annonce_uid.as_str()).collect();
        let cpv: Vec<Option<&str>> = poses.iter().map(|p| p.cpv_uid.as_deref()).collect();
        let detail: Vec<Json<&serde_json::Value>> = poses.iter().map(|p| Json(&p.detail)).collect();
        let acheteur: Vec<Option<&str>> = poses.iter().map(|p| p.acheteur.as_deref()).collect();
        let descr: Vec<Option<&str>> = poses.iter().map(|p| p.description.as_deref()).collect();
        let zones: Vec<Option<String>> = poses
            .iter()
            .map(|p| (!p.zones.is_empty()).then(|| p.zones.join(",")))
            .collect();
        let descr_utile = description_utile("t.d", "a.titre");
        Ok(self
            .conn
            .execute(
                &format!(
                    "
                UPDATE annonce a
                SET cpv_uid = COALESCE(t.c, a.cpv_uid),
                    details = a.details || jsonb_build_object('detail', t.j),
                    acteur_libelle = COALESCE(NULLIF(a.acteur_libelle, ''), t.p, a.acteur_libelle),
                    zones = CASE WHEN a.zones = '{{}}' AND t.z IS NOT NULL
                                 THEN string_to_array(t.z, ',') ELSE a.zones END,
                    description = COALESCE(a.description, {descr_utile})
                FROM unnest($1::text[], $2::text[], $3::jsonb[], $4::text[], $5::text[],
                            $6::text[])
                     AS t(u, c, j, p, z, d)
                WHERE a.uid = t.u
                "
                ),
                &[&uid, &cpv, &detail, &acheteur, &zones, &descr],
            )
            .await?)
    }

    /// Grafts onto rows already in stock what a re-read of their channel gives,
    /// without replaying the reconciliation: the identity of a market never
    /// depends on what its notice says beyond its title.
    #[tracing::instrument(name = "db.poser_contenu_annonces", skip(self, poses), fields(db.system = "postgresql", n = poses.len()))]
    pub async fn poser_contenu_annonces(&self, poses: &[ContenuPose]) -> Result<u64> {
        if poses.is_empty() {
            return Ok(0);
        }
        let source: Vec<&str> = poses.iter().map(|p| p.source_uid.as_str()).collect();
        let descr: Vec<Option<&str>> = poses.iter().map(|p| p.description.as_deref()).collect();
        let contenu: Vec<Json<&serde_json::Value>> =
            poses.iter().map(|p| Json(&p.contenu)).collect();
        let descr_utile = description_utile("t.d", "a.titre");
        Ok(self
            .conn
            .execute(
                &format!(
                    "
                UPDATE annonce a
                SET description = COALESCE({descr_utile}, a.description),
                    details = a.details || t.j
                FROM unnest($1::text[], $2::text[], $3::jsonb[]) AS t(s, d, j)
                     JOIN annonce_source src ON src.source_uid = t.s
                WHERE a.uid = src.annonce_uid
                  AND (a.description IS DISTINCT FROM COALESCE({descr_utile}, a.description)
                       OR NOT a.details @> t.j)
                "
                ),
                &[&source, &descr, &contenu],
            )
            .await?)
    }

    /// Graphies d'une dénomination d'acheteur à confronter au répertoire, en plus
    /// de celle que l'avis publie. Expose `v.cle`, déjà pliée.
    ///
    /// L'Insee aplatit la ponctuation — « DEPARTEMENT DE L ISERE », « SEINE
    /// MARITIME » — là où un avis signe « Département de l'Isère » et
    /// « Seine-Maritime ». Il abrège aussi les formes longues (« CC
    /// PLEYBEN-CHATEAULIN-PORZAY ») et nomme « COMMUNE DE X » ce que la mairie signe
    /// « Ville de X ». Les ponts s'appliquent après pliage, donc sur du minuscule
    /// sans accent — un motif accentué ne filtrerait rien.
    const VARIANTES_ACHETEUR: &'static str = r#"
        CROSS JOIN LATERAL (
          SELECT lj_fold(regexp_replace(a.acteur_libelle, '[''’\-]', ' ', 'g')) AS p
        ) AS n
        CROSS JOIN LATERAL (VALUES
          (n.p),
          (regexp_replace(n.p, '^(ville|mairie) ', 'commune ')),
          (regexp_replace(n.p, '^conseil (departemental|general) ', 'departement ')),
          (regexp_replace(n.p, '^conseil regional ', 'region ')),
          (regexp_replace(n.p, '^communaute de communes ', 'cc ')),
          (regexp_replace(n.p, '^communaute d agglomeration ', 'ca ')),
          (regexp_replace(n.p, '^communaute urbaine ', 'cu ')),
          (regexp_replace(n.p, '^(centre hospitalier|chu|chr) ', 'centre hospitalier '))
        ) AS v(cle)
    "#;

    /// Attaches rows without a buyer edge to an entity, by folded denomination,
    /// trying the spellings of [`VARIANTES_ACHETEUR`]. The folding is stored
    /// nowhere: `lj_fold`, the SQL twin of the Rust folding, computes it on both
    /// sides. A denomination borne by several entities attaches nothing
    /// (`count(DISTINCT uid) = 1`), and neither does a row whose variants name
    /// two different entities.
    ///
    /// The probe goes through the `&&` overlap operator on
    /// `lj_fold_all(denominations)`, the only shape that borrows the
    /// `entity_folded_all_idx` GIN: an `IN (…)` on the unfolded column
    /// recomputes `lj_fold_all` over every entity. The key array is passed
    /// as a parameter, so constant to the planner, and the population is
    /// restricted before the variants are expanded.
    ///
    /// `closes_depuis` widens the population to closed markets from that
    /// deadline on. The buyer edge is what makes the DECP channel exact, and
    /// the daily harvest only ever posts it on open rows.
    ///
    /// Either scope needs the caller to have lifted `statement_timeout`.
    #[tracing::instrument(name = "db.relier_annonces_par_denomination", skip(self), fields(db.system = "postgresql"))]
    pub async fn relier_annonces_par_denomination(
        &self,
        closes_depuis: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<u64> {
        let portee = |placeholder: &str| match closes_depuis {
            Some(_) => format!(
                "a.echeance >= {placeholder} \
                 AND EXISTS (SELECT 1 FROM annonce_source s WHERE s.annonce_uid = a.uid)"
            ),
            None => ouverte("a"),
        };
        let ouverte_a = portee("$1");
        let variantes = Self::VARIANTES_ACHETEUR;
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = closes_depuis
            .iter()
            .map(|d| d as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        let cles: Vec<String> = self
            .conn
            .query(
                &format!(
                    "SELECT DISTINCT v.cle \
                     FROM annonce a {variantes} \
                     WHERE {ouverte_a} AND a.acteur_libelle <> '' \
                       AND NOT EXISTS (SELECT 1 FROM annonce_entity e \
                                       WHERE e.annonce_uid = a.uid AND e.role = 'acheteur')"
                ),
                &params[..],
            )
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect();
        if cles.is_empty() {
            return Ok(0);
        }
        let ouverte_insert = portee("$2");
        let mut params_insert: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![&cles];
        params_insert.extend(
            closes_depuis
                .iter()
                .map(|d| d as &(dyn tokio_postgres::types::ToSql + Sync)),
        );
        Ok(self
            .conn
            .execute(
                &format!(
                    "
                    WITH cle AS MATERIALIZED (
                      SELECT DISTINCT c FROM unnest($1::text[]) AS c
                    ), noms AS (
                      SELECT e.uid, f.folded
                      FROM entity e,
                           LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded)
                      WHERE lj_fold_all(e.denominations) && $1::text[]
                        AND f.folded = ANY(SELECT c FROM cle)
                        AND e.uid NOT LIKE 'cnb:%' AND e.uid NOT LIKE 'oacc:%'
                    ), hit AS (
                      SELECT folded, min(uid) AS uid FROM noms
                      GROUP BY folded HAVING count(DISTINCT uid) = 1
                    ), candidate AS MATERIALIZED (
                      SELECT a.uid, a.acteur_libelle
                      FROM annonce a
                      WHERE {ouverte_insert} AND a.acteur_libelle <> ''
                        AND NOT EXISTS (SELECT 1 FROM annonce_entity e
                                        WHERE e.annonce_uid = a.uid AND e.role = 'acheteur')
                    ), lie AS (
                      SELECT a.uid AS annonce_uid, min(h.uid) AS entity_uid
                      FROM candidate a {variantes}
                      JOIN hit h ON h.folded = v.cle
                      GROUP BY a.uid HAVING count(DISTINCT h.uid) = 1
                    )
                    INSERT INTO annonce_entity (annonce_uid, entity_uid, role)
                    SELECT annonce_uid, entity_uid, 'acheteur' FROM lie
                    ON CONFLICT DO NOTHING
                    "
                ),
                &params_insert[..],
            )
            .await?)
    }

    #[tracing::instrument(name = "db.upsert_profils_acheteurs", skip(self, profils), fields(db.system = "postgresql", n = profils.len()))]
    pub async fn upsert_profils_acheteurs(
        &self,
        profils: &[(String, Option<String>, i32)],
    ) -> Result<u64> {
        if profils.is_empty() {
            return Ok(0);
        }
        let host: Vec<&str> = profils.iter().map(|(h, _, _)| h.as_str()).collect();
        let moteur: Vec<Option<&str>> = profils.iter().map(|(_, m, _)| m.as_deref()).collect();
        let avis: Vec<i32> = profils.iter().map(|(_, _, a)| *a).collect();
        Ok(self
            .conn
            .execute(
                "
                INSERT INTO profil_acheteur (host, moteur, avis_boamp, first_seen_at, last_seen_at)
                SELECT h, m, a, NOW(), NOW()
                FROM unnest($1::text[], $2::text[], $3::int[]) AS t(h, m, a)
                ON CONFLICT (host) DO UPDATE SET
                  -- Un moteur identifié n'est jamais effacé par un sondage muet
                  -- (hôte momentanément injoignable).
                  moteur = COALESCE(EXCLUDED.moteur, profil_acheteur.moteur),
                  avis_boamp = EXCLUDED.avis_boamp,
                  last_seen_at = NOW()
                ",
                &[&host, &moteur, &avis],
            )
            .await?)
    }

    /// Les hôtes recensés d'un moteur multi-tenant (Atexo, Safetender).
    /// `muets` borne les nuits de silence admises : `Some(n)` écarte l'hôte qui
    /// en compte `n` ou plus, `None` les reprend tous.
    #[tracing::instrument(name = "db.hotes_profil_acheteur", skip(self), fields(db.system = "postgresql"))]
    pub async fn hotes_profil_acheteur(
        &self,
        moteur: &str,
        muets: Option<i32>,
    ) -> Result<Vec<String>> {
        let seuil = muets.unwrap_or(i32::MAX);
        let rows = self
            .conn
            .query(
                "
                SELECT host FROM profil_acheteur
                WHERE moteur = $1 AND echecs_consecutifs < $2
                ORDER BY avis_boamp DESC, host
                ",
                &[&moteur, &seuil],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Les hôtes recensés qui n'ont plus rien rendu depuis `seuil` tentatives
    /// consécutives : entrées mortes du recensement, pas pannes du jour.
    #[tracing::instrument(name = "db.hotes_muets", skip(self), fields(db.system = "postgresql"))]
    pub async fn hotes_muets(&self, seuil: i32) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT host FROM profil_acheteur WHERE echecs_consecutifs >= $1",
                &[&seuil],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Enregistre le verdict d'une tentative par hôte : le compteur repart de
    /// zéro dès que l'hôte répond.
    #[tracing::instrument(name = "db.enregistrer_verdicts_hotes", skip_all, fields(db.system = "postgresql", n = verdicts.len()))]
    pub async fn enregistrer_verdicts_hotes(&self, verdicts: &[(String, bool)]) -> Result<u64> {
        if verdicts.is_empty() {
            return Ok(0);
        }
        let host: Vec<&str> = verdicts.iter().map(|(h, _)| h.as_str()).collect();
        let joignable: Vec<bool> = verdicts.iter().map(|(_, j)| *j).collect();
        Ok(self
            .conn
            .execute(
                "
                UPDATE profil_acheteur AS p
                SET echecs_consecutifs =
                      CASE WHEN v.joignable THEN 0 ELSE p.echecs_consecutifs + 1 END
                FROM unnest($1::text[], $2::bool[]) AS v(host, joignable)
                WHERE p.host = v.host
                ",
                &[&host, &joignable],
            )
            .await?)
    }

    /// Tous les hôtes recensés et classés par moteur — les plateformes avérées,
    /// validateurs du dépôt de dernier ressort. Jamais les hôtes non classés : un
    /// avis cite aussi le site institutionnel de son acheteur.
    #[tracing::instrument(name = "db.hotes_plateformes_classees", skip(self), fields(db.system = "postgresql"))]
    pub async fn hotes_plateformes_classees(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT host FROM profil_acheteur WHERE moteur IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// A notice also cites the institutional site of its buyer, so this list
    /// mixes the platforms missed with what is not one.
    #[tracing::instrument(name = "db.hotes_non_classes", skip(self), fields(db.system = "postgresql"))]
    pub async fn hotes_non_classes(&self, limite: i64) -> Result<Vec<(String, i32)>> {
        let rows = self
            .conn
            .query(
                "SELECT host, avis_boamp FROM profil_acheteur \
                 WHERE moteur IS NULL ORDER BY avis_boamp DESC, host LIMIT $1",
                &[&limite],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// The deposit hosts of open rows with no door to a consultation, heaviest
    /// first, with the count of rows each one holds. A host a notice names but
    /// that no notice cites as its own is absent from `profil_acheteur`, hence
    /// out of the census fed by the notices alone.
    #[tracing::instrument(name = "db.hotes_depot_sans_porte", skip(self), fields(db.system = "postgresql"))]
    pub async fn hotes_depot_sans_porte(
        &self,
        fond: &str,
        limite: i64,
    ) -> Result<Vec<(String, i32)>> {
        let publication: Vec<String> = lj_vocab::CANAUX_PUBLICATION
            .iter()
            .map(|c| (*c).to_string())
            .collect();
        let rows = self
            .conn
            .query(
                "WITH ouvertes AS ( \
                   SELECT a.uid, NULLIF(a.details->>'depot', '') AS depot, \
                          bool_or(el->>'moteur' <> ALL($3::text[]) \
                                  AND (el->>'avis' IS NOT NULL OR el->>'rc' IS NOT NULL \
                                       OR el->>'dce' IS NOT NULL)) AS porte \
                   FROM annonce a \
                   LEFT JOIN LATERAL jsonb_array_elements(a.details->'liens') el ON true \
                   WHERE a.fond = $1 AND a.echeance > NOW() \
                   GROUP BY a.uid, 2 \
                 ), hotes AS ( \
                   SELECT lower(split_part(split_part( \
                            regexp_replace(depot, '^https?://', ''), '/', 1), ':', 1)) AS host \
                   FROM ouvertes WHERE depot IS NOT NULL AND COALESCE(porte, false) = false \
                 ) \
                 SELECT h.host, count(*)::int FROM hotes h \
                 LEFT JOIN profil_acheteur a ON a.host = h.host AND a.moteur IS NOT NULL \
                 LEFT JOIN profil_acheteur b \
                   ON b.host = substring(h.host from position('.' in h.host) + 1) \
                      AND b.moteur IS NOT NULL \
                 WHERE h.host <> '' AND a.host IS NULL AND b.host IS NULL \
                 GROUP BY 1 ORDER BY 2 DESC, 1 LIMIT $2",
                &[&fond, &limite, &publication],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// A sample of links from the open rows, `par_moteur` at most for each
    /// engine and kind. Drawn at random: the probe runs every night, and what
    /// counts is the coverage over time rather than the repeatability of a run.
    #[tracing::instrument(name = "db.liens_a_sonder", skip(self), fields(db.system = "postgresql"))]
    pub async fn liens_a_sonder(
        &self,
        fond: &str,
        par_moteur: i64,
    ) -> Result<Vec<(String, String, String)>> {
        let rows = self
            .conn
            .query(
                "
                WITH liens AS (
                  SELECT e->>'moteur' AS moteur, k.kind, e->>k.kind AS url
                  FROM annonce a,
                       jsonb_array_elements(a.details->'liens') e,
                       unnest(ARRAY['avis','rc','dce']) AS k(kind)
                  WHERE a.fond = $1 AND a.echeance > NOW()
                    AND e->>k.kind IS NOT NULL AND e->>'moteur' IS NOT NULL
                ),
                tire AS (
                  SELECT moteur, kind, url,
                         row_number() OVER (PARTITION BY moteur, kind ORDER BY random()) AS n
                  FROM liens
                )
                SELECT moteur, kind, url FROM tire WHERE n <= $2 ORDER BY moteur, kind
                ",
                &[&fond, &par_moteur],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// A second pass on the same day replaces the first: this is a state, not a
    /// journal.
    #[tracing::instrument(name = "db.poser_sonde_liens", skip_all, fields(db.system = "postgresql", n = sondes.len()))]
    pub async fn poser_sonde_liens(&self, sondes: &[SondeLiens]) -> Result<u64> {
        if sondes.is_empty() {
            return Ok(0);
        }
        let moteur: Vec<&str> = sondes.iter().map(|s| s.moteur.as_str()).collect();
        let kind: Vec<&str> = sondes.iter().map(|s| s.kind.as_str()).collect();
        let verdicts: Vec<Json<&serde_json::Value>> =
            sondes.iter().map(|s| Json(&s.verdicts)).collect();
        let exemples: Vec<Json<&serde_json::Value>> =
            sondes.iter().map(|s| Json(&s.exemples)).collect();
        Ok(self
            .conn
            .execute(
                "
                INSERT INTO annonce_lien_sonde (moteur, kind, jour, verdicts, exemples)
                SELECT m, k, CURRENT_DATE, v, x
                FROM unnest($1::text[], $2::text[], $3::jsonb[], $4::jsonb[]) AS t(m, k, v, x)
                ON CONFLICT (moteur, kind, jour) DO UPDATE SET
                  verdicts = EXCLUDED.verdicts, exemples = EXCLUDED.exemples
                ",
                &[&moteur, &kind, &verdicts, &exemples],
            )
            .await?)
    }

    #[tracing::instrument(name = "db.tailles_tables_annonces", skip(self), fields(db.system = "postgresql"))]
    pub async fn tailles_tables_annonces(&self) -> Result<Vec<TableSize>> {
        let rows = self
            .conn
            .query(
                "SELECT t, pg_total_relation_size(t)::bigint \
                 FROM unnest(ARRAY['annonce','annonce_source','annonce_entity','profil_acheteur']) AS t",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| TableSize {
                table: r.get(0),
                bytes: r.get(1),
            })
            .collect())
    }

    #[tracing::instrument(name = "db.annonces", skip(self, f), fields(db.system = "postgresql", fond = %f.fond))]
    pub async fn annonces(&self, f: &AnnonceFiltre) -> Result<Vec<AnnonceRow>> {
        let mut filtre = Filtre::new(f, None);
        let limit = filtre.push(Box::new(f.limit));
        let offset = filtre.push(Box::new(f.offset));
        let sql = format!(
            "SELECT {ANNONCE_COLS} FROM annonce a {ANNONCE_JOINS} \
             WHERE a.fond = $1{} ORDER BY {} LIMIT ${limit} OFFSET ${offset}",
            filtre.clauses,
            order_by(f.ordre),
        );
        let rows = self.conn.query(&sql, &filtre.refs()).await?;
        Ok(rows.iter().map(annonce_from_row).collect())
    }

    #[tracing::instrument(name = "db.annonces_total", skip(self, f), fields(db.system = "postgresql"))]
    pub async fn annonces_total(&self, f: &AnnonceFiltre) -> Result<i64> {
        let filtre = Filtre::new(f, None);
        let sql = format!(
            "SELECT count(*)::bigint FROM annonce a WHERE a.fond = $1{}",
            filtre.clauses
        );
        let row = self.conn.query_one(&sql, &filtre.refs()).await?;
        Ok(row.get(0))
    }

    /// Comptes d'un axe de facette, son propre filtre retiré (multi-sélection).
    /// La valeur nulle (axe non renseigné par la source) n'est pas offerte au
    /// filtre, donc pas comptée.
    #[tracing::instrument(name = "db.annonces_facette", skip(self, f), fields(db.system = "postgresql", axe = axe.nom()))]
    pub async fn annonces_facette(
        &self,
        f: &AnnonceFiltre,
        axe: AnnonceAxe,
    ) -> Result<Vec<(String, Option<String>, i64)>> {
        let filtre = Filtre::new(f, Some(axe));
        let colonne = axe.colonne();
        // Les axes référentiels (`nature_marche:*`, `cpv:*`) tiennent leur libellé
        // et leur ordre éditorial de `facet_value` ; le département est un code brut,
        // son nom est résolu côté API.
        let sql = match axe {
            // The unzoned rows are counted under their own key instead of
            // vanishing with the unnest: a filter that hides without saying so
            // is the defect this axis was rebuilt to close.
            AnnonceAxe::Zone => format!(
                "SELECT COALESCE(z, '{ZONE_SANS}') AS cle, NULL::text AS label, \
                        count(*)::bigint AS n \
                 FROM annonce a LEFT JOIN LATERAL unnest({colonne}) z ON true \
                 WHERE a.fond = $1{} GROUP BY 1 ORDER BY 1",
                filtre.clauses
            ),
            AnnonceAxe::Metier => format!(
                "SELECT m AS cle, fv.label AS label, count(*)::bigint AS n \
                 FROM annonce a JOIN LATERAL unnest({colonne}) m ON true \
                 JOIN facet_value fv ON fv.uid = m \
                 WHERE a.fond = $1{} GROUP BY 1, 2, fv.sort ORDER BY fv.sort",
                filtre.clauses
            ),
            AnnonceAxe::Famille | AnnonceAxe::Cpv => format!(
                "SELECT {colonne} AS cle, fv.label AS label, count(*)::bigint AS n \
                 FROM annonce a JOIN facet_value fv ON fv.uid = {colonne} \
                 WHERE a.fond = $1{} GROUP BY 1, 2, fv.sort ORDER BY fv.sort",
                filtre.clauses
            ),
        };
        let rows = self.conn.query(&sql, &filtre.refs()).await?;
        Ok(rows
            .iter()
            .map(|r| (r.get("cle"), r.get("label"), r.get("n")))
            .collect())
    }

    /// Une annonce par son uid, closes comprises (la fiche d'une consultation
    /// clôturée reste servie : elle a été indexée, et c'est l'archive).
    #[tracing::instrument(name = "db.annonce", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonce(&self, uid: &str) -> Result<Option<AnnonceRow>> {
        let sql = format!("SELECT {ANNONCE_COLS} FROM annonce a {ANNONCE_JOINS} WHERE a.uid = $1");
        Ok(self
            .conn
            .query_opt(&sql, &[&uid])
            .await?
            .as_ref()
            .map(annonce_from_row))
    }

    /// Les canaux d'une annonce, le plus anciennement vu d'abord.
    #[tracing::instrument(name = "db.annonce_canaux", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonce_canaux(&self, uid: &str) -> Result<Vec<AnnonceCanalRow>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, url_avis, first_seen_at \
                 FROM annonce_source WHERE annonce_uid = $1 ORDER BY first_seen_at, source_uid",
                &[&uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceCanalRow {
                source_uid: r.get(0),
                url_avis: r.get(1),
                first_seen_at: r.get(2),
            })
            .collect())
    }

    #[tracing::instrument(name = "db.annonce_entites", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonce_entites(&self, uid: &str) -> Result<Vec<AnnonceEntiteRow>> {
        let rows = self
            .conn
            .query(
                "SELECT ae.entity_uid, ae.role, e.denomination \
                 FROM annonce_entity ae \
                 LEFT JOIN entity e ON e.uid = ae.entity_uid \
                 WHERE ae.annonce_uid = $1 ORDER BY ae.role, ae.entity_uid",
                &[&uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceEntiteRow {
                entity_uid: r.get(0),
                role: r.get(1),
                denomination: r.get(2),
            })
            .collect())
    }

    /// Les annonces ouvertes d'une entité, `par_role` au plus par rôle, par
    /// échéance. Le fond `attribution` échappe à la fenêtre — un fait constaté ne
    /// s'ouvre ni ne se ferme — et se lit par parution : c'est le palmarès
    /// « Marchés remportés ». Le total du rôle accompagne chaque ligne : la section
    /// de la fiche annonce « 12 marchés en cours » en n'en rendant que 5.
    #[tracing::instrument(name = "db.annonces_entite", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonces_entite(
        &self,
        entity_uid: &str,
        par_role: i64,
    ) -> Result<Vec<EntityAnnonceRow>> {
        let ouverte_an = ouverte("an");
        let sql = format!(
            "WITH arete AS (
               SELECT ae.role, ae.annonce_uid,
                      count(*) OVER (PARTITION BY ae.role) AS total_role,
                      row_number() OVER (
                        PARTITION BY ae.role
                        ORDER BY an.echeance ASC NULLS LAST,
                                 an.date_publication DESC, an.uid
                      ) AS rang
               FROM annonce_entity ae
               JOIN annonce an ON an.uid = ae.annonce_uid
               WHERE ae.entity_uid = $1
                 AND (an.fond = 'attribution' OR {ouverte_an})
             )
             SELECT arete.role, arete.total_role, {ANNONCE_COLS}
             FROM arete
             JOIN annonce a ON a.uid = arete.annonce_uid
             {ANNONCE_JOINS}
             WHERE arete.rang <= $2
             ORDER BY arete.role, arete.rang"
        );
        let rows = self.conn.query(&sql, &[&entity_uid, &par_role]).await?;
        Ok(rows
            .iter()
            .map(|r| EntityAnnonceRow {
                role: r.get("role"),
                total_role: r.get("total_role"),
                annonce: annonce_from_row(r),
            })
            .collect())
    }

    /// Les uids à publier au sitemap : la fenêtre ouverte et l'archive récente,
    /// avec la date de parution en `lastmod` — la seule date stable de la ligne. La
    /// récence se lit sur l'horloge la mieux connue : l'échéance, sinon la parution.
    #[tracing::instrument(name = "db.annonces_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonces_sitemap(
        &self,
        fond: &str,
        archive_jours: i32,
    ) -> Result<Vec<(String, chrono::NaiveDate)>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, date_publication FROM annonce \
                 WHERE fond = $1 \
                   AND COALESCE(echeance, date_publication::timestamptz) \
                       >= NOW() - make_interval(days => $2) \
                 ORDER BY uid",
                &[&fond, &archive_jours],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Les uids encore ouverts apparus depuis `since`, pour la soumission
    /// IndexNow — critère « première vue tous canaux ».
    ///
    /// La borne d'échéance sépare les deux canaux d'indexation : IndexNow est la
    /// voie rapide de ce qui a une date de péremption, le sitemap couvre le reste
    /// jusqu'à 180 jours.
    #[tracing::instrument(
        name = "db.annonces_vues_depuis",
        skip(self),
        fields(db.system = "postgresql")
    )]
    pub async fn annonces_vues_depuis(
        &self,
        fond: &str,
        since: chrono::DateTime<chrono::Utc>,
        limit: i64,
    ) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT a.uid FROM annonce a \
                 JOIN annonce_source s ON s.annonce_uid = a.uid \
                 WHERE a.fond = $1 AND a.echeance > now() \
                 GROUP BY a.uid \
                 HAVING min(s.first_seen_at) >= $2 \
                 ORDER BY min(s.first_seen_at) DESC \
                 LIMIT $3",
                &[&fond, &since, &limit],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        acheteur_unique, enrichment_order_by, ouverte, AnnonceAxe, AnnonceFiltre,
        FileEnrichissement, Filtre, ZONE_SANS,
    };

    fn build(poser: impl FnOnce(&mut AnnonceFiltre)) -> AnnonceFiltre {
        let mut f = AnnonceFiltre::nouveau("marche");
        poser(&mut f);
        f
    }

    #[test]
    fn the_living_window_is_the_default_and_closes_opts_out() {
        let ouvert = Filtre::new(&AnnonceFiltre::nouveau("marche"), None);
        assert_eq!(ouvert.clauses, format!(" AND {}", ouverte("a")));
        assert_eq!(ouvert.params.len(), 1);

        let f = build(|f| f.closes = true);
        assert_eq!(Filtre::new(&f, None).clauses, "");
    }

    #[test]
    fn the_placeholders_are_numbered_in_the_order_the_clauses_are_posed() {
        let f = build(|f| {
            f.q = Some("PLU".into());
            f.zones = vec!["69".into()];
            f.familles = vec!["nature_marche:TRAVAUX".into()];
            f.cpv = vec!["cpv:45".into()];
            f.echeance_jours = Some(7);
        });
        let filtre = Filtre::new(&f, None);
        assert_eq!(filtre.params.len(), 6);
        assert!(filtre.clauses.contains("paradedb.match('titre', $2"));
        assert!(filtre
            .clauses
            .contains("paradedb.match('acteur_libelle', $2"));
        assert!(filtre.clauses.contains("a.zones && $3::text[]"));
        assert!(filtre.clauses.contains("a.famille_uid = ANY($4::text[])"));
        assert!(filtre.clauses.contains("a.cpv_uid = ANY($5::text[])"));
        assert!(filtre.clauses.contains("make_interval(days => $6)"));
    }

    #[test]
    fn sauf_drops_its_axis_and_renumbers_the_ones_behind_it() {
        let f = build(|f| {
            f.zones = vec!["69".into()];
            f.familles = vec!["nature_marche:TRAVAUX".into()];
            f.cpv = vec!["cpv:45".into()];
        });
        let filtre = Filtre::new(&f, Some(AnnonceAxe::Zone));
        assert_eq!(filtre.params.len(), 3);
        assert!(!filtre.clauses.contains("a.zones"));
        assert!(filtre.clauses.contains("a.famille_uid = ANY($2::text[])"));
        assert!(filtre.clauses.contains("a.cpv_uid = ANY($3::text[])"));
    }

    #[test]
    fn every_axis_filters_on_its_own_column() {
        for axe in [
            AnnonceAxe::Zone,
            AnnonceAxe::Famille,
            AnnonceAxe::Cpv,
            AnnonceAxe::Metier,
        ] {
            let f = build(|f| {
                let valeurs = vec!["x".to_string()];
                match axe {
                    AnnonceAxe::Zone => f.zones = valeurs,
                    AnnonceAxe::Famille => f.familles = valeurs,
                    AnnonceAxe::Cpv => f.cpv = valeurs,
                    AnnonceAxe::Metier => f.metiers = valeurs,
                }
            });
            let pose = Filtre::new(&f, None).clauses;
            assert!(pose.contains(axe.colonne()), "{} : {pose}", axe.nom());
            let retire = Filtre::new(&f, Some(axe)).clauses;
            assert!(!retire.contains(axe.colonne()), "{} : {retire}", axe.nom());
        }
    }

    /// Nature and division are the two levels of one facet: counting either
    /// through the other collapses the roots the other divisions hang from, and
    /// the list reorders under the hand that clicked it.
    #[test]
    fn the_nature_and_the_division_leave_each_other_out_of_their_counts() {
        let f = build(|f| {
            f.familles = vec!["nature_marche:TRAVAUX".into()];
            f.cpv = vec!["cpv:45".into()];
            f.zones = vec!["973".into()];
        });
        for axe in [AnnonceAxe::Famille, AnnonceAxe::Cpv] {
            let clauses = Filtre::new(&f, Some(axe)).clauses;
            assert!(
                !clauses.contains("a.famille_uid"),
                "{} : {clauses}",
                axe.nom()
            );
            assert!(!clauses.contains("a.cpv_uid"), "{} : {clauses}", axe.nom());
            assert!(clauses.contains("a.zones"), "{} : {clauses}", axe.nom());
        }
    }

    #[test]
    fn asking_for_the_unzoned_rows_reaches_the_empty_arrays() {
        let f = build(|f| f.zones = vec![ZONE_SANS.into(), "69".into()]);
        assert!(Filtre::new(&f, None)
            .clauses
            .contains("(a.zones && $2::text[] OR cardinality(a.zones) = 0)"));
    }

    #[test]
    fn a_blank_q_poses_no_clause() {
        let f = build(|f| f.q = Some("   ".into()));
        assert_eq!(Filtre::new(&f, None).params.len(), 1);
    }

    #[test]
    fn the_spliced_fragments_honour_the_alias_they_are_given() {
        let o = ouverte("an");
        assert!(o.contains("an.echeance > NOW()"));
        assert!(o.contains("s.annonce_uid = an.uid"));
        assert!(acheteur_unique("an").contains("ae.annonce_uid = an.uid"));
    }

    #[test]
    fn only_the_unvisited_queue_is_served_by_arrival() {
        let unvisited = enrichment_order_by(FileEnrichissement::NonVisitees);
        assert!(unvisited.contains("min(v.first_seen_at)"));
        assert!(unvisited.ends_with("DESC"));
        for file in [
            FileEnrichissement::SansAcheteur,
            FileEnrichissement::SansZone,
            FileEnrichissement::SansContenu,
        ] {
            assert_eq!(enrichment_order_by(file), "a.echeance");
        }
    }
}
