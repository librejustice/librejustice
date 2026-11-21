//! Where the anonymised file goes from the workbench: the device, or the host framing it.

use std::future::Future;
use std::pin::Pin;

use leptos::prelude::*;

use super::fault::Fault;

#[derive(Clone, Debug, PartialEq)]
pub enum Handover {
    Idle,
    Busy,
    Done(&'static str),
    Failed(Fault),
}

type Sending = Pin<Box<dyn Future<Output = Result<Option<&'static str>, Fault>>>>;

#[derive(Clone, Copy)]
pub struct Outlet {
    pub label: &'static str,
    pub state: RwSignal<Handover>,
    send: Callback<(String, Vec<u8>), Sending>,
}

impl Outlet {
    /// `send` answers what to tell the user once the file has gone, if anything.
    pub fn new<F, S>(label: &'static str, send: F) -> Self
    where
        F: Fn(String, Vec<u8>) -> S + Send + Sync + 'static,
        S: Future<Output = Result<Option<&'static str>, Fault>> + 'static,
    {
        Self {
            label,
            state: RwSignal::new(Handover::Idle),
            send: Callback::new(move |(name, bytes)| Box::pin(send(name, bytes)) as Sending),
        }
    }

    pub fn device() -> Self {
        Self::new(
            crate::i18n::use_ui().redact.export,
            |name, bytes| async move {
                #[cfg(feature = "hydrate")]
                super::engine::save(&name, &bytes);
                #[cfg(not(feature = "hydrate"))]
                let _ = (name, bytes);
                Ok(None)
            },
        )
    }

    pub fn fire(self) {
        let Some(file) = produced() else {
            return;
        };
        self.state.set(Handover::Busy);
        let sending = self.send.run(file);
        leptos::task::spawn_local(async move {
            self.state.set(match sending.await {
                Ok(Some(said)) => Handover::Done(said),
                Ok(None) => Handover::Idle,
                Err(why) => Handover::Failed(why),
            });
        });
    }
}

fn produced() -> Option<(String, Vec<u8>)> {
    #[cfg(feature = "hydrate")]
    return super::engine::produced_file();
    #[cfg(not(feature = "hydrate"))]
    None
}

/// What the page offers to do with the file, the first foremost.
#[derive(Clone)]
pub struct Outlets(pub Vec<Outlet>);
