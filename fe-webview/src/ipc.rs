#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, bevy::prelude::Message)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum BrowserCommand {
    Navigate { url: String },
    GoBack,
    GoForward,
    Reload,
    OpenExternal { url: url::Url },
    Close,
    GetUrl,
    SwitchTab { tab: BrowserTab },
    /// Mount and show a repository node's bundle; carries no URL, path or hash (protocol.md §2.4).
    OpenBundle { node_id: String },
    /// Mount the build a `portal.open` binding selects at `petal_id`'s nearest scope (bindings.md §6); carries no URL,
    /// path or hash. Written only behind fe-ui's `UiAction::OpenBundle` path so the host chrome and re-judge tick
    /// own the mount (R-POR-10); the host refuses bytes whose package id is not `package_id`.
    OpenBoundBundle { petal_id: String, package_id: String },
    /// Unmount and unload the open bundle, if any.
    CloseBundle,
}

impl BrowserCommand {
    /// The chrome anchor a bundle command's `BundleState` events carry; `None` for non-bundle commands.
    pub fn bundle_anchor(&self) -> Option<String> {
        match self {
            Self::OpenBundle { node_id } => Some(node_id.clone()),
            Self::OpenBoundBundle { petal_id, package_id } => {
                Some(crate::portal_bundle::BundleTarget::Bound { petal_id: petal_id.clone(), package_id: package_id.clone() }.anchor())
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, bevy::prelude::Message)]
#[serde(tag = "evt", rename_all = "snake_case")]
pub enum BrowserEvent {
    UrlChanged { url: url::Url },
    LoadComplete,
    PageLoaded { url: url::Url },
    ExternalLaunchRequested,
    Error { message: String },
    TabChanged { tab: BrowserTab },
    /// Host-decided bundle state for the chrome; never derived from page content.
    BundleState {
        node_id: String,
        /// The origin label while a mount exists (the toolbar's hostname line), else `None`.
        label: Option<String>,
        state: BundleState,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BrowserTab {
    ExternalUrl,
    Config,
}

/// Chrome state of a bundle (admission.md §2, protocol.md §8).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleState {
    /// Rendered with the bridge; `publisher` is the sealing DID.
    Admitted { publisher: String },
    /// Rendered without the bridge.
    StaticSandbox { can_admit: bool },
    Blocked(BlockedReason),
    Revoked(RevokedReason),
    WaitingForBytes,
    Unavailable,
    Closed,
}

/// Why a bundle is not rendered (chrome suffix after "Blocked — ").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockedReason {
    Integrity,
    MalformedManifest,
    TooLarge,
    VersePolicy,
    UnsupportedPlatform,
}

impl BlockedReason {
    /// The exact chrome suffix (admission.md §2).
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Integrity => "integrity failure",
            Self::MalformedManifest => "malformed manifest",
            Self::TooLarge => "too large",
            Self::VersePolicy => "verse policy",
            Self::UnsupportedPlatform => "unsupported platform",
        }
    }
}

/// Why a bundle was unloaded (chrome suffix after "Revoked — ").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevokedReason {
    KillSwitch,
    RevocationList,
    AdmissionWithdrawn,
}

impl RevokedReason {
    /// The exact chrome suffix (admission.md §2).
    pub fn suffix(self) -> &'static str {
        match self {
            Self::KillSwitch => "kill switch",
            Self::RevocationList => "revocation list",
            Self::AdmissionWithdrawn => "admission withdrawn",
        }
    }
}
