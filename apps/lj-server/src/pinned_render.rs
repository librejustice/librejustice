use axum::body::{Body, Bytes, HttpBody};
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use http_body::{Frame, SizeHint};
use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio_util::task::{AbortOnDropHandle, LocalPoolHandle};
use tracing::Instrument as _;

const FRAMES_IN_FLIGHT: usize = 16;

thread_local! {
    static RENDER_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// The render threads, one single-thread runtime each, and the server's runtime.
#[derive(Clone)]
pub struct Renders {
    pool: LocalPoolHandle,
    server: Handle,
}

impl Renders {
    pub fn new(threads: usize) -> Self {
        Self {
            pool: LocalPoolHandle::new(threads),
            server: Handle::current(),
        }
    }
}

pub fn on_render_thread() -> bool {
    RENDER_THREAD.get()
}

/// Polls a render's future with the server's runtime entered: the sockets, timers,
/// tasks and blocking calls the service code starts from it belong to the server.
pub struct Rendering<F> {
    fut: F,
    server: Handle,
}

pub fn rendering<F: Future + Unpin>(server: Handle, fut: F) -> Rendering<F> {
    Rendering { fut, server }
}

impl<F: Future + Unpin> Future for Rendering<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.get_mut();
        RENDER_THREAD.set(true);
        let _server = this.server.enter();
        Pin::new(&mut this.fut).poll(cx)
    }
}

/// Runs the handler on one render thread, its body polled and dropped there: a
/// Leptos render keeps thread-bound values in its reactive owner, which goes with
/// the body's last chunk.
pub async fn render(State(renders): State<Renders>, req: Request, next: Next) -> Response {
    let span = tracing::Span::current();
    let (frames_tx, frames) = mpsc::channel(FRAMES_IN_FLIGHT);
    let server = renders.server;
    let head = renders.pool.spawn_pinned(move || {
        let handler = async move {
            let (parts, mut body) = next.run(req).await.into_parts();
            let hint = body.size_hint();
            let frames = async move {
                while let Some(frame) =
                    std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await
                {
                    if frames_tx.send(frame).await.is_err() {
                        break;
                    }
                }
            };
            let forward = tokio::task::spawn_local(rendering(
                Handle::current(),
                Box::pin(frames.in_current_span()),
            ));
            (parts, hint, AbortOnDropHandle::new(forward))
        };
        rendering(server, Box::pin(handler.instrument(span)))
    });
    let (parts, hint, forward) = match AbortOnDropHandle::new(head).await {
        Ok(head) => head,
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    };
    Response::from_parts(
        parts,
        Body::new(PinnedBody {
            frames,
            hint,
            forward: Some(forward),
        }),
    )
}

/// Dropping it aborts the forwarding task, which drops the handler's body on its
/// own thread.
struct PinnedBody {
    frames: mpsc::Receiver<Result<Frame<Bytes>, axum::Error>>,
    hint: SizeHint,
    forward: Option<AbortOnDropHandle<()>>,
}

