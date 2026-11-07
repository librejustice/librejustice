//! Pure stages (CPU, parallel through rayon): classify, idempotent triage, clean
//! and extract a candidate.

use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};

use lj_core::decision::Decision;
use lj_parse::{
    build_source_fields, build_source_fields_dila, build_source_fields_xml, parse_judilibre,
    parse_xml, FromSourceFields,
};
use lj_sources::downloader::parse_decision_line;
use lj_store::repository::{BulkDecisionWrite, ExistingDecisionState, PassageInputs};

use super::{
    content_checksum, generate_public_id, Candidate, IngestMode, PreparedDecision, WriteMode,
};

/// Parse + classe un XML opendata.
///
/// `archive_name` préfixe le `source_uid` (= `{zip}/{member}`). `None` →
/// UID non reconnu (juridiction inconnue), skip côté appelant.
pub(super) fn classify_xml(raw: Vec<u8>, member: &str, archive_name: &str) -> Option<Candidate> {
    let decision = parse_xml(&raw, member, Some(archive_name));
    if decision.jurisdiction_type.is_none() {
        tracing::warn!(uid = %decision.source_uid, "UID not recognised, skip");
        return None;
    }
    let checksum = content_checksum(&raw);
    Some(Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        decision,
        content_checksum: checksum,
        raw_payload: raw,
        payload_format: "xml".to_string(),
        write_mode: WriteMode::Full,
        passages: true,
        distinct: false,
        prebuilt_source_fields: None,
        prebuilt_extracted: None,
    })
}

/// Parse + classe une ligne JSON Judilibre.
pub(super) fn classify_judilibre(line: Vec<u8>) -> Result<Option<Candidate>> {
    let payload = parse_decision_line(&line).context("ligne Judilibre JSON invalide")?;
    let decision = parse_judilibre(&payload, None);
    if decision.jurisdiction_type.is_none() {
        tracing::warn!(uid = %decision.source_uid, "Judilibre UID not recognised, skip");
        return Ok(None);
    }
    let checksum = content_checksum(&line);
    Ok(Some(Candidate {
        decision_id: None,
        public_id: generate_public_id(),
        decision,
        content_checksum: checksum,
        raw_payload: line,
        payload_format: "json".to_string(),
        write_mode: WriteMode::Full,
        passages: true,
        distinct: false,
        prebuilt_source_fields: None,
        prebuilt_extracted: None,
    }))
}

/// Triage idempotent d'un batch (mode `MISSING_HASH`) : dédup intra-batch par
/// `source_uid` (last-wins) + précheck DB groupé. Renvoie
/// `(survivants, skipped_unchanged, deduped)`.
pub(super) fn triage_candidates(
    candidates: Vec<Candidate>,
    existing: &HashMap<String, ExistingDecisionState>,
    mode: IngestMode,
) -> (Vec<Candidate>, usize, usize) {
    let total = candidates.len();
    let mut by_uid: HashMap<String, Candidate> = HashMap::with_capacity(total);
    for cand in candidates {
        by_uid.insert(cand.decision.source_uid.clone(), cand);
    }
    let deduped = total - by_uid.len();

    let mut survivors = Vec::new();
    let mut skipped = 0usize;

    for (uid, mut cand) in by_uid {
        match existing.get(&uid) {
            None => survivors.push(cand),
            Some(prev) => {
                if mode == IngestMode::All {
                    cand.decision_id = Some(prev.id);
                    cand.public_id = prev.public_id.clone().unwrap_or_else(generate_public_id);
                    cand.write_mode = WriteMode::Full;
                    survivors.push(cand);
                    continue;
                }
                let same_hash = prev.content_checksum == cand.content_checksum;
                if same_hash {
                    if prev.source_uid != uid || prev.public_id.is_none() {
                        cand.decision_id = Some(prev.id);
                        cand.public_id = prev.public_id.clone().unwrap_or_else(generate_public_id);
                        cand.write_mode = WriteMode::SourceXmlOnly;
                        survivors.push(cand);
                        continue;
                    }
                    skipped += 1;
                    continue;
                }
                tracing::info!(
                    source_uid = %uid,
                    old = %prev.content_checksum,
                    new = %cand.content_checksum,
                    "ingest UPDATE detected"
                );
                cand.decision_id = Some(prev.id);
                cand.public_id = prev.public_id.clone().unwrap_or_else(generate_public_id);
                survivors.push(cand);
            }
        }
    }
    (survivors, skipped, deduped)
}

