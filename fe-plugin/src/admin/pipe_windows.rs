//! Windows named-pipe transport: owner-SID-only DACL, remote clients rejected, both ends' SIDs checked.

use std::ffi::c_void;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};

use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, PipeMode, ServerOptions};
use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, HANDLE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows_sys::Win32::Security::{EqualSid, GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER};
use windows_sys::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::transport::{AdminStream, ConnectError, LocalEndpoint, PeerUser};

/// Pipe instances alive at once: served clients, `busy` responders (one each) and the one waiting, plus headroom.
const MAX_PIPE_INSTANCES: usize = 2 * super::MAX_CLIENTS + 2;
/// How long the client retries a pipe whose instances are all busy.
const BUSY_WAIT: Duration = Duration::from_secs(2);
/// `SDDL_REVISION_1`.
const SDDL_REVISION: u32 = 1;

/// `\\.\pipe\fractalengine-admin-<user SID>[-<instance>]`.
pub(super) fn address(endpoint: &LocalEndpoint) -> io::Result<String> {
    let sid = UserSid::current()?.to_string_sid()?;
    Ok(pipe_name(&sid, endpoint))
}

fn pipe_name(sid: &str, endpoint: &LocalEndpoint) -> String {
    match endpoint.instance() {
        Some(instance) => format!(r"\\.\pipe\fractalengine-admin-{sid}-{instance}"),
        None => format!(r"\\.\pipe\fractalengine-admin-{sid}"),
    }
}

/// The server side: the instance waiting for the next client.
pub(super) struct Listener {
    name: String,
    descriptor: SecurityDescriptor,
    own_sid: UserSid,
    waiting: NamedPipeServer,
}

pub(super) fn listen(endpoint: &LocalEndpoint) -> io::Result<Listener> {
    let own_sid = UserSid::current()?;
    let sid_text = own_sid.to_string_sid()?;
    let name = pipe_name(&sid_text, endpoint);
    let descriptor = SecurityDescriptor::owner_only(&sid_text)?;
    // first_pipe_instance: an existing pipe of this name (another app, or a squatter) is an error, never joined.
    let waiting = create_instance(&name, &descriptor, true)?;
    Ok(Listener { name, descriptor, own_sid, waiting })
}

impl Listener {
    pub(super) async fn accept(&mut self) -> io::Result<(AdminStream, PeerUser)> {
        loop {
            let connected = self.waiting.connect().await;
            // Always re-arm before handing the connected instance out, so the name never disappears.
            let fresh = create_instance(&self.name, &self.descriptor, false)?;
            let instance = std::mem::replace(&mut self.waiting, fresh);
            if let Err(error) = connected {
                tracing::debug!(%error, "admin channel: a client left before the pipe connected");
                continue;
            }
            let peer = match client_sid(&instance) {
                Ok(sid) if sid.equals(&self.own_sid) => PeerUser::Same,
                Ok(sid) => PeerUser::Other(sid.to_string_sid().unwrap_or_else(|_| "another user".into())),
                Err(error) => PeerUser::Other(format!("unverified client: {error}")),
            };
            return Ok((AdminStream::pipe_server(instance), peer));
        }
    }
}

fn create_instance(name: &str, descriptor: &SecurityDescriptor, first: bool) -> io::Result<NamedPipeServer> {
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true)
        .pipe_mode(PipeMode::Byte)
        .max_instances(MAX_PIPE_INSTANCES);
    // SAFETY: `attributes` and the descriptor it points at outlive the call; the pipe copies the DACL.
    unsafe { options.create_with_security_attributes_raw(name, (&mut attributes as *mut SECURITY_ATTRIBUTES).cast::<c_void>()) }
}

