//! The embedding backends, and the tokenizer of the exact BPE cut.

use anyhow::{anyhow, Result};
use std::sync::OnceLock;

use lj_chunk::Tokenizer;
use lj_llm::backend::{AnyEmbedder, DummyEmbedder, Link};
use lj_llm::cloudflare::CloudflareWorkersAIEmbedder;
use lj_llm::hosts::{EmbedHost, Entry};
use lj_llm::openai_http::OpenAIHttpEmbedder;

use crate::config::Settings;

/// The passage embedder `settings.embed_backend` names; `none` embeds nothing
/// and is refused.
///
/// `auto` chains the links of `embed_chain` in priority order, each behind its own
/// breaker, up to `cloudflare-aiworker`. The `ingest_neuron_budget` cap applies on
/// every Cloudflare path.
pub(super) async fn build_embedder(settings: &Settings) -> Result<AnyEmbedder> {
    match settings.embed_backend.as_str() {
        "none" => Err(anyhow!("backend=none embeds nothing")),
        "dummy" => Ok(AnyEmbedder::Dummy(DummyEmbedder::default())),
        "openai-http" => Ok(build_openai_http(single_host(settings)?)),
        "cloudflare" => build_cloudflare(settings),
        "auto" => build_auto(settings).await,
        other => Err(anyhow!(
            "backend inconnu : {other:?}. Choix : none | dummy | openai-http | cloudflare | auto"
        )),
    }
}

pub(super) async fn build_local_strict(settings: &Settings) -> Result<AnyEmbedder> {
    match settings.embed_backend.as_str() {
        "dummy" => Ok(AnyEmbedder::Dummy(DummyEmbedder::default())),
        "cloudflare" => Err(anyhow!(
            "re-embed maintenance refuse backend=cloudflare (coût) : configure le local (openai-http/auto)"
        )),
        _ => {
            for host in hosts(settings) {
                if probe_host(host).await {
                    tracing::info!(url = %host.url, "re-embed maintenance: openai-http reachable, using openai-http (strict)");
                    return Ok(build_openai_http(host));
                }
            }
            Err(anyhow!(
                "re-embed maintenance: no local host reachable ({}), no Cloudflare fallback (cost)",
                urls(settings)
            ))
        }
    }
}

/// Embedder local (`/v1/embeddings` OpenAI-compatible).
fn build_openai_http(host: &EmbedHost) -> AnyEmbedder {
    AnyEmbedder::OpenAiHttp(OpenAIHttpEmbedder::new(
        host.url.clone(),
        host.api_key.clone(),
        String::new(),
    ))
}

/// `openai-http` has no breaker to walk a chain with.
fn single_host(settings: &Settings) -> Result<&EmbedHost> {
    match settings.embed_chain.as_slice() {
        [Entry::Host(host)] => Ok(host),
        chain => Err(anyhow!(
            "backend=openai-http takes one URL in LIBREJUSTICE_EMBED_URL, got {} entries: a chain needs backend=auto",
            chain.len()
        )),
    }
}

fn hosts(settings: &Settings) -> impl Iterator<Item = &EmbedHost> {
    settings.embed_chain.iter().filter_map(|entry| match entry {
        Entry::Host(host) => Some(host),
        Entry::Cloudflare => None,
    })
}

fn urls(settings: &Settings) -> String {
    let urls: Vec<&str> = hosts(settings).map(|h| h.url.as_str()).collect();
    urls.join(", ")
}

async fn build_auto(settings: &Settings) -> Result<AnyEmbedder> {
    let chain = &settings.embed_chain;
    // Passages run to 8,192 tokens: the chain ends at Cloudflare.
    let end = chain
        .iter()
        .position(|entry| *entry == Entry::Cloudflare)
        .map_or(chain.len(), |i| i + 1);
    tracing::info!(urls = %urls(settings), "backend=auto: breaker chain armed");
    let links = chain[..end]
        .iter()
        .map(|entry| match entry {
            Entry::Host(host) => Ok(Link::local(host)),
            Entry::Cloudflare => build_cloudflare_embedder(settings).map(Link::cloudflare),
        })
        .collect::<Result<_>>()?;
    Ok(AnyEmbedder::Auto(links))
}

fn build_cloudflare(settings: &Settings) -> Result<AnyEmbedder> {
    build_cloudflare_embedder(settings).map(AnyEmbedder::Cloudflare)
}

/// Embedder Cloudflare Workers AI, avec cap neurons issu de `Settings`.
fn build_cloudflare_embedder(settings: &Settings) -> Result<CloudflareWorkersAIEmbedder> {
    let account_id = settings
        .cloudflare_account_id
        .clone()
        .ok_or_else(|| anyhow!("Cloudflare Workers AI needs LIBREJUSTICE_CLOUDFLARE_ACCOUNT_ID"))?;
    let token = settings.cloudflare_backend_token.clone().ok_or_else(|| {
        anyhow!("Cloudflare Workers AI needs LIBREJUSTICE_CLOUDFLARE_BACKEND_TOKEN")
    })?;
    Ok(CloudflareWorkersAIEmbedder::new(
        account_id,
        token,
        CloudflareWorkersAIEmbedder::DEFAULT_MODEL,
    )
    .with_neuron_budget(settings.ingest_neuron_budget))
}

/// Probes a local host: GET `…/v1/models`, derived from its URL, short timeout.
/// A 2xx means usable, any error (network, timeout, non-2xx) means `false`, with
/// no retry.
async fn probe_host(host: &EmbedHost) -> bool {
    let base = host
        .url
        .trim_end_matches('/')
        .trim_end_matches("/v1/embeddings");
    let url = format!("{base}/v1/models");
    let client = reqwest::Client::new();
    let mut req = client.get(&url).timeout(std::time::Duration::from_secs(3));
    if let Some(key) = &host.api_key {
        req = req.bearer_auth(key);
    }
    matches!(req.send().await, Ok(resp) if resp.status().is_success())
}

/// Tokenizer Qwen3-Embedding-0.6B, récupéré au build (`build.rs`) et figé dans le
/// binaire `lj-ingest` — seul consommateur du mode BPE exact (`lj-core` reste pur,
/// ne reçoit qu'un `&Tokenizer`).
const QWEN_TOKENIZER_JSON: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/qwen3_embedding_tokenizer.json"));

/// Tokenizer partagé, chargé une fois.
/// Mode BPE exact : budget en tokens réels → `embedding_text` garanti ≤ `k`.
pub(super) fn get_tokenizer() -> &'static Tokenizer {
    static TOKENIZER: OnceLock<Tokenizer> = OnceLock::new();
    TOKENIZER.get_or_init(|| {
        Tokenizer::from_bytes(QWEN_TOKENIZER_JSON).expect("tokenizer Qwen3-Embedding illisible")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lj_chunk::{chunk_bpe, embedding_prefix, DEFAULT_CHUNK_TOKENS};

    /// In exact BPE mode no chunk's `embedding_text` exceeds the token budget K:
    /// what makes the fallback cut of `embed-missing` sound.
    #[test]
    fn bpe_rechunk_keeps_embedding_text_within_budget() {
        let tok = get_tokenizer();
        let paragraph = "Il résulte de l'instruction que la requête est rejetée. ";
        let long_text = vec![paragraph; 1200].join("\n\n");
        let prefix = embedding_prefix(
            "Tribunal administratif de Paris, 2024",
            Some("Le tribunal rejette la requête, faute d'intérêt à agir."),
        );

        let chunks = chunk_bpe(&long_text, &prefix, DEFAULT_CHUNK_TOKENS, tok, None).unwrap();
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
