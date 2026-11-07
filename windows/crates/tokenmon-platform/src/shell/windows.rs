//! Win32 implementations: named mutex and named-pipe IPC, plus the HKCU
//! registry in [`registry`] (a separate file so coverage can exclude it; see
//! `windows/.config/coverage-excludes.txt`).
//!
//! Compiled only on Windows (`cfg(windows)` on the module declaration). All
//! `unsafe` is confined to thin FFI calls with a `// SAFETY:` note.

use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED,
    GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_INBOUND, ReadFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, GetCurrentProcessId, INFINITE, ResetEvent, SetEvent,
    WaitForMultipleObjects, WaitForSingleObject,
};

use crate::secrets::{current_user_sid, owner_only_sddl};
use crate::shell::error::ShellError;
use crate::shell::instance::{InstanceGuard, InstanceLock, InstanceState, MUTEX_NAME};
use crate::shell::ipc::{DeadlineRead, IpcClient, IpcMessage, READ_DEADLINE, session_pipe_name};

mod registry;

pub use registry::WinRegistry;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// [`InstanceLock`] using a named mutex in the session namespace.
#[derive(Debug, Default, Clone, Copy)]
pub struct MutexInstanceLock;

impl InstanceLock for MutexInstanceLock {
    fn try_acquire(&self) -> Result<InstanceState, ShellError> {
        let name = wide(MUTEX_NAME);
        // SAFETY: `name` is NUL-terminated; null security attributes are allowed.
        let handle = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
        // SAFETY: reads the calling thread's last error immediately after the call above.
        let err = unsafe { GetLastError() };
        if handle.is_null() {
            return Err(ShellError::Instance(format!("CreateMutexW error {err}")));
        }
        // SAFETY: `handle` is a valid, owned kernel handle that nothing else closes.
        let owned = unsafe { OwnedHandle::from_raw_handle(handle) };
        Ok(if err == ERROR_ALREADY_EXISTS {
            drop(owned);
            InstanceState::Secondary
        } else {
            InstanceState::Primary(InstanceGuard::new(owned))
        })
    }
}

/// An owned kernel event handle (manual reset).
struct Event(HANDLE);

// SAFETY: kernel event handles may be used and closed from any thread.
unsafe impl Send for Event {}
// SAFETY: SetEvent/ResetEvent/Wait* on one event are thread-safe.
unsafe impl Sync for Event {}

impl Event {
    fn new() -> std::io::Result<Self> {
        // SAFETY: null attributes and name are allowed; manual reset, initially unsignaled.
        let h = unsafe { CreateEventW(null(), 1, 0, null()) };
        if h.is_null() {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(Self(h))
        }
    }

    fn set(&self) {
        // SAFETY: `self.0` is a live event handle.
        unsafe { SetEvent(self.0) };
    }

    fn reset(&self) {
        // SAFETY: `self.0` is a live event handle.
        unsafe { ResetEvent(self.0) };
    }

    fn is_set(&self) -> bool {
        // SAFETY: `self.0` is a live event handle; a zero timeout only polls.
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_OBJECT_0 }
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle was created by CreateEventW and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// A `LocalAlloc`ed security descriptor, freed on drop.
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    /// Parses `sddl`.
    fn from_sddl(sddl: &str) -> std::io::Result<Self> {
        let sddl_w = wide(sddl);
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `sddl_w` is NUL-terminated; the out descriptor is freed in Drop.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl_w.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                null_mut(),
            )
        };
        if ok == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(Self(descriptor))
        }
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by the SDDL conversion with LocalAlloc, freed once.
        unsafe { LocalFree(self.0) };
    }
}

/// How a pending overlapped operation ended.
enum Waited {
    /// Completed, transferring this many bytes (0 at end of stream).
    Done(u32),
    /// The timeout elapsed first (the operation was cancelled).
    Timeout,
    /// The stop event fired first (the operation was cancelled).
    Stopped,
}

fn overlapped(event: HANDLE) -> OVERLAPPED {
    // SAFETY: OVERLAPPED is plain data for which all-zero is the documented initial state.
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.hEvent = event;
    ov
}

/// The one server instance of the single-instance pipe: overlapped, so every
/// wait can time out or be interrupted by `stop`.
struct Pipe {
    handle: HANDLE,
    io: Event,
    stop: Arc<Event>,
}

// SAFETY: the pipe handle is only used by the thread that owns the `Pipe`.
unsafe impl Send for Pipe {}

