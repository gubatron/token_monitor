//! Win32 implementations: HKCU registry, named mutex, named-pipe IPC.
//!
//! Compiled only on Windows (`cfg(windows)` on the module declaration). All
//! `unsafe` is confined to thin FFI calls with a `// SAFETY:` note.

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_PIPE_CONNECTED,
    ERROR_SUCCESS, GetLastError, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_BINARY, REG_OPTION_NON_VOLATILE, REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW,
};
use windows_sys::Win32::System::Threading::CreateMutexW;

use crate::shell::error::ShellError;
use crate::shell::instance::{InstanceGuard, InstanceLock, InstanceState, MUTEX_NAME};
use crate::shell::ipc::{IpcClient, IpcMessage};
use crate::shell::registry::Registry;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn reg_err(op: &str, code: u32) -> ShellError {
    ShellError::Registry(format!("{op} failed with Win32 error {code}"))
}

fn is_missing(code: u32) -> bool {
    code == ERROR_FILE_NOT_FOUND || code == ERROR_PATH_NOT_FOUND
}

/// Owns an open registry key and closes it on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RegOpen/RegCreate and is closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

/// [`Registry`] over `HKEY_CURRENT_USER`.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinRegistry;

impl WinRegistry {
    fn open(key: &str, access: u32) -> Result<Option<Key>, ShellError> {
        let key_w = wide(key);
        let mut out: HKEY = null_mut();
        // SAFETY: `key_w` is NUL-terminated and outlives the call; `out` is a valid out-pointer.
        let rc =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key_w.as_ptr(), 0, access, &raw mut out) };
        match rc {
            ERROR_SUCCESS => Ok(Some(Key(out))),
            c if is_missing(c) => Ok(None),
            c => Err(reg_err("RegOpenKeyExW", c)),
        }
    }

    fn create(key: &str) -> Result<Key, ShellError> {
        let key_w = wide(key);
        let mut out: HKEY = null_mut();
        // SAFETY: all pointers are valid for the call; null class/security/disposition are allowed.
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key_w.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE | KEY_READ,
                null(),
                &raw mut out,
                null_mut(),
            )
        };
        if rc == ERROR_SUCCESS {
            Ok(Key(out))
        } else {
            Err(reg_err("RegCreateKeyExW", rc))
        }
    }

    fn query(key: &str, name: &str, want: u32) -> Result<Option<Vec<u8>>, ShellError> {
        let Some(k) = Self::open(key, KEY_READ)? else {
            return Ok(None);
        };
        let name_w = wide(name);
        let mut kind = 0u32;
        let mut len = 0u32;
        // SAFETY: first call with a null buffer only asks for the size.
        let rc = unsafe {
            RegQueryValueExW(
                k.0,
                name_w.as_ptr(),
                null(),
                &raw mut kind,
                null_mut(),
                &raw mut len,
            )
        };
        if is_missing(rc) {
            return Ok(None);
        }
        if rc != ERROR_SUCCESS {
            return Err(reg_err("RegQueryValueExW", rc));
        }
        let mut buf = vec![0u8; len as usize];
        // SAFETY: `buf` has `len` writable bytes, matching the size passed in.
        let rc = unsafe {
            RegQueryValueExW(
                k.0,
                name_w.as_ptr(),
                null(),
                &raw mut kind,
                buf.as_mut_ptr(),
                &raw mut len,
            )
        };
        if is_missing(rc) {
            return Ok(None);
        }
        if rc != ERROR_SUCCESS {
            return Err(reg_err("RegQueryValueExW", rc));
        }
        buf.truncate(len as usize);
        Ok((kind == want).then_some(buf))
    }
}

impl Registry for WinRegistry {
    fn get_string(&self, key: &str, name: &str) -> Result<Option<String>, ShellError> {
        Ok(Self::query(key, name, REG_SZ)?.map(|bytes| {
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .take_while(|&u| u != 0)
                .collect();
            String::from_utf16_lossy(&units)
        }))
    }

