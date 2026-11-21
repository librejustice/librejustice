use super::Run;

/// The API guide.
pub struct ApiGuideUi {
    pub title: &'static str,
    pub description: &'static str,
    pub eyebrow: &'static str,
    pub heading: &'static str,
    /// The emphasised end of the heading, from the adjective of the reader's law if it has one.
    pub heading_law: fn(Option<&str>) -> String,
    pub intro: &'static str,
    pub key_heading: &'static str,
    pub key_steps: &'static [Run],
    /// The sample call, from the site's address.
    pub example: fn(&str) -> String,
    pub key_header: &'static [Run],
    pub served_heading: &'static str,
    /// Each route, then what it serves, in the order a caller chains them.
    pub routes: &'static [(&'static str, &'static str)],
    pub beta: &'static [Run],
    pub rate_heading: &'static str,
    pub rate: &'static [Run],
    pub terms: &'static [Run],
    pub assistant_heading: &'static str,
    pub assistant: &'static [Run],
}
