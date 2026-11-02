//! Terminal host maintenance contracts; see AGENTS.md §terminal-maintenance.

use crate::service_lifecycle::OwnedService;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};

#[derive(Debug)]
pub struct ResetRequest {
    pub request_id: String,
    pub confirmation: String,
}

#[derive(Debug, Clone)]
pub struct ResetRequestSender(pub crossbeam::channel::Sender<ResetRequest>);

#[derive(Default)]
struct JobState {
    frozen: bool,
    active: usize,
    failed: bool,
}

#[derive(Clone, Default)]
pub struct HostJobs(Arc<Mutex<JobState>>);

static HOST_JOBS: OnceLock<HostJobs> = OnceLock::new();

impl HostJobs {
    pub fn install() -> Result<Self, &'static str> {
        let jobs = Self::default();
        HOST_JOBS
            .set(jobs.clone())
            .map_err(|_| "host jobs already installed")?;
        Ok(jobs)
    }

    pub fn freeze(&self) -> Result<(), &'static str> {
        self.0
            .lock()
            .map_err(|_| "host job state unavailable")?
            .frozen = true;
        Ok(())
    }

    pub fn remaining(&self) -> Result<usize, &'static str> {
        let state = self.0.lock().map_err(|_| "host job state unavailable")?;
        if !state.frozen {
            return Err("host jobs have not been frozen");
        }
        if state.failed {
            return Err("a host job panicked before quiescence");
        }
        Ok(state.active)
    }
}

pub struct JobLease(Option<HostJobs>);

impl Drop for JobLease {
    fn drop(&mut self) {
        if let Some(jobs) = &self.0 {
            match jobs.0.lock() {
                Ok(mut state) => {
                    state.active = state.active.saturating_sub(1);
                    state.failed |= std::thread::panicking();
                }
                Err(_) => tracing::error!("host job completion state unavailable"),
            }
        }
    }
}

pub fn begin_job() -> Option<JobLease> {
    let Some(jobs) = HOST_JOBS.get() else {
        return Some(JobLease(None));
    };
    let mut state = jobs.0.lock().ok()?;
    if state.frozen {
        return None;
    }
    state.active = state.active.checked_add(1)?;
    Some(JobLease(Some(jobs.clone())))
}

pub fn spawn_job(work: impl FnOnce() + Send + 'static) {
    let Some(lease) = begin_job() else {
        return;
    };
    if let Err(error) = std::thread::Builder::new()
        .name("fe-host-job".into())
        .spawn(move || {
            let _lease = lease;
            work();
        })
    {
        tracing::error!(%error, "host job could not start");
    }
}

#[derive(Debug, Clone)]
pub struct QuiescenceProof(Arc<AtomicBool>);

impl QuiescenceProof {
    pub fn after_joined(
        services: &mut [OwnedService],
        jobs: &HostJobs,
    ) -> Result<Option<Self>, &'static str> {
        for required in [
            "api",
            "sync",
            "network",
            "plugin",
            "replication",
            "distributed-query",
            "scene-bridge",
            "transform-bridge",
            "listener-fanout",
        ] {
            if services
                .iter()
                .filter(|service| service.name() == required)
                .count()
                != 1
            {
                return Err("required service ownership is incomplete");
            }
        }
        for service in services {
            if !service.stop_requested() {
                return Err("service stop has not been requested");
            }
            if !service.try_join()? {
                return Ok(None);
            }
        }
        if jobs.remaining()? != 0 {
            return Ok(None);
        }
        Ok(Some(Self(Arc::new(AtomicBool::new(false)))))
    }

    pub fn consume(&self) -> bool {
        !self.0.swap(true, Ordering::AcqRel)
    }
}

#[derive(bevy::prelude::Resource, Default)]
pub struct TerminalMode(bool);

impl TerminalMode {
    pub fn enter(&mut self) {
        self.0 = true;
    }
    pub fn is_terminal(&self) -> bool {
        self.0
    }
}

pub fn editor_running(mode: Option<bevy::prelude::Res<TerminalMode>>) -> bool {
    mode.is_none_or(|mode| !mode.0)
}