/// Cleans a survivor and rebuilds it from its `source_fields`. `None` for a
/// text empty once cleaned (a Judilibre embargo, an empty XML), counted in
/// `empty_skipped`. Without the extraction context, which [`extract_write`]
/// reads.
pub(super) fn prepare_write(candidate: Candidate) -> Result<Option<PreparedDecision>> {
    if candidate.write_mode == WriteMode::SourceXmlOnly {
        // Backfill du `public_id` (+ format) seul : la ligne `decisions` n'est pas
        // réécrite → pas de source_fields à bâtir (la provenance porte déjà le sien).
        return Ok(Some(PreparedDecision {
            decision_id: candidate.decision_id,
            public_id: candidate.public_id,
            decision: candidate.decision,
            content_checksum: candidate.content_checksum,
            write_mode: candidate.write_mode,
            passages: candidate.passages,
            payload_format: candidate.payload_format,
            extracted: None,
            prebuilt_extracted: None,
            source_fields: serde_json::Value::Null,
            distinct: candidate.distinct,
        }));
    }

    let source_uid = candidate.decision.source_uid.clone();

    // source_fields : payload moins le texte, offsets rebasés sur full_text. JSON →
    // depuis le payload + sections ; XML → scalaires `<Dossier>`/`<Audience>`
    // (sections recalculées au rendu/re-chunk).
    //
    // `prebuilt_source_fields` fourni ⇒ utilisé tel quel pour TOUTES les familles :
    // c'est le chemin reconstruction-depuis-stockage (re-embed, CEDH/CJUE/ArianeWeb),
    // où le `source_fields` canonique est déjà en base et où `raw_payload` n'existe
    // plus. En ingest normal il est `None` → on reconstruit depuis `raw_payload` par
    // format (ci-dessous).
    let source_fields = match candidate.prebuilt_source_fields.clone() {
        Some(sf) => sf,
        None => match candidate.payload_format.as_str() {
            "json" => {
                let payload = parse_decision_line(&candidate.raw_payload)
                    .with_context(|| format!("re-parse JSON pour source_fields {source_uid}"))?;
                build_source_fields(&payload, &candidate.decision.sections)
            }
            "xml" => build_source_fields_xml(&candidate.raw_payload),
            // `raw_payload` holds the repaired bytes (mojibake, entities), so `source_fields`
            // is clean, while `content_checksum` stays computed on the raw bytes.
            "dila-xml" => build_source_fields_dila(&candidate.raw_payload),
            "html" | "pdf" => {
                // `source_fields` préconstruits au classify, pas reconstruits du `raw_payload`
                // (qui ne porte plus le texte source) : CEDH/CJUE depuis les métadonnées
                // HUDOC/CDM, ArianeWeb depuis le hit xsearch. Les colonnes ne portent pas de texte
                // (il vit dans `full_text`).
                candidate.prebuilt_source_fields.clone().ok_or_else(|| {
                    anyhow!(
                        "source_fields {} sans préconstruction {source_uid}",
                        candidate.payload_format
                    )
                })?
            }
            other => anyhow::bail!("payload_format inconnu pour source_fields: {other:?}"),
        },
    };

    // Chemin LINÉAIRE : TOUTES les familles passent par
    // `Decision::from_source_fields` (le format a déjà été validé ci-dessus en
    // bâtissant `source_fields`). Chunking et extraction tournent ainsi sur la
    // représentation canonique unique, identique à l'aval (re-extract, rendu).
    let decision = Decision::from_source_fields(
        &candidate.decision.texte_integral_clean,
        &source_fields,
        &source_uid,
    );

    if decision.texte_integral_clean.is_empty() {
        tracing::warn!(uid = %source_uid, "Empty text after clean, skip");
        return Ok(None);
    }

    Ok(Some(PreparedDecision {
        decision_id: candidate.decision_id,
        public_id: candidate.public_id,
        decision,
        content_checksum: candidate.content_checksum,
        write_mode: candidate.write_mode,
        passages: candidate.passages,
        payload_format: candidate.payload_format,
        extracted: None,
        prebuilt_extracted: candidate.prebuilt_extracted,
        source_fields,
        distinct: candidate.distinct,
    }))
}

