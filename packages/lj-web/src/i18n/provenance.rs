/// The « Source » block a decision and an article end with.
pub struct ProvenanceUi {
    pub title: &'static str,
    pub source: &'static str,
    pub ecli: &'static str,
    pub diffuser: &'static str,
    pub identifier: &'static str,
    pub freshness: &'static str,
    pub authority: &'static str,
    /// A source's standing, by the French label the API sends; `None` keeps that label.
    pub authority_value: fn(&str) -> Option<&'static str>,
    pub official_page: &'static str,
    pub diffuser_page: &'static str,
    pub upstream: &'static str,
    pub permalink: &'static str,
}
