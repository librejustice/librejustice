//! Cache de requetes (leptos-fetch). Port des conventions de
//! `apps/web/src/lib/query-client.ts` :
//!   - `stale_time` 1 h (aligne sur le TTL du cache recherche API) : tant que la donnee
//!     est fresh, un remontage / une nav SPA de retour vers une recherche deja en
//!     cache ne refetch pas (verifie : retour decision -> liste = 0 appel) -> pas
//!     de flash. A l'hydratation initiale, la ressource streamee (non bloquante,
//!     cf. `search_data`) declenche tout de meme UN fetch client dedupe : sa valeur
//!     est ecrasee par le stream SSR (`SsrStreamedValueOverride` de leptos-fetch),
//!     mais l'execution search cote serveur a bien lieu (cache hit ~6 ms). Cout
//!     inherent au choix streaming (vs `resource_blocking` = page blanche ~3 s),
//!     pas un flash.
//!   - `gc_time` 2 h (> `stale_time`) : une entree inactive survit tout le temps
//!     ou elle est fresh (1 h) PUIS une fenetre stale-while-revalidate (1 h de
//!     plus) ou un retour de nav peint la version stale instantanement puis
//!     revalide en arriere-plan, au lieu d'un fetch a froid.
//!
//! leptos-fetch gere la dualite SSR/navigateur en interne (instance fournie au
//! contexte ; dehydrate/rehydrate des ressources via le SharedContext leptos),
//! la ou le React legacy distinguait manuellement `makeQueryClient` (SSR neuf)
//! vs singleton navigateur. On fournit donc une seule fois au contexte racine.

use std::time::Duration;

use leptos_fetch::{QueryClient, QueryOptions};

/// staleTime aligne sur le TTL du cache recherche API (1 h).
const STALE_TIME: Duration = Duration::from_secs(60 * 60);
/// gcTime 2 h : > staleTime, fenetre stale-while-revalidate (voir doc module).
const GC_TIME: Duration = Duration::from_secs(2 * 60 * 60);

/// Construit le `QueryClient` (SWR) et le fournit au contexte Leptos. Appele une
/// fois dans `App`. Renvoie le client pour un usage immediat eventuel.
pub fn provide_query_client() -> QueryClient {
    QueryClient::new()
        .with_options(
            QueryOptions::new()
                .with_stale_time(STALE_TIME)
                .with_gc_time(GC_TIME),
        )
        .provide()
}
