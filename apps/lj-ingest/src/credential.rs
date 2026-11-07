//! LLM credential pool: pouring keys in, the OpenAI device login and its refresh,
//! naming where a key lives, reading back what the base holds. The secret never
//! leaves through this module.

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use lj_llm::codex;
use lj_llm::mistral::key_fingerprint;
use lj_llm::oauth;
use lj_store::repository::{Credential, DecisionRepository, NewCredential};
use serde_json::json;

/// Pools an empty base is filled with at first start, and the variable each reads.
const SEEDS: [(&str, &str); 2] = [
    ("mistral", "LIBREJUSTICE_SEED_MISTRAL_KEYS"),
    ("gemini", "LIBREJUSTICE_SEED_GEMINI_KEYS"),
];

macro_rules! with_repo {
    ($conn:ident, $repo:ident) => {
        let settings = crate::config::Settings::from_env()?;
        let pool =
            lj_pg::db::build_pool(&settings.db_url, 2).map_err(|e| anyhow!("build_pool: {e}"))?;
        let $conn = pool.get().await.map_err(|e| anyhow!("pool: {e}"))?;
        let $repo = DecisionRepository::new(&$conn);
    };
}

/// Pours the keys a variable carries into a pool.
pub async fn import(
    provider: &str,
    from: &str,
    tenants_var: Option<&str>,
    account: Option<&str>,
) -> Result<()> {
    let raw = std::env::var(from).with_context(|| format!("{from} absent"))?;
    let keys = split_keys(&raw);
    if keys.is_empty() {
        bail!("{from} ne porte aucune clé");
    }
    let tenants = match tenants_var {
        Some(var) => {
            let raw = std::env::var(var).with_context(|| format!("{var} absent"))?;
            let names: Vec<String> = raw.split(',').map(|n| n.trim().to_string()).collect();
            if names.len() != keys.len() {
                bail!("{var} porte {} noms pour {} clés", names.len(), keys.len());
            }
            names
        }
        None => Vec::new(),
    };

    with_repo!(conn, repo);
    let (fresh, seen) = pour(&repo, provider, &keys, &tenants, account).await?;
    println!("{provider}: {fresh} new, {seen} seen again");
    Ok(())
}

/// Fills the pools an empty base has and leaves alone those that already stand.
/// Replayed by every deploy, so what it reads seeds a base and never rules one.
pub async fn seed() -> Result<()> {
    with_repo!(conn, repo);
    for (provider, var) in SEEDS {
        if !repo.pool_fingerprints(provider).await?.is_empty() {
            println!("{provider}: pool in place, {var} not read");
            continue;
        }
        let Ok(raw) = std::env::var(var) else {
            println!("{provider}: pool empty and {var} missing");
            continue;
        };
        let keys = split_keys(&raw);
        if keys.is_empty() {
            println!("{provider}: pool empty and {var} empty");
            continue;
        }
        let (fresh, _) = pour(&repo, provider, &keys, &[], None).await?;
        println!("{provider}: {fresh} key(s) loaded from {var}");
    }
    Ok(())
}

/// Renders the pool as the base holds it, never the secret.
pub async fn list() -> Result<()> {
    with_repo!(conn, repo);
    for provider in ["mistral", "mistral-vibe", "gemini", "openai"] {
        for c in repo.all_credentials(provider).await? {
            let expiry = c
                .expires_at
                .map(|e| e.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "-".to_string());
            println!(
                "{:<8} {:<28} {:<22} {:<8} {:<17} {}{}",
                c.provider,
                c.tenant,
                c.account.as_deref().unwrap_or("-"),
                c.kind,
                expiry,
                c.fingerprint,
                parked(&c).map(|p| format!("  {p}")).unwrap_or_default()
            );
        }
    }
    Ok(())
}

fn parked(c: &Credential) -> Option<String> {
    if parked_until_enabled(c) {
        return Some("parked until `credential enable`".to_string());
    }
    c.disabled_until
        .filter(|d| *d > Utc::now())
        .map(|d| format!("parked until {}", d.format("%Y-%m-%d %H:%M")))
}

