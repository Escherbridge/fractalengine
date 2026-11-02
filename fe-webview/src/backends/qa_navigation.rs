//! Windows fixture redirect guard; see src/AGENTS.md section QA web navigation.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use webview2_com::NavigationStartingEventHandler;
use wry::WebViewExtWindows;

use crate::backend::BackendEvent;

pub(super) fn install(
    webview: &wry::WebView,
    fixture_session: Rc<Cell<bool>>,
    events: Rc<RefCell<Vec<BackendEvent>>>,
) -> anyhow::Result<()> {
    let mut token = 0i64;
    unsafe {
        webview.webview().add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                if !fixture_session.get() { return Ok(()); }
                let Some(args) = args else { return Ok(()); };
                // Cancel first: failures while inspecting metadata remain closed.
                args.SetCancel(true)?;
                let mut redirected = Default::default();
                args.IsRedirected(&mut redirected)?;
                let mut uri = Default::default();
                args.Uri(&mut uri)?;
                let raw = webview2_com::take_pwstr(uri);
                if !redirected.as_bool() && fe_runtime::qa_web::allows(&raw) {
                    args.SetCancel(false)?;
                    if let Ok(url) = raw.parse() {
                        events.borrow_mut().push(BackendEvent::UrlChanged(url));
                    }
                } else {
                    events.borrow_mut().push(BackendEvent::Error(
                        "Controlled fixture navigation refused: redirect, expired helper, or unapproved destination".into(),
                    ));
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}
