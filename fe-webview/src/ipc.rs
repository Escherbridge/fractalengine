#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, bevy::prelude::Message)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum BrowserCommand {
    Navigate { url: url::Url },
    Close,
    GetUrl,
    SwitchTab { tab: BrowserTab },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, bevy::prelude::Message)]
#[serde(tag = "evt", rename_all = "snake_case")]
pub enum BrowserEvent {
    UrlChanged { url: url::Url },
    LoadComplete,
    Error { message: String },
    TabChanged { tab: BrowserTab },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BrowserTab {
    ExternalUrl,
    Config,
}