/// Names where a key lives. Each side holds when its flag is absent.
pub async fn name(fingerprint: &str, tenant: Option<&str>, account: Option<&str>) -> Result<()> {
    with_repo!(conn, repo);
    if !repo.name_credential(fingerprint, tenant, account).await? {
        bail!("aucune clé sous l'empreinte {fingerprint}");
    }
    println!("{fingerprint} named");
    Ok(())
}

pub async fn forget(fingerprint: &str) -> Result<()> {
    with_repo!(conn, repo);
    if !repo.forget_credential(fingerprint).await? {
        bail!("aucune clé sous l'empreinte {fingerprint}");
    }
    println!("{fingerprint} removed from the pool");
    Ok(())
}

/// Parks a key until the monthly allowance resets, or until [`enable`].
pub async fn disable(fingerprint: &str, status: i16, until_enabled: bool) -> Result<()> {
    with_repo!(conn, repo);
    let until = until_enabled.then(until_enabled_instant);
    repo.disable_credential(fingerprint, until, status, "admin")
        .await?;
    if until_enabled {
        println!("{fingerprint} parked until `credential enable`");
    } else {
        println!("{fingerprint} parked until the next monthly reset");
    }
    Ok(())
}

pub async fn enable(fingerprint: &str) -> Result<()> {
    with_repo!(conn, repo);
    repo.enable_credential(fingerprint).await?;
    println!("{fingerprint} back in its pool");
    Ok(())
}

/// Stands for "until enabled by hand": a date every reader already compares.
fn until_enabled_instant() -> DateTime<Utc> {
    DateTime::<Utc>::from_naive_utc_and_offset(
        NaiveDate::from_ymd_opt(9999, 12, 31)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .expect("date valide"),
        Utc,
    )
}

pub(crate) fn parked_until_enabled(c: &Credential) -> bool {
    c.disabled_until == Some(until_enabled_instant())
}

fn split_keys(raw: &str) -> Vec<&str> {
    raw.split([',', ';'])
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .collect()
}

async fn pour(
    repo: &DecisionRepository<'_>,
    provider: &str,
    keys: &[&str],
    tenants: &[String],
    account: Option<&str>,
) -> Result<(usize, usize)> {
    let (mut fresh, mut seen) = (0usize, 0usize);
    for (rank, key) in keys.iter().enumerate() {
        let tenant = tenants
            .get(rank)
            .filter(|n| !n.is_empty())
            .map(String::as_str);
        let inserted = repo
            .upsert_credential(&NewCredential {
                fingerprint: &key_fingerprint(key),
                provider,
                tenant,
                kind: "api_key",
                secret: &json!({ "key": key }),
                expires_at: None,
                account,
            })
            .await?;
        if inserted {
            fresh += 1;
        } else {
            seen += 1;
        }
    }
    Ok((fresh, seen))
}

/// Device login: shows the code, waits for approval, files the tokens.
pub async fn login() -> Result<()> {
    let http = reqwest::Client::builder()
        .user_agent("librejustice-ingest")
        .build()?;
    let auth = oauth::start_device_auth(&http, oauth::CODEX_CLIENT_ID).await?;

    println!("Ouvrir {}", oauth::VERIFICATION_URL);
    println!("Enter the code: {}", auth.user_code);
    println!("Window open until {}", auth.expires_at);

    let deadline = Utc::now() + Duration::minutes(15);
    let tokens = loop {
        if Utc::now() > deadline {
            bail!("fenêtre device expirée sans approbation");
        }
        tokio::time::sleep(auth.interval).await;
        if let Some(t) = oauth::poll_device_token(&http, oauth::CODEX_CLIENT_ID, &auth).await? {
            break t;
        }
    };

    // The fingerprint keys on the pair, which is the quota boundary: one seat carries a
    // window per workspace.
    let print = key_fingerprint(&format!("{}:{}", tokens.user_id, tokens.account_id));
    let tenant = tokens
        .email
        .clone()
        .unwrap_or_else(|| tokens.user_id.clone());
    with_repo!(conn, repo);
    repo.upsert_credential(&NewCredential {
        fingerprint: &print,
        provider: "openai",
        tenant: Some(&tenant),
        kind: "oauth",
        secret: &bundle(&tokens),
        expires_at: tokens
            .expires_in_secs
            .map(|s| Utc::now() + Duration::seconds(s)),
        account: Some(&tokens.account_id),
    })
    .await?;
    println!(
        "seat {tenant} ({}) of workspace {} filed under {print}",
        tokens.plan.as_deref().unwrap_or("unknown plan"),
        tokens.account_id
    );
    Ok(())
}

