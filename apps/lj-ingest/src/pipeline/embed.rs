//! Construction des backends embedding + embed/re-chunk d'un batch (I/O tokio).

use anyhow::{anyhow, Result};
use std::sync::OnceLock;

use lj_llm::backend::{AnyEmbedder, DummyEmbedder, Embedder};
use lj_llm::cloudflare::CloudflareWorkersAIEmbedder;
use lj_llm::error::EmbedError;
use lj_llm::openai_http::OpenAIHttpEmbedder;
use lj_store::repository::{BulkDecisionWrite, ChunkWrite};

use crate::chunking::{
    chunk_bpe, Tokenizer, DEFAULT_OVERLAP_MAX, DEFAULT_OVERLAP_MIN, EMBED_VERSION,
};
use crate::config::Settings;

use super::{PreparedDecision, WriteMode};

/// Construit l'embedder documentaire selon `settings.embed_backend` (port de
/// `cli._build_embedder`). `none` doit être filtré par l'appelant (le pipeline
/// n'instancie pas d'embedder dans ce cas).
///
/// `auto` sonde vLLM (`embed_url`) : joignable → `openai-http` (GPU local,
/// gratuit) ; sinon repli sur Cloudflare Workers AI (avec cap neurons). Le cap
/// `ingest_neuron_budget` s'applique sur tout chemin Cloudflare (explicite ou
/// repli `auto`).
pub(super) async fn build_embedder(settings: &Settings) -> Result<AnyEmbedder> {
    match settings.embed_backend.as_str() {
        "none" => Err(anyhow!(
            "build_embedder appelé avec backend=none (l'appelant doit ne pas embedder)"
        )),
        "dummy" => Ok(AnyEmbedder::Dummy(DummyEmbedder::default())),
        "openai-http" => Ok(build_openai_http(settings)),
        "cloudflare" => build_cloudflare(settings),
        "auto" => {
            if probe_vllm(settings).await {
                tracing::info!(url = %settings.embed_url, "backend=auto : vLLM joignable → openai-http");
                Ok(build_openai_http(settings))
            } else {
                tracing::warn!(
                    url = %settings.embed_url,
                    "backend=auto : vLLM injoignable → repli Cloudflare Workers AI"
                );
                build_cloudflare(settings)
            }
        }
        other => Err(anyhow!(
            "backend inconnu : {other:?}. Choix : none | dummy | openai-http | cloudflare | auto"
        )),
    }
}

/// Embedder « par défaut » des sources qui embeddent à l'ingest : `None` si
/// `embed_backend == "none"` (ingest sans vecteurs, explicite), sinon construit
/// l'embedder du backend configuré (`auto` → vLLM/Cloudflare). Les nouvelles
/// sources (DILA, CEDH, CJUE, ArianeWeb, CNDA) l'utilisent → embeddings par
/// défaut, sans flag (un chunk sans embedding est inutile à la recherche
/// vectorielle). Renvoie `(embedder, require_embeddings)`.
pub(super) async fn build_embedder_opt(settings: &Settings) -> Result<(Option<AnyEmbedder>, bool)> {
    if settings.embed_backend == "none" {
        return Ok((None, false));
    }
    Ok((Some(build_embedder(settings).await?), true))
}

/// Embedder vLLM **strict**, jamais de repli Cloudflare : pour les ops de
/// maintenance (re-embed ciblé #39) qui ne doivent JAMAIS basculer sur un backend
/// payant (règle projet : embeddings via vLLM local uniquement). Probe vLLM et
/// erreur franche s'il est injoignable (contrairement à `backend=auto` qui
/// retombe sur Cloudflare). `dummy` accepté (tests) ; `cloudflare` refusé.
pub(super) async fn build_vllm_strict(settings: &Settings) -> Result<AnyEmbedder> {
    match settings.embed_backend.as_str() {
        "dummy" => Ok(AnyEmbedder::Dummy(DummyEmbedder::default())),
        "cloudflare" => Err(anyhow!(
            "re-embed maintenance refuse backend=cloudflare (coût) — configure vLLM (openai-http/auto)"
        )),
        _ => {
            if !probe_vllm(settings).await {
                return Err(anyhow!(
                    "re-embed maintenance : vLLM injoignable ({}) — pas de repli Cloudflare (coût)",
                    settings.embed_url
                ));
            }
            tracing::info!(url = %settings.embed_url, "re-embed maintenance : vLLM joignable → openai-http (strict)");
            Ok(build_openai_http(settings))
        }
    }
}

/// Embedder vLLM (`/v1/embeddings` OpenAI-compatible).
fn build_openai_http(settings: &Settings) -> AnyEmbedder {
    AnyEmbedder::OpenAiHttp(OpenAIHttpEmbedder::new(
        settings.embed_url.clone(),
        settings.embed_api_key.clone(),
        String::new(),
    ))
}