impl Pipe {
    /// Creates the (only) instance of `name`, reachable by the current user
    /// only (explicit protected DACL), local clients only, and failing if any
    /// process already owns the name.
    fn create(name: &str, stop: Arc<Event>) -> std::io::Result<Self> {
        let descriptor = Descriptor::from_sddl(&owner_only_sddl(&current_user_sid()?))?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        let name_w = wide(name);
        // SAFETY: `name_w` is NUL-terminated and `attributes` outlives the call.
        // FIRST_PIPE_INSTANCE refuses a name another process already created
        // (squatting); a maximum of one instance keeps it that way.
        let handle = unsafe {
            CreateNamedPipeW(
                name_w.as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                0,
                512,
                0,
                &raw const attributes,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            handle,
            io: Event::new()?,
            stop,
        })
    }

    /// Waits for the pending operation `ov` for at most `timeout_ms`, or until
    /// `stop` fires. The operation is always finished (completed or cancelled
    /// and drained) before this returns, so `ov` may go out of scope.
    fn wait(&self, ov: &mut OVERLAPPED, timeout_ms: u32) -> std::io::Result<Waited> {
        let handles = [self.io.0, self.stop.0];
        // SAFETY: both handles are live events owned by `self`.
        let r = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout_ms) };
        let mut n = 0u32;
        if r == WAIT_OBJECT_0 {
            // SAFETY: `ov` belongs to the finished operation on `self.handle`.
            let ok = unsafe { GetOverlappedResult(self.handle, ov, &raw mut n, 0) };
            return if ok != 0 {
                Ok(Waited::Done(n))
            } else {
                Self::finished_error()
            };
        }
        // SAFETY: cancel the operation tied to `ov`, then wait for the kernel
        // to release it (the cancelled completion) so `ov` is no longer in use.
        let ok = unsafe {
            CancelIoEx(self.handle, ov);
            GetOverlappedResult(self.handle, ov, &raw mut n, 1)
        };
        if ok != 0 {
            return Ok(Waited::Done(n)); // it completed just before the cancel
        }
        if r == WAIT_TIMEOUT {
            Ok(Waited::Timeout)
        } else if r == WAIT_OBJECT_0 + 1 {
            Ok(Waited::Stopped)
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    fn finished_error() -> std::io::Result<Waited> {
        // SAFETY: reads the calling thread's last error right after the failed call.
        let code = unsafe { GetLastError() };
        if code == ERROR_BROKEN_PIPE {
            Ok(Waited::Done(0)) // the client closed: end of stream
        } else {
            Err(std::io::Error::from_raw_os_error(
                i32::try_from(code).unwrap_or(i32::MAX),
            ))
        }
    }

    /// Waits for a client. `Ok(false)` means the server is stopping.
    fn connect(&self) -> std::io::Result<bool> {
        let mut ov = overlapped(self.io.0);
        self.io.reset();
        // SAFETY: live pipe handle; `ov` stays alive until the operation is drained below.
        let ok = unsafe { ConnectNamedPipe(self.handle, &raw mut ov) };
        if ok != 0 {
            return Ok(true);
        }
        // SAFETY: reads the last error right after the call.
        match unsafe { GetLastError() } {
            ERROR_PIPE_CONNECTED => Ok(true),
            ERROR_IO_PENDING => match self.wait(&mut ov, INFINITE)? {
                Waited::Done(_) => Ok(true),
                Waited::Stopped | Waited::Timeout => Ok(false),
            },
            code => Err(std::io::Error::from_raw_os_error(
                i32::try_from(code).unwrap_or(i32::MAX),
            )),
        }
    }

    /// Drops the current client so the same instance can accept the next one.
    fn disconnect(&self) {
        // SAFETY: live server pipe handle.
        unsafe { DisconnectNamedPipe(self.handle) };
    }
}

impl DeadlineRead for Pipe {
    fn read_some(&mut self, buf: &mut [u8], timeout: Duration) -> std::io::Result<usize> {
        let mut ov = overlapped(self.io.0);
        self.io.reset();
        let mut n = 0u32;
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        // SAFETY: `buf` and `ov` stay alive until the operation is drained by `wait`.
        let ok = unsafe {
            ReadFile(
                self.handle,
                buf.as_mut_ptr().cast(),
                len,
                &raw mut n,
                &raw mut ov,
            )
        };
        if ok != 0 {
            return Ok(n as usize);
        }
        // SAFETY: reads the last error right after the call.
        match unsafe { GetLastError() } {
            ERROR_BROKEN_PIPE => return Ok(0),
            ERROR_IO_PENDING => {}
            code => {
                return Err(std::io::Error::from_raw_os_error(
                    i32::try_from(code).unwrap_or(i32::MAX),
                ));
            }
        }
        let ms = u32::try_from(timeout.as_millis())
            .unwrap_or(u32::MAX - 1)
            .min(u32::MAX - 1);
        match self.wait(&mut ov, ms)? {
            Waited::Done(n) => Ok(n as usize),
            Waited::Timeout => Err(ErrorKind::TimedOut.into()),
            Waited::Stopped => Err(std::io::Error::new(
                ErrorKind::ConnectionAborted,
                "server stopping",
            )),
        }
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateNamedPipeW and is closed once.
        unsafe { CloseHandle(self.handle) };
    }
}

