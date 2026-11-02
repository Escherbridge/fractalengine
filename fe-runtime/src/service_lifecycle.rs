//! Owned service shutdown primitives; see AGENTS.md §terminal-maintenance.

use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::thread::JoinHandle;

#[derive(Clone, Default)]
pub struct StopSignal(Arc<StopState>);

#[derive(Default)]
struct StopState {
    requested: AtomicBool,
    notify: tokio::sync::Notify,
}

impl StopSignal {
    pub fn request(&self) {
        self.0.requested.store(true, Ordering::Release);
        self.0.notify.notify_waiters();
    }

    pub fn is_requested(&self) -> bool {
        self.0.requested.load(Ordering::Acquire)
    }

    pub async fn cancelled(&self) {
        loop {
            let notified = self.0.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_requested() { return; }
            notified.await;
        }
    }
}

pub struct OwnedService {
    name: &'static str,
    stop: StopSignal,
    thread: Option<JoinHandle<()>>,
}

impl OwnedService {
    pub fn new(name: &'static str, stop: StopSignal, thread: JoinHandle<()>) -> Self {
        Self { name, stop, thread: Some(thread) }
    }

    pub fn name(&self) -> &'static str { self.name }

    pub fn request_stop(&self) { self.stop.request(); }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub fn try_join(&mut self) -> Result<bool, &'static str> {
        if !self.is_finished() { return Ok(false); }
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| self.name)?;
        }
        Ok(true)
    }

    pub fn into_join_handle(mut self) -> JoinHandle<()> {
        self.thread.take().expect("owned service thread already joined")
    }
}
