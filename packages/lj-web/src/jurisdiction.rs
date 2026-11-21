//! The reader's jurisdiction, which scopes the home page, the two searches and the API guide:
//! the first segment of their addresses, else the one the reader chose.

use leptos::context::Provider;
use leptos::prelude::*;
use leptos::reactive::computed::suspense::SuspenseContext;
use leptos_router::hooks::{use_location, use_params_map};
use leptos_router::location::Url;
use leptos_router::params::ParamsMap;
use lj_dtos::ReaderJurisdiction;

use crate::i18n::{in_lang, Lang};
use crate::pages::law_page::data::{sendable, PageError};
use crate::pages::NotFound;

const FRANCE: &str = "fr";

/// Off the scoped routes, France at render and the cookie's once hydrated: those
/// pages are cached for every reader, so the server never reads the cookie.
#[derive(Clone, Copy)]
pub struct ReaderScope {
    chosen: RwSignal<String>,
    known: RwSignal<Option<Vec<ReaderJurisdiction>>>,
    addressed: Memo<Option<String>>,
    cookie_read: RwSignal<bool>,
    lang: Lang,
}

impl ReaderScope {
    /// The jurisdiction the reader reads under, a lower-case code.
    pub fn code(self) -> String {
        let addressed = self.addressed.get().filter(|code| {
            self.known
                .with(|k| k.as_ref().is_none_or(|k| k.iter().any(|j| &j.code == code)))
        });
        addressed.unwrap_or_else(|| self.chosen.get())
    }

    /// Whether `code` is the one the page keeps: at render on a scoped route,
    /// once the cookie is read anywhere else.
    pub fn settled(self) -> bool {
        self.addressed.get().is_some() || self.cookie_read.get()
    }

    /// The name of the reader's jurisdiction, once the list is loaded.
    pub fn label(self) -> Option<String> {
        let code = self.code();
        self.known.with(|known| {
            known
                .iter()
                .flatten()
                .find(|j| j.code == code)
                .map(|j| j.label.clone())
        })
    }

    pub fn known(self) -> Option<Vec<ReaderJurisdiction>> {
        self.known.get()
    }

    /// `path`, written from the root, under the reader's jurisdiction and in their language.
    pub fn path(self, path: &str) -> String {
        in_lang(&scoped_path(&self.code(), path), self.lang)
    }

    pub fn path_untracked(self, path: &str) -> String {
        untrack(|| self.path(path))
    }

    /// An href that follows the reader's jurisdiction.
    pub fn href(
        self,
        path: impl Into<String>,
    ) -> impl Fn() -> String + Clone + Send + Sync + 'static {
        let path = path.into();
        move || self.path(&path)
    }

    /// `path`, filtered on `seat`, under a jurisdiction whose scope covers that seat.
    pub fn seat_href(
        self,
        seat: String,
        path: String,
    ) -> impl Fn() -> String + Clone + Send + Sync + 'static {
        move || {
            let reader = self.code();
            self.known.with(|known| {
                let known = known.as_deref().unwrap_or_default();
                let code = seat_jurisdiction(&seat, &reader, known);
                in_lang(&scoped_path(code, &path), self.lang)
            })
        }
    }

    /// The list a link reads at render, loaded with its page.
    pub fn known_resource(self) -> Resource<Result<Vec<ReaderJurisdiction>, PageError>> {
        Resource::new_blocking(|| (), move |_| sendable(self.load_known()))
    }

    pub fn keep_known(self, list: Vec<ReaderJurisdiction>) {
        if self.known.get_untracked().is_none() {
            self.known.set(Some(list));
        }
    }

    pub async fn load_known(self) -> Result<Vec<ReaderJurisdiction>, PageError> {
        if let Some(known) = self.known.get_untracked() {
            return Ok(known);
        }
        let mut known = crate::api::ApiClient::from_context()
            .fetch_reader_jurisdictions()
            .await?;
        let named = self.lang.ui().shell.state_name;
        for jurisdiction in &mut known {
            if let Some(name) = named(&jurisdiction.code) {
                jurisdiction.label = name.to_string();
            }
        }
        known.sort_by_cached_key(|j| (j.code != "fr", lj_fold::fold(&j.label)));
        Ok(known)
    }

    /// Holds `code` as the reader's jurisdiction, in the cookie the bare addresses read.
    /// A reader who has chosen nothing keeps, for the visit, the jurisdiction of
    /// the last scoped page they opened, which the redirect may have read from their country.
    #[cfg(feature = "hydrate")]
    fn follow_address(self, code: &str) {
        if cookie_code().is_none() && self.chosen.get_untracked() != code {
            self.chosen.set(code.to_string());
        }
    }

    #[cfg(feature = "hydrate")]
    pub fn choose(self, code: &str) {
        use wasm_bindgen::JsCast;
        if let Ok(document) = document().dyn_into::<web_sys::HtmlDocument>() {
            let _ = document.set_cookie(&format!(
                "{}={code}; Path=/; Max-Age=31536000; SameSite=Lax; Secure",
                lj_dtos::READER_JURISDICTION_COOKIE
            ));
        }
        self.chosen.set(code.to_string());
    }

    #[cfg(feature = "hydrate")]
    fn adopt_cookie(self) {
        Effect::new(move |_| {
            let Some(code) = cookie_code().filter(|c| *c != self.chosen.get_untracked()) else {
                self.cookie_read.set(true);
                return;
            };
            leptos::task::spawn_local(async move {
                if let Ok(known) = self.load_known().await {
                    if known.iter().any(|j| j.code == code) {
                        self.chosen.set(code);
                    }
                    self.known.set(Some(known));
                }
                self.cookie_read.set(true);
            });
        });
    }
}