impl HttpBody for PinnedBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
        let body = self.get_mut();
        if let Some(frame) = ready!(body.frames.poll_recv(cx)) {
            return Poll::Ready(Some(frame));
        }
        let Some(forward) = body.forward.as_mut() else {
            return Poll::Ready(None);
        };
        let ended = ready!(Pin::new(forward).poll(cx));
        body.forward = None;
        Poll::Ready(ended.err().map(|e| Err(axum::Error::new(e))))
    }

    fn size_hint(&self) -> SizeHint {
        self.hint.clone()
    }

    fn is_end_stream(&self) -> bool {
        self.hint.exact() == Some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::{on_render_thread, render, Renders};
    use axum::body::{Body, HttpBody};
    use axum::extract::Request;
    use axum::Router;
    use std::sync::{Arc, Mutex};
    use std::thread::ThreadId;
    use tokio::runtime::{Handle, RuntimeFlavor};
    use tokio::sync::oneshot;
    use tower::ServiceExt;

    type Threads = Arc<Mutex<Vec<(&'static str, ThreadId)>>>;

    fn seen(threads: &Threads, event: &'static str) {
        threads
            .lock()
            .unwrap()
            .push((event, std::thread::current().id()));
    }

    fn one_thread(threads: &Threads) -> bool {
        let threads = threads.lock().unwrap();
        threads.iter().all(|(_, id)| *id == threads[0].1)
    }

    struct DropProbe(Threads, Option<oneshot::Sender<()>>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            seen(&self.0, "drop");
            let _ = self.1.take().unwrap().send(());
        }
    }

    /// Three frames, or one and then none for ever when `hang`, each step recording
    /// the thread it runs on, the server's runtime entered.
    fn app(
        renders: &Renders,
        threads: Threads,
        dropped: oneshot::Sender<()>,
        hang: bool,
    ) -> Router {
        let dropped = Arc::new(Mutex::new(Some(dropped)));
        let handler = move || {
            let (threads, dropped) = (threads.clone(), dropped.clone());
            async move {
                assert!(on_render_thread());
                assert_eq!(
                    Handle::current().runtime_flavor(),
                    RuntimeFlavor::MultiThread
                );
                seen(&threads, "create");
                let probe = DropProbe(threads.clone(), dropped.lock().unwrap().take());
                Body::from_stream(futures::stream::unfold(0, move |n| {
                    let _held = &probe;
                    let threads = threads.clone();
                    async move {
                        tokio::task::yield_now().await;
                        seen(&threads, "poll");
                        if hang && n == 1 {
                            std::future::pending::<()>().await;
                        }
                        (n < 3).then(|| (Ok::<_, std::io::Error>(format!("<p>{n}</p>")), n + 1))
                    }
                }))
            }
        };
        Router::new().route("/", axum::routing::get(handler)).layer(
            axum::middleware::from_fn_with_state(renders.clone(), render),
        )
    }

    fn get() -> Request {
        Request::builder().uri("/").body(Body::empty()).unwrap()
    }

    /// Spec: on a multi-thread runtime, the handler's body is created, polled and
    /// dropped on one thread, and arrives whole.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_render_lives_and_dies_on_one_thread() {
        let renders = Renders::new(2);
        for _ in 0..20 {
            let threads = Threads::default();
            let (tx, dropped) = oneshot::channel();
            let response = app(&renders, threads.clone(), tx, false)
                .oneshot(get())
                .await
                .unwrap();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(&bytes[..], b"<p>0</p><p>1</p><p>2</p>");
            dropped.await.unwrap();
            assert!(one_thread(&threads), "{:?}", threads.lock().unwrap());
        }
    }

    /// Spec: a client gone mid-body drops the handler's body on its own thread.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_client_gone_drops_the_render_on_its_thread() {
        let renders = Renders::new(2);
        let threads = Threads::default();
        let (tx, dropped) = oneshot::channel();
        let response = app(&renders, threads.clone(), tx, true)
            .oneshot(get())
            .await
            .unwrap();
        let mut body = response.into_body();
        let first = std::future::poll_fn(|cx| std::pin::Pin::new(&mut body).poll_frame(cx))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.into_data().unwrap(), "<p>0</p>");
        drop(body);
        dropped.await.unwrap();
        assert!(one_thread(&threads), "{:?}", threads.lock().unwrap());
    }

    /// Spec: a body cut by a panic ends on an error, never as a complete response.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_panicking_body_ends_on_an_error() {
        let renders = Renders::new(1);
        let app = Router::new()
            .route(
                "/",
                axum::routing::get(|| async {
                    let mut sent = false;
                    Body::from_stream(futures::stream::poll_fn(move |_| {
                        assert!(!sent, "render failed");
                        sent = true;
                        std::task::Poll::Ready(Some(Ok::<_, std::io::Error>("<p>")))
                    }))
                }),
            )
            .layer(axum::middleware::from_fn_with_state(renders, render));
        let response = app.oneshot(get()).await.unwrap();
        assert!(axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .is_err());
    }
}
