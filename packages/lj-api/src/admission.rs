//! Admission of the page renders: a crawler waits for a slot under its own cap and a
//! shared one, everyone else spends from the budgets of its address and its block, and a
//! link preview from one budget all previews share as well. The corpus reads a hydrated
//! page makes over `/internal` spend from their address's budget.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes, HttpBody};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use lj_crawlers::Admission;
use opentelemetry::KeyValue;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::ApiError;
use crate::ratelimit::{self, Cost, Verdict};
use crate::state::AppState;
use crate::typed_cache::TypedCache;

// Measured 4-11/11/2026 at the edge, renders in flight per minute: PerplexityBot 3 to 12
// on a normal day, 23 to 83 when the origin slowed; every other crawler under 2.
const ALL_CRAWLERS: usize = 24;
const PER_CRAWLER: usize = 8;
// A crawler waits rather than retries: a burst queues, only a held origin refuses.
const WAIT: Duration = Duration::from_secs(5);
const RETRY_AFTER_SECS: &str = "60";
const REFUSAL_PAGE: &str = include_str!("../templates/refused.html");
const MAX_NAMES: u64 = 10_000;
const NAME_IDLE: Duration = Duration::from_secs(600);

/// The concurrency slots of the crawlers, shared and per name.
#[derive(Clone)]
pub struct CrawlerSlots {
    all: Arc<Semaphore>,
    by_name: TypedCache<Arc<Semaphore>>,
    per_crawler: usize,
    wait: Duration,
}

impl Default for CrawlerSlots {
    fn default() -> Self {
        Self::new(ALL_CRAWLERS, PER_CRAWLER, WAIT)
    }
}

impl CrawlerSlots {
    fn new(all: usize, per_crawler: usize, wait: Duration) -> Self {
        Self {
            all: Arc::new(Semaphore::new(all)),
            by_name: TypedCache::builder(MAX_NAMES)
                .time_to_idle(NAME_IDLE)
                .build(),
            per_crawler,
            wait,
        }
    }

    /// Its own slot unless [`lj_crawlers::UNCAPPED`] names it, and a shared one, or
    /// `None` when they did not free up within the wait.
    async fn enter(&self, name: &str) -> Option<Slot> {
        let deadline = tokio::time::Instant::now() + self.wait;
        let own = if lj_crawlers::UNCAPPED.contains(&name) {
            None
        } else {
            let per_crawler = self.per_crawler;
            let own = self
                .by_name
                .get_with(name.to_string(), async move {
                    Arc::new(Semaphore::new(per_crawler))
                })
                .await;
            Some(
                tokio::time::timeout_at(deadline, own.acquire_owned())
                    .await
                    .ok()?
                    .expect("crawler slots are never closed"),
            )
        };
        let all = tokio::time::timeout_at(deadline, self.all.clone().acquire_owned())
            .await
            .ok()?
            .expect("crawler slots are never closed");
        Some(Slot {
            _own: own,
            _all: all,
        })
    }
}

struct Slot {
    _own: Option<OwnedSemaphorePermit>,
    _all: OwnedSemaphorePermit,
}