/// Keeps the passages of a decision a new payload rewrites when the text and
/// header they are cut from come out the same as the stored ones (`stored`: its
/// provenance's `source_fields` and text): so would their embeddings.
pub(super) fn keep_unchanged_passages(
    mut prepared: PreparedDecision,
    stored: &PassageInputs,
) -> PreparedDecision {
    let (source_fields, full_text) = stored;
    let before =
        Decision::from_source_fields(full_text, source_fields, &prepared.decision.source_uid);
    let after = &prepared.decision;
    if before.texte_integral_clean == after.texte_integral_clean
        && before.metadata_header == after.metadata_header
    {
        prepared.write_mode = WriteMode::KeepPassages;
    }
    prepared
}

/// The write of a prepared decision, once [`extract_write`] has filled
/// `prepared.extracted`.
pub(super) fn to_bulk_write(prepared: PreparedDecision) -> BulkDecisionWrite {
    let canonical_ref = lj_extract_rows::canonical_ref(&prepared.decision);
    BulkDecisionWrite {
        decision_id: prepared.decision_id,
        public_id: prepared.public_id,
        decision: prepared.decision,
        content_checksum: prepared.content_checksum,
        canonical_ref,
        write_mode: prepared.write_mode.as_str().to_string(),
        drop_passages: prepared.write_mode == WriteMode::Full && prepared.passages,
        payload_format: prepared.payload_format,
        extracted: prepared.extracted,
        source_fields: prepared.source_fields,
        distinct: prepared.distinct,
    }
}

