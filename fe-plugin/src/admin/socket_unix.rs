//! Unix-socket transport: a 0700 directory owned by the user, a 0600 socket, and a peer-uid check on both ends.

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use tokio::net::{UnixListener, UnixStream};

use super::transport::{AdminStream, ConnectError, LocalEndpoint, PeerUser};

/// `$XDG_RUNTIME_DIR/fractalengine/admin[-<instance>].sock`, else `<temp>/fractalengine-<uid>/…`.
pub(super) fn address(endpoint: &LocalEndpoint) -> io::Result<String> {
    Ok(socket_path(&runtime_dir(euid()), endpoint).display().to_string())
}

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

fn runtime_dir(uid: u32) -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir.join("fractalengine"),
        _ => std::env::temp_dir().join(format!("fractalengine-{uid}")),
    }
}

fn socket_path(dir: &Path, endpoint: &LocalEndpoint) -> PathBuf {
    match endpoint.instance() {
        Some(instance) => dir.join(format!("admin-{instance}.sock")),
        None => dir.join("admin.sock"),
    }
}

/// The directory must be a real directory (not a symlink) owned by `uid` with no group/other bits.
fn check_private_dir(dir: &Path, uid: u32) -> io::Result<()> {
    let meta = fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() || meta.uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a directory owned by this user", dir.display()),
        ));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is accessible to other users (mode {:o})", dir.display(), meta.mode() & 0o777),
        ));
    }
    Ok(())
}

/// Create the directory 0700, or tighten one this user already owns; refuse anything else.
fn ensure_private_dir(dir: &Path, uid: u32) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let meta = fs::symlink_metadata(dir)?;
    if meta.file_type().is_dir() && meta.uid() == uid && meta.mode() & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    check_private_dir(dir, uid)
}

/// The server side; removes its socket file on drop.
pub(super) struct Listener {
    listener: UnixListener,
    path: PathBuf,
    uid: u32,
}

pub(super) fn listen(endpoint: &LocalEndpoint) -> io::Result<Listener> {
    let uid = euid();
    let dir = runtime_dir(uid);
    ensure_private_dir(&dir, uid)?;
    let path = socket_path(&dir, endpoint);
    if fs::symlink_metadata(&path).is_ok() {
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("another FractalEngine already serves the admin channel at {}", path.display()),
            ));
        }
        // A stale socket from a crashed run; the directory is private, so nobody else can have placed it.
        fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(Listener { listener, path, uid })
}

impl Listener {
    pub(super) async fn accept(&mut self) -> io::Result<(AdminStream, PeerUser)> {
        let (stream, _) = self.listener.accept().await?;
        let peer = match stream.peer_cred() {
            Ok(cred) if cred.uid() == self.uid => PeerUser::Same,
            Ok(cred) => PeerUser::Other(format!("uid {}", cred.uid())),
            Err(error) => PeerUser::Other(format!("unverified peer: {error}")),
        };
        Ok((AdminStream::socket(stream), peer))
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) async fn connect(endpoint: &LocalEndpoint) -> Result<AdminStream, ConnectError> {
    let uid = euid();
    let dir = runtime_dir(uid);
    let path = socket_path(&dir, endpoint);
    let shown = path.display().to_string();
    match check_private_dir(&dir, uid) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(ConnectError::NotRunning(shown)),
        Err(_) => return Err(ConnectError::ForeignServer(shown)),
    }
    let stream = match UnixStream::connect(&path).await {
        Ok(stream) => stream,
        Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
            return Err(ConnectError::NotRunning(shown));
        }
        Err(error) => return Err(ConnectError::Io(error)),
    };
    match stream.peer_cred() {
        Ok(cred) if cred.uid() == uid => Ok(AdminStream::socket(stream)),
        _ => Err(ConnectError::ForeignServer(shown)),
    }
}