/// Embedder Cloudflare Workers AI, avec cap neurons issu de `Settings`.
fn build_cloudflare(settings: &Settings) -> Result<AnyEmbedder> {
    let account_id = settings
        .cloudflare_account_id
        .clone()
        .ok_or_else(|| anyhow!("backend=cloudflare : LIBREJUSTICE_CLOUDFLARE_ACCOUNT_ID requis"))?;
    let token = settings.cloudflare_backend_token.clone().ok_or_else(|| {
        anyhow!("backend=cloudflare : LIBREJUSTICE_CLOUDFLARE_BACKEND_TOKEN requis")
    })?;
    Ok(AnyEmbedder::Cloudflare(
        CloudflareWorkersAIEmbedder::new(
            account_id,
            token,
            CloudflareWorkersAIEmbedder::DEFAULT_MODEL,
        )
        .with_neuron_budget(settings.ingest_neuron_budget),
    ))
}

/// Sonde la disponibilité de vLLM : GET `…/v1/models` (dérivé de `embed_url`),
/// timeout court. Un 2xx ⇒ backend local utilisable. Toute erreur (réseau,
/// timeout, statut non-2xx) ⇒ `false` (repli). N'embarque aucun retry : le repli
/// Cloudflare est la stratégie de robustesse, pas une re-tentative locale.
async fn probe_vllm(settings: &Settings) -> bool {
    let base = settings
        .embed_url
        .trim_end_matches('/')
        .trim_end_matches("/v1/embeddings");
    let url = format!("{base}/v1/models");
    let client = reqwest::Client::new();
    let mut req = client.get(&url).timeout(std::time::Duration::from_secs(3));
    if let Some(key) = &settings.embed_api_key {
        req = req.bearer_auth(key);
    }
    matches!(req.send().await, Ok(resp) if resp.status().is_success())
}

/// Construit le `BulkDecisionWrite` à partir d'un préparé + embeddings optionnels
/// (port de `_to_bulk_write`).
fn to_bulk_write(
    prepared: PreparedDecision,
    embeddings: Option<Vec<Vec<f32>>>,
) -> BulkDecisionWrite {
    let n = prepared.chunks.len();
    let had_embeddings = embeddings.is_some();
    let mut embeds: Vec<Option<Vec<f32>>> = match embeddings {
        Some(v) => {
            debug_assert_eq!(v.len(), n, "embeddings/chunks count mismatch");
            v.into_iter().map(Some).collect()
        }
        None => vec![None; n],
    };
    let chunks: Vec<ChunkWrite> = prepared
        .chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| ChunkWrite {
            chunk_index: chunk.chunk_index as i32,
            char_start: chunk.char_start as i32,
            char_end: chunk.char_end as i32,
            body: chunk.body,
            embedding: embeds.get_mut(i).and_then(Option::take),
        })
        .collect();

    // `canonical_ref` (ADR 0100) calculé à l'ingest, passé à l'upsert : lj-store
    // ne tire plus l'extracteur (ADR 0123 §3).
    let canonical_ref = lj_ingest::extract::canonical_ref(&prepared.decision);

    BulkDecisionWrite {
        decision_id: prepared.decision_id,
        public_id: prepared.public_id,
        decision: prepared.decision,
        content_checksum: prepared.content_checksum,
        canonical_ref,
        write_mode: prepared.write_mode.as_str().to_string(),
        chunks,
        payload_format: prepared.payload_format,
        extracted: prepared.extracted,
        source_fields: prepared.source_fields,
        // embed_version tague les embeddings qu'on vient d'écrire ; None quand on
        // chunke sans embedder (les chunks portent un embedding NULL).
        embed_version: had_embeddings.then_some(EMBED_VERSION),
    }
}

/// Tokenizer Qwen3-Embedding-0.6B, récupéré au build (`build.rs`) et figé dans le
/// binaire `lj-ingest` — seul consommateur du mode BPE exact (`lj-core` reste pur,
/// ne reçoit qu'un `&Tokenizer`).
const QWEN_TOKENIZER_JSON: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/qwen3_embedding_tokenizer.json"));

/// Tokenizer partagé, chargé une fois (port de `get_tokenizer`, `@lru_cache`).
/// Mode BPE exact : budget en tokens réels → `embedding_text` garanti ≤ `k`.
pub(super) fn get_tokenizer() -> &'static Tokenizer {
    static TOKENIZER: OnceLock<Tokenizer> = OnceLock::new();
    TOKENIZER.get_or_init(|| {
        Tokenizer::from_bytes(QWEN_TOKENIZER_JSON).expect("tokenizer Qwen3-Embedding illisible")
    })
}

