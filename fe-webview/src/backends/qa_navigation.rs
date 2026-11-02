//! Authoritative Windows QA-host navigation guard; see src/AGENTS.md section QA web navigation.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Controller, ICoreWebView2NavigationStartingEventArgs,
};
use webview2_com::NavigationStartingEventHandler;
use wry::WebViewExtWindows;

use crate::backend::BackendEvent;

pub(super) const TERMINAL_ERROR: &str =
    "Browser disabled: native navigation enforcement failed; restart the application";
pub(super) const UNCONFIRMED_ERROR: &str =
    "Browser disabled: navigation cancellation could not be confirmed; restart the application";

pub(super) trait NavigationAttempt {
    fn metadata(&self) -> Result<(String, bool), ()>;
    fn set_cancel(&self, cancel: bool) -> Result<(), ()>;
    fn stop(&self) -> Result<(), ()>;
    fn close(&self) -> Result<(), ()>;
}

struct NativeAttempt<'a> {
    args: Option<ICoreWebView2NavigationStartingEventArgs>,
    webview: &'a ICoreWebView2,
    controller: &'a ICoreWebView2Controller,
}

impl NavigationAttempt for NativeAttempt<'_> {
    fn metadata(&self) -> Result<(String, bool), ()> {
        let args = self.args.as_ref().ok_or(())?;
        unsafe {
            let mut redirected = Default::default();
            args.IsRedirected(&mut redirected).map_err(|_| ())?;
            let mut uri = Default::default();
            args.Uri(&mut uri).map_err(|_| ())?;
            let raw = webview2_com::take_pwstr(uri);
            if raw.is_empty() || raw.len() > 8192 {
                return Err(());
            }
            Ok((raw, redirected.as_bool()))
        }
    }

    fn set_cancel(&self, cancel: bool) -> Result<(), ()> {
        let args = self.args.as_ref().ok_or(())?;
        unsafe { args.SetCancel(cancel).map_err(|_| ()) }
    }

    fn stop(&self) -> Result<(), ()> {
        unsafe { self.webview.Stop().map_err(|_| ()) }
    }

    fn close(&self) -> Result<(), ()> {
        unsafe { self.controller.Close().map_err(|_| ()) }
    }
}

pub(super) fn enforce(
    attempt: &impl NavigationAttempt,
    fixture_session: &Cell<bool>,
    unusable: &Cell<bool>,
    events: &RefCell<Vec<BackendEvent>>,
) {
    if unusable.get() {
        // Closing can reenter callbacks; the terminal transition already owns Stop/Close.
        if attempt.set_cancel(true).is_err() {
            tracing::error!("Already-disabled browser could not cancel a reentrant navigation");
        }
        return;
    }
    if attempt.set_cancel(true).is_err() {
        disable(attempt, unusable, events, false);
        return;
    }
    let Ok((raw, redirected)) = attempt.metadata() else {
        disable(attempt, unusable, events, true);
        return;
    };
    let fixture_target = fe_runtime::qa_web::preview_allows(&raw);
    let allowed = if fixture_session.get() || fixture_target {
        !redirected && fe_runtime::qa_web::allows(&raw)
    } else {
        crate::security::embedded_allowed(&raw)
    };
    if allowed {
        if attempt.set_cancel(false).is_err() {
            disable(attempt, unusable, events, false);
            return;
        }
        if fixture_target {
            fixture_session.set(true);
        }
        if let Ok(url) = raw.parse() {
            events.borrow_mut().push(BackendEvent::UrlChanged(url));
        }
    } else {
        events.borrow_mut().push(BackendEvent::Error(
            "Navigation refused: URL policy, fixture redirect, or unavailable helper".into(),
        ));
    }
}

fn disable(
    attempt: &impl NavigationAttempt,
    unusable: &Cell<bool>,
    events: &RefCell<Vec<BackendEvent>>,
    canceled: bool,
) {
    unusable.set(true);
    let stopped = attempt.stop().is_ok();
    if !stopped {
        tracing::error!("Browser navigation enforcement failed and Stop failed");
    }
    let closed = attempt.close().is_ok();
    if !closed {
        tracing::error!("Browser navigation enforcement failed and controller Close failed");
    }
    let mut events = events.borrow_mut();
    events.clear();
    events.push(BackendEvent::Error(
        if canceled || stopped || closed {
            TERMINAL_ERROR
        } else {
            UNCONFIRMED_ERROR
        }
        .into(),
    ));
}

pub(super) fn install(
    webview: &wry::WebView,
    fixture_session: Rc<Cell<bool>>,
    unusable: Rc<Cell<bool>>,
    events: Rc<RefCell<Vec<BackendEvent>>>,
) -> anyhow::Result<()> {
    let native = webview.webview();
    let controller = webview.controller();
    let callback_native = native.clone();
    let callback_controller = controller.clone();
    let callback_unusable = unusable.clone();
    let callback_events = events.clone();
    let mut token = 0i64;
    let result = unsafe {
        native.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let attempt = NativeAttempt {
                    args,
                    webview: &callback_native,
                    controller: &callback_controller,
                };
                enforce(
                    &attempt,
                    &fixture_session,
                    &callback_unusable,
                    &callback_events,
                );
                Ok(())
            })),
            &mut token,
        )
    };
    if let Err(error) = result {
        let attempt = NativeAttempt {
            args: None,
            webview: &native,
            controller: &controller,
        };
        disable(&attempt, &unusable, &events, false);
        return Err(anyhow::anyhow!(
            "native navigation guard registration failed: {error}"
        ));
    }
    Ok(())
}
