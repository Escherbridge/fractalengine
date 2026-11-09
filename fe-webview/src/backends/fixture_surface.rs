//! Fixed fixture DOM and native PNG jobs; see src/AGENTS.md section QA Portal evidence.

use crate::portal_bundle::mount::PortalMounts;
use crate::portal_bundle::origin;
use fe_runtime::qa_debug::QaFailure;
use fe_runtime::qa_portal::{
    self as fixture, FixtureCommand, FixtureObservation, FixtureOp, FixtureOutcome, FixtureReceipt,
    FixtureResult, MAX_DIMENSION, MAX_PNG_BYTES,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
use webview2_com::{CapturePreviewCompletedHandler, ExecuteScriptCompletedHandler};
use windows::core::HSTRING;
use wry::WebViewExtWindows;

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
#[derive(Default)]
pub(super) struct NavigationClock {
    generation: Cell<u64>,
    admitted_destination: RefCell<Option<String>>,
    refusals: Cell<u64>,
    last_refusal: RefCell<Option<String>>,
}
impl NavigationClock {
    pub(super) fn bump(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.admitted_destination.borrow_mut().take();
    }
    pub(super) fn admit(&self, raw: String) {
        self.bump();
        *self.admitted_destination.borrow_mut() = Some(raw);
    }
    pub(super) fn page_state(
        &self,
        initial: &str,
        captured: u64,
        current: &str,
        allow_second: bool,
    ) -> Option<PageState> {
        let destination = self.admitted_destination.borrow();
        page_state(
            initial,
            captured,
            current,
            self.generation.get(),
            if allow_second {
                destination.as_deref()
            } else {
                None
            },
        )
    }
    pub(super) fn refuse(&self, raw: String) {
        self.refusals.set(self.refusals.get().wrapping_add(1));
        *self.last_refusal.borrow_mut() = Some(raw);
    }
}

enum Completion {
    Dom(Result<String, ()>),
    Portal(Result<String, ()>),
    Png(Result<Vec<u8>, ()>),
}
#[derive(Clone, Copy)]
enum Phase {
    Initial,
    WaitingSecond,
    SecondDocument,
    WaitingThird,
    Capturing,
}
struct Pending {
    command: FixtureCommand,
    initial_url: String,
    generation: u64,
    refusals: u64,
    phase: Phase,
    observation: Option<FixtureObservation>,
    completion: Rc<RefCell<Option<Completion>>>,
    dimensions: Option<(u32, u32)>,
}

/// Host-side source of the IPC outcome counters (ipc-v1.md section 7.3); installed by the backend.
pub(super) type IpcStatsSource = Arc<dyn Fn() -> serde_json::Value + Send + Sync>;

pub(super) struct Surface {
    pub(super) clock: Rc<NavigationClock>,
    session: u64,
    pending: Option<Pending>,
    portal: Option<PortalPending>,
    mounts: Option<PortalMounts>,
    ipc_stats: Option<IpcStatsSource>,
}

impl Default for Surface {
    fn default() -> Self {
        Self {
            clock: Rc::default(),
            session: NEXT_SESSION.fetch_add(1, Ordering::Relaxed),
            pending: None,
            portal: None,
            mounts: None,
            ipc_stats: None,
        }
    }
}

impl Surface {
    pub(super) fn receipt_is_current(
        &self,
        webview: &wry::WebView,
        visible: bool,
        unusable: bool,
        receipt: &FixtureReceipt,
    ) -> bool {
        if receipt.popup_session != self.session
            || receipt.page_generation != self.clock.generation.get()
        {
            return false;
        }
        if receipt.portal.is_some() {
            let Some(mounts) = &self.mounts else {
                return false;
            };
            return bundle_source(webview, visible, unusable, mounts)
                .is_ok_and(|(_, raw)| raw == receipt.url)
                && receipt.page_generation == self.clock.generation.get();
        }
        let valid = live_source(webview, visible, unusable).is_ok_and(|raw| raw == receipt.url)
            && receipt
                .width
                .zip(receipt.height)
                .is_none_or(|expected| dimensions(webview).ok() == Some(expected));
        valid && receipt.page_generation == self.clock.generation.get()
    }
    pub(super) fn cookie(&self) -> isize {
        self.session as isize
    }
    pub(super) fn start(
        &mut self,
        webview: &wry::WebView,
        visible: bool,
        unusable: bool,
        command: &FixtureCommand,
    ) -> Result<(), QaFailure> {
        if self.pending.is_some() || self.portal.is_some() {
            return Err(fixture::failure(
                "portal_fixture_busy",
                "A native fixture operation is pending",
            ));
        }
        if let Some(kind) = PortalKind::of(command.operation) {
            return self.start_portal(webview, visible, unusable, command, kind);
        }
        let raw = live_source(webview, visible, unusable)?;
        if matches!(command.operation, FixtureOp::ActivateSecondLink) && !raw.ends_with("/web/one") {
            return Err(fixture::failure(
                "portal_fixture_wrong_page",
                "Second-link verification requires fixture page one",
            ));
        }
        if !command.is_current() {
            return Err(fixture::stale());
        }
        let completion = Rc::new(RefCell::new(None));
        let generation = self.clock.generation.get();
        let refusals = self.clock.refusals.get();
        execute(webview, &raw, command.operation, completion.clone())?;
        self.pending = Some(Pending {
            command: command.clone(),
            initial_url: raw,
            generation,
            refusals,
            phase: Phase::Initial,
            observation: None,
            completion,
            dimensions: None,
        });
        Ok(())
    }

    pub(super) fn cancel(&mut self) -> bool {
        self.clock.bump();
        let portal = self.portal.take().is_some();
        self.pending.take().is_some() || portal
    }

    /// Wire the live mount table and the IPC counter source the bundle operations read.
    pub(super) fn bind_portal(&mut self, mounts: PortalMounts, ipc_stats: IpcStatsSource) {
        self.mounts = Some(mounts);
        self.ipc_stats = Some(ipc_stats);
    }

    fn start_portal(
        &mut self,
        webview: &wry::WebView,
        visible: bool,
        unusable: bool,
        command: &FixtureCommand,
        kind: PortalKind,
    ) -> Result<(), QaFailure> {
        let mounts = self.mounts.as_ref().ok_or_else(portal_not_mounted)?;
        let (mount, _raw) = bundle_source(webview, visible, unusable, mounts)?;
        if !command.is_current() {
            return Err(fixture::stale());
        }
        let origin = mount.label.https_origin();
        let completion = Rc::new(RefCell::new(None));
        if !matches!(kind, PortalKind::IpcStats) {
            execute_portal(webview, &origin, kind, completion.clone())?;
        }
        self.portal = Some(PortalPending {
            command: command.clone(),
            mount_id: mount.mount_id,
            origin,
            kind,
            generation: self.clock.generation.get(),
            completion,
        });
        Ok(())
    }

    fn poll_portal(
        &mut self,
        webview: &wry::WebView,
        visible: bool,
        unusable: bool,
    ) -> Option<FixtureOutcome> {
        let job = self.portal.take()?;
        if !job.command.is_current() || self.clock.generation.get() != job.generation {
            return Some(Err(fixture::stale()));
        }
        let Some(mounts) = &self.mounts else {
            return Some(Err(portal_not_mounted()));
        };
        let (mount, raw) = match bundle_source(webview, visible, unusable, mounts) {
            Ok(found) => found,
            Err(error) => return Some(Err(error)),
        };
        if mount.mount_id != job.mount_id {
            return Some(Err(fixture::stale()));
        }
        let mode = match mount.mode {
            crate::portal_bundle::mount::MountMode::Admitted => "admitted",
            crate::portal_bundle::mount::MountMode::Static => "static",
        };
        let label = mount.label.as_str().to_owned();
        let evidence = match job.kind {
            PortalKind::IpcStats => {
                let Some(source) = &self.ipc_stats else {
                    return Some(Err(portal_not_mounted()));
                };
                serde_json::json!({"label": label, "mode": mode, "stats": source()})
            }
            PortalKind::Observe | PortalKind::Slot(_) => {
                let completion = job.completion.borrow_mut().take();
                let Some(Completion::Portal(done)) = completion else {
                    self.portal = Some(job);
                    return None;
                };
                let Ok(json) = done else {
                    return Some(Err(fixture::failure(
                        "portal_native_operation_failed",
                        "Native fixture operation failed",
                    )));
                };
                if json == "\"fixture_document_pending\"" {
                    let again = Rc::new(RefCell::new(None));
                    if let Err(error) = execute_portal(webview, &job.origin, job.kind, again.clone())
                    {
                        return Some(Err(error));
                    }
                    self.portal = Some(PortalPending {
                        completion: again,
                        ..job
                    });
                    return None;
                }
                match portal_evidence(&json, &job, &label, mode) {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                }
            }
        };
        Some(Ok(self.portal_result(&job, raw, evidence)))
    }

    fn portal_result(
        &self,
        job: &PortalPending,
        raw: String,
        evidence: serde_json::Value,
    ) -> FixtureResult {
        let title = evidence["title"].as_str().unwrap_or_default().to_owned();
        let activated = evidence["activated"].as_bool().unwrap_or(false);
        FixtureResult {
            receipt: FixtureReceipt {
                scope: String::new(),
                operation: job.command.operation,
                popup_session: self.session,
                page_generation: job.generation,
                url: raw,
                // The legacy observation is a required field; bundle evidence rides in `portal`.
                observation: FixtureObservation {
                    page: "bundle".into(),
                    heading: title.clone(),
                    title,
                    second_link: String::new(),
                    third_link: String::new(),
                    ready: true,
                    invoked: activated,
                },
                link_invoked: false,
                native_refusal_observed: false,
                capture_source: None,
                width: None,
                height: None,
                sha256: None,
                portal: Some(evidence),
            },
            png: None,
        }
    }

    pub(super) fn poll(
        &mut self,
        webview: &wry::WebView,
        visible: bool,
        unusable: bool,
    ) -> Option<FixtureOutcome> {
        if self.portal.is_some() {
            return self.poll_portal(webview, visible, unusable);
        }
        let mut job = self.pending.take()?;
        if !visible || unusable || !job.command.is_current() {
            return Some(Err(fixture::stale()));
        }
        let raw = match source(webview) {
            Ok(raw) if fe_runtime::qa_web::preview_allows(&raw) => raw,
            _ => return Some(Err(fixture::stale())),
        };
        let generation = self.clock.generation.get();
        let is_second = matches!(job.command.operation, FixtureOp::ActivateSecondLink);
        let Some((origin, _)) = job.initial_url.rsplit_once("/web/") else {
            return Some(Err(fixture::stale()));
        };
        let expected_third = format!("{origin}/web/three");
        if is_second && self.clock.refusals.get() != job.refusals {
            return Some(Err(fixture::stale()));
        }
        let page = self
            .clock
            .page_state(&job.initial_url, job.generation, &raw, is_second);
        let same = page == Some(PageState::Original);
        let second = page == Some(PageState::SecondDocument);
        if page.is_none() || (matches!(job.phase, Phase::SecondDocument) && !second) {
            return Some(Err(fixture::stale()));
        }
        match job.phase {
            Phase::WaitingSecond if second => {
                if live_source(webview, visible, unusable).is_err() {
                    return Some(Err(fixture::stale()));
                }
                if let Err(error) =
                    execute(webview, &raw, FixtureOp::Observe, job.completion.clone())
                {
                    return Some(Err(error));
                }
                job.phase = Phase::SecondDocument;
            }
            Phase::WaitingThird
                if self.clock.refusals.get() == job.refusals.wrapping_add(1)
                    && self.clock.last_refusal.borrow().as_deref()
                        == Some(expected_third.as_str())
                    && same =>
            {
                if live_source(webview, visible, unusable).is_err() {
                    return Some(Err(fixture::stale()));
                }
                let Some(observation) = job.observation.take() else {
                    return Some(Err(document_error()));
                };
                return Some(Ok(self.result(
                    &job,
                    raw,
                    generation,
                    observation,
                    None,
                    true,
                )));
            }
            _ => {}
        }
        let completion = job.completion.borrow_mut().take();
        if let Some(completion) = completion {
            if live_source(webview, visible, unusable).is_err() || !job.command.is_current() {
                return Some(Err(fixture::stale()));
            }
            match completion {
                Completion::Dom(Ok(json)) => {
                    if json == "\"fixture_document_pending\"" {
                        if matches!(job.phase, Phase::SecondDocument) {
                            job.phase = Phase::WaitingSecond;
                            self.pending = Some(job);
                            return None;
                        }
                        return Some(Err(fixture::failure(
                            "portal_fixture_not_ready",
                            "Native fixture document is still loading",
                        )));
                    }
                    let observation: FixtureObservation = match serde_json::from_str(&json) {
                        Ok(value) => value,
                        Err(_) => return Some(Err(document_error())),
                    };
                    let expected = if matches!(job.phase, Phase::SecondDocument) {
                        raw.as_str()
                    } else {
                        job.initial_url.as_str()
                    };
                    if matches!(job.phase, Phase::SecondDocument) && !observation.ready {
                        job.phase = Phase::WaitingSecond;
                    } else if let Err(error) = observation.validate(expected) {
                        return Some(Err(error));
                    } else if matches!(job.phase, Phase::SecondDocument) {
                        return Some(Ok(self.result(
                            &job,
                            raw,
                            generation,
                            observation,
                            None,
                            false,
                        )));
                    } else {
                        match job.command.operation {
                            FixtureOp::Observe if same && !observation.invoked => {
                                return Some(Ok(self.result(
                                    &job,
                                    raw,
                                    generation,
                                    observation,
                                    None,
                                    false,
                                )))
                            }
                            FixtureOp::Capture if same && !observation.invoked => {
                                let dimensions = match dimensions(webview) {
                                    Ok(value) => value,
                                    Err(error) => return Some(Err(error)),
                                };
                                if let Err(error) = capture(webview, &raw, job.completion.clone()) {
                                    return Some(Err(error));
                                }
                                job.dimensions = Some(dimensions);
                                job.observation = Some(observation);
                                job.phase = Phase::Capturing;
                            }
                            FixtureOp::ActivateSecondLink if observation.invoked => {
                                job.phase = Phase::WaitingSecond
                            }
                            FixtureOp::ActivateRefusedThirdLink if observation.invoked && same => {
                                job.observation = Some(observation);
                                job.phase = Phase::WaitingThird;
                            }
                            _ => return Some(Err(document_error())),
                        }
                    }
                }
                Completion::Png(Ok(bytes)) if matches!(job.phase, Phase::Capturing) && same => {
                    let actual = match png_dimensions(&bytes) {
                        Some(value) => value,
                        None => {
                            return Some(Err(fixture::failure(
                                "portal_capture_invalid",
                                "Native capture is not a bounded PNG",
                            )))
                        }
                    };
                    if job.dimensions != Some(actual) || dimensions(webview).ok() != Some(actual) {
                        return Some(Err(fixture::stale()));
                    }
                    let Some(observation) = job.observation.take() else {
                        return Some(Err(document_error()));
                    };
                    return Some(Ok(self.result(
                        &job,
                        raw,
                        generation,
                        observation,
                        Some((bytes, actual)),
                        false,
                    )));
                }
                _ => {
                    return Some(Err(fixture::failure(
                        "portal_native_operation_failed",
                        "Native fixture operation failed",
                    )))
                }
            }
        }
        self.pending = Some(job);
        None
    }

    fn result(
        &self,
        job: &Pending,
        raw: String,
        generation: u64,
        observation: FixtureObservation,
        png: Option<(Vec<u8>, (u32, u32))>,
        refused: bool,
    ) -> FixtureResult {
        let (bytes, width, height, hash) = match png {
            Some((bytes, (width, height))) => {
                let hash = format!("{:x}", Sha256::digest(&bytes));
                (Some(bytes), Some(width), Some(height), Some(hash))
            }
            None => (None, None, None, None),
        };
        FixtureResult {
            receipt: FixtureReceipt {
                scope: String::new(),
                operation: job.command.operation,
                popup_session: self.session,
                page_generation: generation,
                url: raw,
                observation,
                link_invoked: matches!(
                    job.command.operation,
                    FixtureOp::ActivateSecondLink | FixtureOp::ActivateRefusedThirdLink
                ),
                native_refusal_observed: refused,
                capture_source: bytes.as_ref().map(|_| "owned_webview2_capture_preview"),
                width,
                height,
                sha256: hash,
                portal: None,
            },
            png: bytes,
        }
    }
}

fn source(webview: &wry::WebView) -> Result<String, QaFailure> {
    let mut uri = Default::default();
    unsafe {
        webview
            .webview()
            .Source(&mut uri)
            .map_err(|_| fixture::unavailable())?;
    }
    let raw = webview2_com::take_pwstr(uri);
    if raw.len() > 512 {
        return Err(fixture::unavailable());
    }
    Ok(raw)
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PageState {
    Original,
    StartingSecond,
    SecondDocument,
}

pub(super) fn page_state(
    initial: &str,
    captured: u64,
    current: &str,
    generation: u64,
    admitted_destination: Option<&str>,
) -> Option<PageState> {
    if captured == generation && current == initial {
        return Some(PageState::Original);
    }
    let origin = initial.strip_suffix("/web/one")?;
    let expected = format!("{origin}/web/two");
    if generation != captured.wrapping_add(1) || admitted_destination != Some(expected.as_str()) {
        return None;
    }
    if current == initial {
        Some(PageState::StartingSecond)
    } else if current == expected {
        Some(PageState::SecondDocument)
    } else {
        None
    }
}

fn live_source(webview: &wry::WebView, visible: bool, unusable: bool) -> Result<String, QaFailure> {
    if !visible || unusable {
        return Err(fixture::unavailable());
    }
    let raw = source(webview)?;
    if !fe_runtime::qa_web::allows(&raw) {
        return Err(fixture::unavailable());
    }
    Ok(raw)
}

fn dimensions(webview: &wry::WebView) -> Result<(u32, u32), QaFailure> {
    let mut bounds = Default::default();
    unsafe {
        webview
            .controller()
            .Bounds(&mut bounds)
            .map_err(|_| fixture::unavailable())?;
    }
    let width = bounds
        .right
        .checked_sub(bounds.left)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(fixture::unavailable)?;
    let height = bounds
        .bottom
        .checked_sub(bounds.top)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(fixture::unavailable)?;
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(fixture::failure(
            "portal_capture_dimensions",
            "Native viewport must be within 1..2048 physical pixels on each axis",
        ));
    }
    Ok((width, height))
}

pub(super) fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 33
        || bytes.len() > MAX_PNG_BYTES
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || &bytes[8..16] != b"\0\0\0\rIHDR"
    {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return None;
    }
    let mut offset = 8usize;
    let mut image_data = false;
    while offset.checked_add(12)? <= bytes.len() {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().ok()?) as usize;
        let end = offset.checked_add(12)?.checked_add(length)?;
        if end > bytes.len() {
            return None;
        }
        let kind = &bytes[offset + 4..offset + 8];
        if kind == b"IDAT" {
            image_data = true;
        }
        if kind == b"IEND" {
            return (length == 0 && image_data && end == bytes.len()).then_some((width, height));
        }
        offset = end;
    }
    None
}

