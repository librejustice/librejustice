use crate::api::Missing;

/// The top bar, its menu and the footer, on every page.
pub struct ShellUi {
    /// The site's description, on every page that sets none of its own.
    /// The site's description under a jurisdiction, by its code.
    pub site_description: fn(&str) -> String,
    pub skip_to_content: &'static str,
    pub site_menu: &'static str,
    pub menu: &'static str,
    pub home: &'static str,
    pub decisions: &'static str,
    pub texts: &'static str,
    pub directory: &'static str,
    pub notices: &'static str,
    pub tools: &'static str,
    pub sign_in: &'static str,
    pub sign_out: &'static str,
    pub back_to_results: &'static str,
    pub back_to_search: &'static str,
    pub previous_result: &'static str,
    pub next_result: &'static str,
    pub theme: &'static str,
    pub theme_group: &'static str,
    pub theme_system: &'static str,
    pub theme_light: &'static str,
    pub theme_dark: &'static str,
    pub language: &'static str,
    pub language_group: &'static str,
    /// The line offering this language to a reader whose browser asks for it, written in it.
    pub lang_offer: &'static str,
    pub lang_offer_accept: &'static str,
    pub lang_offer_dismiss: &'static str,
    pub jurisdiction: &'static str,
    /// The chip's label, from the name of the reader's jurisdiction.
    pub jurisdiction_of: fn(&str) -> String,
    /// A state's name from its lower-case code, where it differs from the referential's French one.
    pub state_name: fn(&str) -> Option<&'static str>,
    /// A place of the law's vocabulary from its code, `AT` or `CH-VD`, where `state_name` lacks it.
    pub place_name: fn(&str) -> Option<&'static str>,
    /// A place, from its code and name, as the place lines take it: French puts its preposition
    /// first, « au Sénégal », the other languages' lines carry none.
    pub located: fn(&str, &str) -> String,
    pub data: &'static str,
    pub legal_notice: &'static str,
    pub privacy: &'static str,
    pub terms: &'static str,
    pub source_code: &'static str,
    pub support: &'static str,
    pub report_problem: &'static str,
    pub error: &'static str,
    pub not_found: &'static str,
    pub breadcrumb: &'static str,
    pub back_home: &'static str,
    pub not_found_title: &'static str,
    pub error_404: &'static str,
    pub page_not_found: &'static str,
    pub not_found_body: &'static str,
    /// What a page refused by the read budget shows in place of its content.
    pub rate_limited: &'static str,
    /// What a reader refused by the read budget can do about it.
    pub rate_limited_hint: &'static str,
    pub reload: &'static str,
    pub later: &'static str,
    /// A tab left open across a deploy.
    pub new_version: &'static str,
    /// A page whose front never started.
    pub page_incomplete: &'static str,
    pub invalid_id: &'static str,
    pub invalid_entity: &'static str,
    /// What a page says of what the API did not find.
    pub missing: fn(Missing) -> &'static str,
}
