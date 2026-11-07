//! Disposable mail domains: no account is created on one, nor moved onto one.

use lj_store::repository::DecisionRepository;

use crate::error::Result;
use crate::state::AppState;

pub const LOG_TARGET: &str = "lj_api::disposable";

/// The listed domain `email` falls under, `email` lowercase as `normaliser` returns it.
pub(crate) async fn disposable_domain(state: &AppState, email: &str) -> Result<Option<String>> {
    let Some((_, domain)) = email.rsplit_once('@') else {
        return Ok(None);
    };
    let conn = state.pool.get().await?;
    Ok(DecisionRepository::new(&conn)
        .listed_disposable_domain(&with_parents(domain))
        .await?)
}

fn with_parents(domain: &str) -> Vec<&str> {
    let mut domains = vec![domain];
    let mut rest = domain;
    while let Some((_, parent)) = rest.split_once('.') {
        domains.push(parent);
        rest = parent;
    }
    domains
}

/// Logs the listed entry rather than the address's domain, which may sit below a
/// public suffix the list carries.
pub(crate) fn log_refusal(listed: &str, channel: &str) {
    tracing::info!(target: LOG_TARGET, listed, channel, "disposable address refused");
}

#[cfg(test)]
mod tests {
    use super::with_parents;

    #[test]
    fn a_domain_falls_under_its_parents_by_whole_labels() {
        assert_eq!(
            with_parents("mx.throwaway.test"),
            ["mx.throwaway.test", "throwaway.test", "test"]
        );
        assert!(!with_parents("notthrowaway.test").contains(&"throwaway.test"));
    }
}
