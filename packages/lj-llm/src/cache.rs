//! Cache in-process des embeddings de query/passages.
//!
//! Parité sémantique avec le cache côté serveur Python (`embedder.py`) : un
//! vecteur est stable pour un texte donné, TTL 7 jours (`_EMB_TTL`). La clé est
//! fournie par l'appelant (le serveur calcule `emb:md5(text)`).
//!
//! Mono-serveur mono-process (axum) : plus de Valkey/Redis, le cache vit
//! in-process via `moka`. Rien de caché ne nécessite de persistance. La valeur
//! décodée (le vecteur) est stockée directement plutôt qu'un tableau JSON.

use moka::future::Cache;
use ndarray::Array1;
use std::sync::Arc;
use std::time::Duration;

/// TTL d'un vecteur caché : 7 jours (`_EMB_TTL` Python). Un vecteur est stable
/// pour un texte donné.
const EMB_TTL_SECONDS: u64 = 604_800;

/// Plafond **mémoire** par défaut (octets) : 50 Mio des 200 Mio totaux du process
/// (le cache résultats de recherche prend les 150 Mio restants, cf. `lj-api`).
/// Une entrée ≈ vecteur 1024 `f32` (~4 Kio) → ~12 000 vecteurs chauds.
const DEFAULT_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// Overhead moka par entrée (nœud de la map + bookkeeping TinyLFU), ajouté au
/// poids pesé pour que le plafond couvre la mémoire réelle, pas que le payload.
const ENTRY_OVERHEAD: usize = 128;

/// Cache d'embeddings in-process (clé = hash du texte ; valeur = vecteur). TTL
/// 7 jours, éviction par **poids mémoire** (TinyLFU). Clonable et partageable
/// entre tâches async (`moka::future::Cache` est `Arc` en interne).
#[derive(Clone)]
pub struct EmbeddingCache {
    inner: Cache<String, Arc<Array1<f32>>>,
}

impl EmbeddingCache {
    /// Construit le cache avec un plafond mémoire (octets) et le TTL de 7 jours
    /// (parité `_EMB_TTL`). Le `weigher` pèse clé + vecteur + overhead moka.
    pub fn new(max_bytes: u64) -> Self {
        let inner = Cache::builder()
            .weigher(|k: &String, v: &Arc<Array1<f32>>| {
                (k.len() + v.len() * std::mem::size_of::<f32>() + ENTRY_OVERHEAD)
                    .min(u32::MAX as usize) as u32
            })
            .max_capacity(max_bytes)
            .time_to_live(Duration::from_secs(EMB_TTL_SECONDS))
            .build();
        Self { inner }
    }

    /// Récupère un vecteur caché par clé de texte, `None` si absent.
    pub async fn get(&self, key: &str) -> Option<Arc<Array1<f32>>> {
        self.inner.get(key).await
    }

    /// Stocke un vecteur sous une clé de texte (TTL 7 jours).
    pub async fn set(&self, key: &str, vector: Arc<Array1<f32>>) {
        self.inner.insert(key.to_string(), vector).await;
    }
}

impl Default for EmbeddingCache {
    /// Cache avec le plafond mémoire par défaut (`DEFAULT_MAX_BYTES`).
    fn default() -> Self {
        Self::new(DEFAULT_MAX_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_matches_python() {
        // 7 jours en secondes (parité _EMB_TTL).
        assert_eq!(EMB_TTL_SECONDS, 7 * 24 * 3600);
    }

    #[tokio::test]
    async fn get_set_roundtrip() {
        let cache = EmbeddingCache::new(1024 * 1024);
        let key = "emb:deadbeef";
        assert!(cache.get(key).await.is_none());

        let vec = Arc::new(Array1::from(vec![0.1_f32, 0.2, 0.3]));
        cache.set(key, vec.clone()).await;

        let got = cache.get(key).await.expect("vecteur présent après set");
        assert_eq!(&*got, &*vec);
    }

    #[tokio::test]
    async fn default_uses_default_capacity() {
        let cache = EmbeddingCache::default();
        assert_eq!(cache.inner.policy().max_capacity(), Some(DEFAULT_MAX_BYTES));
    }
}