#[cfg(feature = "hydrate")]
fn cookie_code() -> Option<String> {
    use wasm_bindgen::JsCast;
    let document = web_sys::window()?.document()?;
    let cookies = document
        .dyn_into::<web_sys::HtmlDocument>()
        .ok()?
        .cookie()
        .ok()?;
    cookies.split(';').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name.trim() == lj_dtos::READER_JURISDICTION_COOKIE).then(|| value.trim().to_string())
    })
}

/// Provided inside the router, whose location the scoped routes are read from, under the
/// reader's language.
pub fn provide_reader_scope() {
    let location = use_location();
    let addressed = Memo::new(move |_| {
        location
            .pathname
            .with(|p| addressed_code(p).map(str::to_string))
    });
    let scope = ReaderScope {
        chosen: RwSignal::new(FRANCE.to_string()),
        known: RwSignal::new(None),
        addressed,
        cookie_read: RwSignal::new(false),
        lang: crate::i18n::use_reader_lang().get(),
    };
    provide_context(scope);
    #[cfg(feature = "hydrate")]
    scope.adopt_cookie();
}

pub fn use_reader_scope() -> ReaderScope {
    expect_context::<ReaderScope>()
}

/// `path` under `code`: `/` is the jurisdiction's home page.
pub fn scoped_path(code: &str, path: &str) -> String {
    match path {
        "/" => format!("/{code}"),
        _ => format!("/{code}{path}"),
    }
}

/// The jurisdiction a link filtered on `seat` goes under: the reader's when its
/// scope covers the seat, else the first known one whose scope does.
pub fn seat_jurisdiction<'a>(
    seat: &str,
    reader: &'a str,
    known: &'a [ReaderJurisdiction],
) -> &'a str {
    let covers = |j: &ReaderJurisdiction| {
        j.seats.iter().any(|root| {
            seat.strip_prefix(root.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
        })
    };
    if known.iter().any(|j| j.code == reader && covers(j)) {
        return reader;
    }
    known
        .iter()
        .find(|j| covers(j))
        .map_or(reader, |j| j.code.as_str())
}

/// The scoped page at `pathname` under `code`, with the query and the mode only:
/// another filter may name what the new scope does not hold.
pub fn rescoped(pathname: &str, query: &ParamsMap, code: &str) -> Option<String> {
    let current = addressed_code(pathname)?;
    let page = &pathname[1 + current.len()..];
    let kept: Vec<String> = ["q", "mode"]
        .into_iter()
        .filter_map(|key| {
            query
                .get_str(key)
                .map(|v| format!("{key}={}", Url::escape(v)))
        })
        .collect();
    Some(match kept.is_empty() {
        true => format!("/{code}{page}"),
        false => format!("/{code}{page}?{}", kept.join("&")),
    })
}

