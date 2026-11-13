//! OS-user-only local transport for the admin channel (docs/extensions/admin-channel.md §2; see `src/admin/AGENTS.md`).

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio::sync::mpsc;

use super::protocol::{
    decode_reply, decode_request, encode_reply, encode_request, read_frame, write_frame, AdminReply, AdminRequest,
    FrameError, PendingConsent, RefusalCode, ReplyOutcome, MAX_FRAME_BYTES,
};

#[cfg(unix)]
use super::socket_unix as platform;
#[cfg(windows)]
use super::pipe_windows as platform;

/// Env var naming the channel instance, so several apps of one OS user (QA peers) each get their own channel.
pub const ENV_ADMIN_INSTANCE: &str = "FE_EXT_ADMIN_INSTANCE";

/// Which channel of the current OS user: the default one or a named instance.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalEndpoint {
    instance: Option<String>,
}

/// An instance name outside `[A-Za-z0-9_-]{1,32}`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("admin channel instance must be 1-32 characters of A-Z a-z 0-9 _ -")]
pub struct InvalidInstance;

impl LocalEndpoint {
    /// The default channel of the current OS user.
    pub fn default_instance() -> Self {
        Self { instance: None }
    }

    /// A named channel of the current OS user.
    pub fn named(instance: &str) -> Result<Self, InvalidInstance> {
        let valid = !instance.is_empty()
            && instance.len() <= 32
            && instance.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if valid {
            Ok(Self { instance: Some(instance.to_owned()) })
        } else {
            Err(InvalidInstance)
        }
    }

    /// [`ENV_ADMIN_INSTANCE`] when set and non-empty, else the default channel.
    pub fn from_env() -> Result<Self, InvalidInstance> {
        match std::env::var(ENV_ADMIN_INSTANCE) {
            Ok(instance) if !instance.is_empty() => Self::named(&instance),
            _ => Ok(Self::default_instance()),
        }
    }

    /// The instance name, if any.
    pub fn instance(&self) -> Option<&str> {
        self.instance.as_deref()
    }

    /// The pipe name or socket path this endpoint resolves to, for messages.
    pub fn describe(&self) -> io::Result<String> {
        #[cfg(any(unix, windows))]
        {
            platform::address(self)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(unsupported())
        }
    }
}

/// Who is on the other end of an accepted connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerUser {
    /// The OS user this process runs as.
    Same,
    /// Anyone else, or a peer that could not be identified (fail closed); the text is for the log.
    Other(String),
}

/// Why the CLI could not reach the app.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// No app of this OS user serves the endpoint.
    #[error("FractalEngine is not running for this user (no admin channel at {0})")]
    NotRunning(String),
    /// The endpoint exists but belongs to another OS user; refused before sending anything.
    #[error("the admin channel at {0} is not owned by this user; refusing to talk to it")]
    ForeignServer(String),
    /// Transport failure.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// A connected admin stream (pipe, socket or in-memory); byte-oriented, frames via [`super::protocol`].
pub struct AdminStream(StreamInner);

enum StreamInner {
    #[cfg(windows)]
    PipeServer(tokio::net::windows::named_pipe::NamedPipeServer),
    #[cfg(windows)]
    PipeClient(tokio::net::windows::named_pipe::NamedPipeClient),
    #[cfg(unix)]
    Socket(tokio::net::UnixStream),
    Memory(DuplexStream),
}

impl AdminStream {
    #[cfg(windows)]
    pub(super) fn pipe_server(stream: tokio::net::windows::named_pipe::NamedPipeServer) -> Self {
        Self(StreamInner::PipeServer(stream))
    }

    #[cfg(windows)]
    pub(super) fn pipe_client(stream: tokio::net::windows::named_pipe::NamedPipeClient) -> Self {
        Self(StreamInner::PipeClient(stream))
    }

    #[cfg(unix)]
    pub(super) fn socket(stream: tokio::net::UnixStream) -> Self {
        Self(StreamInner::Socket(stream))
    }
}

impl std::fmt::Debug for AdminStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match &self.0 {
            #[cfg(windows)]
            StreamInner::PipeServer(_) | StreamInner::PipeClient(_) => "pipe",
            #[cfg(unix)]
            StreamInner::Socket(_) => "socket",
            StreamInner::Memory(_) => "memory",
        };
        f.debug_tuple("AdminStream").field(&kind).finish()
    }
}

