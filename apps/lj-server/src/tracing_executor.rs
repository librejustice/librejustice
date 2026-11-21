use crate::pinned_render::{on_render_thread, rendering};
use any_spawner::{CustomExecutor, Executor, PinnedFuture, PinnedLocalFuture};
use anyhow::{anyhow, Result};
use tokio::runtime::Handle;
use tracing::Instrument;

/// Every task a render spawns stays on its thread, `Send` ones included: a
/// `Suspense` renders in one, under an owner holding the render's local values.
struct TracingExecutor {
    server: Handle,
}

impl CustomExecutor for TracingExecutor {
    fn spawn(&self, fut: PinnedFuture<()>) {
        let fut = fut.instrument(tracing::Span::current());
        if on_render_thread() {
            tokio::task::spawn_local(rendering(self.server.clone(), fut));
        } else {
            self.server.spawn(fut);
        }
    }

    fn spawn_local(&self, fut: PinnedLocalFuture<()>) {
        tokio::task::spawn_local(rendering(
            self.server.clone(),
            fut.instrument(tracing::Span::current()),
        ));
    }

    fn poll_local(&self) {}
}

/// Call once, before `generate_route_list` and before serving: `leptos_axum`
/// registers the bare tokio executor otherwise.
pub fn install(server: Handle) -> Result<()> {
    Executor::init_custom_executor(TracingExecutor { server }).map_err(|e| {
        anyhow!(
            "any_spawner executor already registered ({e:?}): \
             `install()` must come before any leptos_axum call"
        )
    })
}

#[cfg(test)]
mod tests {
    /// `install()` writes a process-wide `OnceLock`: no other test of the binary
    /// touches the executor.
    #[test]
    fn a_leptos_task_keeps_its_span_and_its_render_thread() {
        use tracing_subscriber::util::SubscriberInitExt as _;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        super::install(runtime.handle().clone()).unwrap();
        let _subscriber = tracing_subscriber::registry().set_default();

        runtime.block_on(async {
            let root = tracing::info_span!("root");
            let expected = root.id();
            assert!(expected.is_some(), "the root span has an id");
            let (tx, rx) = tokio::sync::oneshot::channel();
            root.in_scope(|| {
                super::Executor::spawn(async move {
                    let _ = tx.send(tracing::Span::current().id());
                });
            });
            assert_eq!(rx.await.unwrap(), expected, "span lost at spawn");
        });

        runtime.block_on(async {
            let handler = || async {
                let (tx, rx) = tokio::sync::oneshot::channel();
                super::Executor::spawn(async move {
                    let _ = tx.send(std::thread::current().id());
                });
                let spawned_on = rx.await.unwrap();
                assert_eq!(
                    spawned_on,
                    std::thread::current().id(),
                    "task left its render"
                );
            };
            let app = axum::Router::new()
                .route("/", axum::routing::get(handler))
                .layer(axum::middleware::from_fn_with_state(
                    crate::pinned_render::Renders::new(1),
                    crate::pinned_render::render,
                ));
            let request = axum::extract::Request::builder()
                .uri("/")
                .body(axum::body::Body::empty())
                .unwrap();
            let response = tower::ServiceExt::oneshot(app, request).await.unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK);
        });
    }
}
