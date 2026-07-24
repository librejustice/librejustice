//! Étapes pures (CPU, parallélisables via rayon) : classify, triage idempotent,
//! clean + chunk + extract d'un candidat.

use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};

use lj_core::decision::Decision;
use lj_core::parsing::{
    build_source_fields, build_source_fields_dila, build_source_fields_xml, parse_judilibre,
    parse_xml,
};
use lj_store::repository::ExistingDecisionState;

use crate::chunking::{chunk_char, DEFAULT_OVERLAP_MAX, DEFAULT_OVERLAP_MIN};

use super::{
    content_checksum, generate_public_id, Candidate, IngestMode, PreparedDecision, WriteMode,
};

/// Parse + classe un XML opendata (port de `_classify_raw`).
///
/// `archive_name` préfixe le `source_uid` (= `{zip}/{member}`). `None` →
/// UID non reconnu (juridiction inconnue), skip côté appelant.
pub(super) fn classify_xml(raw: Vec<u8>, member: &str, archive_name: &str) -> Option<Candidate> {
    let decision = parse_xml(&raw, member, Some(archive_name));
    if decision.jurisdiction_type.is_none() {
        tracing::warn!(uid = %decision.source_uid, "UID non reconnu, skip");
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
        dila_fond: None,
        prebuilt_source_fields: None,
        prebuilt_extracted: None,
    })
}

/// Parse + classe une ligne JSON Judilibre (port de `_classify_judilibre_line`).
pub(super) fn classify_judilibre(line: Vec<u8>) -> Result<Option<Candidate>> {
    let payload: serde_json::Value =
        sonic_rs::from_slice(&line).context("ligne Judilibre JSON invalide")?;
    let decision = parse_judilibre(&payload, None);
    if decision.jurisdiction_type.is_none() {
        tracing::warn!(uid = %decision.source_uid, "UID Judilibre non reconnu, skip");
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
        dila_fond: None,
        prebuilt_source_fields: None,
        prebuilt_extracted: None,
    }))
}