pub async fn admit(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if crate::usage::internal_origin(
        req.headers(),
        state.settings.internal_origin_token.as_deref(),
    ) {
        return next.run(req).await;
    }
    let user_agent = crate::usage::user_agent(req.headers()).unwrap_or_default();
    let (name, label) = match lj_crawlers::admission_of(user_agent) {
        Admission::Attended => {
            rendered(&[KeyValue::new("kind", "attended")]);
            return next.run(req).await;
        }
        Admission::Crawler(token) => (token.to_string(), token),
        Admission::Undeclared(name) => (name, "undeclared"),
        Admission::Preview => {
            if spends_a_preview(&state.budgets, req.headers(), Instant::now()).await {
                rendered(&[KeyValue::new("kind", "preview")]);
                return next.run(req).await;
            }
            refused(&[KeyValue::new("kind", "preview")]);
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(
                    header::RETRY_AFTER,
                    HeaderValue::from_static(RETRY_AFTER_SECS),
                )],
            )
                .into_response();
        }
        Admission::Caller => {
            if spends_a_render(&state, req.headers()).await {
                rendered(&[KeyValue::new("kind", "caller")]);
                return next.run(req).await;
            }
            refused(&[KeyValue::new("kind", "caller")]);
            return refusal_page(req.uri());
        }
    };
    let Some(slot) = state.crawler_slots.enter(&name).await else {
        refused(&[
            KeyValue::new("kind", "crawler"),
            KeyValue::new("crawler", label),
        ]);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(
                header::RETRY_AFTER,
                HeaderValue::from_static(RETRY_AFTER_SECS),
            )],
        )
            .into_response();
    };
    rendered(&[
        KeyValue::new("kind", "crawler"),
        KeyValue::new("crawler", label),
    ]);
    let (parts, body) = next.run(req).await.into_parts();
    Response::from_parts(parts, Body::new(Held { body, _slot: slot }))
}

async fn spends_a_render(state: &AppState, headers: &HeaderMap) -> bool {
    let address = ratelimit::caller_key(None, headers);
    let block = ratelimit::block_key(headers);
    if state.budgets.take(Cost::Page, address.as_ref()).await == Verdict::Allowed
        && state.budgets.take(Cost::PageBlock, block.as_ref()).await == Verdict::Allowed
    {
        return true;
    }
    account_spends(state, headers, Cost::Page).await
}

/// The address and its block first, so that one sender empties its own buckets and not the
/// one every preview shares.
async fn spends_a_preview(budgets: &ratelimit::Budgets, headers: &HeaderMap, now: Instant) -> bool {
    let address = ratelimit::caller_key(None, headers);
    let block = ratelimit::block_key(headers);
    budgets.take_at(Cost::Page, address.as_ref(), now).await == Verdict::Allowed
        && budgets.take_at(Cost::PageBlock, block.as_ref(), now).await == Verdict::Allowed
        && budgets
            .take_at(Cost::Preview, Some(&ratelimit::previews_key()), now)
            .await
            == Verdict::Allowed
}

/// A corpus read over `/internal`: one `PageData` token on the address, else on the
/// account of a valid session. The origin's own traffic skips it.
pub async fn admit_read(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let origin = crate::usage::internal_origin(
        req.headers(),
        state.settings.internal_origin_token.as_deref(),
    );
    let address = ratelimit::caller_key(None, req.headers());
    if origin
        || state.budgets.take(Cost::PageData, address.as_ref()).await == Verdict::Allowed
        || account_spends(&state, req.headers(), Cost::PageData).await
    {
        return next.run(req).await;
    }
    refused(&[KeyValue::new("kind", "read")]);
    ApiError::TooManyRequests.into_response()
}

/// The session is read only once the anonymous budgets refuse, so an admitted request
/// costs no extra query.
async fn account_spends(state: &AppState, headers: &HeaderMap, cost: Cost) -> bool {
    let Some((token, _)) = crate::session::from_headers(headers) else {
        return false;
    };
    let crate::session::Verdict::Porteur { sub, .. } = crate::session::resolve(state, &token).await
    else {
        return false;
    };
    let account = ratelimit::caller_key(Some(&sub), headers);
    state.budgets.take(cost, account.as_ref()).await == Verdict::Allowed
}

/// What a refused person reads: a static page offering to sign in and come back to `uri`.
fn refusal_page(uri: &Uri) -> Response {
    let target = uri.path_and_query().map_or("/", |p| p.as_str());
    let next: String = url::form_urlencoded::byte_serialize(target.as_bytes()).collect();
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(
            header::RETRY_AFTER,
            HeaderValue::from_static(RETRY_AFTER_SECS),
        )],
        Html(REFUSAL_PAGE.replacen("{next}", &next, 1)),
    )
        .into_response()
}

fn refused(attributes: &[KeyValue]) {
    lj_telemetry::meter()
        .u64_counter("librejustice.admission.refusals")
        .with_description("Page renders refused by admission control.")
        .build()
        .add(1, attributes);
}

