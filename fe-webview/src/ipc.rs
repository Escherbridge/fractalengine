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
    /// Unmount and unload the open bundle, if any.
    CloseBundle,
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
