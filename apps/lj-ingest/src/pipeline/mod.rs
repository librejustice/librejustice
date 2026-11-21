//! Pipelines d'ingestion.
//!
//! L'orchestration complète vit ici : parse → triage idempotent → chunk →
//! extract → (embed) → upsert.
//!
//! - CPU (parse + chunk + extract) parallélisé via **rayon** ;
//! - I/O DB / embed en **tokio** ;
//! - batch de [`BATCH_SIZE`], idempotence via `content_checksum` (xxh3-64 du
//!   payload brut).
//!
//! Two schedulings over one code path: `run_stream` (decoupled stages, the embed
//! sets the pace) for the volume sources, `drain_batch` (sequential) for maintenance.

mod a2aj;
mod adlc;
mod affiliations;
mod amf;
mod annonces;
mod annonces_couverture;
mod applicability_events;
mod ariane;
mod article_commentaires;
mod article_text;
mod assertions;
mod attract;
mod backfill;
mod batch;
mod bofip;
mod boss;
mod bulletins;
mod cc_site;
mod cce;
mod ce_list;
mod cedh_application;
mod cedh_link;
mod chronicle_move;
mod circulaires;
mod cnb_history;
mod cnb_reduced;
mod cnb_rekey;
mod cnda;
mod cnda_solution;
mod cnil_doctrine;
mod comeu;
mod conclusions;
mod consiliaweb;
mod corpus_toc;
mod court_sites;
mod court_summaries;
mod ddd;
mod decision_text;
mod dila;
mod dila_judi;
mod doctrine_names;
mod doctrine_oai;
mod embed;
mod embed_missing;
mod embed_norms;
mod entscheidsuche;
mod entscheidsuche_rekey;
mod eu_bodies;
pub(crate) mod eu_catalog;
mod eu_law;
mod eu_rproc;
mod europe;
mod expiry;
mod false_merges;
mod files;
mod formations;
mod gazette;
mod hal;
mod identity_probe;
mod jorf;
mod jorf_pieces;
mod jorf_publication;
mod juportal;
mod juradm_site;
mod juricaf;
mod jurisdiction_seed;
mod justice_lu;
mod kali;
mod language_pairs;
mod legal_corpus;
mod legalis;
mod legi;
mod legimonaco;
mod move_to_journal;
mod norms;
mod opencaselaw;
mod opendata;
mod parties;
mod precompute;
mod prepare;
mod quantum;
mod questions;
mod raa;
mod rechtspraak;
mod reconcile;
mod reextract;
mod registries;
mod rekey;
mod roles;
mod scc_csc;
mod scope_alsace;
mod scope_compteurs;
mod seats;
mod service_public;
mod slugs;
pub mod submissions;
mod suggest_build;
mod text_refs;
mod text_rename;
mod text_stamps;
mod toc_paths;
mod treaty_articles;
mod treaty_bodies;
mod treaty_parties;
mod union_filing;
mod wiki_names;

