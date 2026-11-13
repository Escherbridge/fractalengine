//! The app's admin-channel listener: requests from this OS user become ops-worker jobs (docs/extensions/admin-channel.md §1-§5).
//! Admission and framing live in `fe_plugin::admin` (fe-plugin/src/admin/AGENTS.md); this file only relays.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use bevy::prelude::Resource;
use fe_plugin::admin::protocol::{AdminReply, AdminRequest, RefusalCode};
use fe_plugin::admin::transport::{
    listen_local, next_request, write_reply, AdminListener, Incoming, LocalEndpoint, ENV_ADMIN_INSTANCE,
};
use fe_plugin::admin::{channel_enabled, CONSENT_WAIT, ENV_ADMIN_CHANNEL, MAX_CLIENTS, REQUEST_READ_TIMEOUT};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::{mpsc, watch, Semaphore};
use tokio::time::Instant;

use super::ops_drain::{AdminJob, AdminJobSender};

/// How long the listener waits for the ops worker's first answer before replying `internal`, so a hung worker frees the slot.
pub const WORKER_ANSWER_WAIT: Duration = Duration::from_secs(5 * 60);
/// How long a connection refused as `busy` may take to send its request, so the refusal can echo the id.
const BUSY_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Pause after a failed accept so a broken endpoint cannot spin the thread.
const ACCEPT_RETRY: Duration = Duration::from_millis(250);

/// Where admitted requests go: the ops worker in the app, a scripted worker in tests.
pub(crate) trait AdminJobSink: Send + Sync + 'static {
    /// Queue one request; the worker answers on `replies` with at most one `pending-consent`, then one final reply.
    fn submit(&self, request: AdminRequest, replies: mpsc::UnboundedSender<AdminReply>) -> Result<(), String>;
}

impl AdminJobSink for AdminJobSender {
    fn submit(&self, request: AdminRequest, replies: mpsc::UnboundedSender<AdminReply>) -> Result<(), String> {
        self.send(AdminJob { request, replies }).map_err(|_| "the extension ops worker is not running".to_owned())
    }
}

/// Listener timings; the contract values outside tests.
#[derive(Debug, Clone, Copy)]
struct Waits {
    request_read: Duration,
    worker_answer: Duration,
    consent: Duration,
}

impl Default for Waits {
    fn default() -> Self {
        Self { request_read: REQUEST_READ_TIMEOUT, worker_answer: WORKER_ANSWER_WAIT, consent: CONSENT_WAIT }
    }
}

/// What the listener needs from the binary: the ops worker's admin queue and whether a window can show the consent dialog.
pub struct AdminChannelPorts {
    sink: Arc<dyn AdminJobSink>,
    gui_present: bool,
    endpoint: Option<LocalEndpoint>,
    waits: Waits,
}

impl AdminChannelPorts {
    /// Ports over the ops worker's admin queue; `gui_present` is false on a headless host, which keeps the channel off.
    pub fn new(jobs: AdminJobSender, gui_present: bool) -> Self {
        Self::with_sink(Arc::new(jobs), gui_present)
    }

    pub(crate) fn with_sink(sink: Arc<dyn AdminJobSink>, gui_present: bool) -> Self {
        Self { sink, gui_present, endpoint: None, waits: Waits::default() }
    }

    /// Serve this endpoint instead of the one `FE_EXT_ADMIN_INSTANCE` names.
    pub fn with_endpoint(mut self, endpoint: LocalEndpoint) -> Self {
        self.endpoint = Some(endpoint);
        self
    }

    #[cfg(test)]
    fn with_waits(mut self, waits: Waits) -> Self {
        self.waits = waits;
        self
    }
}

/// Whether the channel serves, and where or why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminChannelStatus {
    /// Serving at this pipe name or socket path.
    Listening(String),
    /// Not serving; the text says why (logged once).
    Off(String),
}

#[derive(Debug, Default)]
struct ChannelStats {
    rejected: AtomicU64,
    busy: AtomicU64,
}

/// The running listener; dropping it (or [`AdminChannelHandle::shutdown`]) stops accepting and joins its thread.
#[derive(Resource)]
pub struct AdminChannelHandle {
    status: AdminChannelStatus,
    stop: Option<watch::Sender<bool>>,
    thread: Option<JoinHandle<()>>,
    stats: Arc<ChannelStats>,
}

