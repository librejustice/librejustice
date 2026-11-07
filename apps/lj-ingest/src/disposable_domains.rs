//! The disposable mail domains: disposable/disposable, plus `OWN`, minus `ALLOW`.

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use lj_store::repository::DecisionRepository;

use crate::config::Settings;

const SOURCE: &str =
    "https://raw.githubusercontent.com/disposable/disposable-email-domains/master/domains.txt";

/// Disposable domains the source lacks, each to leave once the source lists it.
const OWN: &[&str] = &[
    // Mail7, moved from mail7.io
    "mail7.app",
];

/// Lasting mailboxes: each leaves the list with its listed parents and subdomains.
const ALLOW: &[&str] = &[
    // Proton Pass
    "passmail.net",
    "passmail.com",
    "passinbox.com",
    "passfwd.com",
    // SimpleLogin
    "simplelogin.com",
    "simplelogin.fr",
    "simplelogin.co",
    "simplelogin.io",
    "aleeas.com",
    "slmails.com",
    "silomails.com",
    "slmail.me",
    // Firefox Relay
    "mozmail.com",
    "relay.firefox.com",
    // Apple Hide My Email
    "privaterelay.appleid.com",
    // addy.io
    "addy.io",
    "addy.to",
    "anonaddy.com",
    "anonaddy.me",
    "addymail.com",
    "mailer.me",
    "4wrd.cc",
    // DuckDuckGo
    "duck.com",
    // Outlook-hosted since 2002, listed by one source unproven
    "dbmail.com",
];

pub async fn refresh() -> Result<()> {
    let settings = Settings::from_env()?;
    let body = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?
        .get(SOURCE)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .context("disposable domains source")?
        .text()
        .await?;
    let list = build(parse(&body), OWN, ALLOW)?;
    for domain in &list.own_upstream {
        tracing::warn!(domain, "own domain the source now lists, to drop from OWN");
    }
    for (domain, allowed) in &list.dropped {
        tracing::info!(domain, allowed, "allowed domain the source lists, kept out");
    }

    let pool =
        lj_pg::db::build_pool(&settings.db_url, 1).map_err(|e| anyhow!("build_pool: {e}"))?;
    let conn = pool.get().await.map_err(|e| anyhow!("pool.get: {e}"))?;
    let (added, removed) = DecisionRepository::new(&conn)
        .replace_disposable_domains(&list.domains)
        .await?;
    tracing::info!(
        domains = list.domains.len(),
        added,
        removed,
        "disposable domains refreshed"
    );
    Ok(())
}

fn parse(body: &str) -> BTreeSet<String> {
    body.lines()
        .map(|line| {
            line.split('#')
                .next()
                .unwrap_or_default()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|domain| !domain.is_empty())
        .collect()
}

struct List {
    domains: Vec<String>,
    own_upstream: Vec<&'static str>,
    dropped: Vec<(String, &'static str)>,
}

fn build(
    mut source: BTreeSet<String>,
    own: &[&'static str],
    allow: &[&'static str],
) -> Result<List> {
    if let Some(both) = own.iter().find(|d| allow.contains(d)) {
        bail!("{both} is in both OWN and ALLOW");
    }
    let own_upstream = own
        .iter()
        .copied()
        .filter(|d| source.contains(*d))
        .collect();
    source.extend(own.iter().map(|d| d.to_string()));
    let malformed: Vec<&String> = source.iter().filter(|d| !is_domain(d)).take(10).collect();
    if !malformed.is_empty() {
        bail!("not a domain, the source format moved: {malformed:?}");
    }

    let mut domains = Vec::with_capacity(source.len());
    let mut dropped = Vec::new();
    for domain in source {
        match allow
            .iter()
            .copied()
            .find(|a| domain == *a || is_below(a, &domain) || is_below(&domain, a))
        {
            Some(allowed) => dropped.push((domain, allowed)),
            None => domains.push(domain),
        }
    }
    Ok(List {
        domains,
        own_upstream,
        dropped,
    })
}

fn is_domain(s: &str) -> bool {
    s.contains('.')
        && s.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn is_below(sub: &str, parent: &str) -> bool {
    sub.strip_suffix(parent)
        .is_some_and(|head| head.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::build;

    #[test]
    fn an_allowed_domain_leaves_with_its_listed_parents_and_subdomains() {
        let source = [
            "aminavin.com",
            "passmail.net",
            "x.mozmail.com",
            "firefox.com",
            "notpassmail.net",
        ]
        .map(String::from)
        .into();
        let list = build(
            source,
            &["mail7.app"],
            &["passmail.net", "mozmail.com", "relay.firefox.com"],
        )
        .unwrap();
        assert_eq!(
            list.domains,
            ["aminavin.com", "mail7.app", "notpassmail.net"]
        );
    }
}
