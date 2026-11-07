//! `cargo test -p lj-llm --test mistral_live_scale -- --ignored --nocapture`, one
//! key. `/v1/conversations` announces a token ceiling and no request one, so what a
//! key is worth in calls a minute is that ceiling divided by what a call bills:
//! asking for one token makes each call as cheap as it gets.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use lj_llm::mistral::{MistralClient, MistralError};

const MODEL: &str = "ministral-3b-2512";
const SYSTEM: &str = "Réponds par un mot.";
const USER: &str = "ping";
const MAX_TOKENS: u32 = 1;
const WARMUP: usize = 4;
const RUN_SECS: f64 = 20.0;
const MARGIN: f64 = 2.0;

/// Throughput a pool reaches with the drivers the measured demand asks for, against
/// one driver per key. The gap is the quota that a fixed driver count leaves
/// unspent.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "calls the live Mistral API"]
async fn a_pool_serves_what_its_quota_pays_for_only_with_the_drivers_it_asks() {
    let slot = std::env::var("LIBREJUSTICE_BENCH_MISTRAL_SLOT").unwrap_or_else(|_| "4".into());
    let key = std::env::var(format!("LIBREJUSTICE_MISTRAL_OFFLINE_KEY{slot}"))
        .unwrap_or_else(|_| panic!("LIBREJUSTICE_MISTRAL_OFFLINE_KEY{slot} is unset"));
    let client =
        Arc::new(MistralClient::new(vec![key], MODEL.to_string()).expect("client mistral"));

    // A bucket claims nothing before it has served one: the warm-up is what gives it
    // a round trip and a billed cost to divide the ceiling by.
    for _ in 0..WARMUP {
        client
            .chat(SYSTEM, USER, Some(MAX_TOKENS))
            .await
            .expect("appel de chauffe");
    }
    let (demande, muettes) = client.demand();
    println!(
        "after warm-up: demand={demande:.1} calls in flight, silent={muettes}, rate={:.0}/min",
        client.rate_per_min()
    );
    assert_eq!(muettes, 0, "la chauffe n'a pas renseigné le seau");

    let seul = burst(&client, 1).await;
    let asservi = (demande * MARGIN).ceil() as usize;
    let large = burst(&client, asservi).await;

    println!(
        "1 driver: {seul:.0}/min | {asservi} drivers: {large:.0}/min | gain x{:.1}",
        large / seul.max(1.0)
    );
    assert!(
        asservi > 1,
        "la demande mesurée ne dépasse pas une clé : {demande:.2}"
    );
    assert!(
        large > seul * 2.0,
        "les drivers asservis n'ont rien rapporté : {large:.0} contre {seul:.0}"
    );
}

/// Calls a minute served by `drivers` of them over [`RUN_SECS`].
async fn burst(client: &Arc<MistralClient>, drivers: usize) -> f64 {
    let served = Arc::new(AtomicU64::new(0));
    let start = tokio::time::Instant::now();
    let mut running = Vec::new();
    for _ in 0..drivers {
        let client = Arc::clone(client);
        let served = Arc::clone(&served);
        running.push(tokio::spawn(async move {
            while start.elapsed().as_secs_f64() < RUN_SECS {
                match client.chat_now(SYSTEM, USER, Some(MAX_TOKENS)).await {
                    Ok(_) => {
                        served.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(MistralError::Busy(wait)) => {
                        tokio::time::sleep(std::time::Duration::from_secs_f64(wait.max(0.05)))
                            .await;
                    }
                    Err(_) => {}
                }
            }
        }));
    }
    for driver in running {
        driver.await.expect("driver");
    }
    60.0 * served.load(Ordering::Relaxed) as f64 / start.elapsed().as_secs_f64()
}