pub use a2aj::sync_a2aj;
pub use adlc::sync_adlc;
pub use affiliations::derive_affiliations;
pub use amf::sync_amf;
pub use annonces::{
    annonces, annonces_backfill, annonces_contenu, annonces_decp, annonces_discover,
    annonces_enrichir, annonces_jumeler, annonces_regulariser, annonces_resoudre,
    annonces_sonder_liens,
};
pub use annonces_couverture::annonces_couverture;
pub use ariane::sync_ariane;
pub use article_commentaires::{reconcile_article_commentaires, seed_article_commentaires};
pub use article_text::{backfill_article_text, ArticleFond};
pub use assertions::{assertions_compile, assertions_control, assertions_coverage, CompileOptions};
pub use backfill::{
    absorb_copies, backfill_canonical_ref, backfill_ecli, merge_cross_source_duplicates,
};
pub use bofip::{ingest_bofip, sync_bofip};
pub use boss::sync_boss;
pub use bulletins::{blank_bulletins_ocr, rebuild_bulletins_ocr, sync_bulletins};
pub use cc_site::sync_cc_site;
pub use cce::sync_cce;
pub use ce_list::sync_ce_list;
pub use chronicle_move::move_chronicle;
pub use circulaires::{
    blank_circulaires_ocr, rebuild_circulaires_ocr, sync_circulaires, sync_circulaires_bodies,
};
pub use cnb_history::replay_cnb_history;
pub use cnb_rekey::rekey_cnb;
pub use cnda::{ingest_cnda, sync_cnda};
pub use cnda_solution::backfill_cnda_solution;
pub use comeu::sync_comeu;
pub use conclusions::{conclusions_authors, page_cjue_conclusions, sync_ce_conclusions};
pub use consiliaweb::{rewrite_consiliaweb, sync_consiliaweb};
pub use court_sites::sync_court_sites;
pub use court_summaries::sync_court_summaries;
pub use ddd::{ingest_ddd, sync_ddd};
pub use decision_text::{
    annex_starts, backfill_decision_text, headnote_tags, repair_texts, TextFond, TextRepair,
};
pub use dila::{ingest_dila, sync_dila, Fond};
pub use doctrine_names::{sync_doctrine_names, DoctrineNamesScan};
pub use doctrine_oai::{sync_doctrine_oai, DoctrineScan};
pub use embed_missing::{embed_missing, parse_shard};
pub use embed_norms::embed_norms;
pub use entscheidsuche::sync_entscheidsuche;
pub use entscheidsuche_rekey::{entscheidsuche_rekey, Pairing as RekeyPairing};
pub use eu_bodies::fill_eu_bodies;
pub(crate) use eu_catalog::{canonical_of, datasets_a_retirer};
pub use eu_catalog::{ingest_eu_catalog, merge_eu_swapped_uids, sync_eu_catalog, sync_eu_jorf};
pub use eu_law::sync_eu_law;
pub use eu_rproc::ingest_eu_rproc;
pub use europe::{
    backfill_cjue_joined_cases, backfill_cjue_titles, cache_cedh, ingest_cedh, ingest_cjue,
    sync_cedh, sync_cedh_stubs, sync_cjue,
};
pub use expiry::expire_named;
pub use false_merges::analyze_false_merges;
pub use formations::resolve_formations;
pub use gazette::{gazette, parse_gazette_source, GazetteMode};
pub use hal::{sync_hal, HalScan};
pub use identity_probe::probe_identity;
pub use jorf::{ingest_jorf, sync_jorf};
pub use jorf_pieces::backfill_jorf_pieces;
pub use jorf_publication::backfill_jorf_publications;
pub use juportal::sync_juportal;
pub use juradm_site::sync_juradm_site;
pub use juricaf::{juricaf_probe, merge_juricaf_twins, sync_juricaf};
pub use jurisdiction_seed::seed_jurisdictions;
pub use justice_lu::sync_justice_lu;
pub use kali::{ingest_kali, requalifier_kali, sync_kali};
pub use language_pairs::merge_language_pairs;
pub use legal_corpus::{
    canonicalize_source_labels, load_legal_corpus, reading_ranks, relabel_sources, stamp_freshness,
    write_text_aliases,
};
pub use legalis::sync_legalis;
pub use legi::{
    audit_legi_versions, backfill_links, backfill_textes, backfill_toc, drop_legi_placeholders,
    ingest_legi, redate_dila_versions, sync_legi,
};
pub use legimonaco::sync_legimonaco;
pub use move_to_journal::{move_to_journal, MoveSelection};
pub use norms::{sync_norms, NormsCountry, NormsScan};
pub use opencaselaw::sync_opencaselaw;
pub use opendata::{ingest_judilibre, ingest_opendata, refetch_judilibre};
pub use parties::relink_parties;
pub use precompute::{precompute_all, resync_corpus_counts, resync_jurisdiction_counts};
pub use quantum::{extract_quantum, load_quantum_gold, load_quantum_readings};
pub use questions::{sync_questions, QuestionSource};
pub use raa::{blank_raa_ocr, rebuild_raa_ocr, sync_raa};
pub use rechtspraak::sync_rechtspraak;
pub use reconcile::{reconcile_all, resolve_pending_citations};
pub use reextract::{reextract_fields, Worklist};
pub use registries::{load_registries, RegistrySource};
pub use rekey::{destar_article_keys, rekey_article_keys, rekey_identity_keys};
pub use roles::backfill_text_roles;
pub use scc_csc::sync_scc_csc;
pub use scope_alsace::scope_backfill_alsace;
pub use scope_compteurs::scope_backfill_compteurs;
pub use seats::resolve_seats;
pub use service_public::sync_service_public;
pub use slugs::{assign_slugs, drop_country_suffix, remint_text_slugs};
pub use suggest_build::build_suggest;
pub use text_refs::extract_text_refs;
pub use text_rename::rename_texts;
pub use text_stamps::stamp_texts;
pub use toc_paths::audit_toc_paths;
pub use treaty_articles::treaty_articles;
pub use treaty_bodies::backfill_treaty_bodies;
pub use treaty_parties::treaty_parties;
pub use union_filing::refile_union_texts;
pub use wiki_names::sync_wiki_names;

