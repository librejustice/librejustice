/// The home page of a jurisdiction.
pub struct HomeUi {
    pub eyebrow: &'static str,
    /// The headline before its accent, from the adjective of the jurisdiction's law.
    pub headline: fn(Option<&str>) -> String,
    pub headline_accent: &'static str,
    pub tagline: &'static str,
    pub query_label: &'static str,
    pub query_placeholder: &'static str,
    pub scope_label: &'static str,
    pub advanced_search: &'static str,
    pub browse_directory: &'static str,
    pub examples_case_law: &'static str,
    /// The example queries of a group, by the code of the jurisdiction the home page is for.
    pub examples_case_law_queries: fn(&str) -> &'static [&'static str],
    pub examples_texts: &'static str,
    pub examples_texts_queries: fn(&str) -> &'static [&'static str],
    pub examples_notices: &'static str,
    pub examples_notices_queries: fn(&str) -> &'static [&'static str],
    pub examples_directory_queries: fn(&str) -> &'static [&'static str],
    pub mcp_eyebrow: &'static str,
    pub mcp_pitch: &'static str,
    pub mcp_setup: &'static str,
    pub stat_decisions: &'static str,
    pub stat_texts: &'static str,
}
