//! The interface's copy, one catalogue per language the interface speaks: a field left out of
//! a catalogue does not compile, so no page mixes two languages.

mod account;
mod api_guide;
mod dates;
mod de;
mod decision;
mod directory;
mod entity;
mod feedback;
mod fr;
mod home;
mod hubs;
mod legal;
mod mcp_guide;
mod nl;
mod norm;
mod notices;
mod numbers;
mod open_source;
mod places;
mod profile;
mod provenance;
mod redact;
mod referential;
mod search;
mod seat;
mod shell;
mod sources;
mod tools;

pub use account::AccountUi;
pub use api_guide::ApiGuideUi;
pub use dates::DatesUi;
pub use decision::DecisionUi;
pub use directory::{DirectoryUi, KindCopy};
pub use entity::EntityUi;
pub use feedback::FeedbackUi;
pub use home::HomeUi;
pub use hubs::HubsUi;
pub use legal::{Block, LegalPage, LegalSection, LegalUi};
pub use mcp_guide::McpGuideUi;
pub use norm::NormUi;
pub use notices::NoticesUi;
pub use numbers::NumbersUi;
pub use open_source::OpenSourceUi;
pub use places::{place_name, relabel_applicability, relabel_title};
pub use profile::ProfileUi;
pub use provenance::ProvenanceUi;
pub use redact::RedactUi;
pub use referential::{relabel_article_facets, relabel_facets, ReferentialUi};
pub use search::SearchUi;
pub use shell::ShellUi;
pub use sources::{SourceCopy, SourcesUi};
pub use tools::{ToolCopy, ToolsUi};

use leptos::prelude::*;
use leptos_router::hooks::use_location;

/// A language the interface speaks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Fr,
    De,
    Nl,
}

/// The cookie a reader's choice of language is kept in, which the bare addresses read.
pub const LANG_COOKIE: &str = "lj-lang";

impl Lang {
    pub const ALL: [Lang; 3] = [Lang::Fr, Lang::De, Lang::Nl];

    /// A language opens once its catalogue is translated whole; a closed one is never served.
    pub fn is_open(self) -> bool {
        match self {
            Lang::Fr | Lang::De | Lang::Nl => true,
        }
    }

    pub fn open() -> impl Iterator<Item = Lang> {
        Lang::ALL.into_iter().filter(|lang| lang.is_open())
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::Fr => "fr",
            Lang::De => "de",
            Lang::Nl => "nl",
        }
    }

    /// The language named in itself, as a reader of it looks for it.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Fr => "français",
            Lang::De => "Deutsch",
            Lang::Nl => "Nederlands",
        }
    }

    /// The tag of the document's `lang`: the region whose usage the catalogue follows.
    pub fn tag(self) -> &'static str {
        match self {
            Lang::Fr => "fr",
            Lang::De => "de-CH",
            Lang::Nl => "nl-BE",
        }
    }

    /// The locale Open Graph names the page's language by.
    pub fn og_locale(self) -> &'static str {
        match self {
            Lang::Fr => "fr_FR",
            Lang::De => "de_CH",
            Lang::Nl => "nl_BE",
        }
    }

    /// An open language by its code.
    pub fn from_code(code: &str) -> Option<Lang> {
        Lang::open().find(|lang| lang.code() == code)
    }

    pub fn ui(self) -> &'static Ui {
        match self {
            Lang::Fr => &fr::FR,
            Lang::De => &de::DE,
            Lang::Nl => &nl::NL,
        }
    }

    /// The `hreflang` of this language's version of an address under `jurisdiction`: the
    /// region where the language is spoken there, the bare language elsewhere.
    pub fn hreflang(self, jurisdiction: Option<&str>) -> String {
        let spoken = match jurisdiction {
            Some("ch") => &[Lang::De, Lang::Fr][..],
            Some("be") => &[Lang::Nl, Lang::Fr, Lang::De][..],
            Some("nl") => &[Lang::Nl][..],
            _ => &[][..],
        };
        match jurisdiction {
            Some(code) if spoken.contains(&self) => {
                format!("{}-{}", self.code(), code.to_ascii_uppercase())
            }
            _ => self.code().to_string(),
        }
    }
}

/// The jurisdiction an address names: the first segment of a scoped page, `/ch/decision-search`,
/// the second of a document or an index, `/decision/ch/…`.
pub fn address_jurisdiction(path: &str) -> Option<&str> {
    let mut segments = path.trim_start_matches('/').split('/');
    let first = segments.next()?;
    let code = match first {
        "decision" | "decision-index" | "norm" | "norm-index" | "journal-index" | "notice"
        | "entity-index" => segments.next()?,
        _ => first,
    };
    (code.len() == 2 && code.bytes().all(|b| b.is_ascii_lowercase())).then_some(code)
}

/// The language a page is in when its address asks for none: its jurisdiction's most spoken
/// language, once open, else French.
pub fn address_default(path: &str) -> Lang {
    let lang = match address_jurisdiction(path) {
        Some("ch") => Lang::De,
        Some("be" | "nl") => Lang::Nl,
        _ => Lang::Fr,
    };
    match lang.is_open() {
        true => lang,
        false => Lang::Fr,
    }
}