/// Consecutive failed accepts after which the server gives up (and says so).
const MAX_ACCEPT_FAILURES: u32 = 50;

/// Background server that turns pipe messages into callbacks. Dropping it stops
/// the thread promptly, even while a client is stalled mid-message.
pub struct PipeServer {
    stop: Arc<Event>,
    thread: Option<JoinHandle<()>>,
}

impl PipeServer {
    /// Creates the pipe and starts serving; `on_message` runs on the server thread
    /// (marshal to the UI thread inside it). Use [`current_user_pipe_name`] for
    /// `name`.
    ///
    /// # Errors
    ///
    /// [`ShellError::Ipc`] when the pipe cannot be created (e.g. name already owned).
    pub fn start(
        name: &str,
        on_message: impl Fn(IpcMessage) + Send + 'static,
    ) -> Result<Self, ShellError> {
        let stop = Arc::new(Event::new().map_err(|e| ShellError::Ipc(e.to_string()))?);
        let pipe = Pipe::create(name, Arc::clone(&stop))
            .map_err(|e| ShellError::Ipc(format!("cannot create the IPC pipe: {e}")))?;
        let thread = std::thread::Builder::new()
            .name("tokenmon-ipc".into())
            .spawn(move || serve(pipe, &on_message))
            .map_err(|e| ShellError::Ipc(e.to_string()))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

fn serve(mut pipe: Pipe, on_message: &dyn Fn(IpcMessage)) {
    let mut failures = 0u32;
    loop {
        if pipe.stop.is_set() {
            break;
        }
        match pipe.connect() {
            Ok(true) => {
                failures = 0;
                match IpcMessage::read_with_deadline(&mut pipe, READ_DEADLINE) {
                    Ok(message) => on_message(message),
                    Err(e) => {
                        tracing::debug!(error = %e, "ignored a bad single-instance IPC client");
                    }
                }
                pipe.disconnect();
            }
            Ok(false) => break,
            Err(e) => {
                failures += 1;
                tracing::warn!(error = %e, failures, "single-instance IPC accept failed");
                if failures >= MAX_ACCEPT_FAILURES {
                    tracing::warn!("single-instance IPC server giving up after repeated failures");
                    return;
                }
                pipe.disconnect();
                // Back off, but wake at once when stopping.
                // SAFETY: the stop event is live for the duration of the call.
                if unsafe { WaitForSingleObject(pipe.stop.0, 100) } == WAIT_OBJECT_0 {
                    break;
                }
            }
        }
    }
    tracing::debug!("single-instance IPC server stopped");
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        // Wakes ConnectNamedPipe and any pending read; the thread then exits.
        self.stop.set();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The current user's pipe name: `\\.\pipe\TokenMon.<session id>.<user SID>`
/// (see [`session_pipe_name`]). Server and client both use it.
///
/// # Errors
///
/// The error of the failed SID or session query.
pub fn current_user_pipe_name() -> std::io::Result<String> {
    let mut session = 0u32;
    // SAFETY: querying our own process id; `session` is a valid out pointer.
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &raw mut session) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(session_pipe_name(&current_user_sid()?, session))
}

/// [`IpcClient`] that writes to the running instance's pipe.
#[derive(Debug, Clone)]
pub struct PipeClient {
    name: String,
}

impl PipeClient {
    /// Client for the given pipe name (see [`crate::shell::ipc::pipe_name`]).
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    /// Client for the current user's pipe ([`current_user_pipe_name`]).
    ///
    /// # Errors
    ///
    /// The error of the failed SID or session query.
    pub fn for_current_user() -> std::io::Result<Self> {
        Ok(Self::new(current_user_pipe_name()?))
    }
}

/// `ERROR_PIPE_BUSY`: all instances are connected; try again shortly.
const PIPE_BUSY: i32 = 231;

impl IpcClient for PipeClient {
    fn send(&self, message: &IpcMessage) -> Result<(), ShellError> {
        use std::io::Write;
        let mut last = String::new();
        for _ in 0..50 {
            match OpenOptions::new().write(true).open(&self.name) {
                Ok(mut pipe) => {
                    return pipe
                        .write_all(&message.encode())
                        .map_err(|e| ShellError::Ipc(e.to_string()));
                }
                Err(e)
                    if e.kind() == ErrorKind::NotFound || e.raw_os_error() == Some(PIPE_BUSY) =>
                {
                    last = e.to_string();
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(ShellError::Ipc(e.to_string())),
            }
        }
        Err(ShellError::Ipc(last))
    }
}
