//! Pipelines d'ingestion (port de `librejustice-store/pipelines/*` +
//! orchestration CLI de `apps/ingest/.../cli.py`).
//!
//! Le contrat Rust replie l'orchestration complète ici (le Python la split entre
//! `librejustice-store/pipelines/ingest*.py` et `cli.py`). On reproduit la
//! logique métier — parse → triage idempotent → chunk → extract → (embed) →
//! upsert — sans la machinerie d'orchestration Python (`asyncio.Queue` à trois
//! étages, `ProcessPoolExecutor`). Côté Rust :
//!
//! - CPU (parse + chunk + extract) parallélisé via **rayon** (sans GIL) ;
//! - I/O DB / embed en **tokio** ;
//! - batch de [`BATCH_SIZE`], idempotence via `content_checksum` (xxh3-64 du
//!   payload brut, règle #7).
//!
//! Le pipeline streaming concurrent (embed batch N+1 pendant l'écriture du batch
//! N) du Python n'est pas reporté tel quel : on traite batch par batch
//! (prepare rayon → embed → write). Cf. `unresolved`.

mod adde;
mod ariane;
mod article_commentaires;
mod backfill;
mod batch;
mod bofip;
mod circulaires;
mod cnda;
mod corpus_toc;
mod dila;
mod embed;
mod embed_missing;
mod eu_catalog;
mod eu_rproc;
mod europe;
mod false_merges;
mod files;
mod jorf;
mod kali;
mod legal_corpus;
mod legi;
mod opendata;
mod parties;
mod prepare;
mod purge_citations;
mod reconcile;
mod reextract;
mod registries;
mod rekey;
mod resplit;
mod roles;
mod slugs;
mod suggest_build;
mod text_refs;
mod treaty_bodies;
mod unmerge;
mod unmerge_dila;

pub use adde::sync_adde;
pub use ariane::sync_ariane;
pub use article_commentaires::seed_article_commentaires;
pub use backfill::{backfill_canonical_ref, backfill_ecli, merge_cross_source_duplicates};
pub use bofip::{ingest_bofip, sync_bofip};
pub use circulaires::{sync_circulaires, sync_circulaires_bodies};
pub use cnda::{ingest_cnda, sync_cnda};
pub use dila::{ingest_dila, sync_dila, Fond};
pub use embed_missing::embed_missing;
pub use eu_catalog::ingest_eu_catalog;
pub use eu_rproc::ingest_eu_rproc;
pub use europe::{cache_cedh, ingest_cedh, ingest_cjue, sync_cedh, sync_cjue};
pub use false_merges::analyze_false_merges;
pub use jorf::{ingest_jorf, sync_jorf};
pub use kali::{ingest_kali, sync_kali};
pub use legal_corpus::{
    canonicalize_source_labels, load_legal_corpus, relabel_sources, stamp_freshness,
};
pub use legi::{backfill_links, backfill_textes, backfill_toc, ingest_legi, sync_legi};
pub use opendata::{ingest_judilibre, ingest_opendata, refetch_judilibre, reingest_stale_opendata};
pub use parties::{backfill_parties, relink_parties};
pub use purge_citations::purge_procedural_citations;
pub use reconcile::reconcile_pending;
pub use reextract::reextract_fields;
pub use registries::{load_registries, RegistrySource};
pub use rekey::{rekey_article_keys, rekey_identity_keys};
pub use resplit::resplit_false_merges;
pub use roles::backfill_text_roles;
pub use slugs::assign_slugs;
pub use suggest_build::build_suggest;
pub use text_refs::extract_text_refs;
pub use treaty_bodies::backfill_treaty_bodies;
pub use unmerge::unmerge_same_source;
pub use unmerge_dila::unmerge_same_source_dila;

use lj_core::decision::Decision;
use lj_core::parsing::DilaFond;
use lj_extract::link::{CatalogText, LinkSnapshot};
use lj_store::db::Connection;
use lj_store::repository::{DecisionRepository, ExtractedFields};