/// Triage idempotent d'un batch (port de `_triage_candidates`, mode
/// `MISSING_HASH`) : dédup intra-batch par `source_uid` (last-wins) + précheck
/// DB groupé. Renvoie `(survivants, skipped_unchanged, deduped)`.
///
/// `require_embeddings` : si la décision existe, qu'elle a le même hash mais
/// qu'il manque ses embeddings, on la re-traite quand même (re-embed).
pub(super) fn triage_candidates(
    candidates: Vec<Candidate>,
    existing: &HashMap<String, ExistingDecisionState>,
    require_embeddings: bool,
    mode: IngestMode,
) -> (Vec<Candidate>, usize, usize) {
    // Dédup intra-batch : dernier candidat par source_uid gagne (dict Python).
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
                // Mode ALL : UPDATE complet inconditionnel (ignore le hash).
                if mode == IngestMode::All {
                    cand.decision_id = Some(prev.id);
                    cand.public_id = prev.public_id.clone().unwrap_or_else(generate_public_id);
                    cand.write_mode = WriteMode::Full;
                    survivors.push(cand);
                    continue;
                }
                let same_hash = prev.content_checksum == cand.content_checksum;
                if same_hash {
                    // Re-embed si embeddings manquants.
                    if require_embeddings && !prev.has_embeddings {
                        cand.decision_id = Some(prev.id);
                        cand.public_id = prev.public_id.clone().unwrap_or_else(generate_public_id);
                        survivors.push(cand);
                        continue;
                    }
                    // Backfill des identifiants (source_uid / public_id) si incomplet.
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
                // Hash différent → UPDATE complet.
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

/// Clean + chunk + extract + gzip d'un survivant (port de `_prepare_write`).
/// `None` → texte vide après clean (embargo Judilibre / XML vide) : skip non
/// fatal, comptabilisé dans `empty_skipped`.
pub(super) fn prepare_write(
    candidate: Candidate,
    chunk_tokens: usize,
    ctx: &super::ExtractCtx,
) -> Result<Option<PreparedDecision>> {
    if candidate.write_mode == WriteMode::SourceXmlOnly {
        // Backfill du `public_id` (+ format) seul : la ligne `decisions` n'est pas
        // réécrite → pas de source_fields à bâtir (la provenance porte déjà le
        // sien). Le payload brut n'est plus stocké (ADR 0085).
        return Ok(Some(PreparedDecision {
            decision_id: candidate.decision_id,
            public_id: candidate.public_id,
            decision: candidate.decision,
            content_checksum: candidate.content_checksum,
            write_mode: candidate.write_mode,
            chunks: Vec::new(),
            payload_format: candidate.payload_format,
            extracted: None,
            source_fields: serde_json::Value::Null,
        }));
    }

    let source_uid = candidate.decision.source_uid.clone();

    // source_fields (ADR 0085) : payload moins le texte, offsets rebasés sur
    // full_text. JSON → depuis le payload + sections ; XML → scalaires
    // <Dossier>/<Audience> (sections recalculées au rendu/re-chunk).
    //
    // Si `prebuilt_source_fields` est fourni, on l'utilise tel quel pour TOUTES
    // les familles : c'est le chemin reconstruction-depuis-stockage (#39 re-embed,
    // CEDH/CJUE/ArianeWeb `html`/`pdf`) où le `source_fields` canonique est déjà
    // en base et où `raw_payload` n'existe plus (ADR 0085). En ingest normal il
    // est `None` → on reconstruit depuis `raw_payload` par format (ci-dessous).
    let source_fields = match candidate.prebuilt_source_fields.clone() {
        Some(sf) => sf,
        None => match candidate.payload_format.as_str() {
            "json" => {
                let payload: serde_json::Value = sonic_rs::from_slice(&candidate.raw_payload)
                    .with_context(|| format!("re-parse JSON pour source_fields {source_uid}"))?;
                build_source_fields(&payload, &candidate.decision.sections)
            }
            "xml" => build_source_fields_xml(&candidate.raw_payload),
            "dila-xml" => {
                let fond = candidate
                    .dila_fond
                    .ok_or_else(|| anyhow!("source_fields dila-xml sans fond {source_uid}"))?;
                // `raw_payload` porte les octets DÉJÀ réparés (mojibake + entités) :
                // `source_fields` est donc propre, alors que `content_checksum` reste
                // calculé sur le brut pré-repair (idempotence #7).
                build_source_fields_dila(&candidate.raw_payload, fond)
            }
            "html" | "pdf" => {
                // `source_fields` préconstruits au classify, pas reconstruits du
                // `raw_payload` (qui ne porte plus le texte source) : CEDH/CJUE depuis
                // les métadonnées HUDOC/CDM (`html`, ADR 0094), ArianeWeb depuis le hit
                // xsearch (`html` AJCE / `pdf` CRP, ADR 0095). Les colonnes ne portent
                // pas de texte (il vit dans `full_text`).
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

    // Chemin LINÉAIRE (#26/#34/#37, ADR 0085) : TOUTES les familles passent par
    // `Decision::from_source_fields` (le format a déjà été validé ci-dessus en
    // bâtissant `source_fields`). Chunking + extraction tournent ainsi sur la
    // représentation canonique unique, identique à l'aval (re-extract, rendu) —
    // parité prouvée à 0 écart (`bench extract-fields-parity`, toutes familles).
    let decision = Decision::from_source_fields(
        &candidate.decision.texte_integral_clean,
        &source_fields,
        &source_uid,
    );

    let cleaned = &decision.texte_integral_clean;
    if cleaned.is_empty() {
        tracing::warn!(uid = %source_uid, "Texte vide après clean, skip");
        return Ok(None);
    }

    // Chunking en mode char (tokenizer=None) : chemin nominal (rapide, sans
    // tokenizer). Si l'heuristique chars/token sous-estime et qu'un chunk dépasse
    // le contexte de l'embedder, `embed_writes` re-chunke ce batch en BPE exact
    // (`rechunk_bpe`) et ré-essaie.
    let chunks = chunk_char(
        cleaned,
        &decision.metadata_header,
        &decision.visa_trim,
        chunk_tokens,
        DEFAULT_OVERLAP_MIN,
        DEFAULT_OVERLAP_MAX,
        None,
    )
    .map_err(|e| anyhow!("chunk_char {source_uid}: {e}"))?;

    // Champs structurés : préconstruits par le parser pour les fonds scrapés hors
    // nomenclature opendata/Judilibre (CNDA, ADR 0096 — `extract::routed` ne route
    // que les 7 ordres FR), sinon dérivés via l'extracteur routé (sur la `Decision`
    // canonique du chemin linéaire pour opendata/Judilibre).
    let extracted = match candidate.prebuilt_extracted.clone() {
        Some(fields) => fields,
        None => lj_ingest::extract::extracted_fields(
            &decision,
            &ctx.link,
            &ctx.vocab,
            &ctx.chrono,
            &ctx.jur_labels,
        )
        .map_err(|e| anyhow!("extract {source_uid}: {e}"))?,
    };

    Ok(Some(PreparedDecision {
        decision_id: candidate.decision_id,
        public_id: candidate.public_id,
        decision,
        content_checksum: candidate.content_checksum,
        write_mode: candidate.write_mode,
        chunks,
        payload_format: candidate.payload_format,
        extracted: Some(extracted),
        source_fields,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::tests_support::test_decision;

    // Spec : triage MISSING_HASH — nouveau survit, hash identique skip,
    // hash différent → update, dédup intra-batch last-wins.
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
                dila_fond: None,
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
                has_embeddings: true,
                public_id: Some("pa".to_string()),
            },
        );
        existing.insert(
            "b".to_string(),
            ExistingDecisionState {
                id: 2,
                source_uid: "b".to_string(),
                content_checksum: "old".to_string(),
                has_embeddings: true,
                public_id: Some("pb".to_string()),
            },
        );

        let candidates = vec![
            cand("a", "h1"),  // identique → skip
            cand("b", "new"), // changé → update (decision_id = 2)
            cand("c", "h3"),  // nouveau → survit
        ];
        let (survivors, skipped, deduped) =
            triage_candidates(candidates, &existing, false, IngestMode::MissingHash);
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
                dila_fond: None,
                prebuilt_source_fields: None,
                prebuilt_extracted: None,
            }
        }
        let existing = HashMap::new();
        let candidates = vec![cand("a", "h1"), cand("a", "h2")];
        let (survivors, _skipped, deduped) =
            triage_candidates(candidates, &existing, false, IngestMode::MissingHash);
        assert_eq!(deduped, 1);
        assert_eq!(survivors.len(), 1);
        assert_eq!(survivors[0].content_checksum, "h2");
    }

    // Spec mode ALL : hash identique → UPDATE complet forcé (pas de skip),
    // decision_id repris de l'existant, write_mode FULL.
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
                dila_fond: None,
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
                has_embeddings: true,
                public_id: Some("pa".to_string()),
            },
        );
        // Hash identique : skippé en MISSING_HASH, mais re-traité en ALL.
        let candidates = vec![cand("a", "same")];
        let (survivors, skipped, _) =
            triage_candidates(candidates, &existing, false, IngestMode::All);
        assert_eq!(skipped, 0);
        assert_eq!(survivors.len(), 1);
        assert_eq!(survivors[0].decision_id, Some(7));
        assert_eq!(survivors[0].write_mode, WriteMode::Full);
    }
}
