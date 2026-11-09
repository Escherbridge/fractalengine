//! Pure portal chrome model: BundleState -> badge text, tone, actions. See `docs/spec/portal-bundle/ux.md` §3.

use fe_webview::ipc::BundleState;

/// Visual weight of a badge; the text always carries the state too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeTone {
    Accent,
    Neutral,
    Error,
}

/// A button the chrome can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeAction {
    Close,
    Admit,
    RevokeLocal,
}

impl ChromeAction {
    /// Visible label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Close => "Close",
            Self::Admit => "Admit…",
            Self::RevokeLocal => "Revoke on this peer",
        }
    }

    /// Stable accessible name (ux.md §5).
    pub fn accessible_name(self) -> &'static str {
        match self {
            Self::Close => "Close portal",
            Self::Admit => "Admit bundle",
            Self::RevokeLocal => "Revoke bundle on this peer",
        }
    }
}

/// What the toolbar shows for one bundle state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeModel {
    /// ASCII accessible name, e.g. "Admitted - z6Mkha...ab12".
    pub badge_name: String,
    /// Visible text (em dash spelling).
    pub badge_text: String,
    pub tone: ChromeTone,
    pub rendered: bool,
    pub ipc: bool,
    pub primary: ChromeAction,
    pub secondary: Option<ChromeAction>,
}

/// Short display form of a DID: scheme-less, first 6 and last 4 characters, joined by an ASCII "..." (it lands in locators).
pub fn short_did(did: &str) -> String {
    let body = did.rsplit(':').next().unwrap_or(did);
    let chars: Vec<char> = body.chars().collect();
    if chars.len() <= 12 {
        return body.to_owned();
    }
    let head: String = chars[..6].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}...{tail}")
}

fn model(
    badge: String,
    tone: ChromeTone,
    rendered: bool,
    ipc: bool,
    primary: ChromeAction,
    secondary: Option<ChromeAction>,
) -> ChromeModel {
    ChromeModel {
        badge_text: badge.replacen(" - ", " — ", 1),
        badge_name: badge,
        tone,
        rendered,
        ipc,
        primary,
        secondary,
    }
}

/// Pure: the chrome for a bundle state; `None` while closed (no badge).
pub fn chrome_model(state: &BundleState) -> Option<ChromeModel> {
    use ChromeAction::{Admit, Close, RevokeLocal};
    Some(match state {
        BundleState::Admitted { publisher } => model(
            format!("Admitted - {}", short_did(publisher)),
            ChromeTone::Accent,
            true,
            true,
            Close,
            Some(RevokeLocal),
        ),
        BundleState::StaticSandbox { can_admit } => model(
            "Static - not admitted on this peer".into(),
            ChromeTone::Neutral,
            true,
            false,
            if *can_admit { Admit } else { Close },
            can_admit.then_some(Close),
        ),
        BundleState::Blocked(reason) => model(
            format!("Blocked - {}", reason.suffix()),
            ChromeTone::Error,
            false,
            false,
            Close,
            None,
        ),
        BundleState::Revoked(reason) => model(
            format!("Revoked - {}", reason.suffix()),
            ChromeTone::Error,
            false,
            false,
            Close,
            None,
        ),
        BundleState::WaitingForBytes => model(
            "Waiting for bundle bytes".into(),
            ChromeTone::Neutral,
            false,
            false,
            Close,
            None,
        ),
        BundleState::Unavailable => model(
            "Embedded browsing unavailable".into(),
            ChromeTone::Error,
            false,
            false,
            Close,
            None,
        ),
        BundleState::Closed => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_webview::ipc::{BlockedReason, RevokedReason};

    #[test]
    fn chrome_table_matches_ux_contract() {
        let blocked = [
            BlockedReason::Integrity,
            BlockedReason::MalformedManifest,
            BlockedReason::TooLarge,
            BlockedReason::VersePolicy,
            BlockedReason::UnsupportedPlatform,
        ];
        let revoked = [
            RevokedReason::KillSwitch,
            RevokedReason::RevocationList,
            RevokedReason::AdmissionWithdrawn,
        ];
        for reason in blocked {
            let m = chrome_model(&BundleState::Blocked(reason)).unwrap();
            assert_eq!(m.badge_name, format!("Blocked - {}", reason.suffix()));
            assert!(!m.rendered && !m.ipc && m.primary == ChromeAction::Close);
        }
        for reason in revoked {
            let m = chrome_model(&BundleState::Revoked(reason)).unwrap();
            assert_eq!(m.badge_name, format!("Revoked - {}", reason.suffix()));
            assert!(!m.rendered && !m.ipc && m.secondary.is_none());
        }
        let admitted = chrome_model(&BundleState::Admitted { publisher: "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK".into() }).unwrap();
        assert!(admitted.rendered && admitted.ipc);
        assert_eq!(admitted.secondary, Some(ChromeAction::RevokeLocal));
        assert!(admitted.badge_name.is_ascii(), "locators are plain ASCII (ux.md §5)");
        assert!(admitted.badge_name.starts_with("Admitted - z6Mkha..."));
        assert!(admitted.badge_text.contains(" — "));
        let can = chrome_model(&BundleState::StaticSandbox { can_admit: true }).unwrap();
        assert_eq!((can.primary, can.secondary), (ChromeAction::Admit, Some(ChromeAction::Close)));
        let cannot = chrome_model(&BundleState::StaticSandbox { can_admit: false }).unwrap();
        assert!(cannot.rendered && !cannot.ipc);
        assert_eq!((cannot.primary, cannot.secondary), (ChromeAction::Close, None));
        assert_eq!(chrome_model(&BundleState::WaitingForBytes).unwrap().badge_name, "Waiting for bundle bytes");
        assert_eq!(chrome_model(&BundleState::Unavailable).unwrap().badge_name, "Embedded browsing unavailable");
        assert!(chrome_model(&BundleState::Closed).is_none());
    }
}