/// Refreshes what expires in less than two days. An account that refuses does not
/// stop the ones after it, and the sweep ends non-zero so a timer says it.
pub async fn refresh() -> Result<()> {
    let http = reqwest::Client::builder()
        .user_agent("librejustice-ingest")
        .build()?;
    with_repo!(conn, repo);
    // The horizon must cover more than the period of whatever calls this, or a token
    // dies between two runs.
    let soon = Utc::now() + Duration::days(2);
    let (mut turned, mut refused) = (0usize, 0usize);
    for c in repo.all_credentials("openai").await? {
        if c.kind != "oauth" || c.expires_at.is_some_and(|e| e > soon) {
            continue;
        }
        let Some(token) = c.refresh_token() else {
            println!("{}: no refresh_token, connection to replay", c.tenant);
            refused += 1;
            continue;
        };
        let fresh = match oauth::refresh(&http, oauth::CODEX_CLIENT_ID, token).await {
            Ok(t) => t,
            Err(e) => {
                println!("{}: refresh refused, {e}", c.tenant);
                refused += 1;
                continue;
            }
        };
        let landed = repo
            .rotate_credential(
                &c.fingerprint,
                &rotated(&c.secret, &fresh),
                fresh
                    .expires_in_secs
                    .map(|s| Utc::now() + Duration::seconds(s))
                    .or(c.expires_at),
                c.rotated_at,
            )
            .await?;
        match landed {
            Some(_) => turned += 1,
            None => println!("{}: another process turned the key first", c.tenant),
        }
    }
    println!("{turned} token(s) refreshed, {refused} refused");
    if refused > 0 {
        bail!("{refused} compte(s) sans jeton frais");
    }
    Ok(())
}

/// Live quota of every OpenAI account of a pool, windows and reset credits.
pub async fn usage() -> Result<()> {
    let http = reqwest::Client::builder()
        .user_agent("librejustice-ingest")
        .build()?;
    with_repo!(conn, repo);
    for c in repo.all_credentials("openai").await? {
        if c.kind != "oauth" {
            continue;
        }
        let (token, account) = access(&c)?;
        let usage = codex::usage(&http, token, account).await?;
        println!(
            "{} ({}) on {account}",
            c.tenant,
            usage.plan_type.as_deref().unwrap_or("plan unknown")
        );
        if let Some(p) = parked(&c) {
            println!("  {p}");
        }
        for w in usage.windows() {
            println!("  {}", window_line(w));
        }
        let credits = codex::reset_credits(&http, token, account).await?;
        println!("  {} reset credit(s)", credits.available_count);
        for credit in &credits.credits {
            println!(
                "    {} {} {}",
                credit.reset_type.as_deref().unwrap_or("type unknown"),
                credit.status.as_deref().unwrap_or("status unknown"),
                credit
                    .expires_at
                    .as_deref()
                    .map(|e| format!("expires on {e}"))
                    .unwrap_or_else(|| "no expiry".to_string())
            );
        }
    }
    Ok(())
}