use lj_core::decision::Decision;
use lj_extract::link::{CatalogText, LinkSnapshot};
use lj_pg::db::Connection;
use lj_store::repository::{DecisionRepository, ExtractedFields};

pub const BATCH_SIZE: usize = 128;

/// Contexte d'extraction du run : snapshot catalogue pour le linker in-pass +
/// vocabulaire compilé (index de snap des citations). Hydraté à la première
/// décision extraite du process, immuable ensuite — le lien est une fonction du
/// catalogue **au moment de la passe**, et la passe intégrale hebdomadaire rejoue
/// tout le fonds sur le catalogue frais.
pub(crate) struct ExtractCtx {
    pub link: LinkSnapshot,
    pub vocab: lj_extract::compiled::CompiledVocab,
    /// Mapping (type, ville) → code de localisation pour les clés pendantes de
    /// chronologie, dérivé du référentiel `jurisdiction`.
    pub chrono: lj_extract::chrono::ChronoSnapshot,
    /// Labels guéris du référentiel `jurisdiction` (code → label avec ville), pour
    /// composer `search_title` quand le libellé source est nu.
    pub jur_labels: std::collections::HashMap<String, String>,
    /// Source code to canonical code, the resolution `ensure_jurisdictions` runs
    /// at write time: the seat paths are derived before it.
    pub jur_codes: std::collections::HashMap<String, String>,
    pub jur_sources: std::collections::HashMap<String, String>,
    pub successors: std::collections::HashMap<String, String>,
    /// Libellés du référentiel `chamber`, keyés `(code source, clé)` comme le
    /// reste des snapshots : le siège d'un titre les lit, il ne les recompose pas.
    /// Usage names, for the citations a body makes by name. Set by the renvois
    /// projection, so as fresh as it: an extraction pass before the projection
    /// cites under stale names.
    pub case_names: lj_extract::case_link::CaseNameSnapshot,
    /// What each court calls its rooms, read from the seat as WRITTEN.
    /// Empty on the pass that first writes the seats, so nothing resolves
    /// before a second one has a corpus to read.
    pub seats: lj_extract::seat_link::SeatVocabulary,
    /// Source code to the court's place.
    pub places: std::collections::HashMap<String, String>,
}

impl ExtractCtx {
    pub fn snapshots(&self) -> lj_extract_rows::Snapshots<'_> {
        lj_extract_rows::Snapshots {
            link: &self.link,
            vocab: &self.vocab,
            chrono: &self.chrono,
            jur_codes: &self.jur_codes,
            jur_sources: &self.jur_sources,
            successors: &self.successors,
            case_names: &self.case_names,
            seats: &self.seats,
            places: &self.places,
        }
    }
}

static EXTRACT_CTX: tokio::sync::OnceCell<ExtractCtx> = tokio::sync::OnceCell::const_new();

