use std::net::IpAddr;

use http::HeaderMap;

const KEY_CONTEXT: &str = "librejustice 2026-08-13 api usage pseudonym";

const PSEUDONYM_HEX: usize = 12;

const SALT_BOUNDARY_SECS: i64 = 3 * 3_600;

const DAY_SECS: i64 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    Account(String),
    Network(String),
    Unknown,
}

impl Identity {
    pub fn resolve(user: Option<&str>, headers: &HeaderMap) -> Self {
        if let Some(sub) = user {
            return Identity::Account(sub.to_string());
        }
        match client_network(headers) {
            Some(network) => Identity::Network(network),
            None => Identity::Unknown,
        }
    }

    /// The identity a request from `address` without a session resolves to.
    pub fn of_address(address: IpAddr) -> Self {
        Identity::Network(network(address))
    }
}

pub(crate) fn client_network(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("cf-connecting-ip")?.to_str().ok()?;
    Some(network(raw.parse::<IpAddr>().ok()?))
}

fn network(address: IpAddr) -> String {
    match address {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

/// The client's IPv4 /24 or IPv6 /48, wider than [`Identity::Network`].
pub fn client_block(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("cf-connecting-ip")?.to_str().ok()?;
    Some(match raw.parse::<IpAddr>().ok()? {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.0/24", o[0], o[1], o[2])
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}::/48", s[0], s[1], s[2])
        }
    })
}

pub fn salt_period(unix_secs: i64) -> i64 {
    (unix_secs - SALT_BOUNDARY_SECS).div_euclid(DAY_SECS)
}

#[derive(Clone)]
pub struct Pseudonymiser {
    key: [u8; 32],
}

impl Pseudonymiser {
    pub fn new(secret: Option<&str>) -> Option<Self> {
        let secret = secret.filter(|s| !s.is_empty())?;
        Some(Self {
            key: blake3::derive_key(KEY_CONTEXT, secret.as_bytes()),
        })
    }

    pub fn caller(&self, id: &Identity) -> Option<String> {
        let Identity::Account(sub) = id else {
            return None;
        };
        Some(self.mac(sub.as_bytes()))
    }

    pub fn visit(&self, id: &Identity, user_agent: Option<&str>, period: i64) -> Option<String> {
        let Identity::Network(network) = id else {
            return None;
        };
        let mut input = Vec::new();
        for field in [
            network.as_bytes(),
            user_agent.unwrap_or_default().as_bytes(),
        ] {
            input.extend_from_slice(&(field.len() as u64).to_le_bytes());
            input.extend_from_slice(field);
        }
        input.extend_from_slice(&period.to_le_bytes());
        Some(self.mac(&input))
    }

    fn mac(&self, input: &[u8]) -> String {
        blake3::keyed_hash(&self.key, input).to_hex()[..PSEUDONYM_HEX].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(ip: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("cf-connecting-ip", ip.parse().unwrap());
        h
    }

    fn pseudonymiser() -> Pseudonymiser {
        Pseudonymiser::new(Some("clé-de-test")).unwrap()
    }

    fn network(s: &str) -> Identity {
        Identity::Network(s.to_string())
    }

    #[test]
    fn ipv6_is_grouped_by_64() {
        let a = Identity::resolve(None, &headers("2001:db8:1:2:3:4:5:6"));
        let b = Identity::resolve(None, &headers("2001:db8:1:2:ffff::1"));
        assert_eq!(a, b);
        assert_eq!(a, network("2001:db8:1:2::/64"));
        assert_ne!(a, Identity::resolve(None, &headers("2001:db8:1:3::1")));
    }

    #[test]
    fn a_block_is_a_24_or_a_48() {
        let block = |ip| client_block(&headers(ip));
        assert_eq!(block("203.0.113.7").as_deref(), Some("203.0.113.0/24"));
        assert_eq!(block("203.0.113.7"), block("203.0.113.250"));
        assert_ne!(block("203.0.113.7"), block("203.0.114.7"));
        assert_eq!(
            block("2001:db8:1:2:3::1").as_deref(),
            Some("2001:db8:1::/48")
        );
        assert_eq!(block("2001:db8:1:2:3::1"), block("2001:db8:1:ffff::1"));
        assert_eq!(client_block(&HeaderMap::new()), None);
    }

    #[test]
    fn salt_period_is_constant_within_a_shifted_day() {
        assert_eq!(salt_period(1_786_590_000), salt_period(1_786_663_800));
        assert_eq!(salt_period(1_786_590_000), salt_period(1_786_676_399));
    }

    #[test]
    fn salt_period_turns_at_three_utc() {
        assert_eq!(salt_period(1_786_589_999) + 1, salt_period(1_786_590_000));
        assert_eq!(salt_period(SALT_BOUNDARY_SECS), 0);
        assert_eq!(salt_period(SALT_BOUNDARY_SECS - 1), -1);
    }

    #[test]
    fn caller_is_stable_and_separates_accounts() {
        let p = pseudonymiser();
        let a = Identity::Account("sub-42".to_string());
        assert_eq!(p.caller(&a), p.caller(&a));
        assert_eq!(p.caller(&a).unwrap().len(), PSEUDONYM_HEX);
        assert_ne!(
            p.caller(&a),
            p.caller(&Identity::Account("sub-43".to_string()))
        );
    }

    #[test]
    fn visit_inputs_cannot_collide() {
        let p = pseudonymiser();
        assert_ne!(
            p.visit(&network("a"), Some("bc"), 0),
            p.visit(&network("ab"), Some("c"), 0)
        );
        assert_ne!(
            p.visit(&network("a"), Some("|b"), 0),
            p.visit(&network("a|"), Some("b"), 0)
        );
        assert_ne!(
            p.visit(&network("203.0.113.7"), Some("curl/8"), 0),
            p.visit(&network("203.0.113.7"), Some("curl/8"), 1)
        );
    }
}