/// The place the norm search reads from under a jurisdiction, when the URL names none.
pub fn home_place(code: &str) -> String {
    match code {
        FRANCE => lj_dtos::DEFAULT_READER_PLACE.to_string(),
        _ => code.to_ascii_uppercase(),
    }
}

fn is_code(segment: &str) -> bool {
    segment.len() == 2 && segment.bytes().all(|b| b.is_ascii_lowercase())
}

/// The code a scoped route's address starts with.
fn addressed_code(pathname: &str) -> Option<&str> {
    let rest = pathname.strip_prefix('/')?;
    let (code, page) = rest.split_once('/').unwrap_or((rest, ""));
    let scoped = matches!(
        page,
        "" | "decision-search" | "norm-search" | "journal-search" | "api-guide"
    );
    (scoped && is_code(code)).then_some(code)
}

/// A scoped route: its page under a jurisdiction the corpus holds, the 404
/// under any other code.
#[component]
pub fn JurisdictionScope(children: ChildrenFn) -> impl IntoView {
    let params = use_params_map();
    let scope = use_reader_scope();
    let known = scope.known_resource();
    let children = StoredValue::new(children);
    view! {
        <Suspense fallback=|| ()>
            {move || {
                let code = params.read().get("juris").unwrap_or_default();
                Suspend::new(async move {
                    match known.await {
                        Ok(list) => {
                            let held = list.iter().any(|j| j.code == code);
                            scope.keep_known(list);
                            match held {
                                true => {
                                    #[cfg(feature = "hydrate")]
                                    scope.follow_address(&code);
                                    // A read below, effects included, would hold this boundary
                                    // and blank the page until the slowest one lands.
                                    view! {
                                        <Provider value=SuspenseContext {
                                            tasks: ArcRwSignal::new(Default::default()),
                                        }>{children.with_value(|c| c())}</Provider>
                                    }
                                        .into_any()
                                }
                                false => view! { <NotFound /> }.into_any(),
                            }
                        }
                        Err(err) => {
                            crate::pages::juridictions_page::set_cache_control(err.status);
                            view! { <crate::pages::juridictions_page::HubError err=err /> }
                                .into_any()
                        }
                    }
                })
            }}
        </Suspense>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jurisdiction(code: &str, seats: &[&str]) -> ReaderJurisdiction {
        ReaderJurisdiction {
            code: code.to_string(),
            label: code.to_string(),
            seats: seats.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// France first, as the API orders them.
    fn known() -> Vec<ReaderJurisdiction> {
        vec![
            jurisdiction("fr", &["FR", "EU", "INTL.CEDH"]),
            jurisdiction("be", &["BE", "EU", "INTL.CEDH"]),
            jurisdiction("ch", &["CH", "INTL.CEDH"]),
            jurisdiction("sn", &["SN", "OHADA", "UEMOA", "CEDEAO"]),
        ]
    }

    /// A seat goes under the reader's jurisdiction when its scope covers the
    /// seat's root, else under the first one whose scope does.
    #[test]
    fn a_seat_link_goes_under_a_jurisdiction_that_covers_it() {
        let known = known();
        let cases = [
            ("CH.TF", "fr", "ch"),
            ("CH.TF", "ch", "ch"),
            ("FR.TJ.PARIS", "ch", "fr"),
            ("BE.CASS", "fr", "be"),
            ("EU.CJUE", "ch", "fr"),
            ("EU.CJUE", "be", "be"),
            ("EU", "ch", "fr"),
            ("OHADA.CCJA", "fr", "sn"),
            ("INTL.CEDH", "ch", "ch"),
            ("INTL.CEDH", "sn", "fr"),
            ("INTL.CIJ", "ch", "ch"),
            ("FRA.X", "ch", "ch"),
        ];
        for (seat, reader, expected) in cases {
            assert_eq!(
                seat_jurisdiction(seat, reader, &known),
                expected,
                "{seat} from {reader}"
            );
        }
    }

    /// Before the list is loaded the reader's jurisdiction holds.
    #[test]
    fn a_seat_link_stays_under_the_reader_without_the_list() {
        assert_eq!(seat_jurisdiction("CH.TF", "fr", &[]), "fr");
    }
}
