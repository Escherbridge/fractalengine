//! Exact-origin navigation admission for bundle origins (protocol.md §6); see `portal_bundle/AGENTS.md` §navigation.

use std::cell::RefCell;

use super::mount::PortalMounts;
use super::origin::{self, Label};

/// The fixed document the host loads to unload a bundle (P-4: one known URI for the unload token).
pub const UNLOAD_URL: &str = "about:blank";

/// One top-frame (or frame) navigation as a guard layer sees it.
#[derive(Debug, Clone, Copy)]
pub struct Attempt<'a> {
    pub raw_url: &'a str,
    /// Only the native WebView2 owner knows this; the wry callback always passes `false` (SP desk check F-4).
    pub is_redirect: bool,
    pub is_main_frame: bool,
}

/// What the top frame currently holds, as the host last admitted it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Current {
    #[default]
    None,
    External,
    Bundle(Label),
    /// Left a bundle for a declared origin: redirect hops are still judged against its list.
    Leaving(Vec<String>),
    /// A bundle document whose mount the host removed; only a host-minted navigation may replace it.
    Detached,
}

/// One-shot host intent, minted by Open, Close/revoke or a host Navigate and voided by the next one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostToken {
    Enter(String),
    Unload,
    Navigate(String),
}

/// The live mount as the decision needs it.
#[derive(Debug, Clone, Copy)]
pub struct MountView<'a> {
    pub label: &'a Label,
    /// Declared `navigation` origins, normalized by [`normalize_origin`].
    pub navigation: &'a [String],
    /// Declared `csp.frame` origins, normalized by [`normalize_origin`].
    pub frames: &'a [String],
}

/// Everything [`admits`] reads.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    pub current: &'a Current,
    pub mount: Option<MountView<'a>>,
    pub token: Option<&'a HostToken>,
}

/// The admission verdict (protocol.md §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    AllowSame,
    AllowEnter,
    AllowLeave,
    AllowHostUnload,
    DeferToRawPolicy,
    Refuse(RefuseReason),
}

/// Why a navigation was refused (protocol.md §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefuseReason {
    EnterWithoutOpen,
    Undeclared,
    OtherBundle,
    Scheme,
    BlockedHost,
    RedirectIntoBundle,
}

impl RefuseReason {
    /// Stable snake_case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EnterWithoutOpen => "enter_without_open",
            Self::Undeclared => "undeclared",
            Self::OtherBundle => "other_bundle",
            Self::Scheme => "scheme",
            Self::BlockedHost => "blocked_host",
            Self::RedirectIntoBundle => "redirect_into_bundle",
        }
    }
}

/// `scheme://host[:port]` of an http(s) origin, as the browser serializes it; `None` when unparseable.
pub fn normalize_origin(origin: &str) -> Option<String> {
    let url = url::Url::parse(origin).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.origin().ascii_serialization())
}