/// Contexte du run (hydraté une fois par process, partagé entre workers).
pub(crate) async fn extract_ctx(conn: &Connection) -> anyhow::Result<&'static ExtractCtx> {
    EXTRACT_CTX
        .get_or_try_init(|| async {
            let repo = DecisionRepository::new(conn);
            let texts: Vec<CatalogText> = repo
                .link_catalog_texts()
                .await?
                .into_iter()
                .map(CatalogText::from_row)
                .collect();
            let articles = repo.link_catalog_articles().await?;
            let (n_texts, n_articles) = (texts.len(), articles.len());
            let link = LinkSnapshot::build(texts.clone(), articles)
                .with_aliases(repo.link_catalog_aliases().await?);
            let vocab = lj_extract::compiled::CompiledVocab::build(&texts, &link);
            let jurisdictions = repo.load_jurisdictions().await?;
            let places = jurisdictions
                .iter()
                .filter_map(|j| Some((j.source_code.clone(), j.place.clone()?)))
                .collect();
            // `jur_labels` and `jur_codes` are keyed by `source_code`, the key
            // the extraction holds; the gazetteer returns the canonical code,
            // the one `decisions.jurisdiction_code` carries and every address
            // joins on.
            let jur_labels = jurisdictions
                .iter()
                .map(|j| (j.source_code.clone(), j.label.clone()))
                .collect();
            let jur_codes = jurisdictions
                .iter()
                .map(|j| (j.source_code.clone(), j.code.clone()))
                .collect();
            let jur_sources = lj_extract_rows::court_sources(&jurisdictions);
            let successors: std::collections::HashMap<String, String> =
                repo.jurisdiction_successors().await?.into_iter().collect();
            let chrono = lj_extract::chrono::ChronoSnapshot::with_closed(
                jurisdictions
                    .into_iter()
                    .filter_map(|j| Some((j.code, j.jurisdiction_type, j.city?))),
                successors.keys().cloned().collect(),
            );
            let case_names = lj_extract_rows::case_name_snapshot(repo.named_cases().await?);
            let seats = lj_extract::seat_link::SeatVocabulary::new(
                repo.seat_vocabulary()
                    .await?
                    .into_iter()
                    .map(|s| lj_extract::seat_link::SeatRow {
                        seat: s.seat,
                        chamber: s.chamber,
                        formation: s.formation,
                        span: s.span,
                    }),
            );
            tracing::info!(n_texts, n_articles, "extraction context hydrated");
            Ok(ExtractCtx {
                link,
                vocab,
                chrono,
                jur_labels,
                jur_codes,
                jur_sources,
                successors,
                case_names,
                seats,
                places,
            })
        })
        .await
}

/// Mode de triage à l'ingest.
///
/// - [`IngestMode::MissingHash`] (défaut) : skip si le hash est identique, et
///   fast-skip d'un fichier entièrement ingéré via le manifeste.
/// - [`IngestMode::All`]: rereads everything, **ignoring hash and manifest**, and
///   rewrites every existing decision, after a change of the pure pipeline. A
///   decision whose text and header come out unchanged keeps its passages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum IngestMode {
    MissingHash,
    All,
}

/// Calcule le checksum source (xxh3-64 hex du payload brut) pour
/// l'idempotence.
///
/// Le hash porte sur le payload **brut** (XML opendata ou ligne JSON Judilibre),
/// jamais sur le texte nettoyé.
pub fn content_checksum(payload: &[u8]) -> String {
    let digest = xxhash_rust::xxh3::xxh3_64(payload);
    format!("{digest:016x}")
}

/// Génère un `public_id` URL-safe base64 sur 9 octets. Aléatoire (non
/// déterministe).
pub(super) fn generate_public_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // 9 octets pseudo-aléatoires → 12 chars base64 urlsafe (sans padding requis).
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut state = nanos as u64 ^ (std::process::id() as u64).rotate_left(32);
    let mut bytes = [0u8; 9];
    for b in bytes.iter_mut() {
        // splitmix64 — suffisant pour un id opaque non-deviné.
        state = state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;
        *b = (z & 0xFF) as u8;
    }
    base64_urlsafe(&bytes)
}

/// Encodage base64 URL-safe (alphabet `-_`, sans padding). 9 octets → 12
/// caractères.
fn base64_urlsafe(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(n & 63) as usize] as char);
        }
    }
    out
}

/// Mode d'écriture d'un candidat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteMode {
    Full,
    /// Rewrites the decision and its provenance, and keeps its passages and
    /// their embeddings: the text they are cut from has not changed.
    KeepPassages,
    SourceXmlOnly,
}

impl WriteMode {
    fn as_str(self) -> &'static str {
        match self {
            WriteMode::Full => "full",
            WriteMode::KeepPassages => "keep_passages",
            WriteMode::SourceXmlOnly => "source_xml_only",
        }
    }
}

/// Candidat issu du parse d'un payload.
struct Candidate {
    decision_id: Option<i64>,
    public_id: String,
    decision: Decision,
    content_checksum: String,
    raw_payload: Vec<u8>,
    payload_format: String,
    write_mode: WriteMode,
    /// `false` for a text its decision is not read in: its write leaves the
    /// passages alone.
    passages: bool,
    /// See [`BulkDecisionWrite::distinct`].
    distinct: bool,
    /// `source_fields` déjà construits (`payload_format == "html"`, CEDH/CJUE) :
    /// les métadonnées HUDOC/CDM verbatim (colonnes/prédicats) plus les sections
    /// rebasées sur `full_text`. `None` pour les formats dont `prepare_write`
    /// reconstruit `source_fields` depuis `raw_payload`.
    prebuilt_source_fields: Option<serde_json::Value>,
    /// Champs structurés déjà extraits par le parser pur (CNDA) : `extract::routed`
    /// ne route que les 7 ordres FR ; un fond scrapé hors nomenclature
    /// opendata/Judilibre porte donc ses propres champs (solution_uid, date_lecture…)
    /// calculés par `parse_cnda`. `None` → `prepare_write` passe par l'extracteur
    /// routé `ExtractedFields::from_decision`.
    prebuilt_extracted: Option<ExtractedFields>,
}

