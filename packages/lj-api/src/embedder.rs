//! Sélection du backend d'embedding côté API (port de `embedder.build_embedder`).
//!
//! L'embedder est partagé pour toute la durée du process : pas de coût d'init
//! par requête (Cloudflare ouvre un pool HTTP `reqwest`, vLLM est un simple
//! POST HTTP). On ne le ré-instancie jamais en réponse à une requête.

use crate::config::Settings;
use lj_llm::backend::{AnyEmbedder, BackendHealth, DummyEmbedder};
use lj_llm::cloudflare::CloudflareWorkersAIEmbedder;
use lj_llm::openai_http::OpenAIHttpEmbedder;

/// Construit l'embedder de requête selon `settings.embed_backend`
/// (`dummy` / `openai-http` / `cloudflare` / `auto`).
///
/// Port de `build_embedder` (Python). Les conditions `ValueError` du Python
/// (creds manquantes pour `openai-http`/`cloudflare`/`auto`) deviennent des
/// `panic!` — la signature est infaillible et l'embedder est construit une
/// fois au démarrage : une config invalide doit faire échouer le boot, pas une
/// requête.
///
/// Mode `auto` (port de `AutoEmbedder`/`_BackendHealth`) : vLLM (OpenAI-HTTP) à
/// l'URL résolue (`embed_url` ou `http://localhost:8400/v1/embeddings`) en
/// priorité, fallback Cloudflare Workers AI piloté par un failure-score (EMA
/// alpha=0.70, demi-vie 1 h, skip au-dessus de 0.80). Les creds Cloudflare sont
/// requises (parité des préconditions Python) car elles servent au fallback.
pub fn build_query_embedder(settings: &Settings) -> AnyEmbedder {
    match settings.embed_backend.as_str() {
        "dummy" => AnyEmbedder::Dummy(DummyEmbedder::default()),
        "openai-http" => {
            let url = settings
                .embed_url
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    panic!("embed_backend=openai-http nécessite LIBREJUSTICE_EMBED_URL.")
                });
            AnyEmbedder::OpenAiHttp(OpenAIHttpEmbedder::new(
                url,
                settings.embed_api_key.clone(),
                "",
            ))
        }
        "cloudflare" => {
            let (account_id, token) = require_cloudflare(
                settings,
                "embed_backend=cloudflare nécessite \
                 LIBREJUSTICE_CLOUDFLARE_ACCOUNT_ID + \
                 LIBREJUSTICE_CLOUDFLARE_BACKEND_TOKEN.",
            );
            AnyEmbedder::Cloudflare(CloudflareWorkersAIEmbedder::new(
                account_id,
                token,
                CloudflareWorkersAIEmbedder::DEFAULT_MODEL,
            ))
        }
        "auto" => {
            // Parité Python : `auto` valide les creds Cloudflare (fallback)
            // avant de construire le backend vLLM primaire.
            let (account_id, token) = require_cloudflare(
                settings,
                "embed_backend=auto nécessite \
                 LIBREJUSTICE_CLOUDFLARE_ACCOUNT_ID + \
                 LIBREJUSTICE_CLOUDFLARE_BACKEND_TOKEN pour le fallback cloudflare.",
            );
            let vllm_url = settings
                .embed_url
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("http://localhost:8400/v1/embeddings");
            let vllm = OpenAIHttpEmbedder::new(vllm_url, settings.embed_api_key.clone(), "");
            let cloudflare = CloudflareWorkersAIEmbedder::new(
                account_id,
                token,
                CloudflareWorkersAIEmbedder::DEFAULT_MODEL,
            );
            AnyEmbedder::Auto {
                vllm,
                cloudflare,
                health: BackendHealth::new(),
            }
        }
        other => panic!("embed_backend inconnu : {other:?}"),
    }
}

/// Retourne `(account_id, backend_token)` ou `panic!` avec `msg` si l'un manque
/// (parité avec les `ValueError` Python pour `cloudflare`/`auto`).
fn require_cloudflare<'a>(settings: &'a Settings, msg: &str) -> (&'a str, &'a str) {
    match (
        settings.cloudflare_account_id.as_deref(),
        settings.cloudflare_backend_token.as_deref(),
    ) {
        (Some(id), Some(token)) if !id.is_empty() && !token.is_empty() => (id, token),
        _ => panic!("{msg}"),
    }
}