pub(super) async fn connect(endpoint: &LocalEndpoint) -> Result<AdminStream, ConnectError> {
    let own_sid = UserSid::current()?;
    let name = pipe_name(&own_sid.to_string_sid()?, endpoint);
    let deadline = Instant::now() + BUSY_WAIT;
    let client = loop {
        match ClientOptions::new().open(&name) {
            Ok(client) => break client,
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => {
                return Err(ConnectError::NotRunning(name));
            }
            Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
                return Err(ConnectError::ForeignServer(name));
            }
            Err(error) => return Err(ConnectError::Io(error)),
        }
    };
    // A pipe squatted by another user under our name must not receive our requests.
    let mut server_pid = 0u32;
    // SAFETY: a valid pipe handle owned by `client`; `server_pid` is a valid out pointer.
    if unsafe { GetNamedPipeServerProcessId(client.as_raw_handle() as HANDLE, &mut server_pid) } == 0 {
        return Err(ConnectError::Io(io::Error::last_os_error()));
    }
    match sid_of_process(server_pid) {
        Ok(sid) if sid.equals(&own_sid) => Ok(AdminStream::pipe_client(client)),
        _ => Err(ConnectError::ForeignServer(name)),
    }
}

fn client_sid(instance: &NamedPipeServer) -> io::Result<UserSid> {
    let mut client_pid = 0u32;
    // SAFETY: a valid, connected pipe handle owned by `instance`; `client_pid` is a valid out pointer.
    if unsafe { GetNamedPipeClientProcessId(instance.as_raw_handle() as HANDLE, &mut client_pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    sid_of_process(client_pid)
}

fn sid_of_process(pid: u32) -> io::Result<UserSid> {
    // SAFETY: plain query-rights open; the handle is closed by the guard.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = HandleGuard(process);
    UserSid::of_process(process.0)
}

/// Closes a kernel handle on drop.
struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        // SAFETY: the guard owns the handle.
        unsafe { CloseHandle(self.0) };
    }
}

/// A `TOKEN_USER` buffer (u64-aligned); the SID pointer is derived on use, so the value is freely movable.
struct UserSid {
    buffer: Vec<u64>,
}

impl UserSid {
    fn current() -> io::Result<Self> {
        // SAFETY: the pseudo-handle of the current process needs no closing.
        Self::of_process(unsafe { GetCurrentProcess() })
    }

    fn of_process(process: HANDLE) -> io::Result<Self> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: `process` is a valid process handle; `token` is a valid out pointer.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = HandleGuard(token);
        let mut needed = 0u32;
        // SAFETY: size query with a null buffer; failure with `needed` set is the expected outcome.
        unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
        if needed == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        let capacity = (buffer.len() * 8) as u32;
        // SAFETY: `buffer` has `capacity` writable, 8-aligned bytes.
        if unsafe { GetTokenInformation(token.0, TokenUser, buffer.as_mut_ptr().cast::<c_void>(), capacity, &mut needed) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { buffer })
    }

    fn sid_ptr(&self) -> *mut c_void {
        // SAFETY: the buffer holds a TOKEN_USER written by GetTokenInformation; its SID lives inside the buffer.
        unsafe { (*self.buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn equals(&self, other: &UserSid) -> bool {
        // SAFETY: both pointers are valid SIDs inside their live buffers.
        unsafe { EqualSid(self.sid_ptr(), other.sid_ptr()) != 0 }
    }

    fn to_string_sid(&self) -> io::Result<String> {
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: valid SID in; `text` receives a LocalAlloc'd NUL-terminated wide string freed below.
        if unsafe { ConvertSidToStringSidW(self.sid_ptr(), &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `text` is NUL-terminated (Win32 contract).
        let length = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
        // SAFETY: `length` u16s are readable at `text`.
        let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
        // SAFETY: allocated by ConvertSidToStringSidW.
        unsafe { LocalFree(text.cast::<c_void>()) };
        Ok(sid)
    }
}

/// A self-relative security descriptor from SDDL, freed on drop.
struct SecurityDescriptor(*mut c_void);

// SAFETY: the descriptor is immutable after creation and only read by CreateNamedPipeW.
unsafe impl Send for SecurityDescriptor {}
// SAFETY: as above; shared reads only.
unsafe impl Sync for SecurityDescriptor {}

impl SecurityDescriptor {
    /// `D:P(A;;GA;;;<sid>)`: a protected DACL granting the user's SID, and nobody else, any access.
    fn owner_only(sid: &str) -> io::Result<Self> {
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})").encode_utf16().chain(std::iter::once(0)).collect();
        let mut descriptor: *mut c_void = std::ptr::null_mut();
        // SAFETY: NUL-terminated SDDL in; `descriptor` receives a LocalAlloc'd descriptor freed on drop.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION, &mut descriptor, std::ptr::null_mut())
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(self.0) };
    }
}