impl std::fmt::Debug for AdminChannelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminChannelHandle").field("status", &self.status).field("stats", &self.stats).finish()
    }
}

impl AdminChannelHandle {
    fn off(reason: impl Into<String>) -> Self {
        Self { status: AdminChannelStatus::Off(reason.into()), stop: None, thread: None, stats: Arc::default() }
    }

    /// Whether the channel serves, and where or why not.
    pub fn status(&self) -> &AdminChannelStatus {
        &self.status
    }

    /// Whether the channel is serving.
    pub fn is_listening(&self) -> bool {
        matches!(self.status, AdminChannelStatus::Listening(_))
    }

    /// Connections refused because the peer was not this OS user (counted up to the last admitted connection).
    pub fn rejected_count(&self) -> u64 {
        self.stats.rejected.load(Ordering::Relaxed)
    }

    /// Connections refused with `busy` because every slot was in use.
    pub fn busy_count(&self) -> u64 {
        self.stats.busy.load(Ordering::Relaxed)
    }

    /// Stop accepting and drop open connections, then join the thread; in-flight jobs and open dialogs carry on in the app.
    pub fn shutdown(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                tracing::warn!("extension admin channel thread panicked");
            }
        }
        if self.is_listening() {
            self.status = AdminChannelStatus::Off("stopped".into());
        }
    }
}

impl Drop for AdminChannelHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Start the listener when a GUI runs and `FE_EXT_ADMIN_CHANNEL` is not `0`; if another app owns the endpoint this one runs without it.
pub fn spawn(ports: AdminChannelPorts) -> AdminChannelHandle {
    if !ports.gui_present {
        let reason = "no GUI on this host: consent needs the app window";
        tracing::info!("extension admin channel off: {reason}");
        return AdminChannelHandle::off(reason);
    }
    if !channel_enabled() {
        let reason = format!("{ENV_ADMIN_CHANNEL}=0");
        tracing::info!("extension admin channel off: {reason}");
        return AdminChannelHandle::off(reason);
    }
    let endpoint = match ports.endpoint.clone().map_or_else(LocalEndpoint::from_env, Ok) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            let reason = format!("{ENV_ADMIN_INSTANCE}: {error}");
            tracing::warn!("extension admin channel off: {reason}");
            return AdminChannelHandle::off(reason);
        }
    };
    start(ports, move || {
        let listener = listen_local(&endpoint)?;
        let address = endpoint.describe().unwrap_or_else(|_| "the local admin channel".into());
        Ok((listener, address))
    })
}

/// Bind on a dedicated thread (the transport needs a runtime) and serve until shutdown; `bind` is the test seam.
fn start<B>(ports: AdminChannelPorts, bind: B) -> AdminChannelHandle
where
    B: FnOnce() -> io::Result<(AdminListener, String)> + Send + 'static,
{
    let AdminChannelPorts { sink, gui_present, waits, .. } = ports;
    let config = Arc::new(ServeConfig { sink, gui_present, waits });
    let stats = Arc::new(ChannelStats::default());
    let (stop_tx, stop_rx) = watch::channel(false);
    let (bound_tx, bound_rx) = std::sync::mpsc::sync_channel::<Result<String, String>>(1);
    let thread_stats = Arc::clone(&stats);
    let spawned = std::thread::Builder::new().name("extension-admin".into()).spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = bound_tx.send(Err(format!("no runtime: {error}")));
                return;
            }
        };
        runtime.block_on(async move {
            let (listener, address) = match bind() {
                Ok(bound) => bound,
                Err(error) => {
                    let _ = bound_tx.send(Err(error.to_string()));
                    return;
                }
            };
            let _ = bound_tx.send(Ok(address));
            serve(listener, config, stop_rx, thread_stats).await;
        });
    });
    let thread = match spawned {
        Ok(thread) => thread,
        Err(error) => {
            let reason = format!("listener thread did not start: {error}");
            tracing::warn!("extension admin channel off: {reason}");
            return AdminChannelHandle::off(reason);
        }
    };
    match bound_rx.recv() {
        Ok(Ok(address)) => {
            tracing::info!(%address, "extension admin channel listening (this OS user only)");
            AdminChannelHandle {
                status: AdminChannelStatus::Listening(address),
                stop: Some(stop_tx),
                thread: Some(thread),
                stats,
            }
        }
        outcome => {
            let reason = match outcome {
                Ok(Err(reason)) => reason,
                _ => "the listener thread exited before binding".to_owned(),
            };
            let _ = thread.join();
            tracing::warn!("extension admin channel off (this app runs without it): {reason}");
            AdminChannelHandle::off(reason)
        }
    }
}