use crate::chunking::Chunk;

/// Taille de batch (port de `_BATCH_SIZE = 128`).
pub const BATCH_SIZE: usize = 128;

/// Contexte d'extraction du run (ADR 0145 / 0156) : snapshot catalogue pour le
/// linker in-pass (~155 k textes + ~2 M paires d'articles) + vocabulaire
/// compilé (index de snap des citations). Hydraté à la première décision
/// extraite du process, immuable ensuite — le lien est une fonction du
/// catalogue **au moment de la passe**, la passe intégrale du dimanche rejoue
/// tout le fonds sur le catalogue frais.
pub(crate) struct ExtractCtx {
    pub link: LinkSnapshot,
    pub vocab: lj_extract::compiled::CompiledVocab,
    /// Mapping (type, ville) → code de localisation pour les clés pendantes
    /// de chronologie (ADR 0161), dérivé du référentiel `jurisdiction`.
    pub chrono: lj_extract::chrono::ChronoSnapshot,
    /// Labels guéris du référentiel `jurisdiction` (code → label avec ville),
    /// pour composer `search_title` quand le libellé source est nu (ADR 0170).
    pub jur_labels: std::collections::HashMap<String, String>,
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
            let link = LinkSnapshot::build(texts.clone(), articles);
            let vocab = lj_extract::compiled::CompiledVocab::build(&texts, &link);
            let jurisdictions = repo.load_jurisdictions().await?;
            // Snapshots keyés par `source_code` (ADR 0201) : l'extraction
            // travaille en codes source (location Judilibre), le code
            // canonique n'apparaît qu'à l'écriture (`ensure_jurisdictions`).
            let jur_labels = jurisdictions
                .iter()
                .map(|j| (j.source_code.clone(), j.label.clone()))
                .collect();
            let chrono = lj_extract::chrono::ChronoSnapshot::new(
                jurisdictions
                    .into_iter()
                    .filter_map(|j| Some((j.source_code, j.jurisdiction_type, j.city?))),
            );
            tracing::info!(n_texts, n_articles, "contexte d'extraction hydraté");
            Ok(ExtractCtx {
                link,
                vocab,
                chrono,
                jur_labels,
            })
        })
        .await
}

/// Mode de triage à l'ingest (port de `SourceMode`).
///
/// - [`IngestMode::MissingHash`] (défaut) : skip si le hash est identique, et
///   fast-skip d'un fichier entièrement ingéré via le manifeste.
/// - [`IngestMode::All`] : relit tout, **ignore le hash ET le manifeste**, force
///   un UPDATE complet de chaque décision existante. Trappe de re-traitement
///   total (ré-chunk / ré-embed) quand le pipeline pur a changé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum IngestMode {
    MissingHash,
    All,
}

/// Calcule le checksum source (xxh3-64 hex du payload brut) pour l'idempotence.
///
/// Port de `xxhash.xxh3_64_hexdigest(raw)` (règle #7). Le hash porte sur le
/// payload **brut** (XML opendata ou ligne JSON Judilibre), jamais sur le texte
/// nettoyé.
pub fn content_checksum(payload: &[u8]) -> String {
    let digest = xxhash_rust::xxh3::xxh3_64(payload);
    format!("{digest:016x}")
}

/// Génère un `public_id` URL-safe base64 sur 9 octets (port de
/// `_generate_public_id`). Aléatoire (non déterministe) comme côté Python.
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

/// Encodage base64 URL-safe (alphabet `-_`, sans padding) — port du
/// `base64.urlsafe_b64encode(token_bytes(9))`. 9 octets → 12 caractères.
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

/// Mode d'écriture d'un candidat (port de `WriteMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteMode {
    Full,
    SourceXmlOnly,
}

impl WriteMode {
    fn as_str(self) -> &'static str {
        match self {
            WriteMode::Full => "full",
            WriteMode::SourceXmlOnly => "source_xml_only",
        }
    }
}

