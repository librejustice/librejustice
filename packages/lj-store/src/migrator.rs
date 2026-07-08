//! Runner de migrations idempotent (port de `migrator.py`).
//!
//! Les `.sql` sont embarqués via `include_str!` (pas de lecture fs au
//! runtime). Crée `schema_migrations`, applique les versions non-vues dans une
//! transaction unique chacune, lève si la base contient une version absente
//! sur disque.
//!
//! **Sérialisation inter-process.** Plusieurs runners peuvent appeler
//! `apply_migrations` en même temps (cron `lj-ingest ingest` + un `load-legal-corpus`
//! manuel, p. ex.). Sans garde, chacun lit les versions appliquées *avant* que
//! l'autre n'ait committé, recalcule le même `pending`, et **rejoue** la même
//! migration : deux reconstructions d'index multi-Go en parallèle, le perdant
//! mourant ensuite sur la PK de `schema_migrations`. On préfixe donc tout
//! l'application par un **verrou consultatif de session** ; le perdant attend,
//! puis relit les versions et ne trouve plus rien à faire.

use crate::db::Connection;
use crate::error::{Result, StoreError};

/// Une migration découverte (version + SQL embarqué).
#[derive(Debug, Clone)]
pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Liste statique des migrations embarquées, triées par version croissante.
/// Source : `packages/lj-store/migrations/NNNN_*.sql` (copie des `.sql` Python).
pub fn embedded_migrations() -> Vec<Migration> {
    vec![
        Migration {
            version: 1,
            name: "0001_init.sql",
            sql: include_str!("../migrations/0001_init.sql"),
        },
        Migration {
            version: 2,
            name: "0002_drop_decision_articles.sql",
            sql: include_str!("../migrations/0002_drop_decision_articles.sql"),
        },
        Migration {
            version: 3,
            name: "0003_store_source_xml.sql",
            sql: include_str!("../migrations/0003_store_source_xml.sql"),
        },
        Migration {
            version: 4,
            name: "0004_add_public_id.sql",
            sql: include_str!("../migrations/0004_add_public_id.sql"),
        },
        Migration {
            version: 5,
            name: "0005_drop_unused_source_xml_size_columns.sql",
            sql: include_str!("../migrations/0005_drop_unused_source_xml_size_columns.sql"),
        },
        Migration {
            version: 6,
            name: "0006_nullable_gzip_blob.sql",
            sql: include_str!("../migrations/0006_nullable_gzip_blob.sql"),
        },
        Migration {
            version: 7,
            name: "0007_drop_gzip_blob.sql",
            sql: include_str!("../migrations/0007_drop_gzip_blob.sql"),
        },
        Migration {
            version: 8,
            name: "0008_rename_xml_gzip.sql",
            sql: include_str!("../migrations/0008_rename_xml_gzip.sql"),
        },
        Migration {
            version: 9,
            name: "0009_add_extracted_fields.sql",
            sql: include_str!("../migrations/0009_add_extracted_fields.sql"),
        },
        Migration {
            version: 10,
            name: "0010_mcp_oauth.sql",
            sql: include_str!("../migrations/0010_mcp_oauth.sql"),
        },
        Migration {
            version: 11,
            name: "0011_semantic_keywords.sql",
            sql: include_str!("../migrations/0011_semantic_keywords.sql"),
        },
        Migration {
            version: 12,
            name: "0012_semantic_keyword_scores.sql",
            sql: include_str!("../migrations/0012_semantic_keyword_scores.sql"),
        },
        Migration {
            version: 13,
            name: "0013_add_jurisdiction_name.sql",
            sql: include_str!("../migrations/0013_add_jurisdiction_name.sql"),
        },
        Migration {
            version: 14,
            name: "0014_corpus_token_df.sql",
            sql: include_str!("../migrations/0014_corpus_token_df.sql"),
        },
        Migration {
            version: 15,
            name: "0015_search_title.sql",
            sql: include_str!("../migrations/0015_search_title.sql"),
        },
        Migration {
            version: 16,
            name: "0016_search_title_stopwords.sql",
            sql: include_str!("../migrations/0016_search_title_stopwords.sql"),
        },
        Migration {
            version: 17,
            name: "0017_drop_unused_tables.sql",
            sql: include_str!("../migrations/0017_drop_unused_tables.sql"),
        },
        Migration {
            version: 18,
            name: "0018_add_content_checksum.sql",
            sql: include_str!("../migrations/0018_add_content_checksum.sql"),
        },
        Migration {
            version: 19,
            name: "0019_rename_payload_and_format.sql",
            sql: include_str!("../migrations/0019_rename_payload_and_format.sql"),
        },
        Migration {
            version: 20,
            name: "0020_revert_search_title.sql",
            sql: include_str!("../migrations/0020_revert_search_title.sql"),
        },
        Migration {
            version: 21,
            name: "0021_search_title_regex_stopwords.sql",
            sql: include_str!("../migrations/0021_search_title_regex_stopwords.sql"),
        },
        Migration {
            version: 22,
            name: "0022_search_title_stopwords_in_tokenizer.sql",
            sql: include_str!("../migrations/0022_search_title_stopwords_in_tokenizer.sql"),
        },
        Migration {
            version: 23,
            name: "0023_rabitq8_embedding.sql",
            sql: include_str!("../migrations/0023_rabitq8_embedding.sql"),
        },
        Migration {
            version: 24,
            name: "0024_drop_lab_tables.sql",
            sql: include_str!("../migrations/0024_drop_lab_tables.sql"),
        },
        Migration {
            version: 25,
            name: "0025_search_title_stopword_a.sql",
            sql: include_str!("../migrations/0025_search_title_stopword_a.sql"),
        },
        Migration {
            version: 26,
            name: "0026_decision_chunks_denorm.sql",
            sql: include_str!("../migrations/0026_decision_chunks_denorm.sql"),
        },
        Migration {
            version: 27,
            name: "0027_legal_codes_3nf.sql",
            sql: include_str!("../migrations/0027_legal_codes_3nf.sql"),
        },
        Migration {
            version: 28,
            name: "0028_dlr_trigger_guard.sql",
            sql: include_str!("../migrations/0028_dlr_trigger_guard.sql"),
        },
        Migration {
            version: 29,
            name: "0029_dlr_trigger_statement_level.sql",
            sql: include_str!("../migrations/0029_dlr_trigger_statement_level.sql"),
        },
        Migration {
            version: 30,
            name: "0030_drop_unused_docket_gin.sql",
            sql: include_str!("../migrations/0030_drop_unused_docket_gin.sql"),
        },
        Migration {
            version: 31,
            name: "0031_full_text_payload_storage_external.sql",
            sql: include_str!("../migrations/0031_full_text_payload_storage_external.sql"),
        },
        Migration {
            version: 32,
            name: "0032_drop_content_hash.sql",
            sql: include_str!("../migrations/0032_drop_content_hash.sql"),
        },
        Migration {
            version: 33,
            name: "0033_chunks_vec_hierarchical.sql",
            sql: include_str!("../migrations/0033_chunks_vec_hierarchical.sql"),
        },
        Migration {
            version: 34,
            name: "0034_users.sql",
            sql: include_str!("../migrations/0034_users.sql"),
        },
        Migration {
            version: 35,
            name: "0035_user_bookmarks.sql",
            sql: include_str!("../migrations/0035_user_bookmarks.sql"),
        },
        Migration {
            version: 36,
            name: "0036_user_search_history.sql",
            sql: include_str!("../migrations/0036_user_search_history.sql"),
        },
        Migration {
            version: 37,
            name: "0037_mcp_oauth_register.sql",
            sql: include_str!("../migrations/0037_mcp_oauth_register.sql"),
        },
        Migration {
            version: 38,
            name: "0038_chunks_legal_article_labels.sql",
            sql: include_str!("../migrations/0038_chunks_legal_article_labels.sql"),
        },
        Migration {
            version: 39,
            name: "0039_decisions_summary.sql",
            sql: include_str!("../migrations/0039_decisions_summary.sql"),
        },
        Migration {
            version: 40,
            name: "0040_search_source_and_decision_views.sql",
            sql: include_str!("../migrations/0040_search_source_and_decision_views.sql"),
        },
        Migration {
            version: 41,
            name: "0041_publication_codes_array.sql",
            sql: include_str!("../migrations/0041_publication_codes_array.sql"),
        },
        Migration {
            version: 42,
            name: "0042_users_track_activity.sql",
            sql: include_str!("../migrations/0042_users_track_activity.sql"),
        },
        Migration {
            version: 43,
            name: "0043_mcp_user_fk.sql",
            sql: include_str!("../migrations/0043_mcp_user_fk.sql"),
        },
        Migration {
            version: 44,
            name: "0044_mcp_client_fk.sql",
            sql: include_str!("../migrations/0044_mcp_client_fk.sql"),
        },
        Migration {
            version: 45,
            name: "0045_sitemaps.sql",
            sql: include_str!("../migrations/0045_sitemaps.sql"),
        },
        Migration {
            version: 46,
            name: "0046_jurisdiction_name_literal.sql",
            sql: include_str!("../migrations/0046_jurisdiction_name_literal.sql"),
        },
        Migration {
            version: 47,
            name: "0047_chunks_legal_article_composite.sql",
            sql: include_str!("../migrations/0047_chunks_legal_article_composite.sql"),
        },
        Migration {
            version: 48,
            name: "0048_chunks_bm25_unique.sql",
            sql: include_str!("../migrations/0048_chunks_bm25_unique.sql"),
        },
        Migration {
            version: 49,
            name: "0049_legal_codes_canonical.sql",
            sql: include_str!("../migrations/0049_legal_codes_canonical.sql"),
        },
        Migration {
            version: 50,
            name: "0050_decisions_extract_version.sql",
            sql: include_str!("../migrations/0050_decisions_extract_version.sql"),
        },
        Migration {
            version: 51,
            name: "0051_decisions_full_text_source_fields_embed_version.sql",
            sql: include_str!(
                "../migrations/0051_decisions_full_text_source_fields_embed_version.sql"
            ),
        },
        Migration {
            version: 52,
            name: "0052_decisions_legal_arrays.sql",
            sql: include_str!("../migrations/0052_decisions_legal_arrays.sql"),
        },
        Migration {
            version: 53,
            name: "0053_decisions_bm25.sql",
            sql: include_str!("../migrations/0053_decisions_bm25.sql"),
        },
        Migration {
            version: 54,
            name: "0054_drop_chunks_bm25_body_source_payload.sql",
            sql: include_str!("../migrations/0054_drop_chunks_bm25_body_source_payload.sql"),
        },
        Migration {
            version: 55,
            name: "0055_decision_links.sql",
            sql: include_str!("../migrations/0055_decision_links.sql"),
        },
        Migration {
            version: 56,
            name: "0056_decision_sources_and_ecli.sql",
            sql: include_str!("../migrations/0056_decision_sources_and_ecli.sql"),
        },
        Migration {
            version: 57,
            name: "0057_legi_referentiel.sql",
            sql: include_str!("../migrations/0057_legi_referentiel.sql"),
        },
        Migration {
            version: 58,
            name: "0058_payload_format_dila_html.sql",
            sql: include_str!("../migrations/0058_payload_format_dila_html.sql"),
        },
        Migration {
            version: 59,
            name: "0059_payload_format_pdf.sql",
            sql: include_str!("../migrations/0059_payload_format_pdf.sql"),
        },
        Migration {
            version: 60,
            name: "0060_referentiel_multi_source.sql",
            sql: include_str!("../migrations/0060_referentiel_multi_source.sql"),
        },
        Migration {
            version: 61,
            name: "0061_multisource_provenance_pivot.sql",
            sql: include_str!("../migrations/0061_multisource_provenance_pivot.sql"),
        },
        Migration {
            version: 62,
            name: "0062_drop_decision_links.sql",
            sql: include_str!("../migrations/0062_drop_decision_links.sql"),
        },
        Migration {
            version: 63,
            name: "0063_drop_decisions_mono_source.sql",
            sql: include_str!("../migrations/0063_drop_decisions_mono_source.sql"),
        },
        Migration {
            version: 64,
            name: "0064_decision_sources_payload_format_pdf.sql",
            sql: include_str!("../migrations/0064_decision_sources_payload_format_pdf.sql"),
        },
        Migration {
            version: 65,
            name: "0065_rename_identity_key_to_canonical_ref.sql",
            sql: include_str!("../migrations/0065_rename_identity_key_to_canonical_ref.sql"),
        },
        Migration {
            version: 66,
            name: "0066_ambiguous_canonical_ref.sql",
            sql: include_str!("../migrations/0066_ambiguous_canonical_ref.sql"),
        },
        Migration {
            version: 67,
            name: "0067_canonical_ref_ambiguous_column.sql",
            sql: include_str!("../migrations/0067_canonical_ref_ambiguous_column.sql"),
        },
        Migration {
            version: 68,
            name: "0068_drop_canonical_ref_ambiguous.sql",
            sql: include_str!("../migrations/0068_drop_canonical_ref_ambiguous.sql"),
        },
        Migration {
            version: 69,
            name: "0069_ecli_non_unique.sql",
            sql: include_str!("../migrations/0069_ecli_non_unique.sql"),
        },
        Migration {
            version: 70,
            name: "0070_source_rank_generated.sql",
            sql: include_str!("../migrations/0070_source_rank_generated.sql"),
        },
        Migration {
            version: 71,
            name: "0071_payload_format_docx.sql",
            sql: include_str!("../migrations/0071_payload_format_docx.sql"),
        },
        Migration {
            version: 72,
            name: "0072_legal_text_legal_article.sql",
            sql: include_str!("../migrations/0072_legal_text_legal_article.sql"),
        },
        Migration {
            version: 73,
            name: "0073_cited_reference_decision_citation.sql",
            sql: include_str!("../migrations/0073_cited_reference_decision_citation.sql"),
        },
        Migration {
            version: 74,
            name: "0074_legal_text_title_key_trgm.sql",
            sql: include_str!("../migrations/0074_legal_text_title_key_trgm.sql"),
        },
        Migration {
            version: 75,
            name: "0075_drop_title_key_trgm.sql",
            sql: include_str!("../migrations/0075_drop_title_key_trgm.sql"),
        },
        Migration {
            version: 76,
            name: "0076_legal_arrays_from_cited_reference.sql",
            sql: include_str!("../migrations/0076_legal_arrays_from_cited_reference.sql"),
        },
        Migration {
            version: 77,
            name: "0077_drop_legacy_3nf_citations.sql",
            sql: include_str!("../migrations/0077_drop_legacy_3nf_citations.sql"),
        },
        Migration {
            version: 78,
            name: "0078_legal_article_bm25.sql",
            sql: include_str!("../migrations/0078_legal_article_bm25.sql"),
        },
        Migration {
            version: 79,
            name: "0079_legal_article_search_title.sql",
            sql: include_str!("../migrations/0079_legal_article_search_title.sql"),
        },
        Migration {
            version: 80,
            name: "0080_legal_text_identity_cascade.sql",
            sql: include_str!("../migrations/0080_legal_text_identity_cascade.sql"),
        },
        Migration {
            version: 81,
            name: "0081_decisions_bm25_en_stopwords.sql",
            sql: include_str!("../migrations/0081_decisions_bm25_en_stopwords.sql"),
        },
        Migration {
            version: 82,
            name: "0082_legal_article_lang_translation.sql",
            sql: include_str!("../migrations/0082_legal_article_lang_translation.sql"),
        },
        Migration {
            version: 83,
            name: "0083_full_text_toast_lz4.sql",
            sql: include_str!("../migrations/0083_full_text_toast_lz4.sql"),
        },
        Migration {
            version: 84,
            name: "0084_lang_rank_authority.sql",
            sql: include_str!("../migrations/0084_lang_rank_authority.sql"),
        },
        Migration {
            version: 85,
            name: "0085_legal_article_source_asof.sql",
            sql: include_str!("../migrations/0085_legal_article_source_asof.sql"),
        },
        Migration {
            version: 86,
            name: "0086_treaty_jorf_get_date.sql",
            sql: include_str!("../migrations/0086_treaty_jorf_get_date.sql"),
        },
        Migration {
            version: 87,
            name: "0087_legal_text_num_prefix_agnostic.sql",
            sql: include_str!("../migrations/0087_legal_text_num_prefix_agnostic.sql"),
        },
        Migration {
            version: 88,
            name: "0088_legal_text_facet_indexes.sql",
            sql: include_str!("../migrations/0088_legal_text_facet_indexes.sql"),
        },
        Migration {
            version: 89,
            name: "0089_citation_overrides.sql",
            sql: include_str!("../migrations/0089_citation_overrides.sql"),
        },
        Migration {
            version: 90,
            name: "0090_decision_citation_spans.sql",
            sql: include_str!("../migrations/0090_decision_citation_spans.sql"),
        },
        Migration {
            version: 91,
            name: "0091_legal_citation_direct_link.sql",
            sql: include_str!("../migrations/0091_legal_citation_direct_link.sql"),
        },
        Migration {
            version: 92,
            name: "0092_gold_annotation.sql",
            sql: include_str!("../migrations/0092_gold_annotation.sql"),
        },
        Migration {
            version: 93,
            name: "0093_decision_extraction_columns.sql",
            sql: include_str!("../migrations/0093_decision_extraction_columns.sql"),
        },
        Migration {
            version: 94,
            name: "0094_trim_extraction_columns.sql",
            sql: include_str!("../migrations/0094_trim_extraction_columns.sql"),
        },
        Migration {
            version: 95,
            name: "0095_drop_legal_citation_source.sql",
            sql: include_str!("../migrations/0095_drop_legal_citation_source.sql"),
        },
        Migration {
            version: 96,
            name: "0096_citation_model.sql",
            sql: include_str!("../migrations/0096_citation_model.sql"),
        },
        Migration {
            version: 97,
            name: "0097_citation_flat.sql",
            sql: include_str!("../migrations/0097_citation_flat.sql"),
        },
        Migration {
            version: 98,
            name: "0098_facettes_ref_uid.sql",
            sql: include_str!("../migrations/0098_facettes_ref_uid.sql"),
        },
        Migration {
            version: 99,
            name: "0099_drop_ancien_monde_citations.sql",
            sql: include_str!("../migrations/0099_drop_ancien_monde_citations.sql"),
        },
        Migration {
            version: 100,
            name: "0100_facet_referentiels.sql",
            sql: include_str!("../migrations/0100_facet_referentiels.sql"),
        },
        Migration {
            version: 101,
            name: "0101_sync_arrays_rapportent_derive.sql",
            sql: include_str!("../migrations/0101_sync_arrays_rapportent_derive.sql"),
        },
        Migration {
            version: 102,
            name: "0102_facet_juridiction_types.sql",
            sql: include_str!("../migrations/0102_facet_juridiction_types.sql"),
        },
        Migration {
            version: 103,
            name: "0103_gold_annotation_revive.sql",
            sql: include_str!("../migrations/0103_gold_annotation_revive.sql"),
        },
        Migration {
            version: 104,
            name: "0104_drop_gold_annotation.sql",
            sql: include_str!("../migrations/0104_drop_gold_annotation.sql"),
        },
        Migration {
            version: 105,
            name: "0105_served_lang_normalise.sql",
            sql: include_str!("../migrations/0105_served_lang_normalise.sql"),
        },
        Migration {
            version: 106,
            name: "0106_lang_materialise.sql",
            sql: include_str!("../migrations/0106_lang_materialise.sql"),
        },
        Migration {
            version: 107,
            name: "0107_drop_ancien_monde_facettes.sql",
            sql: include_str!("../migrations/0107_drop_ancien_monde_facettes.sql"),
        },
        Migration {
            version: 108,
            name: "0108_decision_themes.sql",
            sql: include_str!("../migrations/0108_decision_themes.sql"),
        },
        Migration {
            version: 109,
            name: "0109_sync_arrays_collate_c.sql",
            sql: include_str!("../migrations/0109_sync_arrays_collate_c.sql"),
        },
        Migration {
            version: 110,
            name: "0110_decision_links.sql",
            sql: include_str!("../migrations/0110_decision_links.sql"),
        },
        Migration {
            version: 111,
            name: "0111_slug_universel.sql",
            sql: include_str!("../migrations/0111_slug_universel.sql"),
        },
        Migration {
            version: 112,
            name: "0112_fix_jurisdiction_labels.sql",
            sql: include_str!("../migrations/0112_fix_jurisdiction_labels.sql"),
        },
        Migration {
            version: 113,
            name: "0113_case_citation.sql",
            sql: include_str!("../migrations/0113_case_citation.sql"),
        },
        Migration {
            version: 114,
            name: "0114_portee_facette.sql",
            sql: include_str!("../migrations/0114_portee_facette.sql"),
        },
        Migration {
            version: 115,
            name: "0115_gin_docket_fastupdate_off.sql",
            sql: include_str!("../migrations/0115_gin_docket_fastupdate_off.sql"),
        },
        Migration {
            version: 116,
            name: "0116_drop_vocab_arianeweb.sql",
            sql: include_str!("../migrations/0116_drop_vocab_arianeweb.sql"),
        },
        Migration {
            version: 117,
            name: "0117_formation_structuree.sql",
            sql: include_str!("../migrations/0117_formation_structuree.sql"),
        },
        Migration {
            version: 118,
            name: "0118_chambres_terres_civi.sql",
            sql: include_str!("../migrations/0118_chambres_terres_civi.sql"),
        },
    ]
}