/// Pure decision over one attempt (protocol.md §6.3 rows 1-13).
pub fn admits(attempt: &Attempt<'_>, ctx: &Context<'_>) -> Decision {
    let target = origin::parse(attempt.raw_url);
    let live = |label: &Label| ctx.mount.is_some_and(|mount| mount.label == label);
    if !attempt.is_main_frame {
        return match (ctx.current, ctx.mount) {
            (Current::Bundle(current), Some(mount)) if mount.label == current => {
                frame_rule(attempt.raw_url, target.is_some(), mount.frames)
            }
            _ if target.is_some() => Decision::Refuse(RefuseReason::OtherBundle),
            _ => Decision::DeferToRawPolicy,
        };
    }
    if let Some(label) = target {
        // Row 10: the protocol never redirects, so a redirect into a bundle origin is always an attack.
        if attempt.is_redirect {
            return Decision::Refuse(RefuseReason::RedirectIntoBundle);
        }
        // Row 2: only the host's own Open can enter a bundle origin.
        if matches!(ctx.token, Some(HostToken::Enter(url)) if url == attempt.raw_url) && live(&label) {
            return Decision::AllowEnter;
        }
        return match ctx.current {
            Current::Bundle(current) if *current == label && live(&label) => Decision::AllowSame,
            Current::Bundle(current) if *current != label => Decision::Refuse(RefuseReason::OtherBundle),
            _ => Decision::Refuse(RefuseReason::EnterWithoutOpen),
        };
    }
    // Row 9: the host's own unload, admitted by token, never by a scheme exception.
    if attempt.raw_url == UNLOAD_URL && matches!(ctx.token, Some(HostToken::Unload)) {
        return Decision::AllowHostUnload;
    }
    if !attempt.is_redirect && matches!(ctx.token, Some(HostToken::Navigate(url)) if url == attempt.raw_url) {
        return Decision::DeferToRawPolicy;
    }
    match ctx.current {
        Current::Bundle(current) => match ctx.mount {
            Some(mount) if mount.label == current => leave_rule(attempt.raw_url, mount.navigation),
            _ => Decision::Refuse(RefuseReason::Undeclared),
        },
        Current::Detached => Decision::Refuse(RefuseReason::Undeclared),
        // Row 11: every hop of a leave is judged against the bundle's list.
        Current::Leaving(declared) if attempt.is_redirect => leave_rule(attempt.raw_url, declared),
        // Row 12: external behaviour is unchanged.
        Current::None | Current::External | Current::Leaving(_) => Decision::DeferToRawPolicy,
    }
}

/// Rows 4, 5, 7, 8 and 13: an https origin the bundle declared and the raw policy admits.
fn leave_rule(raw: &str, declared: &[String]) -> Decision {
    let Ok(url) = url::Url::parse(raw) else {
        return Decision::Refuse(RefuseReason::Scheme);
    };
    if url.scheme() != "https" {
        return Decision::Refuse(RefuseReason::Scheme);
    }
    if !fe_runtime::portal_url::is_url_allowed(&url) {
        return Decision::Refuse(RefuseReason::BlockedHost);
    }
    let origin = url.origin().ascii_serialization();
    if declared.iter().any(|allowed| *allowed == origin) {
        Decision::AllowLeave
    } else {
        Decision::Refuse(RefuseReason::Undeclared)
    }
}

/// §6.6: a frame inside a bundle loads only a declared `csp.frame` origin; never a bundle origin.
fn frame_rule(raw: &str, is_bundle: bool, frames: &[String]) -> Decision {
    if is_bundle {
        return Decision::Refuse(RefuseReason::OtherBundle);
    }
    match leave_rule(raw, frames) {
        Decision::AllowLeave => Decision::DeferToRawPolicy,
        refused => refused,
    }
}

/// A host notice the bundle host turns into a `BundleState` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateNotice {
    /// The page left its bundle for a declared origin; the mount is gone.
    Left { node_id: String, label: Label },
}

#[derive(Debug, Default)]
struct GateState {
    current: Current,
    token: Option<HostToken>,
    notices: Vec<GateNotice>,
}

/// Main-thread guard state shared by `decide_navigation` and `qa_navigation::enforce`.
///
/// Never hold a borrow of it across a webview call: navigation callbacks re-enter it.
#[derive(Debug)]
pub struct NavigationGate {
    mounts: PortalMounts,
    state: RefCell<GateState>,
}

impl NavigationGate {
    /// A gate over `mounts` with nothing loaded.
    pub fn new(mounts: PortalMounts) -> Self {
        Self {
            mounts,
            state: RefCell::new(GateState::default()),
        }
    }