/// `url`, a path or an absolute address, in `lang`: its `lang` parameter set, or dropped where
/// `lang` is the address's default.
pub fn in_lang(url: &str, lang: Lang) -> String {
    let (url, hash) = url.split_at(url.find('#').unwrap_or(url.len()));
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let path_start = match base.find("://") {
        Some(scheme) => base[scheme + 3..]
            .find('/')
            .map_or(base.len(), |p| scheme + 3 + p),
        None => 0,
    };
    let mut pairs: Vec<String> = query
        .split('&')
        .filter(|pair| !pair.is_empty() && *pair != "lang" && !pair.starts_with("lang="))
        .map(str::to_string)
        .collect();
    if address_default(&base[path_start..]) != lang {
        pairs.push(format!("lang={}", lang.code()));
    }
    match pairs.is_empty() {
        true => format!("{base}{hash}"),
        false => format!("{base}?{}{hash}", pairs.join("&")),
    }
}

/// The copy of the interface in one language.
pub struct Ui {
    /// A language named in this one, `allemand`; `None` for a code the table lacks.
    pub lang_name: fn(&str) -> Option<&'static str>,
    /// The adjective a title gives the law of a jurisdiction, `suisse`; `None` for a code the table lacks.
    pub law_adjective: fn(&str) -> Option<&'static str>,
    /// Items joined as a sentence lists them, `a, b et c`.
    pub list: fn(&[&str]) -> String,
    pub shell: ShellUi,
    pub home: HomeUi,
    pub search: SearchUi,
    pub notices: NoticesUi,
    pub norm: NormUi,
    pub hubs: HubsUi,
    pub sources: SourcesUi,
    pub legal: LegalUi,
    pub open_source: OpenSourceUi,
    pub api_guide: ApiGuideUi,
    pub mcp_guide: McpGuideUi,
    pub account: AccountUi,
    pub profile: ProfileUi,
    pub decision: DecisionUi,
    pub tools: ToolsUi,
    pub feedback: FeedbackUi,
    pub dates: DatesUi,
    pub directory: DirectoryUi,
    pub entity: EntityUi,
    pub redact: RedactUi,
    pub provenance: ProvenanceUi,
    pub numbers: NumbersUi,
    pub referential: ReferentialUi,
}