struct ServeConfig {
    sink: Arc<dyn AdminJobSink>,
    gui_present: bool,
    waits: Waits,
}

/// Accept until stopped: at most `MAX_CLIENTS` served connections, one more is answered `busy` and closed.
async fn serve(
    mut listener: AdminListener,
    config: Arc<ServeConfig>,
    mut stop: watch::Receiver<bool>,
    stats: Arc<ChannelStats>,
) {
    let slots = Arc::new(Semaphore::new(MAX_CLIENTS));
    let busy_slots = Arc::new(Semaphore::new(MAX_CLIENTS));
    loop {
        let accepted = tokio::select! {
            _ = stop.changed() => break,
            accepted = listener.accept() => accepted,
        };
        stats.rejected.store(listener.rejected_count(), Ordering::Relaxed);
        let stream = match accepted {
            Ok(stream) => stream,
            Err(error) => {
                tracing::warn!(%error, "extension admin channel: accept failed");
                tokio::select! {
                    _ = stop.changed() => break,
                    () = tokio::time::sleep(ACCEPT_RETRY) => continue,
                }
            }
        };
        match Arc::clone(&slots).try_acquire_owned() {
            Ok(permit) => {
                let config = Arc::clone(&config);
                tokio::spawn(async move {
                    serve_connection(stream, &config).await;
                    drop(permit);
                });
            }
            Err(_) => {
                stats.busy.fetch_add(1, Ordering::Relaxed);
                // Past the busy responders too, the connection is simply closed.
                if let Ok(permit) = Arc::clone(&busy_slots).try_acquire_owned() {
                    tokio::spawn(async move {
                        refuse_busy(stream).await;
                        drop(permit);
                    });
                }
            }
        }
    }
    tracing::info!("extension admin channel stopped");
}

/// Read the request (briefly) so the `busy` refusal can echo its id, answer, close.
async fn refuse_busy<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S) {
    let id = match tokio::time::timeout(BUSY_READ_TIMEOUT, next_request(&mut stream)).await {
        Ok(Incoming::Request(request)) => request.id,
        Ok(Incoming::Refuse(refusal)) => {
            send(&mut stream, refusal).await;
            return;
        }
        _ => String::new(),
    };
    let message = format!("the app already serves {MAX_CLIENTS} admin connections; try again shortly");
    send(&mut stream, AdminReply::refused(id, RefusalCode::Busy, message)).await;
}

/// One connection: requests one at a time until the client closes, idles past the read wait, or breaks the framing.
async fn serve_connection<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, config: &ServeConfig) {
    loop {
        let Ok(incoming) = tokio::time::timeout(config.waits.request_read, next_request(&mut stream)).await else {
            tracing::debug!("extension admin channel: closed an idle connection");
            return;
        };
        match incoming {
            Incoming::Closed => return,
            Incoming::Refuse(refusal) => {
                send(&mut stream, refusal).await;
                return;
            }
            Incoming::Request(request) => {
                if !answer(&mut stream, request, config).await {
                    return;
                }
            }
        }
    }
}

/// What ended a wait for the worker.
enum Wait {
    Answer(AdminReply),
    WorkerGone,
    Deadline,
    ClientGone,
    ClientSpoke,
}

