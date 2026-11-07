//! The embedding chain, in priority order.

/// Host used when `LIBREJUSTICE_EMBED_URL` is unset.
pub const DEFAULT_URL: &str = "http://127.0.0.1:8400/v1/embeddings";

/// The entry of `LIBREJUSTICE_EMBED_URL` that places Cloudflare Workers AI in the chain.
pub const CLOUDFLARE: &str = "cloudflare-aiworker";

/// One link of the chain `LIBREJUSTICE_EMBED_URL` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Host(EmbedHost),
    Cloudflare,
}

/// One OpenAI-compatible embedding host of the ordered list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedHost {
    pub url: String,
    pub api_key: Option<String>,
    /// `host:port`, what traces and metrics name the host by.
    pub label: String,
}

/// Reads `LIBREJUSTICE_EMBED_URL` as a comma-separated chain of URLs and `cloudflare-aiworker`,
/// and `LIBREJUSTICE_EMBED_API_KEY` as the parallel list of the URLs' keys, an empty entry for
/// a host without auth. The keys stay out of the URL list because every URL is written to the
/// HTTP spans.
pub fn parse_chain(entries: &str, keys: Option<&str>) -> Result<Vec<Entry>, String> {
    let entries: Vec<&str> = entries.split(',').map(str::trim).collect();
    let urls = entries.iter().filter(|e| **e != CLOUDFLARE).count();
    let mut keys = match keys {
        None => vec![None; urls],
        Some(raw) => raw
            .split(',')
            .map(str::trim)
            .map(|k| (!k.is_empty()).then(|| k.to_string()))
            .collect(),
    }
    .into_iter();
    if keys.len() != urls {
        return Err(format!(
            "LIBREJUSTICE_EMBED_API_KEY: {} keys for {urls} URLs, one per URL in the same order",
            keys.len()
        ));
    }
    entries
        .into_iter()
        .map(|entry| match entry {
            CLOUDFLARE => Ok(Entry::Cloudflare),
            url => host(url, keys.next().flatten()).map(Entry::Host),
        })
        .collect()
}

fn host(url: &str, api_key: Option<String>) -> Result<EmbedHost, String> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| format!("LIBREJUSTICE_EMBED_URL: invalid URL {url:?}: {e}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("LIBREJUSTICE_EMBED_URL: no host in {url:?}"))?;
    let port = parsed.port_or_known_default().unwrap_or_default();
    Ok(EmbedHost {
        url: url.to_string(),
        api_key,
        label: format!("{host}:{port}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(chain: &[Entry]) -> Vec<&EmbedHost> {
        chain
            .iter()
            .filter_map(|e| match e {
                Entry::Host(h) => Some(h),
                Entry::Cloudflare => None,
            })
            .collect()
    }

    #[test]
    fn a_single_url_is_a_chain_of_one() {
        let chain = parse_chain(DEFAULT_URL, Some("k")).unwrap();
        assert_eq!(chain.len(), 1);
        let [host] = hosts(&chain)[..] else { panic!() };
        assert_eq!(host.api_key.as_deref(), Some("k"));
        assert_eq!(host.label, "127.0.0.1:8400");
    }

    #[test]
    fn keys_pair_with_urls_by_position_and_an_empty_key_means_none() {
        let chain = parse_chain(
            "http://a:8400/v1/embeddings, http://b:8401/v1/embeddings",
            Some(",kb"),
        )
        .unwrap();
        let hosts = hosts(&chain);
        assert_eq!(hosts[0].label, "a:8400");
        assert_eq!(hosts[0].api_key, None);
        assert_eq!(hosts[1].label, "b:8401");
        assert_eq!(hosts[1].api_key.as_deref(), Some("kb"));
    }

    #[test]
    fn cloudflare_takes_its_place_in_the_chain_and_no_key() {
        let chain = parse_chain(
            "http://a:8400/v1/embeddings, cloudflare-aiworker, http://cpu:8080/v1/embeddings",
            Some("ka,"),
        )
        .unwrap();
        assert_eq!(chain[1], Entry::Cloudflare);
        let hosts = hosts(&chain);
        assert_eq!(
            (hosts[0].label.as_str(), hosts[0].api_key.as_deref()),
            ("a:8400", Some("ka"))
        );
        assert_eq!(
            (hosts[1].label.as_str(), hosts[1].api_key.as_deref()),
            ("cpu:8080", None)
        );
    }

    #[test]
    fn a_key_count_that_differs_from_the_url_count_is_refused() {
        assert!(parse_chain("http://a:8400,http://b:8400", Some("k")).is_err());
        assert!(parse_chain("http://a:8400,,http://b:8400", None).is_err());
        assert!(parse_chain("http://a:8400,cloudflare-aiworker", Some("k,")).is_err());
    }
}
