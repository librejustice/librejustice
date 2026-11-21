//! Codes and constants the server, the ingest and the browser build share:
//! text paths, hub pages, notice fonds, zones, passage tags, the tokenizer
//! calibration and the workbench's document types and files.

pub mod cantons;
pub mod courts;
pub mod files;
pub mod languages;
pub mod tokens;

/// The first path segment of a text URL, naming the jurisdiction that enacts
/// it.
pub fn url_segment(jurisdiction: &str) -> String {
    jurisdiction.to_ascii_lowercase()
}

/// The stored jurisdiction an URL segment names, inverse of [`url_segment`].
pub fn jurisdiction_of_segment(segment: &str) -> String {
    segment.to_ascii_uppercase()
}

/// What the Journal officiel appends to the title of an act on a person's civil
/// status or nationality, whose body the law keeps out of search engines.
pub const PROTECTED_ACCESS_MARKER: &str = "(Accès protégé";

/// The path of a text at the address its enacting jurisdiction gives it. An
/// article, a division or a date is appended to it.
pub fn norm_path(jurisdiction: &str, slug: &str) -> String {
    format!("/norm/{}/{slug}", url_segment(jurisdiction))
}

/// The jurisdiction segment of an entity URL: every namespace is a French
/// registry.
pub fn entity_segment(ns: &str) -> Option<&'static str> {
    matches!(ns, "siren" | "rna" | "cnb" | "oacc").then_some("fr")
}

/// The page of an entity, from the two halves of its `ns:id` uid.
pub fn entity_path(ns: &str, id: &str) -> String {
    let seg = entity_segment(ns)
        .unwrap_or_else(|| panic!("entity namespace without a jurisdiction: {ns}"));
    format!("/entity/{seg}/{ns}/{id}")
}

/// The page of a notice, under the place its journal publishes for; the uid keeps its slashes.
pub fn notice_path(place: &str, uid: &str) -> String {
    format!("/notice/{}/{uid}", url_segment(place))
}

/// Links per page of the `/decision-index/{juris}/{code}/{year}` and
/// `/norm-index/{juris}/{fond}/{year}` hubs. With a second copy, the sitemaps would
/// declare `?page=N` pages that do not exist.
pub const HUB_PAGE_SIZE: u32 = 100;

/// What a passage of a decision holds, the tags of `decision_chunks.zones`,
/// several at once.
pub mod passage_zones {
    /// Its prefix carries the decision's summary.
    pub const SUMMARY: &str = "summary";
    /// Its body crosses the text before the moyens annexés.
    pub const TEXT: &str = "text";
    /// Its body crosses the moyens annexés.
    pub const ANNEXES: &str = "annexes";
    /// Its decision's text carries the court's summary, a tag the base keeps.
    pub const HEADNOTE: &str = "headnote";
}

/// What a row of `decisions` is, `decisions.kind`, repeated on its texts and passages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum DocumentKind {
    /// A decision of a court.
    #[default]
    Decision,
    /// The conclusions of an Advocate General, a procureur-generaal or a rapporteur
    /// public, which propose an answer to the court and decide nothing.
    Conclusions,
}

impl DocumentKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            DocumentKind::Decision => "decision",
            DocumentKind::Conclusions => "conclusions",
        }
    }
}

/// The fonds of the notice pot, in display order. Each has its own backend
/// (the `annonce` table for `marche`, the gazette tables for `bodacc` and
/// `joafe`, the JORF publications for `jo`, the prefectures' recueils for `raa`), but
/// all project onto the same hit: the fond is a facet axis, not a separate engine.
pub const JOURNAL_FOND_MARCHE: &str = "marche";
pub const JOURNAL_FOND_BODACC: &str = "bodacc";
pub const JOURNAL_FOND_JOAFE: &str = "joafe";
pub const JOURNAL_FOND_JO: &str = "jo";
pub const JOURNAL_FOND_RAA: &str = "raa";

/// The categories of the norms search, each text in exactly one.
pub const NORM_CATEGORIES: [&str; 9] = [
    "constitution",
    "codes",
    "legislation",
    "reglementation",
    "traites",
    "conventions_collectives",
    "doctrine_administrative",
    "information_publique",
    "travaux_parlementaires",
];

/// The channels that publish a notice without carrying the consultation: they
/// name the platform, they do not give access to it. A row seen by these alone
/// has no door, whatever their number, so it is a candidate for twinning and
/// for the search by reference.
pub const CANAUX_PUBLICATION: [&str; 3] = ["boamp", "ted", "presse"];

/// The CPV division the lot table covers, the one that splits works by trade.
pub const DIVISION_TRAVAUX: &str = "45";

/// The two scopes that are not places, and the name of what no source located.
/// Disjoint from every département code, which are digits or the two Corsican
/// ones. [`ZONE_SANS`] is never stored: the empty set is what the column
/// carries, and this is the key that ticks it.
pub const ZONE_FRANCE: &str = "fr";
pub const ZONE_EU: &str = "eu";
pub const ZONE_SANS: &str = "sans";

pub fn zone_est_departement(code: &str) -> bool {
    !matches!(code, ZONE_FRANCE | ZONE_EU | ZONE_SANS)
}