/// Clé du verrou consultatif sérialisant les runs de migration concurrents.
/// Forme à **deux int4** (`pg_advisory_lock(int4, int4)`) → espace de verrous
/// **disjoint** de celui des `pg_advisory_xact_lock(decision_id)` (bigint) du
/// repository : aucune collision possible. `classid` = `'LJ'`, `objid` = `'mig'`.
const MIGRATION_LOCK_CLASS: i32 = 0x4C4A; // "LJ"
const MIGRATION_LOCK_OBJ: i32 = 0x6D6967; // "mig"

/// Prend le verrou consultatif de **session** sérialisant les migrations. Le
/// `SELECT pg_advisory_lock` peut bloquer bien au-delà des 30 s de
/// `statement_timeout` posées par le pool (un pair reconstruit un index
/// multi-Go) ; on le scoppe donc dans une transaction avec `SET LOCAL
/// statement_timeout = 0`. Le verrou de session **survit au COMMIT** (il n'est
/// pas lié à la transaction) et reste tenu jusqu'à `pg_advisory_unlock` ou la fin
/// de session.
async fn acquire_migration_lock(conn: &Connection) -> Result<()> {
    conn.batch_execute("BEGIN").await?;
    let acquired = async {
        conn.batch_execute("SET LOCAL statement_timeout = 0")
            .await?;
        conn.execute(
            "SELECT pg_advisory_lock($1, $2)",
            &[&MIGRATION_LOCK_CLASS, &MIGRATION_LOCK_OBJ],
        )
        .await?;
        Ok::<(), StoreError>(())
    }
    .await;
    match acquired {
        Ok(()) => {
            conn.batch_execute("COMMIT").await?;
            Ok(())
        }
        Err(err) => {
            let _ = conn.batch_execute("ROLLBACK").await;
            Err(err)
        }
    }
}