fn execute(
    webview: &wry::WebView,
    expected: &str,
    operation: FixtureOp,
    completion: Rc<RefCell<Option<Completion>>>,
) -> Result<(), QaFailure> {
    if source(webview)? != expected || !fe_runtime::qa_web::allows(expected) {
        return Err(fixture::stale());
    }
    let expected = serde_json::to_string(expected).map_err(|_| fixture::stale())?;
    let action = match operation {
        FixtureOp::ActivateSecondLink => "o.invoked=true; links[0].click();",
        FixtureOp::ActivateRefusedThirdLink => "o.invoked=true; links[1].click();",
        _ => "",
    };
    let script = format!(
        r#"(()=>{{
        const expected={expected}; if(location.href!==expected) return null;
        if(document.readyState!=='complete') return 'fixture_document_pending';
        const page=location.pathname.slice(5), origin=location.origin;
        if(!['one','two'].includes(page)||location.href!==origin+'/web/'+page) return null;
        const title='QA Portal '+page, headings=document.querySelectorAll('h1'), links=document.querySelectorAll('a');
        if(document.title!==title||headings.length!==1||headings[0].textContent!==title||links.length!==2||links[0].getAttribute('href')!=='/web/two'||links[1].getAttribute('href')!=='/web/three'||links[0].href!==origin+'/web/two'||links[1].href!==origin+'/web/three') return null;
        const o={{page,title:document.title,heading:headings[0].textContent,second_link:'/web/two',third_link:'/web/three',ready:document.readyState==='complete',invoked:false}};
        {action} return o;
    }})()"#
    );
    unsafe {
        webview
            .webview()
            .ExecuteScript(
                &HSTRING::from(script),
                &ExecuteScriptCompletedHandler::create(Box::new(move |error, json| {
                    *completion.borrow_mut() =
                        Some(Completion::Dom(if error.is_ok() && json.len() <= 4096 {
                            Ok(json)
                        } else {
                            Err(())
                        }));
                    Ok(())
                })),
            )
            .map_err(|_| {
                fixture::failure(
                    "portal_script_unavailable",
                    "Fixed native fixture script is unavailable",
                )
            })
    }
}