fn rendered(attributes: &[KeyValue]) {
    lj_telemetry::meter()
        .u64_counter("librejustice.admission.renders")
        .with_description("Page renders admitted by admission control.")
        .build()
        .add(1, attributes);
}

/// The response body, holding the crawler's slots until it is sent: a streamed render
/// does most of its work after the headers.
struct Held {
    body: Body,
    _slot: Slot,
}

impl HttpBody for Held {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, axum::Error>>> {
        Pin::new(&mut self.get_mut().body).poll_frame(cx)
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.body.size_hint()
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(20);

    #[tokio::test]
    async fn a_crawler_past_its_cap_is_refused_and_leaves_the_others_their_slots() {
        let slots = CrawlerSlots::new(3, 2, SHORT);
        let first = slots.enter("a").await.unwrap();
        let _second = slots.enter("a").await.unwrap();
        assert!(slots.enter("a").await.is_none());
        let _other = slots.enter("b").await.unwrap();
        assert!(slots.enter("c").await.is_none(), "the shared cap is full");
        drop(first);
        assert!(slots.enter("c").await.is_some());
    }

    #[tokio::test]
    async fn an_uncapped_crawler_is_held_by_the_shared_slots_alone() {
        let slots = CrawlerSlots::new(2, 1, SHORT);
        let _first = slots.enter("PerplexityBot").await.unwrap();
        let _second = slots.enter("PerplexityBot").await.unwrap();
        assert!(slots.enter("PerplexityBot").await.is_none());
    }

    #[tokio::test]
    async fn a_crawler_waiting_gets_the_slot_freed_within_the_wait() {
        let slots = CrawlerSlots::new(1, 1, Duration::from_secs(5));
        let held = slots.enter("a").await.unwrap();
        let waiting = tokio::spawn({
            let slots = slots.clone();
            async move { slots.enter("a").await.is_some() }
        });
        tokio::time::sleep(SHORT).await;
        drop(held);
        assert!(waiting.await.unwrap());
    }

    #[tokio::test]
    async fn a_refused_person_is_offered_to_sign_in_and_come_back() {
        let uri: Uri = "/decision/abc?q=bail%20commercial&p=2".parse().unwrap();
        let resp = refusal_page(&uri);
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(resp.headers()[header::RETRY_AFTER], RETRY_AFTER_SECS);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(body
            .contains(r#"href="/login?next=%2Fdecision%2Fabc%3Fq%3Dbail%2520commercial%26p%3D2""#));
    }

    fn from(address: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", address.parse().unwrap());
        headers
    }

    async fn admitted(budgets: &ratelimit::Budgets, senders: &[HeaderMap], now: Instant) -> usize {
        let mut admitted = 0;
        for headers in senders {
            if spends_a_preview(budgets, headers, now).await {
                admitted += 1;
            }
        }
        admitted
    }

    #[tokio::test]
    async fn one_sender_empties_its_own_buckets_and_many_meet_the_shared_one() {
        let budgets = ratelimit::Budgets::new(1, 1);
        let now = Instant::now();
        let one = vec![from("198.51.100.7"); 1000];
        let own = admitted(&budgets, &one, now).await;
        assert_eq!(own, 2 * ratelimit::PAGES_PER_MIN as usize);
        let many: Vec<HeaderMap> = (0..1000)
            .map(|i| from(&format!("203.{}.{}.1", i / 256, i % 256)))
            .collect();
        let shared = 2 * ratelimit::PREVIEWS_PER_MIN as usize;
        assert_eq!(admitted(&budgets, &many, now).await, shared - own);
    }

    #[tokio::test]
    async fn the_slots_are_held_until_the_body_is_sent() {
        let slots = CrawlerSlots::new(1, 1, SHORT);
        let slot = slots.enter("a").await.unwrap();
        let body = Body::new(Held {
            body: Body::from("page"),
            _slot: slot,
        });
        assert!(slots.enter("a").await.is_none());
        drop(body);
        assert!(slots.enter("a").await.is_some());
    }
}
