/// The browsable hubs: the norms catalogue, the codes catalogue, what applies in a place.
pub struct HubsUi {
    pub unknown_jurisdiction: &'static str,
    pub invalid_year: &'static str,
    pub undated: &'static str,
    pub texts_and_codes: &'static str,
    pub search_these: &'static str,
    /// A count of texts, from its formatted figure and its value.
    pub n_texts: fn(&str, i64) -> String,
    pub catalogue_title: &'static str,
    pub catalogue_title_for: fn(&str) -> String,
    pub catalogue_description: &'static str,
    pub catalogue_heading: &'static str,
    pub catalogue_intro: &'static str,
    pub other_jurisdictions: &'static str,
    pub in_force_lead: &'static str,
    pub in_force_link: &'static str,
    pub fond_title: fn(&str) -> String,
    /// A fond's description, from its formatted count of texts and its label.
    pub fond_description: fn(&str, &str) -> String,
    /// A fond's count of texts, with the span of its dated years.
    pub fond_intro: fn(&str, Option<(i32, i32)>) -> String,
    pub by_year: &'static str,
    /// A year's page title, from the fond, the year and the page number.
    pub year_title: fn(&str, &str, u32) -> String,
    /// A year's description, from its formatted count, the year (none when undated) and the fond.
    pub year_description: fn(&str, Option<i32>, &str) -> String,
    pub year_heading: fn(&str, &str) -> String,
    pub year_intro: fn(&str, Option<i32>) -> String,
    pub search_year: fn(i32) -> String,
    pub codes_title: &'static str,
    pub codes_description: &'static str,
    pub codes: &'static str,
    pub constitutions: &'static str,
    pub codes_count: fn(usize) -> String,
    pub n_articles: fn(i64) -> String,
    /// The corpus's title, heading and description take the place as `ShellUi::located` gives it.
    pub corpus_title: fn(&str) -> String,
    pub corpus_description: fn(&str, &str) -> String,
    pub applicability: &'static str,
    pub corpus_heading: fn(&str) -> String,
    /// The corpus's lead, from the day and the count of texts.
    pub corpus_intro: fn(&str, &str) -> String,
    pub corpus_empty: &'static str,
    pub and_more: fn(&str) -> String,
    pub on: &'static str,
    pub not_found_title: &'static str,
    pub pagination: &'static str,
    pub first: &'static str,
    pub previous: &'static str,
    pub next: &'static str,
    pub last: &'static str,
    pub page_of: fn(u32, u32) -> String,
    pub jurisdictions_title: &'static str,
    pub jurisdictions_description: &'static str,
    pub case_law: &'static str,
    pub jurisdictions: &'static str,
    pub jurisdictions_intro: &'static str,
    pub search_decisions: &'static str,
    /// A count of decisions, from its formatted figure and its value.
    pub n_decisions: fn(&str, i64) -> String,
    pub court_title: fn(&str) -> String,
    /// A court's description, from its formatted count of decisions and its name.
    pub court_description: fn(&str, &str) -> String,
    /// A court's count of decisions, with its first and last years.
    pub court_intro: fn(&str, i32, i32) -> String,
    pub decisions_by_year: &'static str,
    /// A court's year page title, from the court, the year and the page number.
    pub court_year_title: fn(&str, i32, u32) -> String,
    /// A court's year described, from its formatted count, the year and the court.
    pub court_year_description: fn(&str, i32, &str) -> String,
    pub court_year_heading: fn(&str, i32) -> String,
    pub court_year_intro: fn(&str, i32) -> String,
    pub search_year_decisions: fn(i32) -> String,
}
