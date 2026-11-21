//! The decisions of the Conseil constitutionnel its site's feed lists before the DILA carries them.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use lj_core::decision::Decision;
use lj_parse::{cc_site_decision, CcSiteRefused, FromSourceFields};
use lj_sources::cc_site::CcClient;
use lj_store::repository::DecisionRepository;

use super::batch::drain_batch;
use super::europe::europe_pool;
use super::{content_checksum, generate_public_id, Candidate, IngestCounts, IngestMode};

/// Writes the decisions the feed lists and no provenance names yet.
pub async fn sync_cc_site() -> Result<()> {
    let client = CcClient::new()?;
    let listed = client.feed().await?;
    let pool = europe_pool().await?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let held = DecisionRepository::new(&conn)
        .constit_pages_held(&listed)
        .await?;
    let mut batch = Vec::new();
    let mut refused: HashMap<&'static str, usize> = HashMap::new();
    for url in listed.iter().filter(|url| !held.contains(url)) {
        let page = client.page(url).await?;
        let read = match cc_site_decision(&page) {
            Ok(read) => read,
            Err(why) => {
                tracing::warn!(url, ?why, "cc-site: a decision page is not read");
                *refused.entry(refused_label(why)).or_default() += 1;
                continue;
            }
        };
        batch.push(Candidate {
            decision_id: None,
            public_id: generate_public_id(),
            content_checksum: content_checksum(read.text.as_bytes()),
            decision: Decision::from_source_fields(
                &read.text,
                &read.source_fields,
                &read.source_uid,
            ),
            prebuilt_source_fields: Some(read.source_fields),
            prebuilt_extracted: None,
            raw_payload: Vec::new(),
            payload_format: "html".to_string(),
            write_mode: super::WriteMode::Full,
            passages: true,
            distinct: false,
        });
    }
    let mut counts = IngestCounts::default();
    drain_batch(&conn, batch, IngestMode::MissingHash, &mut counts).await?;
    tracing::info!(
        listed = listed.len(),
        held = held.len(),
        created = counts.created,
        updated = counts.updated,
        skipped = counts.skipped,
        refused = ?refused,
        "cc-site"
    );
    Ok(())
}

fn refused_label(why: CcSiteRefused) -> &'static str {
    match why {
        CcSiteRefused::NoNumber => "no_number",
        CcSiteRefused::NoDate => "no_date",
        CcSiteRefused::NoText => "no_text",
    }
}