fn capture(
    webview: &wry::WebView,
    expected: &str,
    completion: Rc<RefCell<Option<Completion>>>,
) -> Result<(), QaFailure> {
    if source(webview)? != expected || !fe_runtime::qa_web::allows(expected) {
        return Err(fixture::stale());
    }
    let (stream, buffer) = super::fixture_stream::create();
    let retained = stream.clone();
    unsafe {
        webview
            .webview()
            .CapturePreview(
                COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                &stream,
                &CapturePreviewCompletedHandler::create(Box::new(move |error| {
                    let _retained = &retained;
                    let bytes = if error.is_ok() {
                        buffer
                            .lock()
                            .map(|mut state| std::mem::take(&mut state.bytes))
                            .map_err(|_| ())
                    } else {
                        Err(())
                    };
                    *completion.borrow_mut() = Some(Completion::Png(bytes));
                    Ok(())
                })),
            )
            .map_err(|_| {
                fixture::failure(
                    "portal_capture_unavailable",
                    "Native CapturePreview is unavailable",
                )
            })
    }
}

fn document_error() -> QaFailure {
    fixture::failure(
        "portal_fixture_document_mismatch",
        "Native document does not match the fixed fixture",
    )
}

/// Which fixed bundle operation a command is.
#[derive(Clone, Copy)]
enum PortalKind {
    Observe,
    Slot(u8),
    IpcStats,
}
impl PortalKind {
    fn of(operation: FixtureOp) -> Option<Self> {
        match operation {
            FixtureOp::PortalObserve => Some(Self::Observe),
            FixtureOp::PortalSlot(slot) => Some(Self::Slot(slot)),
            FixtureOp::PortalIpcStats => Some(Self::IpcStats),
            _ => None,
        }
    }
}