/// Extraction structurée d'un préparé : l'extracteur routé tourne pour TOUTES les
/// familles, sur la `Decision` canonique du chemin linéaire — même entrée que le
/// reextract. Un parser source qui a préconstruit des champs de grain source
/// (CNDA : solution du dispositif ; ArianeWeb : dates/dossiers de la fiche) les
/// superpose au résultat via `merge_prebuilt` — l'extraction fournit le reste
/// (citations, domaine, instruments, parties…).
pub(super) fn extract_write(
    mut prepared: PreparedDecision,
    ctx: &super::ExtractCtx,
) -> Result<PreparedDecision> {
    if prepared.write_mode == WriteMode::SourceXmlOnly {
        return Ok(prepared);
    }
    let full = lj_extract_rows::extracted_fields(&prepared.decision, &ctx.snapshots())
        .map_err(|e| anyhow!("extract {}: {e}", prepared.decision.source_uid))?;
    // Ce qui ne vit que dans `source_fields` (le nom d'affaire du docname CEDH)
    // se relit ici comme à la ré-extraction.
    let prebuilt = prepared.prebuilt_extracted.take().or_else(|| {
        lj_extract_rows::prebuilt_from_source_fields(
            &prepared.decision.source_uid,
            &prepared.source_fields,
        )
    });
    let merged = match prebuilt {
        Some(pre) => lj_extract_rows::merge_prebuilt(pre, full),
        None => full,
    };
    prepared.extracted = Some(lj_extract_rows::with_search_title(
        merged,
        prepared.decision.jurisdiction_type.as_deref(),
        &ctx.jur_labels,
        None,
    ));
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::tests_support::test_decision;

    #[test]
    fn triage_skips_unchanged_and_updates_changed() {
        fn cand(uid: &str, checksum: &str) -> Candidate {
            let d = test_decision(uid);
            Candidate {
                decision_id: None,
                public_id: "p".to_string(),
                decision: d,
                content_checksum: checksum.to_string(),
                raw_payload: vec![],
                payload_format: "xml".to_string(),
                write_mode: WriteMode::Full,
                passages: true,
                distinct: false,
                prebuilt_source_fields: None,
                prebuilt_extracted: None,
            }
        }
        let mut existing = HashMap::new();
        existing.insert(
            "a".to_string(),
            ExistingDecisionState {
                id: 1,
                source_uid: "a".to_string(),
                content_checksum: "h1".to_string(),
                public_id: Some("pa".to_string()),
            },
        );
        existing.insert(
            "b".to_string(),
            ExistingDecisionState {
                id: 2,
                source_uid: "b".to_string(),
                content_checksum: "old".to_string(),
                public_id: Some("pb".to_string()),
            },
        );

        let candidates = vec![cand("a", "h1"), cand("b", "new"), cand("c", "h3")];
        let (survivors, skipped, deduped) =
            triage_candidates(candidates, &existing, IngestMode::MissingHash);
        assert_eq!(skipped, 1);
        assert_eq!(deduped, 0);
        let by_uid: HashMap<&str, &Candidate> = survivors
            .iter()
            .map(|c| (c.decision.source_uid.as_str(), c))
            .collect();
        assert_eq!(survivors.len(), 2);
        assert_eq!(by_uid["b"].decision_id, Some(2));
        assert_eq!(by_uid["c"].decision_id, None);
    }

    #[test]
    fn a_republication_keeps_its_passages_unless_what_they_are_cut_from_changes() {
        fn prepared(
            publication: serde_json::Value,
            solution: &str,
            text: &str,
        ) -> PreparedDecision {
            let line = serde_json::json!({
                "id": "69f2e231cdc6046d470bcc33",
                "jurisdiction": "tj",
                "location": "tj75056",
                "chamber": "Chambre JAF",
                "decision_date": "2026-04-29",
                "update_date": if publication.is_null() { "2026-04-30" } else { "2026-10-28" },
                "number": "26/00220",
                "numbers": ["26/00220"],
                "type": "other",
                "solution": solution,
                "publication": publication,
                "text": text,
                "zones": {
                    "introduction": [{ "start": 0, "end": 28 }],
                    "dispositif": [{ "start": 29, "end": text.chars().count() }],
                },
            });
            let cand = classify_judilibre(serde_json::to_vec(&line).unwrap())
                .unwrap()
                .unwrap();
            prepare_write(cand).unwrap().unwrap()
        }
        let text = "TRIBUNAL JUDICIAIRE DE PARIS\nPAR CES MOTIFS, prononce le divorce des époux.";
        let first = prepared(serde_json::Value::Null, "Prononce le divorce", text);
        let stored = (
            first.source_fields.clone(),
            first.decision.texte_integral_clean.clone(),
        );

        let republished = keep_unchanged_passages(
            prepared(serde_json::json!([]), "Prononce le divorce", text),
            &stored,
        );
        assert_eq!(republished.write_mode, WriteMode::KeepPassages);

        let other_text = text.replace("des époux", "des deux époux");
        let rewritten = keep_unchanged_passages(
            prepared(serde_json::json!([]), "Prononce le divorce", &other_text),
            &stored,
        );
        assert_eq!(rewritten.write_mode, WriteMode::Full);

        let other_header = keep_unchanged_passages(
            prepared(serde_json::json!([]), "Rejette la demande", text),
            &stored,
        );
        assert_eq!(other_header.write_mode, WriteMode::Full);
    }

    #[test]
    fn triage_dedups_intra_batch_last_wins() {
        fn cand(uid: &str, checksum: &str) -> Candidate {
            let d = test_decision(uid);
            Candidate {
                decision_id: None,
                public_id: "p".to_string(),
                decision: d,
                content_checksum: checksum.to_string(),
                raw_payload: vec![],
                payload_format: "xml".to_string(),
                write_mode: WriteMode::Full,
                passages: true,
                distinct: false,
                prebuilt_source_fields: None,
                prebuilt_extracted: None,
            }
        }
        let existing = HashMap::new();
        let candidates = vec![cand("a", "h1"), cand("a", "h2")];
        let (survivors, _skipped, deduped) =
            triage_candidates(candidates, &existing, IngestMode::MissingHash);
        assert_eq!(deduped, 1);
        assert_eq!(survivors.len(), 1);
        assert_eq!(survivors[0].content_checksum, "h2");
    }

    #[test]
    fn triage_all_mode_forces_full_update_ignoring_hash() {
        fn cand(uid: &str, checksum: &str) -> Candidate {
            Candidate {
                decision_id: None,
                public_id: "p".to_string(),
                decision: test_decision(uid),
                content_checksum: checksum.to_string(),
                raw_payload: vec![],
                payload_format: "xml".to_string(),
                write_mode: WriteMode::Full,
                passages: true,
                distinct: false,
                prebuilt_source_fields: None,
                prebuilt_extracted: None,
            }
        }
        let mut existing = HashMap::new();
        existing.insert(
            "a".to_string(),
            ExistingDecisionState {
                id: 7,
                source_uid: "a".to_string(),
                content_checksum: "same".to_string(),
                public_id: Some("pa".to_string()),
            },
        );
        let candidates = vec![cand("a", "same")];
        let (survivors, skipped, _) = triage_candidates(candidates, &existing, IngestMode::All);
        assert_eq!(skipped, 0);
        assert_eq!(survivors.len(), 1);
        assert_eq!(survivors[0].decision_id, Some(7));
        assert_eq!(survivors[0].write_mode, WriteMode::Full);
    }
}
