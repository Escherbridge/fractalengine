//! Fixed fixture DOM and native PNG jobs; see src/AGENTS.md section QA Portal evidence.

use fe_runtime::qa_debug::QaFailure;
use fe_runtime::qa_portal::{
    self as fixture, FixtureCommand, FixtureObservation, FixtureOp, FixtureOutcome, FixtureReceipt,
    FixtureResult, MAX_DIMENSION, MAX_PNG_BYTES,
};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
use webview2_com::{CapturePreviewCompletedHandler, ExecuteScriptCompletedHandler};
use windows::core::HSTRING;
use wry::WebViewExtWindows;

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
#[derive(Default)]
pub(super) struct NavigationClock {
    generation: Cell<u64>,
    refusals: Cell<u64>,
    last_refusal: RefCell<Option<String>>,
}
impl NavigationClock {
    pub(super) fn bump(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
    }
    pub(super) fn refuse(&self, raw: String) {
        self.refusals.set(self.refusals.get().wrapping_add(1));
        *self.last_refusal.borrow_mut() = Some(raw);
    }
}

enum Completion {
    Dom(Result<String, ()>),
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

pub(super) struct Surface {
    pub(super) clock: Rc<NavigationClock>,
    session: u64,
    pending: Option<Pending>,
}

impl Default for Surface {
    fn default() -> Self {
        Self {
            clock: Rc::default(),
            session: NEXT_SESSION.fetch_add(1, Ordering::Relaxed),
            pending: None,
        }
    }
}

impl Surface {
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
        if self.pending.is_some() {
            return Err(fixture::failure(
                "portal_fixture_busy",
                "A native fixture operation is pending",
            ));
        }
        let raw = live_source(webview, visible, unusable)?;
        if Instant::now() >= command.deadline
            || command.deadline.saturating_duration_since(Instant::now()) > fixture::JOB_LIMIT
        {
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
        self.pending.take().is_some()
    }

    pub(super) fn poll(
        &mut self,
        webview: &wry::WebView,
        visible: bool,
        unusable: bool,
    ) -> Option<FixtureOutcome> {
        let mut job = self.pending.take()?;
        if !visible || unusable || Instant::now() >= job.command.deadline {
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
        let expected_second = format!("{origin}/web/two");
        let expected_third = format!("{origin}/web/three");
        let same = generation == job.generation && raw == job.initial_url;
        let second =
            is_second && generation == job.generation.wrapping_add(1) && raw == expected_second;
        if !same && !second {
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
            if live_source(webview, visible, unusable).is_err()
                || Instant::now() >= job.command.deadline
            {
                return Some(Err(fixture::stale()));
            }
            match completion {
                Completion::Dom(Ok(json)) => {
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
    let width = u32::try_from(bounds.right - bounds.left).map_err(|_| fixture::unavailable())?;
    let height = u32::try_from(bounds.bottom - bounds.top).map_err(|_| fixture::unavailable())?;
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