/// Candidat issu du parse d'un payload (port de `_Candidate`).
struct Candidate {
    decision_id: Option<i64>,
    public_id: String,
    decision: Decision,
    content_checksum: String,
    raw_payload: Vec<u8>,
    payload_format: String,
    write_mode: WriteMode,
    /// Fond DILA (`payload_format == "dila-xml"`) : sélectionne le sous-bloc
    /// `META_JURI_*` pour reconstruire `source_fields`. `None` pour les autres
    /// formats (XML opendata, JSON Judilibre).
    dila_fond: Option<DilaFond>,
    /// `source_fields` déjà construits (`payload_format == "html"`, CEDH/CJUE,
    /// ADR 0094) : les métadonnées HUDOC/CDM verbatim (colonnes/prédicats) plus
    /// les sections rebasées sur `full_text`. `None` pour les formats dont
    /// `prepare_write` reconstruit `source_fields` depuis `raw_payload`.
    prebuilt_source_fields: Option<serde_json::Value>,
    /// Champs structurés déjà extraits par le parser pur (CNDA, ADR 0096) :
    /// `extract::routed` ne route que les 7 ordres FR ; un fond scrapé hors
    /// nomenclature opendata/Judilibre porte donc ses propres champs (solution_uid,
    /// date_lecture…) calculés par `parse_cnda`. `None` → `prepare_write` passe par
    /// l'extracteur routé `ExtractedFields::from_decision`.
    prebuilt_extracted: Option<ExtractedFields>,
}

/// Décision préparée (clean + chunk + extract + gzip), port de `_PreparedDecision`.
struct PreparedDecision {
    decision_id: Option<i64>,
    public_id: String,
    decision: Decision,
    content_checksum: String,
    write_mode: WriteMode,
    chunks: Vec<Chunk>,
    payload_format: String,
    extracted: Option<ExtractedFields>,
    /// Payload moins le texte, offsets rebasés sur `full_text` (ADR 0085).
    source_fields: serde_json::Value,
}

/// Cumul des compteurs d'un run (port des dataclasses `IngestSummary`/counts).
#[derive(Debug, Default, Clone, Copy)]
struct IngestCounts {
    created: usize,
    updated: usize,
    skipped: usize,
    errors: usize,
    empty_skipped: usize,
    chunks_created: usize,
    dedup_in_batch: usize,
}

impl IngestCounts {
    fn merge(&mut self, other: &IngestCounts) {
        self.created += other.created;
        self.updated += other.updated;
        self.skipped += other.skipped;
        self.errors += other.errors;
        self.empty_skipped += other.empty_skipped;
        self.chunks_created += other.chunks_created;
        self.dedup_in_batch += other.dedup_in_batch;
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
            texte_integral_raw: String::new(),
            texte_integral_clean: String::new(),
            sections: vec![],
            metadata_header: String::new(),
            visa_trim: String::new(),
            themes: Vec::new(),
            attacked: None,
            parse_warnings: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec règle #7 : content_checksum = xxh3-64 hex du payload brut.
    #[test]
    fn content_checksum_is_xxh3_64_hex() {
        // xxh3_64(b"") = 0x2d06800538d394c2 (vecteur de référence xxHash).
        assert_eq!(content_checksum(b""), "2d06800538d394c2");
        // Stable et 16 hex.
        let h = content_checksum(b"hello world");
        assert_eq!(h.len(), 16);
        assert_eq!(h, content_checksum(b"hello world"));
        assert_ne!(h, content_checksum(b"hello worlD"));
    }

    // Spec : base64 urlsafe, 9 octets → 12 chars, alphabet -_ sans padding.
    #[test]
    fn base64_urlsafe_matches_python() {
        // base64.urlsafe_b64encode(bytes(range(9))) == b'AAECAwQFBgcI'
        let bytes: [u8; 9] = [0, 1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(base64_urlsafe(&bytes), "AAECAwQFBgcI");
        // Octets hauts → caractères - et _.
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