/// Relay one request to the worker and its answers back; `false` when the connection must close.
async fn answer<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, request: AdminRequest, config: &ServeConfig) -> bool {
    let id = request.id.clone();
    let verb = request.verb.name();
    let may_raise_consent = request.verb.may_raise_consent();
    if may_raise_consent && !config.gui_present {
        let reply = AdminReply::refused(id, RefusalCode::ConsentNeedsGui, "this host has no window to show the consent dialog");
        return send(stream, reply).await;
    }
    let (replies, mut answers) = mpsc::unbounded_channel();
    if let Err(message) = config.sink.submit(request, replies) {
        return send(stream, AdminReply::refused(id, RefusalCode::Internal, message)).await;
    }
    let mut pending_sent = false;
    let mut deadline = Instant::now() + config.waits.worker_answer;
    loop {
        match wait_for_answer(stream, &mut answers, deadline).await {
            Wait::Answer(reply) => {
                // The client matches replies by id; the worker's copy of it is not trusted.
                let reply = AdminReply { id: id.clone(), ..reply };
                if reply.is_final() {
                    return send(stream, reply).await;
                }
                if pending_sent || !may_raise_consent {
                    tracing::warn!(verb = verb, "extension admin channel: dropped an unexpected pending-consent from the ops worker");
                    continue;
                }
                pending_sent = true;
                deadline = Instant::now() + config.waits.consent;
                if !send(stream, reply).await {
                    return false;
                }
            }
            Wait::WorkerGone => {
                let reply = AdminReply::refused(id, RefusalCode::Internal, "the extension host stopped before answering");
                return send(stream, reply).await;
            }
            Wait::Deadline if pending_sent => {
                let message = format!(
                    "no answer in the app within {}; the dialog stays open",
                    describe_wait(config.waits.consent)
                );
                return send(stream, AdminReply::refused(id, RefusalCode::ConsentTimeout, message)).await;
            }
            Wait::Deadline => {
                let message =
                    format!("the extension host did not answer within {}", describe_wait(config.waits.worker_answer));
                return send(stream, AdminReply::refused(id, RefusalCode::Internal, message)).await;
            }
            // Nothing is cancelled: the job and any dialog it opened belong to the app now.
            Wait::ClientGone => return false,
            Wait::ClientSpoke => {
                tracing::warn!(verb = verb, "extension admin channel: client sent before the final reply (no pipelining); closing");
                return false;
            }
        }
    }
}

/// The worker's next answer, or why none is coming; a byte from the client mid-request is a protocol violation.
async fn wait_for_answer<S: AsyncRead + Unpin>(
    stream: &mut S,
    answers: &mut mpsc::UnboundedReceiver<AdminReply>,
    deadline: Instant,
) -> Wait {
    let mut probe = [0u8; 1];
    tokio::select! {
        biased;
        answer = answers.recv() => answer.map_or(Wait::WorkerGone, Wait::Answer),
        read = stream.read(&mut probe) => match read {
            Ok(0) | Err(_) => Wait::ClientGone,
            Ok(_) => Wait::ClientSpoke,
        },
        () = tokio::time::sleep_until(deadline) => Wait::Deadline,
    }
}

/// Write one reply frame; `false` when the client is gone.
async fn send<S: AsyncWrite + Unpin>(stream: &mut S, reply: AdminReply) -> bool {
    match write_reply(stream, &reply).await {
        Ok(()) => true,
        Err(error) => {
            tracing::debug!(%error, "extension admin channel: reply not delivered");
            false
        }
    }
}

fn describe_wait(wait: Duration) -> String {
    let seconds = wait.as_secs();
    if seconds >= 60 {
        format!("{} minutes", seconds / 60)
    } else {
        format!("{} seconds", seconds.max(1))
    }
}

#[cfg(test)]
mod tests {
    //! Batch R11-ADMIN (`cargo test -p fractalengine -- extension_host::admin_channel::tests::`).

    use super::*;
    use fe_plugin::admin::protocol::{
        decode_reply, encode_request, read_frame, write_frame, AdminResult, AdminVerb, InstallationSummary,
        PendingConsent, ReplyOutcome, MAX_FRAME_BYTES,
    };
    use fe_plugin::admin::transport::{exchange, memory_listener, AdminStream, ExchangeError, PeerUser};
    use tokio::io::AsyncWriteExt;

    type Job = (AdminRequest, mpsc::UnboundedSender<AdminReply>);
    type Asked = (AdminStream, Result<AdminReply, ExchangeError>, Vec<PendingConsent>);

    /// Stands in for the ops worker: the test answers each job by hand.
    struct ScriptedWorker(mpsc::UnboundedSender<Job>);

    impl AdminJobSink for ScriptedWorker {
        fn submit(&self, request: AdminRequest, replies: mpsc::UnboundedSender<AdminReply>) -> Result<(), String> {
            self.0.send((request, replies)).map_err(|_| "the extension ops worker is not running".to_owned())
        }
    }

    fn listener_on_memory(gui_present: bool, waits: Waits) -> (AdminChannelHandle, fe_plugin::admin::transport::MemoryConnector, mpsc::UnboundedReceiver<Job>) {
        let (jobs_tx, jobs) = mpsc::unbounded_channel();
        let (listener, connector) = memory_listener();
        let ports = AdminChannelPorts::with_sink(Arc::new(ScriptedWorker(jobs_tx)), gui_present).with_waits(waits);
        let handle = start(ports, move || Ok((listener, "memory".to_owned())));
        (handle, connector, jobs)
    }