/// Re-chunke un batch en mode BPE exact (port de `_rechunk_bpe`).
///
/// Appelé quand l'embedder rejette un chunk trop long (sous-estimation du mode
/// char). Le tokenizer Qwen donne le budget token exact → `embedding_text`
/// garanti ≤ `chunk_tokens`. Les écritures `SourceXmlOnly` (sans chunk
/// embeddable) sont laissées telles quelles.
fn rechunk_bpe(
    writes: Vec<PreparedDecision>,
    chunk_tokens: usize,
) -> Result<Vec<PreparedDecision>> {
    let tokenizer = get_tokenizer();
    let mut out = Vec::with_capacity(writes.len());
    for mut w in writes {
        if w.write_mode == WriteMode::SourceXmlOnly {
            out.push(w);
            continue;
        }
        w.chunks = chunk_bpe(
            &w.decision.texte_integral_clean,
            &w.decision.metadata_header,
            &w.decision.visa_trim,
            chunk_tokens,
            DEFAULT_OVERLAP_MIN,
            DEFAULT_OVERLAP_MAX,
            tokenizer,
            None,
        )
        .map_err(|e| anyhow!("rechunk BPE {}: {e}", w.decision.source_uid))?;
        out.push(w);
    }
    Ok(out)
}

/// Embed les chunks d'un batch préparé (port de `_embed_writes`).
///
/// Aplatit tous les textes de chunk, appelle `embed_passages`, redécoupe par
/// décision. Sans embedder → tous les embeddings à `None`.
pub(super) async fn embed_writes<E: Embedder>(
    embedder: Option<&E>,
    writes: Vec<PreparedDecision>,
    chunk_tokens: usize,
) -> Result<Vec<BulkDecisionWrite>> {
    let Some(embedder) = embedder else {
        return Ok(writes.into_iter().map(|w| to_bulk_write(w, None)).collect());
    };

    let mut writes = writes;
    let mut flat_texts: Vec<String> = writes
        .iter()
        .flat_map(|w| w.chunks.iter().map(|c| c.embedding_text()))
        .collect();
    if flat_texts.is_empty() {
        return Ok(writes.into_iter().map(|w| to_bulk_write(w, None)).collect());
    }

    // Le chunk nominal est en mode char (heuristique chars/token) : il peut
    // sous-estimer sur un texte dense et produire un chunk > 8192 tokens, que
    // l'embedder rejette. On re-chunke alors le batch en BPE exact (budget token
    // réel, plus d'overflow) et on ré-essaie une fois (port de `_embed_writes` +
    // `_rechunk_bpe`, Python ; tokenisation exacte offline, ADR 0010).
    let vecs = match embedder.embed_passages(&flat_texts).await {
        Ok(v) => v,
        Err(EmbedError::InputTooLong) => {
            tracing::warn!(
                batch = writes.len(),
                "embed: overflow contexte (mode char) → re-chunk BPE exact + retry"
            );
            writes = rechunk_bpe(writes, chunk_tokens)?;
            flat_texts = writes
                .iter()
                .flat_map(|w| w.chunks.iter().map(|c| c.embedding_text()))
                .collect();
            embedder
                .embed_passages(&flat_texts)
                .await
                .map_err(|e| anyhow!("embed_passages (post re-chunk BPE): {e}"))?
        }
        Err(e) => return Err(anyhow!("embed_passages: {e}")),
    };

    let mut out = Vec::with_capacity(writes.len());
    let mut offset = 0usize;
    for write in writes {
        let count = write.chunks.len();
        let mut embeds = Vec::with_capacity(count);
        for i in 0..count {
            embeds.push(vecs.row(offset + i).to_vec());
        }
        offset += count;
        out.push(to_bulk_write(write, Some(embeds)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunking::{DEFAULT_CHUNK_TOKENS, DEFAULT_OVERLAP_MAX, DEFAULT_OVERLAP_MIN};

    /// Spec : en mode BPE exact, `embedding_text` d'aucun chunk ne dépasse le
    /// budget token K. Invariant qui rend correct le re-chunk de secours
    /// (`rechunk_bpe`) déclenché sur overflow embedder.
    #[test]
    fn bpe_rechunk_keeps_embedding_text_within_budget() {
        let tok = get_tokenizer();
        let paragraph = "Il résulte de l'instruction que la requête est rejetée. ";
        let long_text = vec![paragraph; 1200].join("\n\n");
        let header = "Tribunal administratif de Paris — 2024";
        let visa =
            "Vu la requête enregistrée le 1er janvier 2024 ; Vu le code de justice administrative ;";

        let chunks = chunk_bpe(
            &long_text,
            header,
            visa,
            DEFAULT_CHUNK_TOKENS,
            DEFAULT_OVERLAP_MIN,
            DEFAULT_OVERLAP_MAX,
            tok,
            None,
        )
        .unwrap();
        assert!(
            chunks.len() >= 2,
            "le texte doit produire plusieurs chunks, got {}",
            chunks.len()
        );
        for c in &chunks {
            let n_tok = tok
                .encode_char_offsets(c.embedding_text(), true)
                .unwrap()
                .get_ids()
                .len();
            assert!(
                n_tok <= DEFAULT_CHUNK_TOKENS,
                "overflow E_i chunk[{}] en mode BPE: {} > {}",
                c.chunk_index,
                n_tok,
                DEFAULT_CHUNK_TOKENS
            );
        }
    }
}
