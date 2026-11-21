use opentelemetry::{global, KeyValue};
use std::sync::Mutex;
use std::time::Duration;
use tracing::info;

use crate::state::AppState;

const SCRAPE_INTERVAL: Duration = Duration::from_secs(60);

struct Usage {
    cache: &'static str,
    entries: u64,
    weight: u64,
    capacity: Option<u64>,
}

static SNAPSHOT: Mutex<Vec<Usage>> = Mutex::new(Vec::new());

pub(crate) fn record_access(cache: &'static str, hit: bool) {
    let counter = global::meter("librejustice-cache")
        .u64_counter("librejustice.cache.requests")
        .with_description("Accès au cache, par issue.")
        .build();
    counter.add(
        1,
        &[
            KeyValue::new("cache", cache),
            KeyValue::new("outcome", if hit { "hit" } else { "miss" }),
        ],
    );
}

pub async fn run(state: AppState) {
    register_gauges();
    info!(
        interval_secs = SCRAPE_INTERVAL.as_secs(),
        "cache_metrics: scraper started"
    );

    let mut ticker = tokio::time::interval(SCRAPE_INTERVAL);
    loop {
        ticker.tick().await;
        scrape(&state).await;
    }
}

fn register_gauges() {
    let meter = global::meter("librejustice-cache");

    let _entries = meter
        .u64_observable_gauge("librejustice.cache.entries")
        .with_description("Entrées vivantes dans le cache.")
        .with_unit("{entry}")
        .with_callback(|observer| {
            for u in SNAPSHOT.lock().unwrap().iter() {
                observer.observe(u.entries, &[KeyValue::new("cache", u.cache)]);
            }
        })
        .build();

    let _utilization = meter
        .f64_observable_gauge("librejustice.cache.utilization")
        .with_description(
            "Fraction du plafond occupée, au sens du `weigher` : 1 signifie que \
             le cache évince à chaque insertion.",
        )
        .with_unit("1")
        .with_callback(|observer| {
            for u in SNAPSHOT.lock().unwrap().iter() {
                if let Some(cap) = u.capacity.filter(|c| *c > 0) {
                    observer.observe(
                        u.weight as f64 / cap as f64,
                        &[KeyValue::new("cache", u.cache)],
                    );
                }
            }
        })
        .build();
}

async fn scrape(state: &AppState) {
    macro_rules! usage {
        ($name:literal, $cache:expr) => {{
            let c = &$cache;
            c.run_pending_tasks().await;
            Usage {
                cache: $name,
                entries: c.entry_count(),
                weight: c.weighted_size(),
                capacity: c.max_capacity(),
            }
        }};
    }

    let mut out = vec![
        usage!("recherche", state.search_cache),
        usage!("parcours", state.browse_cache),
        usage!("textes", state.textes_cache),
        usage!("rerank", state.rerank_cache),
        usage!("notes_rerank", state.rerank_scores_cache),
        usage!("annonces", state.journals_cache),
        usage!("registre", state.registre_cache),
        usage!("texte_identite", state.text_ref_cache),
        usage!("sommaire_adresses", state.toc_address_cache),
        usage!("sommaire_versions", state.toc_source_cache),
        usage!("articles_versions", state.article_versions_cache),
    ];
    let (entries, weight, capacity) = state.embedding_cache.usage().await;
    out.push(Usage {
        cache: "embedding",
        entries,
        weight,
        capacity,
    });

    *SNAPSHOT.lock().unwrap() = out;
}