/// Crée la table `schema_migrations` si absente (port de
/// `_ensure_schema_migrations`).
async fn ensure_schema_migrations(conn: &Connection) -> Result<()> {
    conn.batch_execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (\
           version INTEGER PRIMARY KEY,\
           applied_at TIMESTAMPTZ NOT NULL\
         )",
    )
    .await?;
    Ok(())
}

/// Versions déjà appliquées (port de `_applied_versions`).
async fn applied_versions(conn: &Connection) -> Result<Vec<i32>> {
    let rows = conn
        .query("SELECT version FROM schema_migrations", &[])
        .await?;
    Ok(rows.iter().map(|row| row.get::<_, i32>(0)).collect())
}

/// Applique les migrations non-vues. Renvoie les versions effectivement
/// appliquées (port de `apply_migrations`).
///
/// Sérialise les runners concurrents via un verrou consultatif de session
/// (cf. doc de module), puis délègue à [`apply_migrations_locked`]. Le verrou est
/// **toujours relâché** au retour, succès comme erreur.
pub async fn apply_migrations(conn: &Connection) -> Result<Vec<i32>> {
    acquire_migration_lock(conn).await?;
    let result = apply_migrations_locked(conn).await;
    // Relâche le verrou de session sur tous les chemins (le perdant suivant
    // pourra entrer et constater qu'il n'y a plus rien à faire).
    let _ = conn
        .execute(
            "SELECT pg_advisory_unlock($1, $2)",
            &[&MIGRATION_LOCK_CLASS, &MIGRATION_LOCK_OBJ],
        )
        .await;
    result
}

