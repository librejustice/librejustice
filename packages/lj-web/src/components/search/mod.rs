//! Composants de recherche.

pub mod active_filters;
pub mod annonce_results;
pub mod compact_search;
pub mod corpus_intro;
pub mod cross_teaser;
pub mod facet_widgets;
pub mod filter_bar;
pub mod filter_dropdown;
pub mod freshness;
pub mod hero_search;
pub mod legal_instrument_filter;
pub mod mode_select;
pub mod pagination;
pub mod query_length;
pub mod result_card;
pub mod result_empty;
pub mod result_error;
pub mod result_list;
pub mod result_skeleton;
pub mod results_rail;
pub mod search_submit;
pub mod sort_select;
pub mod suggest_box;
pub mod syntax_hint;
pub mod text_results;
pub mod view_prefs;

pub use active_filters::ActiveFilterChips;
pub use annonce_results::AnnoncesView;
pub use compact_search::CompactSearch;
pub use corpus_intro::{corpus_stats, DecisionsCorpusTeaser, TextesCorpusTeaser};
pub use cross_teaser::{DecisionsTeaser, TextesTeaser};
pub use facet_widgets::StaleCounts;
pub use filter_bar::DecisionsFilterBar;
pub use freshness::{answered, is_stale};
pub use hero_search::HeroSearch;
pub use mode_select::{ModeSelect, DECISION_MODES, TEXTES_MODES};
pub use pagination::Pagination;
pub use query_length::{truncate_to_cap, QueryLengthNotice};
pub use result_card::ResultCard;
pub use result_empty::ResultEmpty;
pub use result_error::ResultError;
pub use result_list::ResultList;
pub use result_skeleton::ResultSkeleton;
pub use results_rail::SearchRail;
pub use search_submit::SearchSubmit;
pub use sort_select::SortSelect;
pub use syntax_hint::{HelpCorpus, SyntaxHint};
pub use text_results::TextesView;
pub use view_prefs::{
    effective_card_view, provide_card_view_pref, CardView, CardViewPref, ViewControls,
};