    /// Decide one attempt and apply its effects (token consumption, current document, generation, leave).
    pub fn decide(&self, attempt: &Attempt<'_>) -> Decision {
        let live = self.mounts.current();
        let mut state = self.state.borrow_mut();
        // A leave context lasts only for the top-frame redirect hops that immediately follow it.
        if attempt.is_main_frame && matches!(state.current, Current::Leaving(_)) && !attempt.is_redirect {
            state.current = Current::External;
        }
        let decision = {
            let view = live.as_deref().map(|mount| MountView {
                label: &mount.label,
                navigation: &mount.navigation_origins,
                frames: &mount.frame_origins,
            });
            admits(
                attempt,
                &Context {
                    current: &state.current,
                    mount: view,
                    token: state.token.as_ref(),
                },
            )
        };
        if !attempt.is_main_frame {
            return decision;
        }
        match decision {
            Decision::AllowSame => self.mounts.bump_generation(),
            Decision::AllowEnter => {
                state.token = None;
                if let Some(label) = origin::parse(attempt.raw_url) {
                    state.current = Current::Bundle(label);
                }
                self.mounts.bump_generation();
            }
            Decision::AllowLeave => {
                let from_bundle = matches!(state.current, Current::Bundle(_));
                match live.as_deref() {
                    Some(mount) if from_bundle => {
                        self.mounts.unmount_if(mount.mount_id);
                        state.notices.push(GateNotice::Left {
                            node_id: mount.node_id.clone(),
                            label: mount.label.clone(),
                        });
                        state.current = Current::Leaving(mount.navigation_origins.clone());
                    }
                    _ => self.mounts.bump_generation(),
                }
            }
            Decision::AllowHostUnload => {
                state.token = None;
                state.current = Current::None;
                self.mounts.bump_generation();
            }
            Decision::DeferToRawPolicy => {
                if matches!(&state.token, Some(HostToken::Navigate(url)) if url == attempt.raw_url) {
                    state.token = None;
                }
            }
            Decision::Refuse(_) => {
                // A refused hop leaves the old bundle document in place, without its mount.
                if matches!(state.current, Current::Leaving(_)) {
                    state.current = Current::Detached;
                }
            }
        }
        decision
    }

    /// The layer's raw policy admitted a `DeferToRawPolicy` navigation: the top frame is now external.
    pub fn external_committed(&self) {
        self.state.borrow_mut().current = Current::External;
        self.mounts.bump_generation();
    }

    /// Open: admit exactly one navigation to `entry_url`.
    pub fn expect_entry(&self, entry_url: String) {
        self.mint(HostToken::Enter(entry_url));
    }

    /// Close or revoke: admit exactly one navigation to [`UNLOAD_URL`].
    pub fn expect_unload(&self) {
        self.mint(HostToken::Unload);
    }

    /// Host `Navigate` to an external URL while a bundle document may still be loaded.
    pub fn expect_navigation(&self, url: String) {
        self.mint(HostToken::Navigate(url));
    }

    /// The host removed the mount without unloading (a replacing load follows).
    pub fn detach(&self) {
        let mut state = self.state.borrow_mut();
        if matches!(state.current, Current::Bundle(_) | Current::Leaving(_)) {
            state.current = Current::Detached;
        }
    }

    fn mint(&self, token: HostToken) {
        let mut state = self.state.borrow_mut();
        state.token = Some(token);
        if matches!(state.current, Current::Bundle(_) | Current::Leaving(_)) {
            state.current = Current::Detached;
        }
    }

    /// §6.5: no download starts from a bundle origin or while a bundle document is loaded.
    pub fn allows_download(&self, url: &str) -> bool {
        let inner = url.strip_prefix("blob:").unwrap_or(url);
        if origin::parse(inner).is_some() {
            return false;
        }
        !matches!(self.state.borrow().current, Current::Bundle(_) | Current::Detached)
    }

    /// What the top frame holds now.
    pub fn current(&self) -> Current {
        self.state.borrow().current.clone()
    }