/// Corps de [`apply_migrations`] exécuté **sous verrou** :
///
/// 1. crée `schema_migrations` ;
/// 2. liste les versions appliquées (relue *après* le verrou : le gagnant a déjà
///    committé, donc le perdant voit ses migrations et n'en rejoue aucune) ;
/// 3. lève [`StoreError::UnknownMigrations`] si une version en base n'a pas de
///    fichier embarqué correspondant (signe d'un binaire plus ancien que la
///    base) ;
/// 4. applique chaque migration en attente, chacune dans sa propre transaction
///    (`BEGIN`/`COMMIT`, `ROLLBACK` sur erreur), puis insère la ligne
///    `schema_migrations`.
async fn apply_migrations_locked(conn: &Connection) -> Result<Vec<i32>> {
    ensure_schema_migrations(conn).await?;

    // `available` est déjà trié par version croissante (cf. embedded_migrations).
    let available = embedded_migrations();
    let available_versions: Vec<i32> = available.iter().map(|m| m.version).collect();

    let applied = applied_versions(conn).await?;

    let missing: Vec<i32> = applied
        .iter()
        .copied()
        .filter(|v| !available_versions.contains(v))
        .collect();
    if !missing.is_empty() {
        let mut missing_sorted = missing;
        missing_sorted.sort_unstable();
        let max = *missing_sorted.last().expect("non-empty");
        return Err(StoreError::UnknownMigrations {
            version: max,
            missing: missing_sorted,
        });
    }

    let pending: Vec<&Migration> = available
        .iter()
        .filter(|m| !applied.contains(&m.version))
        .collect();
    if pending.is_empty() {
        return Ok(Vec::new());
    }

    let mut newly_applied: Vec<i32> = Vec::with_capacity(pending.len());
    for migration in pending {
        tracing::info!(
            version = migration.version,
            name = migration.name,
            "application migration"
        );
        if let Err(err) = apply_one(conn, migration).await {
            // best-effort rollback : on tente d'annuler la transaction ouverte
            // avant de remonter l'erreur d'origine.
            let _ = conn.batch_execute("ROLLBACK").await;
            return Err(err);
        }
        newly_applied.push(migration.version);
    }
    Ok(newly_applied)
}