    fn set_string(&self, key: &str, name: &str, value: &str) -> Result<(), ShellError> {
        let k = Self::create(key)?;
        let name_w = wide(name);
        let data: Vec<u8> = wide(value).iter().flat_map(|u| u.to_le_bytes()).collect();
        // SAFETY: `data` is valid for `data.len()` bytes and includes the terminating NUL.
        let rc = unsafe {
            RegSetValueExW(
                k.0,
                name_w.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr(),
                data.len() as u32,
            )
        };
        if rc == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(reg_err("RegSetValueExW", rc))
        }
    }

    fn get_binary(&self, key: &str, name: &str) -> Result<Option<Vec<u8>>, ShellError> {
        Self::query(key, name, REG_BINARY)
    }

    fn delete_value(&self, key: &str, name: &str) -> Result<(), ShellError> {
        let Some(k) = Self::open(key, KEY_WRITE)? else {
            return Ok(());
        };
        let name_w = wide(name);
        // SAFETY: `name_w` is NUL-terminated and `k` is an open key.
        let rc = unsafe { RegDeleteValueW(k.0, name_w.as_ptr()) };
        if rc == ERROR_SUCCESS || is_missing(rc) {
            Ok(())
        } else {
            Err(reg_err("RegDeleteValueW", rc))
        }
    }

    fn delete_key_tree(&self, key: &str) -> Result<(), ShellError> {
        let key_w = wide(key);
        // SAFETY: `key_w` is NUL-terminated and outlives the call.
        let rc = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key_w.as_ptr()) };
        if rc == ERROR_SUCCESS || is_missing(rc) {
            Ok(())
        } else {
            Err(reg_err("RegDeleteTreeW", rc))
        }
    }

    fn key_exists(&self, key: &str) -> Result<bool, ShellError> {
        Ok(Self::open(key, KEY_READ)?.is_some())
    }
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

fn create_pipe(name: &str, first: bool) -> std::io::Result<File> {
    let name_w = wide(name);
    let mut open_mode = PIPE_ACCESS_INBOUND;
    if first {
        // Refuse to serve if another process already owns this pipe name (squatting).
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: `name_w` is NUL-terminated; default security (creator + admins write, others read).
    let handle = unsafe {
        CreateNamedPipeW(
            name_w.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            512,
            512,
            0,
            null(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: valid, owned pipe handle; the File takes over closing it.
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn wait_for_client(pipe: &File) -> bool {
    use std::os::windows::io::AsRawHandle;
    // SAFETY: the handle is a live server pipe handle; blocking mode, no OVERLAPPED.
    let ok = unsafe { ConnectNamedPipe(pipe.as_raw_handle(), null_mut()) };
    // SAFETY: reads the last error right after the call.
    ok != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED
}

/// Background server that turns pipe messages into callbacks. Dropping it stops the thread.
pub struct PipeServer {
    name: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PipeServer {
    /// Creates the pipe and starts serving; `on_message` runs on the server thread
    /// (marshal to the UI thread inside it).
    ///
    /// # Errors
    ///
    /// [`ShellError::Ipc`] when the pipe cannot be created (e.g. name already owned).
    pub fn start(
        name: &str,
        on_message: impl Fn(IpcMessage) + Send + 'static,
    ) -> Result<Self, ShellError> {
        let first = create_pipe(name, true).map_err(|e| ShellError::Ipc(e.to_string()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_name = name.to_owned();
        let thread = std::thread::Builder::new()
            .name("tokenmon-ipc".into())
            .spawn(move || serve(first, &thread_name, &thread_stop, &on_message))
            .map_err(|e| ShellError::Ipc(e.to_string()))?;
        Ok(Self {
            name: name.to_owned(),
            stop,
            thread: Some(thread),
        })
    }
}

fn serve(first: File, name: &str, stop: &AtomicBool, on_message: &dyn Fn(IpcMessage)) {
    let mut pipe = first;
    loop {
        let connected = wait_for_client(&pipe);
        if stop.load(Ordering::SeqCst) {
            return;
        }
        if connected && let Ok(message) = IpcMessage::read_from(&pipe) {
            on_message(message);
        }
        drop(pipe);
        pipe = match create_pipe(name, false) {
            Ok(p) => p,
            Err(_) => return,
        };
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking ConnectNamedPipe with a throwaway connection.
        drop(OpenOptions::new().write(true).open(&self.name));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
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
}

/// `ERROR_PIPE_BUSY`: all instances are connected; try again shortly.
const PIPE_BUSY: i32 = 231;

impl IpcClient for PipeClient {
    fn send(&self, message: &IpcMessage) -> Result<(), ShellError> {
        use std::io::Write;
        let mut last = String::new();
        for _ in 0..20 {
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
