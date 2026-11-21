use super::Run;

/// The open-source components page.
pub struct OpenSourceUi {
    pub title: &'static str,
    pub heading: &'static str,
    pub intro: &'static [Run],
    pub component: &'static str,
    pub version: &'static str,
    pub licence: &'static str,
    pub notes: &'static str,
    pub ocr_weights: &'static str,
    pub word_lists: &'static str,
    pub font_sources: &'static str,
    pub liberation_sources: &'static str,
    pub tahoma_sources: &'static str,
    pub opensymbol_sources: &'static str,
    /// Joins the last item of a list.
    pub and: &'static str,
    /// A component's name or licence written in French, in this language.
    pub named: fn(&str) -> Option<&'static str>,
}