impl AsyncRead for AdminStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            #[cfg(windows)]
            StreamInner::PipeServer(stream) => Pin::new(stream).poll_read(cx, buf),
            #[cfg(windows)]
            StreamInner::PipeClient(stream) => Pin::new(stream).poll_read(cx, buf),
            #[cfg(unix)]
            StreamInner::Socket(stream) => Pin::new(stream).poll_read(cx, buf),
            StreamInner::Memory(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for AdminStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().0 {
            #[cfg(windows)]
            StreamInner::PipeServer(stream) => Pin::new(stream).poll_write(cx, data),
            #[cfg(windows)]
            StreamInner::PipeClient(stream) => Pin::new(stream).poll_write(cx, data),
            #[cfg(unix)]
            StreamInner::Socket(stream) => Pin::new(stream).poll_write(cx, data),
            StreamInner::Memory(stream) => Pin::new(stream).poll_write(cx, data),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            #[cfg(windows)]
            StreamInner::PipeServer(stream) => Pin::new(stream).poll_flush(cx),
            #[cfg(windows)]
            StreamInner::PipeClient(stream) => Pin::new(stream).poll_flush(cx),
            #[cfg(unix)]
            StreamInner::Socket(stream) => Pin::new(stream).poll_flush(cx),
            StreamInner::Memory(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            #[cfg(windows)]
            StreamInner::PipeServer(stream) => Pin::new(stream).poll_shutdown(cx),
            #[cfg(windows)]
            StreamInner::PipeClient(stream) => Pin::new(stream).poll_shutdown(cx),
            #[cfg(unix)]
            StreamInner::Socket(stream) => Pin::new(stream).poll_shutdown(cx),
            StreamInner::Memory(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

/// Accepts admin connections and drops every peer that is not this OS user, in one place for every transport.
pub struct AdminListener {
    inner: ListenerInner,
    rejected: u64,
}

enum ListenerInner {
    #[cfg(any(unix, windows))]
    Platform(platform::Listener),
    Memory(mpsc::UnboundedReceiver<(DuplexStream, PeerUser)>),
}

impl std::fmt::Debug for AdminListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminListener").field("rejected", &self.rejected).finish_non_exhaustive()
    }
}

impl AdminListener {
    /// The next connection from this OS user; foreign or unidentifiable peers are closed and counted.
    pub async fn accept(&mut self) -> io::Result<AdminStream> {
        loop {
            let (stream, peer) = match &mut self.inner {
                #[cfg(any(unix, windows))]
                ListenerInner::Platform(listener) => listener.accept().await?,
                ListenerInner::Memory(receiver) => match receiver.recv().await {
                    Some((stream, peer)) => (AdminStream(StreamInner::Memory(stream)), peer),
                    None => return Err(io::Error::new(io::ErrorKind::BrokenPipe, "memory connector dropped")),
                },
            };
            match peer {
                PeerUser::Same => return Ok(stream),
                PeerUser::Other(who) => {
                    self.rejected += 1;
                    tracing::warn!(peer = %who, "admin channel: refused a connection from another OS user");
                    drop(stream);
                }
            }
        }
    }

    /// Connections refused because the peer was not this OS user.
    pub fn rejected_count(&self) -> u64 {
        self.rejected
    }
}

/// Bind this OS user's admin channel. Call inside a tokio runtime; a second app on the same endpoint gets an error.
pub fn listen_local(endpoint: &LocalEndpoint) -> io::Result<AdminListener> {
    #[cfg(any(unix, windows))]
    {
        Ok(AdminListener { inner: ListenerInner::Platform(platform::listen(endpoint)?), rejected: 0 })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = endpoint;
        Err(unsupported())
    }
}

/// Connect to this OS user's running app; the server's identity is checked before anything is sent.
pub async fn connect_local(endpoint: &LocalEndpoint) -> Result<AdminStream, ConnectError> {
    #[cfg(any(unix, windows))]
    {
        platform::connect(endpoint).await
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = endpoint;
        Err(ConnectError::Io(unsupported()))
    }
}

/// An in-process listener whose connections declare their peer user; the listener applies the same admission.
pub fn memory_listener() -> (AdminListener, MemoryConnector) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (AdminListener { inner: ListenerInner::Memory(receiver), rejected: 0 }, MemoryConnector { sender })
}

/// The client half of [`memory_listener`] (tests and the in-process simulator).
#[derive(Debug, Clone)]
pub struct MemoryConnector {
    sender: mpsc::UnboundedSender<(DuplexStream, PeerUser)>,
}

impl MemoryConnector {
    /// Open a connection that presents as `peer`.
    pub fn connect(&self, peer: PeerUser) -> io::Result<AdminStream> {
        let (client, server) = tokio::io::duplex(2 * (MAX_FRAME_BYTES + 4));
        self.sender
            .send((server, peer))
            .map_err(|_| io::Error::new(io::ErrorKind::ConnectionRefused, "memory listener dropped"))?;
        Ok(AdminStream(StreamInner::Memory(client)))
    }
}

/// What the server reads next on a connection.
#[derive(Debug)]
pub enum Incoming {
    /// A valid request.
    Request(AdminRequest),
    /// Send this refusal, then close the connection (a bad frame cannot be resynchronised).
    Refuse(AdminReply),
    /// The client closed, or the transport failed.
    Closed,
}

/// Read and decode the next request (server side).
pub async fn next_request<S: AsyncRead + Unpin>(stream: &mut S) -> Incoming {
    match read_frame(stream).await {
        Ok(Some(body)) => match decode_request(&body) {
            Ok(request) => Incoming::Request(request),
            Err(refusal) => Incoming::Refuse(refusal),
        },
        Ok(None) => Incoming::Closed,
        Err(FrameError::TooLarge(length)) => Incoming::Refuse(AdminReply::refused(
            "",
            RefusalCode::FrameTooLarge,
            format!("request frame of {length} bytes exceeds the {MAX_FRAME_BYTES}-byte limit"),
        )),
        Err(FrameError::Empty) => Incoming::Refuse(AdminReply::refused("", RefusalCode::BadRequest, "empty frame")),
        Err(FrameError::Truncated | FrameError::Io(_)) => Incoming::Closed,
    }
}

/// Write one reply frame (server side); an oversize reply goes out as `reply-too-large`.
pub async fn write_reply<S: AsyncWrite + Unpin>(stream: &mut S, reply: &AdminReply) -> Result<(), FrameError> {
    write_frame(stream, &encode_reply(reply)).await
}

/// Why a client exchange failed.
#[derive(Debug, thiserror::Error)]
pub enum ExchangeError {
    /// Frame or transport failure.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// The app closed the connection before the final reply.
    #[error("the app closed the admin channel before answering")]
    Closed,
    /// The app answered something this client cannot accept.
    #[error("admin channel protocol error: {0}")]
    Protocol(String),
}

/// Send one request and wait for its final reply; `on_pending` runs once if the app opens the GUI consent dialog.
pub async fn exchange<S, F>(stream: &mut S, request: &AdminRequest, mut on_pending: F) -> Result<AdminReply, ExchangeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnMut(&PendingConsent),
{
    write_frame(stream, &encode_request(request)?).await?;
    let mut pending_seen = false;
    loop {
        let body = read_frame(stream).await?.ok_or(ExchangeError::Closed)?;
        let reply = decode_reply(&body).map_err(|error| ExchangeError::Protocol(error.to_string()))?;
        let unattributed_refusal = reply.id.is_empty() && matches!(reply.outcome, ReplyOutcome::Refused { .. });
        if reply.id != request.id && !unattributed_refusal {
            return Err(ExchangeError::Protocol(format!("reply for `{}` while waiting for `{}`", reply.id, request.id)));
        }
        if let ReplyOutcome::PendingConsent { consent } = &reply.outcome {
            if pending_seen || !request.verb.may_raise_consent() {
                return Err(ExchangeError::Protocol(format!("unexpected pending-consent for `{}`", request.verb.name())));
            }
            pending_seen = true;
            on_pending(consent);
            continue;
        }
        return Ok(reply);
    }
}

#[cfg(not(any(unix, windows)))]
fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "the admin channel needs Windows named pipes or Unix sockets")
}