/// Burns a reset credit on one account.
pub async fn reset_credit(label: &str, force: bool) -> Result<()> {
    let http = reqwest::Client::builder()
        .user_agent("librejustice-ingest")
        .build()?;
    with_repo!(conn, repo);
    let mut named: Vec<Credential> = repo
        .all_credentials("openai")
        .await?
        .into_iter()
        .filter(|c| c.kind == "oauth" && (c.tenant == label || c.fingerprint == label))
        .collect();
    if named.len() > 1 {
        let lines: Vec<String> = named
            .iter()
            .map(|c| {
                format!(
                    "{} on {}",
                    c.fingerprint,
                    c.account.as_deref().unwrap_or("workspace unknown")
                )
            })
            .collect();
        bail!(
            "{label} holds {} workspaces, name one by its fingerprint: {}",
            named.len(),
            lines.join(" ; ")
        );
    }
    let credential = named
        .pop()
        .ok_or_else(|| anyhow!("no OpenAI account named {label} in the pool"))?;
    let (token, account) = access(&credential)?;

    let before = codex::usage(&http, token, account).await?;
    for w in before.windows() {
        println!("before: {}", window_line(w));
    }
    // A credit resets every window the seat announces.
    let full = before
        .windows()
        .into_iter()
        .filter(|w| w.used_percent >= 100.0)
        .map(|w| w.limit_window_seconds)
        .max();
    let place: Vec<String> = before
        .windows()
        .into_iter()
        .filter(|w| w.used_percent < 100.0 && full.is_none_or(|f| w.limit_window_seconds > f))
        .map(window_line)
        .collect();
    if before.windows().is_empty() {
        if !force {
            bail!("no window in the answer, `--force` to go through");
        }
        println!("no window announced, forced");
    } else if !place.is_empty() {
        if !force {
            bail!(
                "one credit resets every window at once, and these still have room: {}. \
                 Exhaust them first, `--force` to go through",
                place.join(" ; ")
            );
        }
        println!("forced, {} thrown away: {}", place.len(), place.join("; "));
    }
    if before.credits_available() == 0 {
        bail!("no reset credit available on {label}");
    }

    let redeem = format!("lj-{}-{}", credential.fingerprint, Utc::now().timestamp());
    codex::consume_reset_credit(&http, token, account, &redeem).await?;

    println!("credit spent, the reset takes about 75 s to show");
    let after = poll_reset(&http, token, account, &before).await?;
    for w in after.windows() {
        println!("after: {}", window_line(w));
    }
    // The 429 that exhausted the week parked the seat until its natural reset; a
    // seat parked by hand stays parked.
    if !parked_until_enabled(&credential) {
        repo.enable_credential(&credential.fingerprint).await?;
    }
    println!("{label}: credit burnt, {} left", after.credits_available());
    Ok(())
}

/// Reads back until a window drops, and gives up after three minutes. The last
/// reading is returned whether or not anything moved.
async fn poll_reset(
    http: &reqwest::Client,
    token: &str,
    account: &str,
    before: &codex::Usage,
) -> Result<codex::Usage> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        let after = codex::usage(http, token, account).await?;
        let moved = after.windows().into_iter().any(|w| {
            before
                .window(w.limit_window_seconds)
                .is_some_and(|b| w.used_percent < b.used_percent)
        });
        if moved || std::time::Instant::now() >= deadline {
            return Ok(after);
        }
    }
}

fn access(c: &Credential) -> Result<(&str, &str)> {
    let token = c
        .access_token()
        .ok_or_else(|| anyhow!("{} sans access_token", c.tenant))?;
    let account = c
        .account
        .as_deref()
        .ok_or_else(|| anyhow!("{} sans compte ChatGPT", c.tenant))?;
    Ok((token, account))
}

fn window_line(w: &codex::Window) -> String {
    let reset = w
        .reset_after_seconds
        .map(|s| format!("resets in {}", duree(s)))
        .unwrap_or_else(|| "reset unknown".to_string());
    format!(
        "window {}: {:.0} % used, {reset}",
        duree(w.limit_window_seconds),
        w.used_percent
    )
}

fn duree(secs: i64) -> String {
    match secs {
        s if s >= 2 * 86_400 => format!("{} j", s / 86_400),
        s if s >= 3_600 => format!("{} h", s / 3_600),
        s => format!("{} min", s / 60),
    }
}

fn bundle(t: &oauth::Tokens) -> serde_json::Value {
    json!({
        "access_token": t.access_token,
        "refresh_token": t.refresh_token,
        "id_token": t.id_token,
        "account_id": t.account_id,
    })
}

/// The stored bundle after a refresh, which answers with what it renewed and keeps
/// silent about the rest. `account_id` is never re-read: it is the row's identity.
fn rotated(current: &serde_json::Value, fresh: &oauth::Refreshed) -> serde_json::Value {
    let kept = |renewed: &Option<String>, name: &str| {
        renewed.clone().or_else(|| {
            current
                .get(name)
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
    };
    json!({
        "access_token": kept(&fresh.access_token, "access_token"),
        "refresh_token": kept(&fresh.refresh_token, "refresh_token"),
        "id_token": kept(&fresh.id_token, "id_token"),
        "account_id": current.get("account_id").cloned(),
    })
}
