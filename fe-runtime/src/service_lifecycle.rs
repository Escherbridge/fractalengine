//! Owned service shutdown primitives; see AGENTS.md §terminal-maintenance.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
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
            if self.is_requested() {
                return;
            }
            notified.await;
        }
    }
}

pub struct OwnedService {
    name: &'static str,
    stop: StopSignal,
    thread: Option<JoinHandle<()>>,
    failed: bool,
}

impl OwnedService {
    pub fn spawn(name: &'static str, work: impl FnOnce(StopSignal) + Send + 'static) -> Self {
        let stop = StopSignal::default();
        let worker_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name(format!("fe-{name}"))
            .spawn(move || work(worker_stop))
            .expect("host service could not start");
        Self::new(name, stop, thread)
    }

    pub fn new(name: &'static str, stop: StopSignal, thread: JoinHandle<()>) -> Self {
        Self {
            name,
            stop,
            thread: Some(thread),
            failed: false,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn request_stop(&self) {
        self.stop.request();
    }

    pub fn stop_requested(&self) -> bool {
        self.stop.is_requested()
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub fn try_join(&mut self) -> Result<bool, &'static str> {
        if self.failed {
            return Err(self.name);
        }
        if !self.is_finished() {
            return Ok(false);
        }
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                self.failed = true;
                return Err(self.name);
            }
        }
        Ok(true)
    }

    pub fn into_join_handle(mut self) -> JoinHandle<()> {
        self.thread
            .take()
            .expect("owned service thread already joined")
    }
}