    /// Drain notices for the bundle host.
    pub fn take_notices(&self) -> Vec<GateNotice> {
        std::mem::take(&mut self.state.borrow_mut().notices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portal_bundle::origin::{admitted_label, static_label};

    /// protocol.md §6.3 rows 1-13 plus the frame rule, over the pure function.
    #[test]
    fn admits_follows_the_decision_table() {
        let a = admitted_label("did:key:publisher", "org.example.a");
        let b = static_label(&"ab".repeat(32)).expect("static label");
        let a_entry = a.document_url("web/index.html").unwrap();
        let a_page = a.document_url("web/docs/page.html").unwrap();
        let b_entry = b.document_url("web/index.html").unwrap();
        let (a_entry, a_page, b_entry) = (a_entry.as_str(), a_page.as_str(), b_entry.as_str());
        let navigation = vec!["https://docs.example.org".to_owned()];
        let frames = vec!["https://maps.example.org".to_owned()];
        let mounted_a = MountView { label: &a, navigation: &navigation, frames: &frames };
        let enter_a = HostToken::Enter(a_entry.to_owned());
        let unload = HostToken::Unload;
        let navigate = HostToken::Navigate("https://example.com/".to_owned());
        let (none, external, detached) = (Current::None, Current::External, Current::Detached);
        let in_a = Current::Bundle(a.clone());
        let leaving = Current::Leaving(navigation.clone());
        use Decision::*;
        use RefuseReason::*;
        // (current, mount, token, url, redirect, main frame, expected)
        let rows: Vec<(&Current, Option<MountView<'_>>, Option<&HostToken>, &str, bool, bool, Decision)> = vec![
            (&in_a, Some(mounted_a), None, a_page, false, true, AllowSame),
            (&none, Some(mounted_a), Some(&enter_a), a_entry, false, true, AllowEnter),
            (&external, Some(mounted_a), Some(&enter_a), a_entry, false, true, AllowEnter),
            (&none, Some(mounted_a), None, a_entry, false, true, Refuse(EnterWithoutOpen)),
            (&external, None, Some(&enter_a), a_entry, false, true, Refuse(EnterWithoutOpen)),
            (&in_a, None, None, a_page, false, true, Refuse(EnterWithoutOpen)),
            (&in_a, Some(mounted_a), None, "https://docs.example.org/guide", false, true, AllowLeave),
            (&in_a, Some(mounted_a), None, "https://docs.example.org:443/", false, true, AllowLeave),
            (&in_a, Some(mounted_a), None, "https://evil.example.com/", false, true, Refuse(Undeclared)),
            (&in_a, Some(mounted_a), None, b_entry, false, true, Refuse(OtherBundle)),
            (&in_a, Some(mounted_a), None, "file:///C:/", false, true, Refuse(Scheme)),
            (&in_a, Some(mounted_a), None, "javascript:alert(1)", false, true, Refuse(Scheme)),
            (&in_a, Some(mounted_a), None, "data:text/html,x", false, true, Refuse(Scheme)),
            (&in_a, Some(mounted_a), None, "http://docs.example.org/", false, true, Refuse(Scheme)),
            (&in_a, Some(mounted_a), None, "https://192.168.1.1/", false, true, Refuse(BlockedHost)),
            (&in_a, Some(mounted_a), None, "https://127.0.0.1/", false, true, Refuse(BlockedHost)),
            (&in_a, Some(mounted_a), Some(&unload), UNLOAD_URL, false, true, AllowHostUnload),
            (&none, None, Some(&unload), UNLOAD_URL, false, true, AllowHostUnload),
            (&in_a, Some(mounted_a), None, UNLOAD_URL, false, true, Refuse(Scheme)),
            (&external, Some(mounted_a), Some(&enter_a), a_entry, true, true, Refuse(RedirectIntoBundle)),
            (&leaving, None, None, "https://docs.example.org/next", true, true, AllowLeave),
            (&leaving, None, None, "https://elsewhere.example.org/", true, true, Refuse(Undeclared)),
            (&external, None, None, "https://example.com/", false, true, DeferToRawPolicy),
            (&none, None, None, "http://localhost/", false, true, DeferToRawPolicy),
            (&detached, None, None, "https://example.com/", false, true, Refuse(Undeclared)),
            (&detached, None, Some(&navigate), "https://example.com/", false, true, DeferToRawPolicy),
            (&in_a, Some(mounted_a), None, "https://maps.example.org/tile", false, false, DeferToRawPolicy),
            (&in_a, Some(mounted_a), None, "https://evil.example.com/", false, false, Refuse(Undeclared)),
            (&in_a, Some(mounted_a), None, a_page, false, false, Refuse(OtherBundle)),
        ];
        for (index, (current, mount, token, url, redirect, main, expected)) in rows.into_iter().enumerate() {
            let attempt = Attempt { raw_url: url, is_redirect: redirect, is_main_frame: main };
            let got = admits(&attempt, &Context { current, mount, token });
            assert_eq!(got, expected, "row {index}: {url}");
        }
    }
}