struct PortalPending {
    command: FixtureCommand,
    mount_id: u64,
    origin: String,
    kind: PortalKind,
    generation: u64,
    completion: Rc<RefCell<Option<Completion>>>,
}

const PORTAL_SLOT_MAX: u8 = 32;
const PORTAL_PROBE_MAX: usize = 64;
const PORTAL_TEXT_MAX: usize = 256;
const PORTAL_DIGEST_INPUT_MAX: usize = 64 * 1024;
const PORTAL_REPLY_MAX: usize = 96 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservedPage {
    origin: String,
    title: String,
    ready: bool,
    has_api: bool,
    probes: Vec<(String, String)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SlotReply {
    origin: String,
    activated: bool,
}

fn portal_not_mounted() -> QaFailure {
    fixture::failure(
        "portal_bundle_not_mounted",
        "A live mounted bundle is required",
    )
}

/// The live mount plus the webview's current source, only when that source is the mount's own origin.
fn bundle_source(
    webview: &wry::WebView,
    visible: bool,
    unusable: bool,
    mounts: &PortalMounts,
) -> Result<(Arc<crate::portal_bundle::mount::Mount>, String), QaFailure> {
    if !visible || unusable {
        return Err(fixture::unavailable());
    }
    let mount = mounts.current().ok_or_else(portal_not_mounted)?;
    if mount.is_faulted() {
        return Err(portal_not_mounted());
    }
    let raw = source(webview)?;
    match origin::parse(&raw) {
        Some(label) if label == mount.label => Ok((mount, raw)),
        _ => Err(fixture::stale()),
    }
}

/// Validate a fixed-script reply and shape the evidence object (protocol.md section 9).
fn portal_evidence(
    json: &str,
    job: &PortalPending,
    label: &str,
    mode: &str,
) -> Result<serde_json::Value, QaFailure> {
    if json.len() > PORTAL_REPLY_MAX {
        return Err(document_error());
    }
    match job.kind {
        PortalKind::Observe => {
            let page: ObservedPage = serde_json::from_str(json).map_err(|_| document_error())?;
            if page.origin != job.origin || !page.ready || page.probes.len() > PORTAL_PROBE_MAX {
                return Err(document_error());
            }
            let mut probes = BTreeMap::new();
            let mut input_bytes = 0usize;
            for (name, text) in page.probes {
                input_bytes += text.len();
                let name_ok = !name.is_empty()
                    && name.len() <= 64
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
                if !name_ok
                    || text.chars().count() > PORTAL_TEXT_MAX
                    || input_bytes > PORTAL_DIGEST_INPUT_MAX
                    || probes.insert(name, text).is_some()
                {
                    return Err(document_error());
                }
            }
            // Digest: BLAKE3 over probe texts in name order, each followed by NUL (probe elements only).
            let mut hasher = blake3::Hasher::new();
            for text in probes.values() {
                hasher.update(text.as_bytes());
                hasher.update(&[0]);
            }
            Ok(serde_json::json!({
                "label": label, "mode": mode, "title": page.title,
                "digest": hasher.finalize().to_hex().to_string(),
                "has_api": page.has_api, "probes": probes,
            }))
        }
        PortalKind::Slot(slot) => {
            let reply: SlotReply = serde_json::from_str(json).map_err(|_| document_error())?;
            if reply.origin != job.origin {
                return Err(document_error());
            }
            Ok(serde_json::json!({
                "label": label, "mode": mode, "slot": slot, "activated": reply.activated,
            }))
        }
        PortalKind::IpcStats => Err(document_error()),
    }
}

/// Run the fixed observe or slot script; the origin is re-checked inside the document before any read.
fn execute_portal(
    webview: &wry::WebView,
    origin: &str,
    kind: PortalKind,
    completion: Rc<RefCell<Option<Completion>>>,
) -> Result<(), QaFailure> {
    let expected = serde_json::to_string(origin).map_err(|_| fixture::stale())?;
    let body = match kind {
        PortalKind::Observe => format!(
            "const probes=[]; for(const el of document.querySelectorAll('[data-fe-probe]')){{ \
             if(probes.length>={PORTAL_PROBE_MAX}) break; \
             probes.push([String(el.getAttribute('data-fe-probe')).slice(0,64),(el.textContent||'').trim().slice(0,{PORTAL_TEXT_MAX})]); }} \
             return {{origin:location.origin,title:document.title.slice(0,256),ready:true,\
             has_api:typeof window.fe==='object'&&window.fe!==null,probes}};"
        ),
        PortalKind::Slot(slot) if (1..=PORTAL_SLOT_MAX).contains(&slot) => format!(
            "const el=document.querySelector('[data-fe-slot=\"{slot}\"]'); \
             if(!el||el.disabled) return {{origin:location.origin,activated:false}}; \
             el.click(); return {{origin:location.origin,activated:true}};"
        ),
        _ => {
            return Err(fixture::failure(
                "invalid_fixture_operation",
                "Slot must be 1..32",
            ))
        }
    };
    let script = format!(
        "(()=>{{const expected={expected}; if(location.origin!==expected) return null; \
         if(document.readyState!=='complete') return 'fixture_document_pending'; {body}}})()"
    );
    unsafe {
        webview
            .webview()
            .ExecuteScript(
                &HSTRING::from(script),
                &ExecuteScriptCompletedHandler::create(Box::new(move |error, json| {
                    *completion.borrow_mut() = Some(Completion::Portal(
                        if error.is_ok() && json.len() <= PORTAL_REPLY_MAX {
                            Ok(json)
                        } else {
                            Err(())
                        },
                    ));
                    Ok(())
                })),
            )
            .map_err(|_| {
                fixture::failure(
                    "portal_script_unavailable",
                    "Fixed native fixture script is unavailable",
                )
            })
    }
}