/// A decision prepared: cleaned and rebuilt from its `source_fields`. The
/// structured extraction comes after (`extract_write`), the only stage that
/// needs the extraction context.
struct PreparedDecision {
    decision_id: Option<i64>,
    public_id: String,
    decision: Decision,
    content_checksum: String,
    write_mode: WriteMode,
    /// See [`Candidate::passages`].
    passages: bool,
    payload_format: String,
    /// Filled by `extract_write`, `None` out of the prepare stage.
    extracted: Option<ExtractedFields>,
    /// Champs préconstruits par le parser source (CNDA, ArianeWeb…), superposés
    /// au résultat de l'extracteur routé par `extract_write`.
    prebuilt_extracted: Option<ExtractedFields>,
    /// Payload moins le texte, offsets rebasés sur `full_text`.
    source_fields: serde_json::Value,
    distinct: bool,
}

/// Cumul des compteurs d'un run.
#[derive(Debug, Default, Clone, Copy)]
struct IngestCounts {
    created: usize,
    updated: usize,
    skipped: usize,
    errors: usize,
    empty_skipped: usize,
    passages_dropped: usize,
    passages_kept: usize,
    dedup_in_batch: usize,
    copies: usize,
    held: usize,
}

impl IngestCounts {
    fn merge(&mut self, other: &IngestCounts) {
        self.created += other.created;
        self.updated += other.updated;
        self.skipped += other.skipped;
        self.errors += other.errors;
        self.empty_skipped += other.empty_skipped;
        self.passages_dropped += other.passages_dropped;
        self.passages_kept += other.passages_kept;
        self.dedup_in_batch += other.dedup_in_batch;
        self.copies += other.copies;
        self.held += other.held;
    }
}

/// Helpers de test partagés entre les sous-modules de `pipeline`.
#[cfg(test)]
mod tests_support {
    use lj_core::decision::Decision;

    /// Decision minimale pour les tests (pas de `Default` sur `Decision`).
    pub(crate) fn test_decision(uid: &str) -> Decision {
        Decision {
            source_uid: uid.to_string(),
            member_name: uid.to_string(),
            ecli: None,
            jurisdiction_source_code: None,
            chamber: None,
            nac: None,
            jurisdiction_name: None,
            jurisdiction_type: Some("ta".to_string()),
            jurisdiction_location: None,
            numero_dossier: None,
            numero_dossiers: None,
            numero_role: None,
            date_lecture: None,
            date_audience: None,
            date_mise_jour: None,
            formation: None,
            type_decision: None,
            type_recours: None,
            solution: None,
            publication_codes: vec![],
            avocat_requerant: None,
            listed_counsel: Vec::new(),
            texte_integral_raw: String::new(),
            texte_integral_clean: String::new(),
            sections: vec![],
            metadata_header: String::new(),
            themes: Vec::new(),
            attacked: None,
            parse_warnings: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_checksum_is_xxh3_64_hex() {
        // xxh3_64(b"") = 0x2d06800538d394c2 (vecteur de référence xxHash).
        assert_eq!(content_checksum(b""), "2d06800538d394c2");
        let h = content_checksum(b"hello world");
        assert_eq!(h.len(), 16);
        assert_eq!(h, content_checksum(b"hello world"));
        assert_ne!(h, content_checksum(b"hello worlD"));
    }

    #[test]
    fn base64_urlsafe_matches_python() {
        let bytes: [u8; 9] = [0, 1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(base64_urlsafe(&bytes), "AAECAwQFBgcI");
        assert_eq!(base64_urlsafe(&[0xff, 0xff, 0xff]), "____");
        assert_eq!(base64_urlsafe(&[0xfb, 0xff, 0xfe]), "-__-");
    }

    #[test]
    fn public_id_is_12_urlsafe_chars() {
        let id = generate_public_id();
        assert_eq!(id.len(), 12);
        assert!(id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }
}
