use url::Url;

/// Screen-space rectangle in physical pixels (screen coordinates).
/// Computed by the Bevy plugin from the primary window's position and layout.
#[derive(Debug, Clone, Copy)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Events emitted by the backend toward Bevy.
/// Converted into `BrowserEvent` messages by the plugin layer.
#[derive(Debug, Clone)]
pub enum BackendEvent {
    UrlChanged(Url),
    LoadComplete,
    Error(String),
    /// The backend's window was closed externally (e.g. Alt+F4).
    WindowClosed,
}

/// Abstraction over a webview rendering backend.
///
/// Implementations are `!Send` (native window handles are thread-bound).
/// The Bevy plugin holds this as a `NonSend` resource on the main thread.
///
/// Lifecycle: `create` → `navigate`/`show`/`hide`/`reposition` → `destroy`
///
/// The backend owns its own window (or surface). It creates that window
/// positioned relative to the main application window. The backend handles
/// all input events within its own window; the Bevy side never forwards
/// keyboard/mouse events.
pub trait WebViewBackend: 'static {
    /// Create the backend's window/surface at `geometry`.
    /// `parent_handle` is the raw window handle of the main Bevy window
    /// so the backend can set ownership/z-order relationships.
    ///
    /// The window starts hidden; call `show` to make it visible.
    fn create(
        parent_handle: &raw_window_handle::RawWindowHandle,
        geometry: WindowGeometry,
        trust_bar_js: &str,
    ) -> anyhow::Result<Self>
    where
        Self: Sized;

    /// Navigate to the given URL. Shows the window if hidden.
    fn navigate(&mut self, url: &Url) -> anyhow::Result<()>;

    /// Go back one page in history.
    fn go_back(&mut self) -> anyhow::Result<()>;

    /// Show the backend's window.
    fn show(&mut self) -> anyhow::Result<()>;

    /// Hide the backend's window without destroying it.
    fn hide(&mut self) -> anyhow::Result<()>;

    /// Reposition and resize the backend's window.
    fn reposition(&mut self, geometry: WindowGeometry) -> anyhow::Result<()>;

    /// Destroy the backend's window and release all resources.
    fn destroy(&mut self);

    /// Drain pending events from the backend. Called once per frame.
    fn drain_events(&mut self) -> Vec<BackendEvent>;

    /// Whether the backend's window is alive.
    fn is_alive(&self) -> bool;
}
