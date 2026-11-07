//! `cargo test -p lj-llm --test gemini_live_rate -- --ignored --nocapture`, one key, four drivers, ninety seconds.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const MODEL: &str = "gemini-3.1-flash-lite";
const DRIVERS: usize = 4;
const RUN_SECS: f64 = 90.0;
const SYSTEM: &str = "Réponds par un seul mot.";
const USER: &str = "Quelle est la capitale de la France ?";

/// What a learnt bucket converges to against the real service, which no fixture can
/// state: the service announces a ceiling per project and per model, a bucket that
/// has learnt it holds within a cut of that, and one that ratcheted to the floor
/// serves one a minute.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "calls the live Gemini API and spends one key's daily quota"]
async fn a_learnt_bucket_holds_near_the_announced_ceiling() {
    let keys = std::env::var("LIBREJUSTICE_GEMINI_KEYS").expect("LIBREJUSTICE_GEMINI_KEYS");
    let key = keys
        .split(',')
        .map(str::trim)
        .find(|k| !k.is_empty())
        .expect("au moins une clé")
        .to_string();
    let client = Arc::new(
        lj_llm::gemini::GeminiClient::new(vec![key], MODEL.to_string(), None)
            .expect("client gemini"),
    );

    let served = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let start = tokio::time::Instant::now();
    let mut drivers = Vec::new();
    for _ in 0..DRIVERS {
        let client = Arc::clone(&client);
        let served = Arc::clone(&served);
        let failed = Arc::clone(&failed);
        drivers.push(tokio::spawn(async move {
            while start.elapsed().as_secs_f64() < RUN_SECS {
                match client.generate_now(SYSTEM, USER, 16).await {
                    Ok(_) => {
                        served.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(lj_llm::gemini::GeminiError::Busy(wait)) => {
                        tokio::time::sleep(std::time::Duration::from_secs_f64(wait.max(0.05)))
                            .await;
                    }
                    Err(_) => {
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }
    for driver in drivers {
        driver.await.expect("driver");
    }

    let elapsed = start.elapsed().as_secs_f64();
    let served = served.load(Ordering::Relaxed);
    let debit = 60.0 * served as f64 / elapsed;
    println!(
        "served={served} failed={} refused={} bare_refusals={} useful_rate={debit:.1}/min learned_rate={:.1}/min quota={}",
        failed.load(Ordering::Relaxed),
        client.refusals(),
        client.bare_refusals(),
        client.rate_per_min(),
        client.refused_quota().unwrap_or_default(),
    );

    assert!(
        client.rate_per_min() > 5.0,
        "le seau s'est effondré : {:.2}/min",
        client.rate_per_min()
    );
    assert!(debit > 5.0, "débit utile effondré : {debit:.2}/min");
}