/// Applique une migration dans une transaction explicite. `applied_at` est posé
/// par `now()` côté Postgres (équivaut au `datetime.now(UTC)` Python, sans
/// dépendre de l'horloge du client).
async fn apply_one(conn: &Connection, migration: &Migration) -> Result<()> {
    conn.batch_execute("BEGIN").await?;
    // Les migrations sont du SQL de maintenance de confiance ; certaines
    // reconstruisent des index de plusieurs Go (DROP+CREATE chunks_bm25, cf.
    // 0026/0041/0046) bien au-delà des 30 s que `build_pool` arme sur chaque
    // connexion pour borner les requêtes API. `SET LOCAL` : scoppé à la
    // transaction de la migration → la connexion repart propre dans le pool.
    conn.batch_execute("SET LOCAL statement_timeout = 0")
        .await?;
    conn.batch_execute(migration.sql).await?;
    conn.execute(
        "INSERT INTO schema_migrations (version, applied_at) VALUES ($1, now())",
        &[&migration.version],
    )
    .await?;
    conn.batch_execute("COMMIT").await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_migrations_are_contiguous() {
        let migs = embedded_migrations();
        assert!(!migs.is_empty());
        // versions contiguës 1..=N, strictement croissantes (pas de trou ni
        // de doublon possible quand on ajoute une migration).
        for (i, m) in migs.iter().enumerate() {
            assert_eq!(m.version, (i + 1) as i32, "version contiguë");
        }
    }

    #[test]
    fn migration_names_match_versions() {
        for m in embedded_migrations() {
            let prefix: String = m.name.chars().take(4).collect();
            let parsed: i32 = prefix.parse().expect("prefix NNNN numérique");
            assert_eq!(parsed, m.version, "nom {} vs version {}", m.name, m.version);
            assert!(m.name.ends_with(".sql"));
        }
    }

    #[test]
    fn first_migration_sql_is_non_empty() {
        let migs = embedded_migrations();
        let init = &migs[0];
        assert_eq!(init.name, "0001_init.sql");
        // le SQL embarqué doit contenir la création de la table decisions.
        assert!(init.sql.contains("schema_migrations") || init.sql.contains("decisions"));
        assert!(!init.sql.trim().is_empty());
    }
}
