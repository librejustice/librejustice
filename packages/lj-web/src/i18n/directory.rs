/// The directory of entities: its search, its categories and their listings.
pub struct DirectoryUi {
    pub title: &'static str,
    pub description: &'static str,
    pub directory: &'static str,
    pub heading: &'static str,
    pub intro: &'static str,
    pub search_placeholder: &'static str,
    pub search_label: &'static str,
    pub search: &'static str,
    pub categories: &'static str,
    pub categories_label: &'static str,
    pub results: &'static str,
    pub all: &'static str,
    pub in_register: &'static str,
    /// Under a category's card, from the count of its entities in a decision and the category's participle.
    pub linked_to_decisions: fn(&str, &str) -> String,
    pub min_query: &'static str,
    pub min_name: &'static str,
    pub no_match: &'static str,
    /// From the error's message.
    pub search_unavailable: fn(&str) -> String,
    /// From the error's message.
    pub listing_unavailable: fn(&str) -> String,
    pub empty_category: &'static str,
    pub register_count: fn(i64) -> String,
    pub in_court: fn(i64) -> String,
    pub sorted_by_decisions: &'static str,
    pub decision_count: fn(i64) -> String,
    pub ceased: &'static str,
    pub pagination: &'static str,
    pub previous: &'static str,
    pub next: &'static str,
    pub page_of: fn(i64, i64) -> String,
    /// A listing's title, from the category's lowercase plural.
    pub listing_title: fn(&str) -> String,
    /// A listing's description, from the category's plural and tagline.
    pub listing_description: fn(&str, &str) -> String,
    /// The name search of a category of natural persons, from its lowercase singular.
    pub named_title: fn(&str) -> String,
    /// From the category's lowercase singular and tagline.
    pub named_description: fn(&str, &str) -> String,
    pub named_placeholder: fn(&str) -> String,
    pub named_label: fn(&str) -> String,
    pub named_empty: fn(&str) -> String,
    pub sign_in: &'static str,
    pub sign_in_to_search: &'static str,
    pub unknown_title: &'static str,
    pub unknown_heading: &'static str,
    pub back: &'static str,
    pub companies: KindCopy,
    pub public_bodies: KindCopy,
    pub associations: KindCopy,
    pub lawyers: KindCopy,
    pub firms: KindCopy,
}

/// A category of the directory.
pub struct KindCopy {
    pub plural: &'static str,
    pub tagline: &'static str,
    pub plural_lower: &'static str,
    pub singular_lower: &'static str,
    /// The participle of « linked », agreed with the category.
    pub linked: &'static str,
}
