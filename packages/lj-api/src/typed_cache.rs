//! The in-process caches: moka caches keyed by `String`, their values erased
//! underneath and restored by type on the way out.

use futures::future::BoxFuture;
use std::any::Any;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

type Erased = Arc<dyn Any + Send + Sync>;
type Inner = moka::future::Cache<String, Erased>;

/// A moka cache of `V` by `String` key. moka monomorphises its machinery once
/// per `Cache<K, V>` and once per init future, a large share of this crate's LLVM
/// IR: one erased value type and boxed futures keep a single copy.
pub struct TypedCache<V> {
    inner: Inner,
    value: PhantomData<fn() -> V>,
}

impl<V> Clone for TypedCache<V> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            value: PhantomData,
        }
    }
}

pub struct Builder<V> {
    inner: moka::future::CacheBuilder<String, Erased, Inner>,
    value: PhantomData<fn() -> V>,
}

impl<V: Clone + Send + Sync + 'static> Builder<V> {
    pub fn time_to_live(self, ttl: Duration) -> Self {
        Self {
            inner: self.inner.time_to_live(ttl),
            value: PhantomData,
        }
    }

    pub fn time_to_idle(self, tti: Duration) -> Self {
        Self {
            inner: self.inner.time_to_idle(tti),
            value: PhantomData,
        }
    }

    /// The capacity then counts the weights, instead of the entries.
    pub fn weigher(self, weigh: fn(&str, &V) -> u32) -> Self {
        Self {
            inner: self
                .inner
                .weigher(move |k: &String, v: &Erased| weigh(k, restore_ref(v))),
            value: PhantomData,
        }
    }

    pub fn build(self) -> TypedCache<V> {
        TypedCache {
            inner: self.inner.build(),
            value: PhantomData,
        }
    }
}

impl<V: Clone + Send + Sync + 'static> TypedCache<V> {
    pub fn builder(max_capacity: u64) -> Builder<V> {
        Builder {
            inner: moka::future::Cache::builder().max_capacity(max_capacity),
            value: PhantomData,
        }
    }

    pub async fn get(&self, key: &str) -> Option<V> {
        self.inner
            .get(key)
            .await
            .map(|v| restore_ref::<V>(&v).clone())
    }

    pub async fn insert(&self, key: String, value: V) {
        self.inner.insert(key, erase(value)).await
    }

    pub async fn get_with(&self, key: String, init: impl Future<Output = V> + Send) -> V {
        let init: BoxFuture<'_, Erased> = Box::pin(async move { erase(init.await) });
        restore_ref::<V>(&self.inner.get_with(key, init).await).clone()
    }

    /// Only one caller of a key runs `init`, and a `None` is not kept.
    pub async fn optionally_get_with(
        &self,
        key: String,
        init: impl Future<Output = Option<V>> + Send,
    ) -> Option<V> {
        let init: BoxFuture<'_, Option<Erased>> = Box::pin(async move { init.await.map(erase) });
        self.inner
            .optionally_get_with(key, init)
            .await
            .map(|v| restore_ref::<V>(&v).clone())
    }

    /// Only one caller of a key runs `init`, and an error is not kept.
    pub async fn try_get_with<E: Send + Sync + 'static>(
        &self,
        key: String,
        init: impl Future<Output = Result<V, E>> + Send,
    ) -> Result<V, Arc<E>> {
        let init: BoxFuture<'_, Result<Erased, E>> = Box::pin(async move { init.await.map(erase) });
        self.inner
            .try_get_with(key, init)
            .await
            .map(|v| restore_ref::<V>(&v).clone())
    }

    pub async fn upsert_with(&self, key: String, f: impl FnOnce(Option<V>) -> V) {
        self.inner
            .entry(key)
            .and_upsert_with(move |old| {
                let old = old.map(|e| restore_ref::<V>(e.value()).clone());
                std::future::ready(erase(f(old)))
            })
            .await;
    }

    /// `entry_count` and `weighted_size` are exact only after this.
    pub async fn run_pending_tasks(&self) {
        self.inner.run_pending_tasks().await
    }

    pub fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }

    pub fn weighted_size(&self) -> u64 {
        self.inner.weighted_size()
    }

    pub fn max_capacity(&self) -> Option<u64> {
        self.inner.policy().max_capacity()
    }
}

fn erase<V: Send + Sync + 'static>(value: V) -> Erased {
    Arc::new(value)
}

fn restore_ref<V: 'static>(value: &Erased) -> &V {
    value
        .downcast_ref::<V>()
        .expect("a TypedCache only holds its own V")
}