/// A piece of a sentence. A link is its address then its text: `Link` opens another site in a
/// new tab, `Local` a page of ours or a mail address in place, `File` a file we serve, past the
/// router, `Download` a file to save.
#[derive(Clone, Copy)]
pub enum Run {
    Text(&'static str),
    Strong(&'static str),
    Em(&'static str),
    Code(&'static str),
    Link(&'static str, &'static str),
    Local(&'static str, &'static str),
    File(&'static str, &'static str),
    Download(&'static str, &'static str),
}

/// The reader's language, read from the address alone, `?lang=` else the address's default,
/// so the server as it renders and the browser as it hydrates agree. It holds for the visit:
/// another language loads a new page.
#[derive(Clone, Copy)]
pub struct ReaderLang(Lang);

impl ReaderLang {
    pub fn get(self) -> Lang {
        self.0
    }

    /// `path`, an address no jurisdiction scopes, in the reader's language.
    pub fn path(self, path: &str) -> String {
        in_lang(path, self.0)
    }
}

/// Under the router: the address is what it reads.
pub fn provide_reader_lang() {
    let location = use_location();
    let lang = untrack(|| {
        location
            .query
            .with(|q| addressed(&location.pathname.get(), q.get_str("lang")))
    });
    provide_context(ReaderLang(lang));
    #[cfg(feature = "hydrate")]
    keep_in_address(lang, location);
}

/// A page reached in place keeps the reader's language and says so in its address; one whose
/// address asks for another language is loaded anew.
#[cfg(feature = "hydrate")]
fn keep_in_address(lang: Lang, location: leptos_router::location::Location) {
    Effect::new(move |_| {
        let path = location.pathname.get();
        let asked = location
            .query
            .with(|q| q.get_str("lang").map(str::to_string));
        if addressed(&path, asked.as_deref()) == lang {
            return;
        }
        let Some(window) = web_sys::window() else {
            return;
        };
        if asked.is_some() {
            let _ = window.location().reload();
            return;
        }
        // The router writes a new page's address only once it has rendered; links carry it before.
        if window.location().pathname().ok() != Some(path.clone()) {
            return;
        }
        let search = location.search.get_untracked();
        let target = match search.is_empty() {
            true => in_lang(&path, lang),
            false => in_lang(&format!("{path}?{search}"), lang),
        };
        let target = format!("{target}{}", location.hash.get_untracked());
        if let Ok(history) = window.history() {
            let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&target));
        }
    });
    carry_on_links(lang);
}

/// Gives a clicked link the reader's language before the router reads it; `hreflang` keeps its own.
#[cfg(feature = "hydrate")]
fn carry_on_links(lang: Lang) {
    use wasm_bindgen::{closure::Closure, JsCast};
    let on_click = Closure::<dyn Fn(web_sys::Event)>::new(move |event: web_sys::Event| {
        let Some(link) = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .and_then(|element| element.closest("a[href]").ok().flatten())
            .and_then(|element| element.dyn_into::<web_sys::HtmlAnchorElement>().ok())
        else {
            return;
        };
        let here = window().location().origin().ok();
        if link.has_attribute("hreflang") || Some(link.origin()) != here {
            return;
        }
        let search = link.search();
        let query = search.trim_start_matches('?');
        if query
            .split('&')
            .any(|pair| pair == "lang" || pair.starts_with("lang="))
        {
            return;
        }
        let kept = in_lang(&format!("{}{search}{}", link.pathname(), link.hash()), lang);
        link.set_href(&kept);
    });
    let _ = document().add_event_listener_with_callback_and_bool(
        "click",
        on_click.as_ref().unchecked_ref(),
        true,
    );
    on_click.forget();
}

/// Above the router, where the server writes the document's head: the request's address, read
/// the same way.
#[cfg(feature = "ssr")]
pub fn request_lang() -> Lang {
    let url = use_context::<leptos_router::location::RequestUrl>().and_then(|url| url.parse().ok());
    match url {
        Some(url) => addressed(url.path(), url.search_params().get_str("lang")),
        None => Lang::Fr,
    }
}

/// The open language `?lang=` asks for, else the address's default.
fn addressed(path: &str, code: Option<&str>) -> Lang {
    code.and_then(Lang::from_code)
        .unwrap_or_else(|| address_default(path))
}

/// Outside the router, where no address says it: the language given.
pub fn provide_lang(lang: Lang) {
    provide_context(ReaderLang(lang));
}

pub fn use_reader_lang() -> ReaderLang {
    expect_context::<ReaderLang>()
}

/// The copy in the reader's language.
pub fn use_ui() -> &'static Ui {
    use_reader_lang().get().ui()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default follows the jurisdiction the address names, wherever it names one, and
    /// falls back to French while that language is closed.
    #[test]
    fn an_address_defaults_to_its_jurisdiction_s_open_language() {
        let ch = match Lang::De.is_open() {
            true => Lang::De,
            false => Lang::Fr,
        };
        let cases = [
            ("/ch", ch),
            ("/ch/decision-search", ch),
            ("/decision/ch/abc", ch),
            ("/norm-index/ch/codes", ch),
            ("/nl", Lang::Nl),
            ("/decision/nl/abc", Lang::Nl),
            ("/fr/decision-search", Lang::Fr),
            ("/decision/contribute", Lang::Fr),
            ("/sources", Lang::Fr),
            ("/", Lang::Fr),
        ];
        for (path, expected) in cases {
            assert_eq!(address_default(path), expected, "{path}");
        }
    }

    /// `lang` is set where it differs from the default, dropped where it repeats it, and the
    /// other parameters, their order and the fragment are kept.
    #[test]
    fn a_url_in_a_language_carries_lang_only_off_its_default() {
        let cases = [
            ("/sources", Lang::Fr, "/sources"),
            ("/sources?lang=fr", Lang::Fr, "/sources"),
            ("/sources?lang=de&x=1", Lang::Fr, "/sources?x=1"),
            (
                "/fr/decision-search?q=a&mode=b#top",
                Lang::Fr,
                "/fr/decision-search?q=a&mode=b#top",
            ),
            (
                "https://librejustice.fr/fr?lang=nl",
                Lang::Fr,
                "https://librejustice.fr/fr",
            ),
            (
                "https://librejustice.fr",
                Lang::Fr,
                "https://librejustice.fr",
            ),
        ];
        for (url, lang, expected) in cases {
            assert_eq!(in_lang(url, lang), expected, "{url}");
        }
        assert_eq!(in_lang("/sources?q=a", Lang::De), "/sources?q=a&lang=de");
        assert_eq!(
            in_lang("https://librejustice.fr/fr#x", Lang::Nl),
            "https://librejustice.fr/fr?lang=nl#x"
        );
    }

    /// A version under Switzerland or Belgium names the region where its language is spoken
    /// there; elsewhere, and for a language not spoken there, the bare language.
    #[test]
    fn hreflang_names_the_region_only_where_the_language_is_spoken() {
        assert_eq!(Lang::De.hreflang(Some("ch")), "de-CH");
        assert_eq!(Lang::Fr.hreflang(Some("ch")), "fr-CH");
        assert_eq!(Lang::Nl.hreflang(Some("ch")), "nl");
        assert_eq!(Lang::Nl.hreflang(Some("be")), "nl-BE");
        assert_eq!(Lang::De.hreflang(Some("be")), "de-BE");
        assert_eq!(Lang::Nl.hreflang(Some("nl")), "nl-NL");
        assert_eq!(Lang::Fr.hreflang(Some("nl")), "fr");
        assert_eq!(Lang::Fr.hreflang(Some("fr")), "fr");
        assert_eq!(Lang::Fr.hreflang(None), "fr");
    }
}