    fn ask(mut stream: AdminStream, request: AdminRequest) -> tokio::task::JoinHandle<Asked> {
        tokio::spawn(async move {
            let mut seen = Vec::new();
            let reply = exchange(&mut stream, &request, |consent| seen.push(consent.clone())).await;
            (stream, reply, seen)
        })
    }

    fn refusal_code(reply: &AdminReply) -> &str {
        match &reply.outcome {
            ReplyOutcome::Refused { code, .. } => code.as_str(),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn pending(is_update: bool) -> PendingConsent {
        PendingConsent {
            installation_id: "01JINST".into(),
            package_id: "org.example.fleet-sim".into(),
            name: "Fleet sim".into(),
            version: "1.2.0".into(),
            capability_digest: "b71e".into(),
            is_update,
        }
    }

    fn fleet_sim() -> AdminResult {
        AdminResult {
            installations: vec![InstallationSummary {
                installation_id: "01JINST".into(),
                package_id: "org.example.fleet-sim".into(),
                name: "Fleet sim".into(),
                version: "1.2.0".into(),
                status: "Running".into(),
                ..Default::default()
            }],
            message: None,
        }
    }

    fn package_path() -> String {
        std::env::temp_dir().join("fleet-sim.hexon").display().to_string()
    }

    /// A CLI session against the app's listener: foreign peers never reach the worker, consent is only ever relayed
    /// from the GUI (approve, timeout, a leaving CLI), the worker's stray frames are filtered, a headless host refuses
    /// consent verbs, the fifth connection is `busy`, a bad frame closes, and shutdown stops the channel.
    #[tokio::test]
    async fn admin_listener_journey_admission_consent_relay_busy_and_shutdown() {
        let headless = spawn(AdminChannelPorts::with_sink(Arc::new(ScriptedWorker(mpsc::unbounded_channel().0)), false));
        assert!(!headless.is_listening(), "a headless host never opens the channel");

        let waits = Waits {
            request_read: Duration::from_secs(30),
            worker_answer: Duration::from_secs(30),
            consent: Duration::from_secs(2),
        };
        let (mut handle, connector, mut jobs) = listener_on_memory(true, waits);
        assert!(handle.is_listening());

        // Another OS user is dropped before any request is read; this user is admitted.
        let mut foreign = connector.connect(PeerUser::Other("S-1-5-21-other".into())).unwrap();
        let own = connector.connect(PeerUser::Same).unwrap();
        let mut probe = [0u8; 1];
        assert_eq!(foreign.read(&mut probe).await.unwrap(), 0, "the foreign client sees the channel close");

        // list: one job, relayed answer.
        let task = ask(own, AdminRequest::new("c1", AdminVerb::List {}));
        let (request, replies) = jobs.recv().await.unwrap();
        assert_eq!(request.verb, AdminVerb::List {});
        replies.send(AdminReply::ok(&request.id, fleet_sim())).unwrap();
        let (own, reply, seen) = task.await.unwrap();
        assert_eq!(reply.unwrap().outcome, ReplyOutcome::Ok { result: fleet_sim() });
        assert!(seen.is_empty());
        assert_eq!(handle.rejected_count(), 1, "the foreign connection was counted");

        // install-file: the dialog opens once (a duplicate notice is filtered), the operator approves in the GUI.
        let task = ask(own, AdminRequest::new("c2", AdminVerb::InstallFile { path: package_path() }));
        let (request, replies) = jobs.recv().await.unwrap();
        replies.send(AdminReply::pending_consent(&request.id, pending(false))).unwrap();
        replies.send(AdminReply::pending_consent(&request.id, pending(false))).unwrap();
        replies.send(AdminReply::ok(&request.id, fleet_sim())).unwrap();
        let (own, reply, seen) = task.await.unwrap();
        assert_eq!(seen, vec![pending(false)], "the CLI is pointed at the GUI dialog exactly once");
        assert!(matches!(reply.unwrap().outcome, ReplyOutcome::Ok { .. }));

        // update: nobody answers the dialog; the CLI gets consent-timeout, the worker's late answer goes nowhere.
        let task = ask(own, AdminRequest::new("c3", AdminVerb::Update { installation_id: "01JINST".into() }));
        let (request, replies) = jobs.recv().await.unwrap();
        replies.send(AdminReply::pending_consent(&request.id, pending(true))).unwrap();
        let (own, reply, seen) = task.await.unwrap();
        assert_eq!(seen, vec![pending(true)]);
        assert_eq!(refusal_code(&reply.unwrap()), "consent-timeout");
        tokio::time::timeout(Duration::from_secs(2), replies.closed()).await.expect("the listener let go of the job");

        // enable never raises consent: a stray notice is dropped and the final reply carries the request's id.
        let task = ask(own, AdminRequest::new("c4", AdminVerb::Enable { installation_id: "01JINST".into(), binding_id: None }));
        let (_, replies) = jobs.recv().await.unwrap();
        replies.send(AdminReply::pending_consent("c4", pending(false))).unwrap();
        replies.send(AdminReply::ok("not-c4", fleet_sim())).unwrap();
        let (mut own, reply, seen) = task.await.unwrap();
        assert!(seen.is_empty());
        assert_eq!(reply.unwrap().id, "c4");

        // A CLI that leaves while the dialog is open cancels nothing; the listener just stops waiting.
        let mut leaving = connector.connect(PeerUser::Same).unwrap();
        let install = AdminRequest::new(
            "c5",
            AdminVerb::InstallRegistry { source_id: "default".into(), package_id: "org.example.fleet-sim".into(), version: None },
        );
        write_frame(&mut leaving, &encode_request(&install).unwrap()).await.unwrap();
        let (_, replies) = jobs.recv().await.unwrap();
        replies.send(AdminReply::pending_consent("c5", pending(false))).unwrap();
        let notice = decode_reply(&read_frame(&mut leaving).await.unwrap().unwrap()).unwrap();
        assert!(!notice.is_final());
        drop(leaving);
        tokio::time::timeout(Duration::from_secs(1), replies.closed()).await.expect("the slot is freed when the CLI leaves");

        // A headless listener refuses consent verbs without queueing them, closes idle connections and reports a dead worker.
        let short = Waits { request_read: Duration::from_millis(200), ..waits };
        let (mut headless, headless_connector, headless_jobs) = listener_on_memory(false, short);
        let mut cli = headless_connector.connect(PeerUser::Same).unwrap();
        let install = AdminRequest::new("h1", AdminVerb::InstallFile { path: package_path() });
        let reply = exchange(&mut cli, &install, |_| panic!("no dialog on a headless host")).await.unwrap();
        assert_eq!(refusal_code(&reply), "consent-needs-gui");
        drop(headless_jobs);
        let reply = exchange(&mut cli, &AdminRequest::new("h2", AdminVerb::List {}), |_| {}).await.unwrap();
        assert_eq!(refusal_code(&reply), "internal");
        let mut idle = headless_connector.connect(PeerUser::Same).unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(2), idle.read(&mut probe)).await.expect("idle connection closed");
        assert_eq!(closed.unwrap(), 0);
        headless.shutdown();

        // Four served connections (own + three idle); the fifth is refused `busy` with its id, never queued.
        let idle: Vec<AdminStream> = (0..3).map(|_| connector.connect(PeerUser::Same).unwrap()).collect();
        let mut fifth = connector.connect(PeerUser::Same).unwrap();
        let reply = exchange(&mut fifth, &AdminRequest::new("c6", AdminVerb::List {}), |_| {}).await.unwrap();
        assert_eq!((reply.id.as_str(), refusal_code(&reply)), ("c6", "busy"));
        assert_eq!(handle.busy_count(), 1);
        assert!(jobs.try_recv().is_err(), "a busy request never reaches the worker");
        drop(idle);

        // An oversize frame is answered once, then the connection closes.
        own.write_all(&((MAX_FRAME_BYTES as u32) + 1).to_be_bytes()).await.unwrap();
        let refusal = decode_reply(&read_frame(&mut own).await.unwrap().unwrap()).unwrap();
        assert_eq!(refusal_code(&refusal), "frame-too-large");
        assert!(read_frame(&mut own).await.unwrap().is_none(), "the app closed the connection");

        handle.shutdown();
        assert!(!handle.is_listening());
        assert!(connector.connect(PeerUser::Same).is_err(), "nothing accepts after shutdown");
    }
}
